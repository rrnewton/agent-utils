// The app page's tests: what the reader has read: read modes, unread replies, the bookmark, pins and filters.
//
// One feature's file of the suite tests/js/voice_page.test.mjs runs, against the fixture in
// ./fixture.mjs. Run with the rest of the suite, or alone: node --test tests/js/page/reading-state.mjs.

import assert from "node:assert/strict";

import { ACKNOWLEDGED, ALICE, BOTTOM_SLACK_PX, BOUND_NOTICE, CHANNEL, CODER, COUNTS_ALL_READ_MS,
  CSS, CSS_CODE, DISCORD_POLL_MS, GATHERED, HTML, HTML_CODE, INBOX, LINE_PX, MESSAGE_CACHE_KEY,
  MESSAGE_SPACING_MS, OTHER_CHANNEL, OWN, OWNERS_MESSAGE, PAGE_ELEMENTS, RUN_TAIL, RUN_THREAD,
  SCRIPT, SCRIPT_CODE, SCROLL_VIEWPORT_PX, UI_STATE_KEY, allLeavingOut, arrowStyle,
  assertMarkupContains, assistantSays, atBottomOf, atTheHead, authorChannel, authorState, backlog,
  bid, bookmarkAt, bookmarkClearOf, bookmarkRow, brokenScript, brokenScriptEach, channelRow,
  chipsOf, chooseAuthor, chooseReadMode, clearingPage, clickRow, collapsedRows, conversation,
  cssBlock, cssBlockIn, cssRules, delayedReply, deliver, doneButton, enterRunThread, errorResponse,
  floatingLine, focusName, focused, foldButton, forwardPage, freshMessageData, freshMessagePage,
  gate, gatherChip, holdChannelReads, holdCountsRead, inboxChannel, inboxId, inboxOnMain,
  joiningChannel, json, jumpSaysArrived, keptAt, keyboardPage, keyboardPageAtTop, litIds, litRows,
  longMessage, markupHolds, mdBoxes, mediaBody, message, messageCacheStatus, messageRows, mouseTo,
  newPage, olderReads, outgoingRetry, outgoingRows, pagedChannel, passing, pickThread, pinChip,
  pinItem, pinnedIds, pinnedPlacePage, placeChannel, placeRow, pollOnce, press, pressHere,
  proposedDuringACall, quotePage, reReadChannel, readChannel, readModeSaid, readRunLineOf,
  readThroughButton, reloadPage, reloadWith, reopenedChannel, repliesPage, replyAndSend,
  replyButton, row, rowHoldingId, rowMoreButton, rowMoreMenu, rowOf, rowShowing, rowStates,
  rowWithId, runId, runThread, savedOutgoing, savedUiState, scatteredPage, scopedData, scrollToGap,
  search, selectedId, sentWithoutKey, serveScoped, servedBookmarkPage, showDiscord, showLinks,
  showPinned, shownIds, sideIds, sideRow, signIn, sourceConstant, sseMessage, stackPlaces,
  startTalking, statusText, storedPin, tagged, tallChannel, tapArrow, tapBookmarkClear, tapKind,
  tapPin, tapRow, test, threadBadge, threadButton, threadData, threadPage, threadedService,
  throughHere, timelineAnswer, turnTodoOn, twoColumnServer, twoColumns, unfiltered, unreadChip,
  unreadSaid, viewOf, wordsIn, yOf, youSay } from "./fixture.mjs";

const MESSAGE_EPOCH = Date.parse("2026-08-19T04:31:00.000Z");

const shownPinnedIds = (page) =>
  page.el("pinned-log").children.filter((li) => !li.hasClass("search-hidden")).map((li) => li.getAttribute("data-id"));

test("the ⋯ menu is Copy text, its vibe-talk link, then Pin, then the bookmark, then the two that mark read under one caption", async () => {
  for (const upstream of [false, true]) {
    const page = newPage();
    page.upstreamReadMarkSupported = upstream;
    await signIn(page);
    const rows = await showDiscord(page, backlog(2));
    const items = rowMoreMenu(rows[0]).children;
    // `#462 webapp-deep-links`: the app's own link with the copies, Pin after them.
    assert.deepStrictEqual(items.map((node) => node.className),
      ["row-copy-button", "row-copy-deep-link-button", "row-pin-button", "row-bookmark-button", "row-more-group"], `the menu's order (upstream ${upstream})`);
    assert.equal(items[0].textContent, "Copy text", "Copy text is not first");
    assert.equal(items[1].textContent, "Copy vibe-talk link");
    assert.equal(items[2].textContent, "Pin message");
    // `#240 reading-bookmark`.
    assert.equal(items[3].textContent, "Bookmark above");
    const group = items[4];
    assert.equal(group.getAttribute("role"), "group", "the two that mark read are not a group");
    assert.equal(group.getAttribute("aria-label"), "Mark read");
    const [caption, through, provider] = group.children;
    assert.equal(caption.className, "row-more-caption");
    assert.equal(caption.textContent, "Mark read");
    assert.equal(caption.getAttribute("aria-hidden"), "true", "the caption is read twice to a screen reader");
    assert.equal(through.className, "row-read-through-button");
    assert.equal(provider.className, "upstream-read-button");
    // Where the provider has no read marker of its own, the group is the one item.
    const offered = group.children.filter((node) => node.className !== "row-more-caption" && !node.hidden);
    assert.equal(offered.length, upstream ? 2 : 1, `the group offered ${offered.length} items`);
    assert.equal(through.textContent, upstream ? "Through here · also in Discord" : "Through here");
  }
  // Grouped where an eye can see it, with every item still the full thumb size.
  assert.match(cssBlock(".row-more-group"), /border-top: 1px solid var\(--edge\)/, "the group has no rule above it");
  assert.match(cssBlock(".row-more-caption"), /color: var\(--muted\)/);
  assert.match(cssBlock(".row-more-group > button"), /min-height: 2\.75rem/, "the grouped items shrank");
});

test("the ⋯ menu hangs from the row's meta line, so a ⋯ wrapped to the left cannot carry it off the screen", () => {
  // The meta line wraps, and the "Pinned" chip, a narrow phone, a zoomed page or a larger system
  // font each put a row's "⋯" at the START of a line. Hung from the button's right edge, the menu
  // then opened wholly off the left of the screen, Unpin with it. This suite lays nothing out, so it
  // pins the frame that rules that out; tests/offline_cache_browser.py measures it in Chromium.
  for (const list of ["#discord-log", "#pinned-log"]) {
    assert.match(cssBlock(`${list} .meta`), /position: relative/, `${list}'s meta line is not the menu's frame`);
  }
  assert.match(cssBlock(".row-more"), /position: static/, "the ⋯ is the menu's frame again");
  assert.doesNotMatch(cssBlock(".row-more"), /position: relative/);
  const menu = cssBlock(".row-more-menu");
  assert.match(menu, /position: absolute/);
  assert.match(menu, /right: 0/, "the menu's right edge is not the row's");
  assert.match(menu, /max-width: 100%/, "a menu wider than the row is not held inside it");
  assert.doesNotMatch(menu, /white-space: nowrap/, "the menu's labels cannot wrap, so it outgrows the row");
});

test("Pin shows on the row at once, sends the row as what to keep, and the answer confirms it", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, backlog(3));
  assert.equal(pinChip(rows[1]).hidden, true, "an unpinned row says Pinned");
  page.holdPinWrites = true;
  await rowMoreButton(rows[1]).click();
  await pinItem(rows[1]).click();
  await page.settle();
  // BEFORE the server has answered.
  assert.equal(pinChip(rows[1]).hidden, false, "the row waited for the server to say Pinned");
  assert.equal(pinChip(rows[1]).textContent, "Pinned", "the marker is not a word");
  assert.equal(rows[1].getAttribute("data-pinned"), "true");
  assert.equal(pinItem(rows[1]).textContent, "Unpin message");
  assert.equal(rowMoreMenu(rows[1]).hidden, true, "the menu stayed open after Pin");
  assert.equal(pinChip(rows[0]).hidden, true, "a row nobody pinned says Pinned");
  const sent = page.messages[1];
  assert.deepStrictEqual(page.pinCalls, [{
    method: "PUT", id: String(sent.id),
    body: { author: sent.author, author_id: sent.author_id, author_is_bot: true, content: "message 1",
      timestamp: sent.timestamp, thread_id: null, thread_root: false },
  }], "the pin did not carry the row it pinned");
  page.holdPinWrites = false;
  page.pendingPinWrites.splice(0).forEach((release) => release());
  await page.settle();
  await page.settle();
  assert.equal(page.el("status").textContent, "Pinned here, not in Discord.");
  assert.equal(pinChip(rows[1]).hidden, false);
  assert.ok(page.pinned.has(String(sent.id)), "the server was not told");
  // Read once after the change: another device may have changed the list meanwhile.
  assert.equal(page.pinReads, 1, "the list was not read again after the change");

  // And off again, the same way.
  await tapPin(page, rows[1]);
  assert.equal(page.pinCalls.at(-1).method, "DELETE");
  assert.equal(pinChip(rows[1]).hidden, true, "Unpin left the row saying Pinned");
  assert.equal(page.el("status").textContent, "Unpinned.");
  assert.ok(!page.pinned.has(String(sent.id)));
});

test("a pin the server refuses is taken back and says why; offline, nothing is sent and it says so", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, backlog(2));
  page.pinFailure = { status: 503, error: "storage_not_configured", detail: "storage.path is not configured" };
  await tapPin(page, rows[0]);
  assert.equal(page.pinCalls.length, 1);
  assert.equal(pinChip(rows[0]).hidden, true, "a refused pin was left on the row");
  assert.equal(rows[0].getAttribute("data-pinned"), "false");
  assert.equal(pinItem(rows[0]).textContent, "Pin message");
  assert.equal(page.el("status").textContent, "That message was not pinned: storage.path is not configured");

  // Taken back the other way too: an unpin the server refuses leaves the pin where it was.
  page.pinFailure = null;
  await tapPin(page, rows[0]);
  assert.equal(pinChip(rows[0]).hidden, false);
  page.pinFailure = { status: 500, error: "storage_error", detail: "the disk is full" };
  await tapPin(page, rows[0]);
  assert.equal(pinChip(rows[0]).hidden, false, "a refused unpin took the pin away");
  assert.equal(page.el("status").textContent, "That message was not unpinned: the disk is full");
  page.pinFailure = null;

  // A connection that drops mid-request is said as what it is.
  page.pinNetworkDown = true;
  await tapPin(page, rows[1]);
  assert.equal(pinChip(rows[1]).hidden, true, "a pin that never reached the server stayed");
  assert.equal(page.el("status").textContent, "Offline — that message was not pinned. Try again once the connection is back.");
  page.pinNetworkDown = false;

  // Known to be offline: nothing is sent, nothing is queued, and the reader is told.
  const calls = page.pinCalls.length;
  await page.setOnline(false);
  await tapPin(page, rows[1]);
  assert.equal(page.pinCalls.length, calls, "a pin was sent while offline");
  assert.equal(pinChip(rows[1]).hidden, true, "an offline pin was shown as made");
  assert.equal(page.el("status").textContent, "Offline — nothing was pinned. Try again once the connection is back.");
  await page.setOnline(true);
  await page.settle();
  assert.equal(page.pinCalls.length, calls, "the offline pin was queued and sent later");
});

test("a pinned row is marked in words and by an edge of its own, legible on the dark theme", () => {
  const chip = cssBlock(".pin-chip");
  assert.match(chip, /border: 1px solid var\(--pin-edge\)/);
  assert.match(chip, /color: var\(--pin-ink\)/);
  assert.match(chip, /font-size: 0\.72rem/);
  // An edge as well, on the right: the left belongs to the row being read and the kept place.
  for (const list of ["#discord-log", "#pinned-log"]) {
    const edge = cssBlock(`${list} li.discord-message[data-pinned="true"]::after`);
    assert.match(edge, /right: 0/, `${list}: the pinned edge is not on the right`);
    assert.match(edge, /background: var\(--pin-edge\)/);
  }
  // Declared for both schemes, so the marker is not invisible in one of them.
  for (const token of ["--pin-ink", "--pin-edge", "--pin-bg"]) {
    assert.equal(CSS_CODE.split(`${token}:`).length - 1, 2, `${token} is not declared for both schemes`);
  }
  // Every rule that draws a channel row draws a row of the Pinned filter the same way.
  for (const selector of ['#discord-log li.discord-message[data-who="me"]', "#discord-log li.discord-message",
    '#discord-log li.discord-message[data-archived="true"]', "#discord-log .meta", "#discord-log .fold"]) {
    assert.ok(cssRules(CSS, selector.replace("#discord-log", "#pinned-log")).length > 0,
      `a pinned row is not drawn like a channel row: no rule for ${selector} in the Pinned filter`);
  }
});

test("the Pinned filter sits right beside the glass, only while the bar is open, and says what it does", async () => {
  // Markup: field, count, the filter, the glass — the filter IMMEDIATELY before the glass.
  assertMarkupContains("search-float", "pinned-filter");
  const bar = HTML_CODE.slice(HTML_CODE.indexOf('id="search-float"'));
  const at = (id) => bar.indexOf(`id="${id}"`);
  assert.ok(at("search-count") < at("pinned-filter") && at("pinned-filter") < at("search-toggle"),
    "the filter is not between the count and the glass");
  assert.equal(bar.slice(bar.indexOf("</button>", at("pinned-filter")) + "</button>".length, at("search-toggle")).trim(),
    "<button", "something stands between the filter and the glass");
  // An icon in the house style: the glass's own stroke, cap and join.
  const svgOf = (id) => {
    const from = bar.indexOf("<svg", at(id));
    return bar.slice(from, bar.indexOf(">", from));
  };
  for (const attribute of ['class="icon"', 'stroke-width="1.9"', 'stroke-linecap="round"', 'aria-hidden="true"']) {
    assert.ok(svgOf("pinned-filter").includes(attribute) && svgOf("search-toggle").includes(attribute),
      `the pin icon does not share ${attribute} with the glass`);
  }
  // A whole 44px target, kept out of the glass's own square — since `#211 link-filter` by a margin
  // on the glass, which holds for whichever filter stands beside it.
  const target = cssBlock("#pinned-filter");
  assert.match(target, /width: var\(--toggle-hit\)/);
  assert.match(target, /height: var\(--toggle-hit\)/);
  // Since `#212 link-filter` that margin is a token, because the kinds of link under Links are placed
  // by it too.
  assert.match(cssBlock("#screen-main[data-searching] #search-toggle"), /margin-left: var\(--glass-clear\)/);
  assert.match(cssBlock(":root"),
    /--glass-clear: max\(0px, calc\(var\(--search-hit\) - var\(--search-disc\) - var\(--float-edge\)/);

  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog(2));
  // Named for what it does, and a toggle that says whether it is on.
  const markup = bar.slice(at("pinned-filter"), bar.indexOf(">", at("pinned-filter")));
  assert.match(markup, /aria-label="Show only pinned messages"/);
  assert.match(markup, /aria-pressed="false"/);
  const filter = page.el("pinned-filter");
  assert.equal(filter.hidden, true, "the filter is offered before the search bar is open");
  assert.equal(filter.getAttribute("aria-pressed"), "false");
  await page.el("search-toggle").click();
  assert.equal(filter.hidden, false, "the open bar has no Pinned filter");
  await filter.click();
  await page.settle();
  assert.equal(filter.getAttribute("aria-pressed"), "true", "the filter does not say it is on");
  // The call's transcript has no pins; the filter is not offered over it, and comes back with the channel.
  await page.el("view-switch").click();
  await page.settle();
  assert.equal(filter.hidden, true, "the filter is offered over the call view");
  await page.el("view-switch").click();
  await page.settle();
  assert.equal(filter.hidden, false);
  assert.equal(filter.getAttribute("aria-pressed"), "true");
});

test("Pinned shows this channel's pins in the order they were sent, loaded or not, whatever the view and Hide read", async () => {
  const page = newPage();
  const data = threadData();
  page.threadingSupported = true;
  page.threads = data.threads;
  const old = message({ id: "150", content: "an old decision about the release",
    timestamp: new Date(Date.parse(data.messages[0].timestamp) - 3_600_000).toISOString() });
  const answer = data.messages.find((m) => m.id === "202");
  const announcement = data.messages.find((m) => m.id === "200");
  // Pinned in an order unlike the order they were sent, the newest message first.
  for (const [m, at] of [[answer, 1], [announcement, 2], [old, 3]]) page.pinned.set(String(m.id), storedPin(m, at));
  page.servePinsRevision = true;
  page.pinsRevision = 7;
  page.dealtWith.add("200");
  await signIn(page);
  await showDiscord(page, data.messages);
  await pickThread(page, "main");
  // Main, with Hide read on: the thread's answer is not in Main, and the announcement is Done.
  await turnTodoOn(page);
  assert.deepStrictEqual(shownIds(page), ["201", "203"], "the fixture is not the case under test");
  assert.equal(page.pinReads, 1, "the channel's pins were not read on entering it");

  await showPinned(page);
  assert.equal(page.el("discord-log").hidden, true, "the channel's list stayed up under the filter");
  assert.equal(page.el("pinned-log").hidden, false);
  assert.deepStrictEqual(pinnedIds(page), ["150", "200", "202"],
    "the filter did not show every pin, oldest message first");
  const rows = page.el("pinned-log").children;
  for (const row of rows) assert.equal(pinChip(row).hidden, false, "a row in the filter does not say Pinned");
  // The one older than anything loaded is drawn from what the pin kept, and says so.
  assert.match(rows[0].text(), /an old decision about the release/);
  assert.match(pinChip(rows[0]).getAttribute("title"), /copy kept when it was pinned/);
  assert.doesNotMatch(pinChip(rows[1]).getAttribute("title"), /copy kept/);
  // Done is shown as Done, and shown: a pin outranks Hide read.
  assert.equal(rows[1].getAttribute("data-archived"), "true");
  // The thread's answer can be opened in its thread from here, whatever view is underneath.
  assert.ok(threadBadge(rows[2]), "a pinned thread message offers no way into its thread");
  assert.equal(page.el("search-count").textContent, "3 pinned");
  assert.equal(page.el("search-empty").hidden, true);

  // The search text narrows the pins further — both have to hold.
  await page.el("search-field").setValue("decision");
  assert.deepStrictEqual(shownPinnedIds(page), ["150"], "the search did not narrow the pins");
  assert.equal(page.el("search-count").textContent, "1 of 3 pinned");
  await page.el("search-field").setValue("nothing like this");
  assert.deepStrictEqual(shownPinnedIds(page), []);
  assert.equal(page.el("search-empty").hidden, false);
  assert.equal(page.el("search-empty").textContent, "None of this channel's pinned messages match that search.");

  // Closing the bar clears the text AND turns the filter off: nothing is left filtering unseen.
  await page.el("search-toggle").click();
  await page.settle();
  assert.equal(page.el("pinned-filter").getAttribute("aria-pressed"), "false", "the filter outlived the bar");
  assert.equal(page.el("pinned-log").hidden, true);
  assert.equal(page.el("discord-log").hidden, false, "the channel's list did not come back");
  assert.deepStrictEqual(shownIds(page), ["201", "203"], "the channel's own filters did not survive the filter");
  assert.equal(page.el("search-empty").hidden, true);
  assert.equal(page.el("search-empty").textContent, "Nothing in the messages loaded so far matches that search.");
});

test("Escape closes the bar and the filter with it, and a pin opened in its thread leaves the filter", async () => {
  const page = newPage();
  const data = threadData();
  page.threadingSupported = true;
  page.threads = data.threads;
  page.pinned.set("202", storedPin(data.messages.find((m) => m.id === "202")));
  page.servePinsRevision = true;
  page.pinsRevision = 3;
  await signIn(page);
  await showDiscord(page, data.messages);
  await pickThread(page, "main");
  await showPinned(page);
  await page.el("search-field").dispatch("keydown", { key: "Escape" });
  await page.settle();
  assert.equal(page.el("pinned-filter").getAttribute("aria-pressed"), "false", "Escape left the filter on");
  assert.equal(page.el("pinned-log").hidden, true);

  await showPinned(page);
  await threadBadge(page.el("pinned-log").children[0]).click();
  await page.settle();
  await page.settle();
  assert.equal(page.el("pinned-filter").getAttribute("aria-pressed"), "false", "opening the thread left the filter on");
  assert.equal(page.el("thread-heading").hidden, false, "the pin's thread did not open");
  assert.deepStrictEqual(shownIds(page), ["201", "202"]);
});

test("unpinning in the Pinned filter takes the row out of it, and an empty filter says what to do", async () => {
  const page = newPage();
  const messages = backlog(3);
  page.pinned.set(messages[2].id, storedPin(messages[2]));
  page.servePinsRevision = true;
  page.pinsRevision = 1;
  await signIn(page);
  await showDiscord(page, messages);
  await showPinned(page);
  assert.deepStrictEqual(pinnedIds(page), [messages[2].id]);
  await tapPin(page, page.el("pinned-log").children[0]);
  assert.deepStrictEqual(page.pinCalls.map((call) => call.method), ["DELETE"]);
  assert.deepStrictEqual(pinnedIds(page), [], "the unpinned row stayed in the filter");
  assert.equal(page.el("search-empty").hidden, false);
  assert.equal(page.el("search-empty").textContent,
    "No pinned messages in this channel yet — pin one from its ⋯ menu.");
  assert.equal(page.el("search-count").textContent, "0 pinned");
  // The channel's own row is unpinned too.
  assert.equal(pinChip(page.el("discord-log").children[2]).hidden, true);
});

test("a list that cannot be read is said where the pins would be, not left blank", async () => {
  const page = newPage();
  page.pinListFailure = { status: 503, error: "storage_not_configured", detail: "storage.path is not configured" };
  await signIn(page);
  await showDiscord(page, backlog(2));
  await showPinned(page);
  assert.equal(page.pinReads, 1, "turning the filter on did not read the pins");
  assert.equal(page.el("search-empty").hidden, false);
  assert.equal(page.el("search-empty").textContent,
    "This channel's pinned messages could not be loaded: storage.path is not configured");
  assert.equal(page.el("error-wrap").hidden, true, "a pins read raised the error panel over the channel");
});

test("the pins are read again only when a channel read says their revision moved", async () => {
  const page = newPage();
  const messages = backlog(3);
  page.servePinsRevision = true;
  await signIn(page);
  await showDiscord(page, messages);
  assert.equal(page.pinReads, 1, "entering the channel did not read its pins");
  page.expireTimers(DISCORD_POLL_MS);
  await page.settle();
  await page.settle();
  assert.equal(page.pageReads >= 2, true, "the poll did not read the channel");
  assert.equal(page.pinReads, 1, "an ordinary refresh read the pins again");
  // Another device pins a message: the next read carries the new revision, and that is what reads it.
  page.pinned.set(messages[0].id, storedPin(messages[0]));
  page.pinsRevision += 1;
  page.expireTimers(DISCORD_POLL_MS);
  await page.settle();
  await page.settle();
  assert.equal(page.pinReads, 2, "a moved revision did not read the pins");
  assert.equal(pinChip(page.el("discord-log").children[0]).hidden, false, "another device's pin did not show");
});

test("a delta read carries the pins' revision too, so the incremental refresh keeps them current", async () => {
  const { page, server } = await forwardPage({}, (p) => { p.pinsRevision = 4; });
  const inner = page.timeline;
  page.timeline = async (path) => {
    const response = await inner(path);
    const body = JSON.parse(await response.text());
    return json(response.status, response.ok ? { ...body, pins_revision: page.pinsRevision } : body);
  };
  await pollOnce(page);
  assert.equal(server.reads.at(-1).after !== null, true, "the refresh was not a delta");
  assert.equal(page.pinReads, 1, "the first revision seen did not read the pins");
  await pollOnce(page);
  assert.equal(server.reads.at(-1).after !== null, true);
  assert.equal(page.pinReads, 1, "a delta with an unchanged revision read the pins again");
  page.pinned.set("200", storedPin(page.messages.find((m) => m.id === "200")));
  page.pinsRevision += 1;
  await pollOnce(page);
  assert.equal(server.reads.at(-1).after !== null, true);
  assert.equal(page.pinReads, 2, "a delta's moved revision did not read the pins");
  assert.equal(rowShowing(page, "main channel announcement").getAttribute("data-pinned"), "true");
});

test("at the channel's bound the server unpins the oldest pin, and the page says so and drops its marker", async () => {
  const page = newPage();
  const messages = backlog(3);
  page.pinLimit = 2;
  page.pinned.set(messages[0].id, storedPin(messages[0], 1));
  page.pinned.set(messages[1].id, storedPin(messages[1], 2));
  page.servePinsRevision = true;
  page.pinsRevision = 2;
  await signIn(page);
  const rows = await showDiscord(page, messages);
  assert.equal(pinChip(rows[0]).hidden, false);
  await tapPin(page, rows[2]);
  assert.equal(page.el("status").textContent, "Pinned. The oldest pin here was unpinned to stay at 2 pins.");
  assert.equal(pinChip(rows[0]).hidden, true, "the pin the server dropped still shows");
  assert.equal(pinChip(rows[2]).hidden, false);
});

test("pinning a combined row pins every message in it, and unpinning takes every one", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [
    message({ id: "8000000000000000300", content: "first half of a long answer", timestamp: "2026-08-19T04:31:00.000Z" }),
    message({ id: "8000000000000000301", content: "second half", timestamp: "2026-08-19T04:31:01.000Z" }),
  ]);
  assert.equal(rows.length, 1, "the fixture did not produce one combined row");
  await tapPin(page, rows[0]);
  assert.deepStrictEqual(page.pinCalls.map((call) => `${call.method} ${call.id}`),
    ["PUT 8000000000000000300", "PUT 8000000000000000301"], "half of the row was pinned");
  await tapPin(page, rows[0]);
  assert.deepStrictEqual(page.pinCalls.slice(2).map((call) => `${call.method} ${call.id}`),
    ["DELETE 8000000000000000300", "DELETE 8000000000000000301"]);
  assert.equal(page.pinned.size, 0);
});

/** What a run's line says of its messages, the days it brings (`#250 date-separators`) left out. */
const runCountWords = (line) =>
  String(line.textContent).replace(/\u2060/g, "").replace(/\u00a0/g, " ").replace(/ · [^…]+ …$/, " …");

test("entering a thread lands on its first unread message, unfolded, at the head of the list", async () => {
  const page = newPage();
  const messages = runThread(page, { tail: RUN_TAIL });
  await enterRunThread(page, messages);
  assert.equal(page.el("thread-select").value, `thread:${RUN_THREAD}`, "the thread did not open");
  assert.equal(shownIds(page).length, 18, "Show read left a message out");
  const answer = rowWithId(page, runId(500));
  assert.equal(answer.getAttribute("data-collapsed"), "false", "the answer the reader came for is folded");
  assert.equal(foldButton(answer).getAttribute("aria-expanded"), "true");
  assert.ok(atTheHead(page, answer),
    `the answer landed at ${answer.getBoundingClientRect().top}px, not under the floating line at ${floatingLine(page)}px`);
  // Show read shows what is read: nothing is collapsed, and the root, a long run above, is scrolled past.
  assert.deepStrictEqual(collapsedRows(page), [], "Show read collapsed read messages on entry");
  assert.ok(rowWithId(page, runId(0)).getBoundingClientRect().bottom <= 0, "the run fit after all; this proves nothing");
  // Only the one message: a long read reply stays as it arrived.
  assert.equal(rowWithId(page, runId(3)).getAttribute("data-collapsed"), "true", "entry opened a message already read");
});

test("...a thread with nothing unread opens on its newest message, folded, as it always has", async () => {
  const page = newPage();
  const messages = runThread(page);
  page.dealtWith.add(runId(500));
  await enterRunThread(page, messages);
  assert.ok(atBottomOf(page.el("scroll-area")), "a thread with nothing unread did not open on its newest line");
  assert.equal(rowWithId(page, runId(500)).getAttribute("data-collapsed"), "true", "a read message was opened");
});

test("...once: the reader's own fold, a redraw and a scroll after the entry open nothing and move nobody", async () => {
  const page = newPage();
  const messages = runThread(page, { tail: RUN_TAIL });
  await enterRunThread(page, messages);
  const answer = () => rowWithId(page, runId(500));
  assert.equal(answer().getAttribute("data-collapsed"), "false");
  // The reader folds it again, and goes up to the question.
  await foldButton(answer()).click();
  assert.equal(answer().getAttribute("data-collapsed"), "true");
  const area = page.el("scroll-area");
  area.scrollTop = 0;
  await reReadChannel(page);
  assert.equal(answer().getAttribute("data-collapsed"), "true", "a redraw opened what the reader folded");
  assert.equal(area.scrollTop, 0, "a redraw took the reader back to the entry's landing");
  // A message arriving in the thread is not an entry either.
  page.messages = [...messages, message({ id: runId(600), content: "one more thing",
    thread: { ...messages[1].thread }, ...CODER })];
  await reReadChannel(page);
  assert.equal(answer().getAttribute("data-collapsed"), "true", "an arrival opened what the reader folded");
  assert.equal(area.scrollTop, 0, "an arrival moved the reader");
  // Entering again is a new entry: it lands on the first unread, which is the answer still.
  await pickThread(page, "flat");
  await pickThread(page, `thread:${RUN_THREAD}`);
  assert.equal(answer().getAttribute("data-collapsed"), "false", "entering again did not open the first unread");
  assert.ok(atTheHead(page, answer()), "entering again did not land on the first unread");
});

/**
 * All's newest page, from here on: the last `n` messages of the thread, or, with `n` 0, the
 * channel's own `newer` messages. Either way it has more, and the thread began before it, so the
 * thread is not covered by All (`#220 view-switch-instant`): what All holds of it is drawn at once
 * and the thread is read behind it, and with nothing of it held the thread is read on an empty list.
 */
function allWindow(page, n, newer = []) {
  const serve = page.timeline;
  page.timeline = async (path, options) => {
    if (new URL(path, "http://fixture.test").searchParams.get("view") !== "flat") return serve(path, options);
    const recent = n === 0 ? newer : page.messages.slice(-n);
    return json(200, timelineAnswer({ view: "flat", messages: recent, has_threads: true, has_more: true,
      next_before: "older-than-the-window",
      dismissed: recent.map((m) => m.id).filter((id) => page.dealtWith.has(id)) }));
  };
}

test("...and a thread read for itself lands when its read does, not on the empty list before it", async () => {
  const page = newPage();
  const messages = runThread(page, { tail: RUN_TAIL });
  // All's window is three later messages of the channel's own: nothing of the thread is held.
  allWindow(page, 0, [1, 2, 3].map((n) => message({ id: String(9100 + n), content: `later, ${n}`, ...CODER })));
  await signIn(page);
  await showDiscord(page, messages);
  const hold = holdChannelReads(page, "timeline");
  const entering = pickThread(page, `thread:${RUN_THREAD}`);
  await page.settle();
  assert.deepStrictEqual(shownIds(page), [], "the thread was drawn before it was read");
  assert.ok(hold.held.length > 0, "the thread was not read");
  hold.release();
  await entering;
  await page.settle();
  assert.equal(shownIds(page).length, 18, "the thread's read did not land");
  const answer = rowWithId(page, runId(500));
  assert.equal(answer.getAttribute("data-collapsed"), "false", "the landing was spent on the empty list");
  assert.ok(atTheHead(page, answer), `the answer landed at ${answer.getBoundingClientRect().top}px`);
});

// `#221 read-modes` x `#220 view-switch-instant`. A thread All's window holds part of, but not its
// start, is drawn at once from the rows the page holds and read behind them. The landing is paid on
// those rows — the reader is not kept waiting for the read to open the message they came for — and
// the read behind them, which brings the root and the older replies above, leaves the landed message
// where it is: no second jump, and not folded again.

/** The ids of the messages a row stands for, oldest first. */
const rowIds = (li) => li.getAttribute("data-ids").split(" ");

/** Enter the run thread with its read held, from All holding its last `n` messages. */
async function enterFromHeldRows(page, messages, n, before = async () => {}) {
  allWindow(page, n);
  await signIn(page);
  await showDiscord(page, messages);
  await before();
  const hold = holdChannelReads(page, "timeline");
  const entering = pickThread(page, `thread:${RUN_THREAD}`);
  await page.settle();
  assert.ok(hold.held.length > 0, "the thread was not read behind the held rows");
  return { hold, entering };
}

for (const mode of ["Show read", "Fold read", "Hide read"]) {
  test(`${mode}: a thread drawn from held rows lands on its first unread at once, and the read behind them leaves it there`, async () => {
    const page = newPage();
    // Two more replies after the answer than RUN_TAIL, so the answer can stand at the head.
    const messages = runThread(page, { tail: [...RUN_TAIL, ["agent", true], ["own", true]] });
    const held = messages.slice(-8);
    const { hold, entering } = await enterFromHeldRows(page, messages, held.length,
      () => chooseReadMode(page, mode));
    const unread = new Set([runId(500), runId(502), runId(503)]);
    assert.deepStrictEqual(shownIds(page),
      held.map((m) => m.id).filter((id) => mode !== "Hide read" || unread.has(id)),
      "the rows the page held were not drawn while the thread was read");
    const answer = () => rowWithId(page, runId(500));
    assert.equal(answer().getAttribute("data-collapsed"), "false", "the held rows' first unread was not opened");
    assert.ok(atTheHead(page, answer()),
      `the answer landed at ${answer().getBoundingClientRect().top}px, not under the floating line at ${floatingLine(page)}px`);
    const top = answer().getBoundingClientRect().top;
    hold.release();
    await entering;
    await page.settle();
    assert.equal(rowWithId(page, runId(0)) !== undefined, mode !== "Hide read", "the thread's read did not land");
    assert.equal(answer().getAttribute("data-collapsed"), "false", "the read behind the held rows folded the landed message");
    assert.equal(foldButton(answer()).getAttribute("aria-expanded"), "true");
    assert.equal(answer().getBoundingClientRect().top, top, "the read behind the held rows moved the landed message");
    if (mode === "Fold read") {
      assert.equal(readRunLineOf(rowWithId(page, runId(1))).textContent, "… 12 read messages …",
        "the read messages the read brought above the landing were not collapsed");
    }
  });
}

test("...and the read behind held rows keeps the landing when the row above it grows by a message the read brought", async () => {
  const run = async (script) => {
    const page = newPage(new Map(), script);
    const messages = runThread(page, { tail: RUN_TAIL });
    // Reply 11 in the owner's words too, two seconds before reply 12, so the two are one row once 11
    // is drawn. All's window starts at 12, the row above the landing, and the read brings 11 into it.
    const at = Date.parse(messages[12].timestamp);
    messages[11] = { ...messages[11], ...OWN, timestamp: new Date(at - 2000).toISOString() };
    page.dealtWith.delete(runId(11));
    const { hold, entering } = await enterFromHeldRows(page, messages, 6);
    const answer = () => rowWithId(page, runId(500));
    assert.equal(answer().getAttribute("data-collapsed"), "false", "the held rows' first unread was not opened");
    assert.ok(atTheHead(page, answer()), `the answer landed at ${answer().getBoundingClientRect().top}px`);
    assert.deepStrictEqual(rowIds(rowWithId(page, runId(12))), [runId(12)], "reply 11 was held after all; this proves nothing");
    const top = answer().getBoundingClientRect().top;
    hold.release();
    await entering;
    await page.settle();
    assert.deepStrictEqual(rowIds(rowWithId(page, runId(12))), [runId(11), runId(12)],
      "the read did not join 11 to the row above the landing; this proves nothing");
    assert.equal(answer().getAttribute("data-collapsed"), "false", "the read behind the held rows folded the landed message");
    return answer().getBoundingClientRect().top - top;
  };
  assert.equal(await run(SCRIPT), 0, "the read behind the held rows moved the landed message");
  // Negative control: without putting the landing back, the read keeps the reader by the row at the
  // top of the screen, which now starts a message earlier, and the answer goes down by that message.
  assert.notEqual(await run(brokenScript("      if (landing) keepEntryLanding(landing);\n", "")), 0,
    "the read left the landing where it was without being told to; the control proves nothing");
});

test("...a reader who scrolls before the read behind held rows lands is left where they scrolled to", async () => {
  const page = newPage();
  const messages = runThread(page, { tail: [...RUN_TAIL, ["agent", true], ["own", true]] });
  const { hold, entering } = await enterFromHeldRows(page, messages, 8);
  const area = page.el("scroll-area");
  area.scrollTop = 0;
  const first = page.el("discord-log").children[0];
  const top = first.getBoundingClientRect().top;
  hold.release();
  await entering;
  await page.settle();
  assert.equal(shownIds(page).length, 20, "the thread's read did not land");
  assert.equal(rowWithId(page, first.getAttribute("data-id")).getBoundingClientRect().top, top,
    "the read put the reader back on the landing they had scrolled away from");
});

test("...held rows with nothing unread leave the landing to the read, which finds the older unread", async () => {
  const page = newPage();
  const messages = runThread(page, { tail: [["own", true], ["agent", true], ["own", true]] });
  // All holds the last two replies, both read: the agent's answer, older, is not among them.
  const { hold, entering } = await enterFromHeldRows(page, messages, 2);
  assert.deepStrictEqual(shownIds(page), messages.slice(-2).map((m) => m.id));
  assert.ok(atBottomOf(page.el("scroll-area")), "held rows with nothing unread did not open on the newest line");
  hold.release();
  await entering;
  await page.settle();
  const answer = rowWithId(page, runId(500));
  assert.equal(answer.getAttribute("data-collapsed"), "false", "the read did not open the thread's first unread");
  assert.ok(atTheHead(page, answer), `the answer landed at ${answer.getBoundingClientRect().top}px`);
});

test("...but not for a reader who has scrolled since the held rows were drawn", async () => {
  const page = newPage();
  const messages = runThread(page, { tail: [["own", true], ["agent", true], ["own", true]] });
  const { hold, entering } = await enterFromHeldRows(page, messages, 3);
  const area = page.el("scroll-area");
  area.scrollTop = 0;
  assert.ok(!atBottomOf(area), "the held rows fit on one screen; this proves nothing");
  const first = page.el("discord-log").children[0];
  const top = first.getBoundingClientRect().top;
  hold.release();
  await entering;
  await page.settle();
  assert.equal(rowWithId(page, runId(500)).getAttribute("data-collapsed"), "true", "the read opened a message under a reader who had moved");
  assert.equal(rowWithId(page, first.getAttribute("data-id")).getBoundingClientRect().top, top, "the read moved a reader who had moved");
});

/** Where the root and the answer stand, whether the answer is open, and what the read run's line says. */
function entryLayout(page) {
  const answer = rowWithId(page, runId(500));
  const line = readRunLineOf(rowWithId(page, runId(1)));
  return { root: rowWithId(page, runId(0)).getBoundingClientRect().top, answer: answer.getBoundingClientRect().top,
    open: answer.getAttribute("data-collapsed"), line: line && !line.hidden ? line.textContent : null };
}

// An old thread is exactly one whose root precedes All's window, so the held rows never hold its
// root, and the rule that keeps the root above the first unread when both fit could not apply when
// the held rows landed. It applies when the read behind them brings the root: the thread ends as one
// entered with every row of it held. The owner's words for Fold read: "show the thread root then
// '… 45 read messages …' then the expanded next message".
for (const [mode, read, tail, held] of [
  ["Fold read", 12, RUN_TAIL, 5],
  ["Show read", 2, [...RUN_TAIL, ["agent", true], ["own", true]], 7],
]) {
  test(`${mode}: an old thread drawn from held rows ends on its root above the first unread when both fit, as one entered with its root held does`, async () => {
    const enter = async (script, fromHeldRows, interrupt = async () => {}) => {
      const page = newPage(new Map(), script);
      const messages = runThread(page, { read, tail });
      const before = async () => {
        await chooseReadMode(page, mode);
        // A viewport the root and the start of the answer fit in, as in "Fold read: a thread opens
        // on its root, one line for the read run, and the first unread unfolded".
        page.el("scroll-area").clientHeight = 700;
      };
      if (!fromHeldRows) {
        await enterRunThread(page, messages, before);
        return { ...entryLayout(page), rootAtHead: atTheHead(page, rowWithId(page, runId(0))) };
      }
      const { hold, entering } = await enterFromHeldRows(page, messages, held, before);
      const answer = rowWithId(page, runId(500));
      assert.equal(rowWithId(page, runId(0)), undefined, "All's window held the root; this proves nothing");
      assert.equal(answer.getAttribute("data-collapsed"), "false", "the held rows' first unread was not opened");
      assert.ok(atTheHead(page, answer), `the held rows' answer landed at ${answer.getBoundingClientRect().top}px`);
      await interrupt(page);
      hold.release();
      await entering;
      await page.settle();
      await page.settle();
      return { ...entryLayout(page), rootAtHead: atTheHead(page, rowWithId(page, runId(0))) };
    };
    const covered = await enter(SCRIPT, false);
    assert.equal(covered.rootAtHead, true, "the premise: entered with every row held, the thread opens on its root");
    assert.ok(covered.answer > covered.root, "the premise: the answer is under the root");
    assert.equal(covered.line, mode === "Fold read" ? "… 12 read messages …" : null);
    assert.deepStrictEqual(await enter(SCRIPT, true), covered,
      "the read behind the held rows left the root off screen above the answer");
    // ...and the same when a live message overtakes that read, which then lands as a page folded into
    // the store: its read messages are drawn read, and collapsed, before the landing is made again.
    const live = async (page) => {
      page.stream().push(sseMessage(message({ id: "9900001", content: "elsewhere in the channel", ...CODER })));
      await page.settle();
    };
    assert.deepStrictEqual(await enter(SCRIPT, true, live), covered,
      "the read behind the held rows, overtaken by a live message, did not end as the covered thread does");
    // Negative control: the answer kept where it landed, as before. The root is above the screen.
    const control = await enter(brokenScript("  const head = entryHeadRow(first);\n", "  const head = first;\n"), true);
    assert.equal(control.rootAtHead, false, "the control still put the root at the head; it proves nothing");
    assert.ok(control.root < covered.root, "the control did not leave the root above the screen");
  });
}

test("...and keeps the landed message open when the read joins an older message of the same author at the head of its row", async () => {
  const run = async (script) => {
    const page = newPage(new Map(), script);
    const messages = runThread(page, { tail: [...RUN_TAIL, ["agent", true], ["own", true]] });
    // Reply 12 the first part of the agent's answer, a second before it and unread: the two are one
    // row once 12 is drawn, and a row keeps its fold under its first message. All's window starts at
    // the answer, so the read brings 12 into the landed row.
    const at = Date.parse(messages[13].timestamp);
    messages[12] = { ...messages[12], ...CODER, content: "the canary answer, part one", timestamp: new Date(at - 1000).toISOString() };
    page.dealtWith.delete(runId(12));
    const { hold, entering } = await enterFromHeldRows(page, messages, 7);
    const answer = () => rowWithId(page, runId(500));
    assert.deepStrictEqual(rowIds(answer()), [runId(500)], "reply 12 was held after all; this proves nothing");
    assert.equal(answer().getAttribute("data-collapsed"), "false", "the held rows' first unread was not opened");
    assert.ok(atTheHead(page, answer()), `the answer landed at ${answer().getBoundingClientRect().top}px`);
    const top = answer().getBoundingClientRect().top;
    hold.release();
    await entering;
    await page.settle();
    assert.deepStrictEqual(rowIds(answer()), [runId(12), runId(500)], "the read did not join 12 to the landed row; this proves nothing");
    return { page, open: answer().getAttribute("data-collapsed"), moved: answer().getBoundingClientRect().top - top };
  };
  const { page, open, moved } = await run(SCRIPT);
  assert.equal(open, "false", "the read folded the landed message under the message it joined to its row");
  assert.equal(foldButton(rowWithId(page, runId(500))).getAttribute("aria-expanded"), "true");
  assert.equal(moved, 0, "the read moved the landed message");
  // Opened through its own control: the reader folding it afterwards holds through the next read.
  await foldButton(rowWithId(page, runId(500))).click();
  await reReadChannel(page);
  assert.equal(rowWithId(page, runId(500)).getAttribute("data-collapsed"), "true", "a read opened the row the reader folded");
  // Negative control: without opening the row again, it is drawn folded under message 12.
  const control = await run(brokenScript("    fold.click();\n  }\n  const head = entryHeadRow(first);",
    "  }\n  const head = entryHeadRow(first);"));
  assert.equal(control.open, "true", "the row stayed open without being opened; the control proves nothing");
});

test("...and keeps it when another read overtakes the read behind the held rows: a live message, the poll, or All and back", async () => {
  const run = async (script, interrupt) => {
    const page = newPage(new Map(), script);
    const messages = runThread(page, { tail: RUN_TAIL });
    // As in "...when the row above it grows by a message the read brought": reply 11 joins the row
    // above the landing, which the read keeps its reader by, unless the landing is kept.
    const at = Date.parse(messages[12].timestamp);
    messages[11] = { ...messages[11], ...OWN, timestamp: new Date(at - 2000).toISOString() };
    page.dealtWith.delete(runId(11));
    const { hold, entering } = await enterFromHeldRows(page, messages, 6);
    await interrupt(page);
    const answer = () => rowWithId(page, runId(500));
    assert.ok(atTheHead(page, answer()), `the answer stands at ${answer().getBoundingClientRect().top}px`);
    const top = answer().getBoundingClientRect().top;
    hold.release();
    await entering;
    await page.settle();
    await page.settle();
    assert.deepStrictEqual(rowIds(rowWithId(page, runId(12))), [runId(11), runId(12)],
      "the read did not join 11 to the row above the landing; this proves nothing");
    return { moved: answer().getBoundingClientRect().top - top, open: answer().getAttribute("data-collapsed") };
  };
  const interruptions = {
    "a live message": async (page) => {
      page.stream().push(sseMessage(message({ id: "9900001", content: "elsewhere in the channel", ...CODER })));
      await page.settle();
    },
    "the poll": async (page) => {
      assert.ok(page.expireTimers(DISCORD_POLL_MS) > 0, "no poll was armed");
      await page.settle();
    },
    "All and back": async (page) => {
      await pickThread(page, "flat");
      await page.settle();
      await pickThread(page, `thread:${RUN_THREAD}`);
      await page.settle();
    },
  };
  for (const [name, interrupt] of Object.entries(interruptions)) {
    assert.deepStrictEqual(await run(SCRIPT, interrupt), { moved: 0, open: "false" },
      `${name} during the read behind the held rows moved or folded the landed message`);
  }
  // Negative control: the overtaken read folded into the store without keeping the landing.
  const control = await run(brokenScript("    if (landing) keepEntryLanding(landing);\n    payThreadEntry();\n  } else if",
    "    payThreadEntry();\n  } else if"), interruptions["a live message"]);
  assert.notEqual(control.moved, 0, "the overtaken read kept the landing without being told to; the control proves nothing");
});

// Opening the run of read messages under a root at the head scrolls nothing, so the landing still
// holds — and the root no longer fits above the first unread, twelve messages further down. A read
// that keeps the landing then must keep the reader on the root they are reading down from, not make
// the landing again on the message below the run they have just opened.
test("...and a root the landing put at the head stays there when the reader opens the read run under it, through a poll a live message overtakes", async () => {
  const run = async (script, fromHeldRows) => {
    const page = newPage(new Map(), script);
    const messages = runThread(page, { tail: RUN_TAIL });
    // As in "Fold read: an old thread drawn from held rows ends on its root ...": root and answer fit.
    const before = async () => {
      await chooseReadMode(page, "Fold read");
      page.el("scroll-area").clientHeight = 700;
    };
    if (fromHeldRows) {
      const { hold, entering } = await enterFromHeldRows(page, messages, 5, before);
      hold.release();
      await entering;
    } else {
      await enterRunThread(page, messages, before);
    }
    await page.settle();
    const root = () => rowWithId(page, runId(0));
    const answer = () => rowWithId(page, runId(500));
    assert.ok(atTheHead(page, root()), `the thread did not open on its root, at ${root().getBoundingClientRect().top}px; this proves nothing`);
    const line = readRunLineOf(rowWithId(page, runId(1)));
    assert.equal(line && line.textContent, "… 12 read messages …", "no line for the read run; this proves nothing");
    const top = root().getBoundingClientRect().top;
    await line.click();
    await page.settle();
    assert.deepStrictEqual(collapsedRows(page), [], "the tap did not open the run; this proves nothing");
    assert.equal(root().getBoundingClientRect().top, top, "opening the run moved the reader; this proves nothing");
    assert.ok(answer().getBoundingClientRect().top > page.el("scroll-area").clientHeight,
      "the answer still shows under the opened run; this proves nothing");
    // The poll reads the thread, and a live message asks for another read before that one lands: the
    // poll's page is folded into the store as late, and the landing is kept through it.
    const hold = holdChannelReads(page, "timeline");
    assert.ok(page.expireTimers(DISCORD_POLL_MS) > 0, "no poll was armed");
    await page.settle();
    page.stream().push(sseMessage(message({ id: "9900001", content: "elsewhere in the channel", ...CODER })));
    await page.settle();
    assert.ok(hold.held.length > 0, "the poll did not read the thread; this proves nothing");
    // Measured from here: the line saying the thread is loading went up above the list meanwhile.
    const held = root().getBoundingClientRect().top;
    hold.release();
    for (let settles = 0; settles < 4; settles += 1) await page.settle();
    return { moved: root().getBoundingClientRect().top - held, runOpen: collapsedRows(page).length === 0,
      rootShown: root().getBoundingClientRect().top >= 0 };
  };
  for (const fromHeldRows of [false, true]) {
    assert.deepStrictEqual(await run(SCRIPT, fromHeldRows), { moved: 0, runOpen: true, rootShown: true },
      `${fromHeldRows ? "drawn from held rows" : "entered with every row held"}: the overtaken poll moved the reader off the root`);
  }
  // Negative control: the landing made again on the first unread whenever the root no longer fits,
  // whatever the landing had at the head. The reader goes down past the run, and the root off screen.
  const control = brokenScript("  if (head !== first && head !== kept) {", "  if (head !== first || kept !== first) {");
  for (const fromHeldRows of [false, true]) {
    const { moved, rootShown } = await run(control, fromHeldRows);
    assert.equal(rootShown, false, `the control left the root on screen (moved ${moved}px); it proves nothing`);
  }
});

test("...and a landing left to the read behind held rows is paid by that read when it is overtaken, not by the next poll", async () => {
  const run = async (script) => {
    const page = newPage(new Map(), script);
    const messages = runThread(page, { tail: [["own", true], ["agent", true], ["own", true]] });
    // All holds the last two replies, both read: the landing waits for the read, which finds the answer.
    const { hold, entering } = await enterFromHeldRows(page, messages, 2);
    await pickThread(page, "flat");
    await page.settle();
    await pickThread(page, `thread:${RUN_THREAD}`);
    await page.settle();
    assert.ok(atBottomOf(page.el("scroll-area")), "the thread did not come back on the newest line; this proves nothing");
    hold.release();
    await entering;
    await page.settle();
    await page.settle();
    assert.equal(shownIds(page).length, messages.length, "the thread's read did not land; this proves nothing");
    const answer = rowWithId(page, runId(500));
    return { open: answer.getAttribute("data-collapsed"), atTheHead: atTheHead(page, answer) };
  };
  assert.deepStrictEqual(await run(SCRIPT), { open: "false", atTheHead: true },
    "the read behind the held rows, overtaken by the way out and back, did not land on the answer");
  // Negative control: the overtaken read pays nothing, and the landing is left to the next poll.
  assert.deepStrictEqual(await run(brokenScript("    if (landing) keepEntryLanding(landing);\n    payThreadEntry();\n  } else if",
    "    if (landing) keepEntryLanding(landing);\n  } else if")), { open: "true", atTheHead: false },
  "the overtaken read landed without paying the landing; the control proves nothing");
  // ...nor on the page's read messages drawn unread: the landing goes to the oldest of them instead.
  assert.deepStrictEqual(await run(brokenScript("    noteArchived(payload, false);\n    // Words and all", "    // Words and all")),
    { open: "true", atTheHead: false }, "the overtaken read found the answer without what it says is read; the control proves nothing");
});

test("...but not a landing made on an earlier visit, when the thread comes back with nothing unread", async () => {
  // A thread the page holds comes back at the scroll position it was left at, the position the first
  // visit's landing was made with. Read meanwhile in All, none of its rows is unread now, so this visit
  // lands nowhere, and the read behind them, landing late, is not held to the first visit's landing —
  // which would open a row nobody opened. (Since `#306 done-stale-across-views` the rows are drawn
  // again as the thread comes back, with the Done made in All; before, they came back as they were left.)
  const run = async (script) => {
    const page = newPage(new Map(), script);
    const messages = runThread(page, { tail: [...RUN_TAIL, ["agent", true], ["own", true]] });
    // As in "...keeps the landed message open when the read joins ...": the read joins reply 12, unread,
    // to the head of the answer's row, which is then kept folded under 12.
    const at = Date.parse(messages[13].timestamp);
    messages[12] = { ...messages[12], ...CODER, content: "the canary answer, part one", timestamp: new Date(at - 1000).toISOString() };
    page.dealtWith.delete(runId(12));
    const { hold, entering } = await enterFromHeldRows(page, messages, 7);
    const answer = rowWithId(page, runId(500));
    assert.ok(atTheHead(page, answer), "the premise: the first visit landed on the answer");
    await pickThread(page, "flat");
    for (const n of [500, 502, 503]) {
      await doneButton(rowWithId(page, runId(n))).click();
      await page.settle();
    }
    await pickThread(page, `thread:${RUN_THREAD}`);
    await page.settle();
    // Where it was left. Since `#306 done-stale-across-views` its rows are drawn again as it comes back,
    // what was marked Done in All being Done in its copy too, which retires the row the first visit
    // landed on as well.
    const back = rowWithId(page, runId(500));
    assert.ok(atTheHead(page, back), "the thread did not come back where it was left; this proves nothing");
    const doneOnReturn = back.getAttribute("data-archived");
    hold.release();
    await entering;
    await page.settle();
    await page.settle();
    assert.deepStrictEqual(rowIds(rowWithId(page, runId(500))), [runId(12), runId(500)],
      "the read did not join 12 to the answer's row; this proves nothing");
    return { collapsed: rowWithId(page, runId(500)).getAttribute("data-collapsed"), doneOnReturn };
  };
  assert.deepStrictEqual(await run(SCRIPT), { collapsed: "true", doneOnReturn: "true" },
    "the first visit's landing opened the row the read drew under message 12, or the thread came back without the Done made in All");
  // Negative control: the landing left standing when the view changes, and the thread's rows come back
  // as they were left, the very row the landing was made on among them.
  const control = brokenScriptEach(["  entryState().landing = null;\n  if (view === \"thread\")", "  if (view === \"thread\")"],
    ["if (!caughtUp && (held.readMode !== readMode || rowsMissDone())) renderCachedTimeline();",
      "if (!caughtUp && held.readMode !== readMode) renderCachedTimeline();"]);
  assert.equal((await run(control)).collapsed, "false", "the first visit's landing did not hold without the reset; the control proves nothing");
});

test("...and held rows with nothing unread whose read fails leave the reader on the newest line, through the next poll", async () => {
  const run = async (script) => {
    const page = newPage(new Map(), script);
    const messages = runThread(page, { tail: [["own", true], ["agent", true], ["own", true]] });
    const { hold, entering } = await enterFromHeldRows(page, messages, 2);
    const area = page.el("scroll-area");
    assert.ok(atBottomOf(area), "held rows with nothing unread did not open on the newest line");
    hold.fail();
    await entering;
    await page.settle();
    assert.deepStrictEqual(shownIds(page), messages.slice(-2).map((m) => m.id), "the failed read drew something");
    assert.ok(atBottomOf(area), "the failed read moved the reader");
    // Forty-five seconds on, the poll reads the thread: the reader has been reading what was drawn.
    assert.ok(page.expireTimers(DISCORD_POLL_MS) > 0, "no poll was armed");
    await page.settle();
    await page.settle();
    assert.equal(shownIds(page).length, messages.length, "the poll did not read the thread; this proves nothing");
    return { atBottom: atBottomOf(area), open: rowWithId(page, runId(500)).getAttribute("data-collapsed") };
  };
  assert.deepStrictEqual(await run(SCRIPT), { atBottom: true, open: "true" },
    "the poll after the failed read paid the landing the read owed");
  // Negative control: the deferral kept through the failure, as before.
  assert.deepStrictEqual(await run(brokenScript("      if (entryState().threadAwaitsRead) entryState().threadOwed = null;\n", "")),
    { atBottom: false, open: "false" }, "the poll moved nobody without the failure ending the wait; the control proves nothing");
});

test("Fold read: a switch from All to Main keeps the reader's message by the line of the run it is hidden in there", async () => {
  const run = async (script) => {
    const page = newPage(new Map(), script);
    page.threadingSupported = true;
    const thread = { id: "spaces/A/threads/long", root_message_id: "302", is_root: true,
      reply_count: 7, reply_count_exact: true };
    // Every other message from 303 to 315 is a reply in the one thread, which Main does not show, and
    // unread. 304, 306 and 308, posted to the channel itself, are read: one at a time between unread
    // replies in All, and one run of three in Main, where the replies are not drawn.
    // All 24 within one day, at times of their own, as written while `message`'s clock still ran through
    // the whole suite and could put them across a midnight: a run that begins a new day says that day on
    // its line (`#250 date-separators`), which is not what this test is about.
    const messages = Array.from({ length: 24 }, (_, i) => {
      const id = String(300 + i);
      const record = id === "302" ? thread : i > 2 && i < 17 && i % 2 === 1 ? { ...thread, is_root: false } : null;
      return message({ id, content: longMessage(`row ${id}`), ...(record ? { thread: record } : {}),
        timestamp: new Date(MESSAGE_EPOCH + i * MESSAGE_SPACING_MS).toISOString() });
    });
    for (const id of ["304", "306", "308"]) page.dealtWith.add(id);
    page.threads = [{ id: thread.id, root: messages[2], title: "the long thread", reply_count: 7,
      reply_count_exact: true, updated_at: messages[15].timestamp }];
    await signIn(page);
    await showDiscord(page, messages);
    await chooseReadMode(page, "Fold read");
    assert.deepStrictEqual(collapsedRows(page), [], "All collapsed a read message standing alone; this proves nothing");
    const area = page.el("scroll-area");
    area.scrollTop = rowWithId(page, "306").offsetTop();
    assert.equal(rowWithId(page, "306").getBoundingClientRect().top, 0, "the fixture could not put 306 at the top");
    await pickThread(page, "main");
    const line = rowWithId(page, "304");
    assert.equal(readRunLineOf(line) && readRunLineOf(line).textContent, "… 3 read messages …",
      "Main did not collapse 304, 306 and 308; this proves nothing");
    assert.equal(rowWithId(page, "306").getAttribute("data-read-run"), "member");
    return line.getBoundingClientRect().top;
  };
  assert.equal(await run(SCRIPT), 0, "Main did not put the run hiding the reader's message where the message was");
  // Negative control: found by the hidden row's own empty box, which stands below the line's row.
  assert.notEqual(await run(brokenScript("    const row = shownRowFor(rows.find((candidate) => idsOf(candidate).some((id) => seen.ids.includes(id))));",
    "    const row = rows.find((candidate) => idsOf(candidate).some((id) => seen.ids.includes(id)));")), 0,
  "the hidden row's box put the reader in the same place; the control proves nothing");
});

test("a landed message keeps the body the server rendered and its reactions, and a collapsed run holds them behind its line", async () => {
  const page = newPage();
  const messages = runThread(page, { tail: RUN_TAIL }).map((m) => {
    if (m.id === runId(500)) return { ...m, ...OWNERS_MESSAGE, reactions: ACKNOWLEDGED };
    if (m.id === runId(1)) return { ...m, content: "**checked** step 1", reactions: [{ emoji: "👍", count: 2 }],
      content_html: "<p><strong>checked</strong> step 1</p>\n" };
    return m;
  });
  page.messages = messages;
  await enterRunThread(page, messages, () => chooseReadMode(page, "Fold read"));
  const answer = rowWithId(page, runId(500));
  assert.equal(answer.getAttribute("data-collapsed"), "false", "the rendered answer was not opened");
  assert.ok(atTheHead(page, answer), `the answer landed at ${answer.getBoundingClientRect().top}px`);
  const [box] = mdBoxes(answer);
  assert.deepStrictEqual(box.children.filter((kid) => kid.nodeType === 1).map((kid) => kid.tagName),
    ["p", "ul", "p", "p"], "the landed message is not the server's rendering");
  assert.deepStrictEqual(chipsOf(answer), ["👀 1", "✅ 1"], "the landed message lost its reactions");
  // The run's line is drawn on its first row, which is still that message: its rendered body and its
  // reactions are behind the line (web/voice.css hides every child but the line), and come back with it.
  const head = rowWithId(page, runId(1));
  assert.equal(head.getAttribute("data-read-run"), "head");
  assert.equal(head.children[0], readRunLineOf(head), "the line is not the head row's first child");
  assert.equal(tagged(mdBoxes(head)[0], "strong").length, 1, "the run's first row lost its rendered body");
  assert.deepStrictEqual(chipsOf(head), ["👍 2"], "the run's first row lost its reactions");
  assert.match(CSS_CODE, /#discord-log li\.discord-message\[data-read-run="head"\] > :not\(\.read-run\) \{\s*display: none !important;/,
    "the line no longer hides the rest of its row, the reactions among it");
  await readRunLineOf(head).click();
  await page.settle();
  assert.equal(head.hasAttribute("data-read-run"), false, "the run did not open");
  assert.equal(readRunLineOf(head).hidden, true, "the line stayed up over the opened run");
  assert.deepStrictEqual(chipsOf(head), ["👍 2"]);
});

test("the Hide read button cycles Show read, Fold read and Hide read, names each, and keeps it", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog());
  assert.deepStrictEqual(readModeSaid(page), ["Show read", "false", "show"], "the default is not Show read");
  assert.equal(page.el("todo-filter").getAttribute("aria-label"), "Show read. Tap for Fold read");
  const steps = [
    [["Fold read", "mixed", "fold"], "fold", false, "Fold read. Tap for Hide read"],
    [["Hide read", "true", "hide"], "hide", true, "Hide read. Tap for Show read"],
    [["Show read", "false", "show"], "show", false, "Show read. Tap for Fold read"],
  ];
  for (const [said, read, todo, name] of steps) {
    await page.el("todo-filter").click();
    await page.settle();
    assert.deepStrictEqual(readModeSaid(page), said);
    assert.equal(page.el("todo-filter").getAttribute("aria-label"), name);
    assert.ok(page.el("todo-filter").getAttribute("title").length > 20, "the mode is not explained under the pointer");
    const saved = savedUiState(page);
    assert.equal(saved.read, read, "the mode was not kept");
    assert.equal(saved.todo, todo, "Hide read, as older pages read it, was not kept beside the mode");
  }
  // Kept across a reload, like Hide read always was.
  await page.el("todo-filter").click();
  await page.settle();
  const reopened = reloadWith(page.storage, backlog());
  await reopenedChannel(reopened);
  assert.deepStrictEqual(readModeSaid(reopened), ["Fold read", "mixed", "fold"], "a reload lost Fold read");
  // A record from before the modes says only Hide read or not, and means what it meant.
  for (const [todo, said] of [[true, ["Hide read", "true", "hide"]], [false, ["Show read", "false", "show"]]]) {
    const record = { ...savedUiState(reopened), todo };
    delete record.read;
    reopened.storage.set(UI_STATE_KEY, JSON.stringify(record));
    const older = reloadWith(reopened.storage, backlog());
    await reopenedChannel(older);
    assert.deepStrictEqual(readModeSaid(older), said, `a record with todo: ${todo} and no mode`);
  }
  // Each mode has its own icon, and the page first draws Show read's alone. (Which one is drawn
  // after each tap is measured in Chromium: tests/read_modes_browser.py.)
  for (const [mode, hidden] of [["show", ""], ["fold", " hidden"], ["hide", " hidden"]]) {
    assert.match(HTML_CODE, new RegExp(`class="control-icon read-mode-icon" data-mode="${mode}"${hidden} viewBox`),
      `the ${mode} icon is not drawn as the page starts`);
  }
  // The name is two words, and the desk's one-row dock stacks them, whole: a lone "Collapse" reads as
  // the act a tap performs. (This page was left on Fold read before the reloads. That the dock
  // stays one row is measured in Chromium.)
  const words = page.el("todo-filter-label").children.map((node) => [node.className, node.textContent]);
  assert.deepStrictEqual(words, [["read-mode-verb", "Fold"], ["read-mode-noun", " read"]]);
  const desk = "(min-width: 900px) and (pointer: fine)";
  for (const word of ["verb", "noun"]) {
    assert.match(cssBlockIn(desk, `#control-pane #todo-filter .read-mode-${word}`), /display:\s*block/,
      "a desk's dock does not stand the name in two lines");
  }
  assert.doesNotMatch(CSS_CODE, /\.read-mode-noun\s*\{\s*display:\s*none/, "a desk's dock cuts the name to its first word");
});

test("`#480 fold-one-message-key`: Shift+O folds one message away, marks it read, and steps above it, to the root", async () => {
  const page = newPage();
  const messages = runThread(page, { read: 2, tail: [["agent", false], ["agent", false]] });
  await enterRunThread(page, messages);
  const row = (n) => rowWithId(page, runId(n));
  const shiftO = async (target) => {
    const event = await press(page, target, "O", { shiftKey: true });
    await page.settle();
    await page.settle();
    return event;
  };
  // One unread reply folded away on its own, in Show read: a line of one, the selection above it.
  await shiftO(row(501));
  assert.equal(row(501).getAttribute("data-read-run"), "head", "one folded message drew no line");
  assert.equal(readRunLineOf(row(501)).textContent, "… 1 read message …");
  assert.equal(row(501).getAttribute("data-archived"), "true", "folding a message away did not mark it read");
  assert.equal(selectedId(page), runId(500), "the selection did not step to the message above");
  // Again and again from where the selection landed: adjacent folds merge into one counted line.
  await shiftO(focused(page));
  assert.equal(row(500).getAttribute("data-read-run"), "head");
  assert.equal(row(501).getAttribute("data-read-run"), "member", "adjacent folded messages did not merge");
  assert.equal(readRunLineOf(row(500)).textContent, "… 2 read messages …");
  assert.equal(selectedId(page), runId(2));
  await shiftO(focused(page));
  await shiftO(focused(page));
  assert.equal(readRunLineOf(row(1)).textContent, "… 4 read messages …", "the run did not grow to the root");
  assert.equal(selectedId(page), runId(0), "the selection did not arrive on the root");
  // The root stays, and the key says so rather than folding the thread's first message.
  await shiftO(focused(page));
  assert.equal(row(0).hasAttribute("data-read-run"), false, "the root was folded away");
  assert.equal(selectedId(page), runId(0));
  assert.equal(statusText(page), "The thread's first message stays.");
  // Enter on the line opens the run again, and a redraw keeps it open (nothing is folded now).
  await press(page, row(1), "Enter");
  await page.settle();
  assert.deepStrictEqual(collapsedRows(page), [], "Enter on the line did not open the run");
  // Folded away again, it survives the redraw a poll brings.
  await shiftO(row(501));
  assert.equal(row(501).getAttribute("data-read-run"), "head");
  await pollOnce(page);
  await page.settle();
  assert.equal(rowWithId(page, runId(501)).getAttribute("data-read-run"), "head",
    "a redraw unfolded what was folded away");
});

test("Fold read: a thread opens on its root, one line for the read run, and the first unread unfolded", async () => {
  const page = newPage();
  const messages = runThread(page, { tail: RUN_TAIL });
  await enterRunThread(page, messages, async () => {
    await chooseReadMode(page, "Fold read");
    // A viewport the root, the line and the start of the answer fit in, as they do on a phone in
    // Chromium. This fixture lays a row's meta line out as a column of five lines, and draws the
    // line's row with the message it hides; neither is so in a browser.
    page.el("scroll-area").clientHeight = 700;
  });
  const rows = page.el("discord-log").children;
  assert.equal(rows.length, 18, "Fold read took a message out of the list");
  assert.equal(rows[0].getAttribute("data-id"), runId(0));
  assert.equal(rows[0].hasAttribute("data-read-run"), false, "the root, read as it is, was collapsed");
  assert.equal(rows[0].hidden, false);
  assert.equal(rows[1].getAttribute("data-read-run"), "head");
  const line = readRunLineOf(rows[1]);
  assert.ok(line, "the run has no line");
  assert.equal(line.textContent, "… 12 read messages …");
  assert.equal(line.hidden, false);
  // It goes when it opens the run, so it never says it is open: an `aria-expanded` would stay "false".
  assert.equal(line.hasAttribute("aria-expanded"), false, "the line claims a state it never has");
  assert.equal(line.getAttribute("type"), "button");
  for (const row of rows.slice(2, 13)) {
    assert.equal(row.getAttribute("data-read-run"), "member");
    assert.equal(row.hidden, true, "a read message in the run is still drawn");
  }
  const answer = rows[13];
  assert.equal(answer.getAttribute("data-id"), runId(500));
  assert.equal(answer.hasAttribute("data-read-run"), false);
  assert.equal(answer.getAttribute("data-collapsed"), "false", "the first unread is folded");
  const area = page.el("scroll-area");
  const rootTop = rows[0].getBoundingClientRect().top;
  assert.ok(atTheHead(page, rows[0]), `the root is not at the head of the list: ${rootTop}px`);
  assert.ok(answer.getBoundingClientRect().top < area.clientHeight / 2, "the answer is not on screen below the root");
  // The folds under the line are not the list's: Expand all has nothing it could show.
  assert.equal(page.el("expand-all").hidden, true, "Expand all offered to open a message behind the line");

  // A tap on the line opens the run where it is: the root does not move. Pressed from the keyboard,
  // the focus goes to the run's first message, where the line was, and not back to the page's start.
  line.focusFrom("keyboard");
  await line.click();
  await page.settle();
  assert.deepStrictEqual(collapsedRows(page), [], "the run did not open");
  assert.ok(rows.every((row) => !row.hidden), "a message of the opened run is still hidden");
  assert.equal(line.hidden, true, "the line stayed over the run it opened");
  assert.equal(rows[0].getBoundingClientRect().top, rootTop, "opening the run moved the reader");
  assert.equal(page.document.activeElement, rows[1], "the focus was left on the line that went");
  assert.equal(rows[1].getAttribute("tabindex"), "-1", "the run's first message cannot take the focus");
  assert.equal(rows[1].focusedWith && rows[1].focusedWith.preventScroll, true, "taking the focus may scroll the reader");
  assert.equal(page.el("expand-all").hidden, false, "the long read message in the run cannot be opened");
  // Open stays open: a redraw does not collapse it again, nor does a message joining it.
  page.dealtWith.add(runId(500));
  await reReadChannel(page);
  assert.deepStrictEqual(collapsedRows(page), [], "a redraw collapsed the run the reader opened");
});

/**
 * A channel without threads: an unread message, one read message alone, then two read messages
 * the agent posted a second apart — one combined row — and another read one, an unread message,
 * one read message and the newest message, read.
 */
function splitRunChannel(page) {
  const at = (minutes, seconds = 0) => new Date(MESSAGE_EPOCH + minutes * 60000 + seconds * 1000).toISOString();
  const rows = [
    ["8800000000000000001", "agent", false, at(0)],
    ["8800000000000000002", "agent", true, at(1)],
    ["8800000000000000003", "unread", false, at(2)],
    ["8800000000000000004", "agent", true, at(3)],
    ["8800000000000000005", "agent", true, at(3, 1)],
    ["8800000000000000006", "own", true, at(4)],
    ["8800000000000000007", "unread", false, at(5)],
    ["8800000000000000008", "agent", true, at(6)],
    ["8800000000000000009", "own", true, at(7)],
  ];
  return rows.map(([id, who, read, timestamp]) => {
    if (read && who === "agent") page.dealtWith.add(id);
    return message({ id, timestamp, content: `message ${id.slice(-1)}`,
      ...(who === "own" ? OWN : who === "agent" ? CODER : { author: "alice", author_id: "30", author_is_bot: false }) });
  });
}

test("Fold read collapses runs of two rows or more, split by unread messages, never the newest", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, splitRunChannel(page));
  assert.equal(page.el("discord-log").children.length, 8, "the two messages a second apart were not combined");
  await chooseReadMode(page, "Fold read");
  const role = (id) => rowWithId(page, id).getAttribute("data-read-run");
  // One read message alone between two unread ones is shown: a line would save nothing.
  assert.equal(role("8800000000000000002"), null, "a single read message was collapsed");
  // The combined row and the read one after it: two rows, three messages.
  assert.equal(role("8800000000000000004"), "head");
  assert.equal(readRunLineOf(rowWithId(page, "8800000000000000004")).textContent, "… 3 read messages …",
    "the line counted rows, not the messages in them");
  assert.equal(role("8800000000000000006"), "member");
  assert.equal(role("8800000000000000003"), null, "an unread message was collapsed");
  assert.equal(role("8800000000000000007"), null, "an unread message was collapsed");
  // The newest message is where the conversation stands, and what the composer answers.
  assert.equal(role("8800000000000000009"), null, "the newest message was collapsed");
  assert.equal(role("8800000000000000008"), null, "a run of one, cut short by the newest, was collapsed");
  // A search, or Links, chooses rows: every match is shown, and closing it collapses them again.
  await search(page, "message");
  assert.deepStrictEqual(collapsedRows(page), [], "a search left matches behind a line");
  await page.el("search-toggle").click();
  await page.settle();
  assert.equal(role("8800000000000000004"), "head", "closing the search did not collapse the run again");
  // Show read and Fold read are one read of this channel, drawn two ways.
  const reads = page.pageReads;
  await chooseReadMode(page, "Hide read");
  await chooseReadMode(page, "Show read");
  assert.deepStrictEqual(collapsedRows(page), [], "Show read left a run collapsed");
  await chooseReadMode(page, "Fold read");
  assert.equal(role("8800000000000000004"), "head");
  assert.equal(page.pageReads, reads + 1, "Fold read read the channel again; only leaving Hide read should");
});

test("Fold read: a message marked Done joins the runs around it, and the reader does not move", async () => {
  const page = newPage();
  const messages = joiningChannel(page);
  await signIn(page);
  await showDiscord(page, messages);
  await chooseReadMode(page, "Fold read");
  const head = (id) => readRunLineOf(rowWithId(page, id));
  assert.equal(head(messages[1].id).textContent, "… 2 read messages …");
  assert.equal(head(messages[4].id).textContent, "… 2 read messages …");
  // The reader is on the sixth message, at the top of the screen, below both runs.
  const area = page.el("scroll-area");
  const reading = () => rowWithId(page, messages[6].id);
  area.scrollTop += reading().getBoundingClientRect().top;
  assert.equal(reading().getBoundingClientRect().top, 0, "the fixture cannot put the reader there");
  await doneButton(rowWithId(page, messages[3].id)).click();
  await page.settle();
  assert.equal(head(messages[1].id).textContent, "… 5 read messages …", "the runs did not join round it");
  assert.equal(rowWithId(page, messages[3].id).hidden, true, "the message dealt with is still drawn");
  assert.equal(reading().getBoundingClientRect().top, 0, "joining the runs moved the reader");
  // Put back, it splits them again, where the reader is.
  await doneButton(rowWithId(page, messages[3].id)).click();
  await page.settle();
  assert.equal(head(messages[1].id).textContent, "… 2 read messages …", "the run did not split round it");
  assert.equal(reading().getBoundingClientRect().top, 0, "splitting the run moved the reader");
});

test("Fold read: a run opened from the keyboard hands the focus to its first message, and one tapped open does not", async () => {
  // The line goes as it opens the run. A keyboard left on it would fall back to the start of the page.
  // A tap or a click focuses the line as well, but a row holding the focus is lit as under the
  // pointer, and a read message must not be drawn as though it were not: that focus stays put.
  const page = newPage();
  const messages = joiningChannel(page);
  await signIn(page);
  await showDiscord(page, messages);
  await chooseReadMode(page, "Fold read");
  const tapped = readRunLineOf(rowWithId(page, messages[1].id));
  tapped.focusFrom("pointer");
  await tapped.click();
  await page.settle();
  assert.equal(rowWithId(page, messages[1].id).hasAttribute("data-read-run"), false, "the tap did not open the run");
  assert.equal(page.document.activeElement, tapped, "a tap moved the focus onto a read message");
  assert.equal(rowWithId(page, messages[1].id).hasAttribute("tabindex"), false);
  const pressed = readRunLineOf(rowWithId(page, messages[4].id));
  pressed.focusFrom("keyboard");
  await pressed.click();
  await page.settle();
  assert.equal(rowWithId(page, messages[4].id).hasAttribute("data-read-run"), false, "the key did not open the run");
  assert.equal(page.document.activeElement, rowWithId(page, messages[4].id),
    "the keyboard was left on the line that went, not the run's first message");
});

test("...and a read that finds a message read elsewhere grows the run, without moving the reader", async () => {
  const page = newPage();
  const messages = joiningChannel(page);
  await signIn(page);
  await showDiscord(page, messages);
  await chooseReadMode(page, "Fold read");
  const area = page.el("scroll-area");
  const reading = () => rowWithId(page, messages[6].id);
  area.scrollTop += reading().getBoundingClientRect().top;
  // Dealt with on another device; the poll reads the channel again.
  page.dealtWith.add(messages[3].id);
  await reReadChannel(page);
  assert.equal(readRunLineOf(rowWithId(page, messages[1].id)).textContent, "… 5 read messages …",
    "a read did not grow the run");
  assert.equal(reading().getBoundingClientRect().top, 0, "a read that grew a run moved the reader");
  // ...and one put back elsewhere shrinks it.
  page.dealtWith.delete(messages[2].id);
  await reReadChannel(page);
  assert.equal(rowWithId(page, messages[1].id).hasAttribute("data-read-run"), false, "a run of one stayed collapsed");
  assert.equal(readRunLineOf(rowWithId(page, messages[3].id)).textContent, "… 3 read messages …");
  assert.equal(reading().getBoundingClientRect().top, 0, "a read that shrank a run moved the reader");
});

test("...and the reader's own words put back split the run they were in, with no read and no filter; the setting moves nothing", async () => {
  // A channel without threads re-derives the rows it holds: nothing is read and no filter changes,
  // so the pass over the rows is the only thing that can decide the runs again.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, splitRunChannel(page));
  await chooseReadMode(page, "Fold read");
  const role = (id) => rowWithId(page, id).getAttribute("data-read-run");
  assert.equal(role("8800000000000000004"), "head");
  assert.equal(role("8800000000000000006"), "member");
  const reads = page.pageReads;
  // `#549 own-read-stored`: the setting decides where messages first seen afterwards start; the run
  // drawn from what was already seen stays as it is.
  await page.el("mark-own-read").setChecked(false);
  await page.settle();
  assert.equal(page.pageReads, reads, "the setting read the channel again");
  assert.equal(role("8800000000000000006"), "member", "the setting put the reader's own message back");
  // Put back by hand, as any message is.
  await page.pure("toggleArchived")(["8800000000000000006"], rowWithId(page, "8800000000000000006"));
  await page.settle();
  assert.equal(page.pageReads, reads, "the premise: the put back redrew what the page holds, without a read");
  assert.equal(role("8800000000000000006"), null, "the reader's own message, unread now, is hidden in the run");
  assert.equal(rowWithId(page, "8800000000000000006").hidden, false);
  assert.equal(role("8800000000000000004"), null, "what is left of the run, one row, stayed collapsed");
});

test("Fold read: a jump to a message inside a run opens the run; Pinned and Hide read are as they were", async () => {
  const page = newPage();
  const messages = joiningChannel(page);
  messages[13] = { ...messages[13], reply_to: messages[5].id };
  page.pinned.set(messages[1].id, storedPin(messages[1]));
  page.servePinsRevision = true;
  page.pinsRevision = 3;
  await signIn(page);
  await showDiscord(page, messages);
  await chooseReadMode(page, "Fold read");
  assert.equal(rowWithId(page, messages[5].id).getAttribute("data-read-run"), "member");
  await tapArrow(page, rowWithId(page, messages[13].id));
  const answered = rowWithId(page, messages[5].id);
  assert.equal(answered.hidden, false, "the jump landed on a message hidden in a run");
  assert.equal(answered.hasAttribute("data-read-run"), false, "the run the jump went into stayed collapsed");
  assert.equal(answered.getAttribute("data-landed"), "true", "the message the reply answers was not lit");
  assert.equal(rowWithId(page, messages[1].id).getAttribute("data-read-run"), "head",
    "the jump opened a run it did not go into");
  // A pin outranks every read mode: the Pinned list is never collapsed.
  await showPinned(page);
  assert.deepStrictEqual(pinnedIds(page), [messages[1].id]);
  assert.ok(page.el("pinned-log").children.every((li) => !li.hasAttribute("data-read-run") && !li.hidden));
  await page.el("search-toggle").click();
  await page.settle();
  // Hide read, as before: what is read is left out, and nothing is collapsed.
  await chooseReadMode(page, "Hide read");
  assert.deepStrictEqual(shownIds(page), [0, 3, 6, 7, 8, 9, 10, 11, 12, 13].map((i) => messages[i].id));
  assert.deepStrictEqual(collapsedRows(page), []);
});

test("Hide read: entering a thread lands on its first unread message too", async () => {
  const page = newPage();
  const messages = runThread(page, { tail: [["agent", false]] });
  await enterRunThread(page, messages, () => chooseReadMode(page, "Hide read"));
  assert.deepStrictEqual(shownIds(page), [runId(500), runId(501)], "Hide read showed a read message");
  assert.equal(rowWithId(page, runId(500)).getAttribute("data-collapsed"), "false", "the first unread is folded");
});

// `#324 live-pill-covers-row`. On a 412px phone entering a thread landed its root's TILE just under the
// floating line, and the date rule the row draws above its tile went under "Live · updated". A row is
// landed by where it starts (`drawnTop`): its day rule, a gap's line or the bookmark's line included.
test("...and lands the first unread's day rule under the floating line with it, never under the pill", async () => {
  const land = async (script) => {
    const page = newPage(new Map(), script);
    const messages = runThread(page, { tail: [["agent", false]] });
    await enterRunThread(page, messages, () => chooseReadMode(page, "Hide read"));
    const answer = rowWithId(page, runId(500));
    const rule = answer.children.find((kid) => kid.hasClass("row-seam") && !kid.hasClass("row-seam-below"));
    return { seam: answer.getAttribute("data-seam"), rule: rule ? rule.getBoundingClientRect().top : NaN,
      tile: answer.getBoundingClientRect().top, line: floatingLine(page) };
  };
  const real = await land(SCRIPT);
  assert.equal(real.seam, "day", "the premise: the first unread row draws a day rule above its tile");
  assert.ok(real.rule >= real.line && real.rule <= real.line + LINE_PX,
    `the day rule landed at ${real.rule}px, not just under the floating line at ${real.line}px`);
  assert.ok(real.tile > real.rule, "the tile is not under its rule");
  // THE CONTROL: the landing measured from the tile, as it was.
  const control = await land(brokenScript(
    "  area.scrollTop += drawnTop(row) - floatingClearance(area) - REPLY_LANDING_GAP_PX;\n  entryState().landing =",
    "  area.scrollTop += row.getBoundingClientRect().top - floatingClearance(area) - REPLY_LANDING_GAP_PX;\n" +
      "  entryState().landing ="));
  assert.ok(control.rule < control.line,
    "the control left the day rule clear of the floating line too, so the check above proves nothing");
  assert.ok(Math.abs(control.tile - real.rule) <= 1, "the control did not land the tile where the rule now lands");
});

test("...and the dock and the chips changing as the thread opens leave the first unread where the entry put it", async () => {
  // On a desk the picker takes the thread's name as the thread opens and the dock wraps onto a second
  // row (`#218 desktop-dock`); opening the message's fold can put "Collapse all" up under the list. A
  // browser lays both out before the landing measures, and reports them AFTER it, to observers that
  // keep a reader within the slack of the newest line on it. A short thread in Hide read leaves the
  // landing that near the end on purpose, and the dock's observer took the message up under the pill.
  const GROWTH = 42;
  const run = async (script) => {
    const page = newPage(new Map(), script, (p) => p.enableResizeObserver());
    const messages = runThread(page, { tail: [["agent", false]] });
    const area = page.el("scroll-area");
    const dock = page.el("dock");
    await enterRunThread(page, messages, async () => {
      await chooseReadMode(page, "Hide read");
      // The dock as All had it, reported. Then the thread's name wraps it: laid out, and so measured
      // by the landing, but not yet reported to anything watching the list. (498px: the answer's day
      // rule is landed under the floating line with it since `#324 live-pill-covers-row`, 28px further
      // from the end, so the list is that much taller for the landing to stay as near its end.)
      area.clientHeight = 498;
      dock.offsetHeight = 50;
      page.resized(area);
      dock.offsetHeight += GROWTH;
      area.clientHeight -= GROWTH;
    });
    const answer = rowWithId(page, runId(500));
    const landed = atTheHead(page, answer);
    const short = area.scrollHeight - area.scrollTop - area.clientHeight;
    page.resized(area);
    const afterDock = atTheHead(page, answer);
    page.resized(page.el("scroll-tools"));
    const afterChips = atTheHead(page, answer);
    // Once the reader has moved, the landing is spent: on the newest line, the dock growing again
    // keeps them there, as it always has.
    await scrollToGap(page, 0);
    dock.offsetHeight += GROWTH;
    area.clientHeight -= GROWTH;
    page.resized(area);
    return { landed, nearTheEnd: short > 0 && short <= BOTTOM_SLACK_PX, afterDock, afterChips,
      newestKept: atBottomOf(area) };
  };
  const real = await run(SCRIPT);
  assert.equal(real.landed, true, "the entry did not land the answer at the head");
  assert.equal(real.nearTheEnd, true, "the premise: the landing is within the slack of the list's end");
  assert.equal(real.afterDock, true, "the dock wrapping took the landed answer up under the floating line");
  assert.equal(real.afterChips, true, "the chips changing took the landed answer up under the floating line");
  assert.equal(real.newestKept, true, "a reader who went down to the newest line was not kept on it");

  // THE CONTROLS: either observer reading the landing as a reader on the newest line.
  const dockOnly = await run(brokenScript(
    "    if (entryLandingHolds()) {\n      placeEntryLanding(entryState().landing.row, entryState().landing.first);\n      return;\n    }\n", ""));
  assert.deepStrictEqual(dockOnly, { ...real, afterDock: false, afterChips: false });
  const chipsOnly = await run(brokenScript(
    "const landed = currentScreen === \"main\" && currentView === \"discord\" && entryLandingHolds();",
    "const landed = false;", 2));
  assert.deepStrictEqual(chipsOnly, { ...real, afterChips: false });
});

test("a collapsed run's line is the row's only face, a thumb tall, and not something a swipe can archive", () => {
  const head = cssBlock('#discord-log li.discord-message[data-read-run="head"] > :not(.read-run)');
  assert.match(head, /display:\s*none !important/, "the message shows through its run's line");
  const box = cssBlock('#discord-log li.discord-message[data-read-run="head"]');
  for (const property of ["background: none", "opacity: 1", "filter: none", "margin-left: 0", "padding: 0"]) {
    assert.ok(box.includes(`${property} !important`), `the line wears the row's ${property.split(":")[0]}`);
  }
  assert.match(cssBlock('#discord-log li.discord-message[data-read-run="head"]::after'), /content:\s*none !important/);
  const line = cssBlock("#discord-log .read-run");
  assert.match(line, /min-height:\s*2\.75rem/, "the line is not a 44px target");
  assert.match(line, /width:\s*100%/);
  // Its row's selection is drawn on the line, which gives up the row's tile and ring. `#239 single-selection`.
  const selected = cssBlock('#discord-log li.discord-message[data-read-run="head"][data-selected="true"] > .read-run');
  assert.match(selected, /color:\s*var\(--ink\)/);
  assert.match(selected, /outline:\s*2px solid var\(--accent\)/, "a selected run's line has no ring");
  // A finger on the line presses the line: it is neither a swipe to Done nor a hold for details.
  assert.match(SCRIPT_CODE, /if \(li\.getAttribute\("data-read-run"\) === "head"\) \{\s*return;\s*\}/);
});

test("...and the message the reader is on, marked Done, is held where it was by the line it joins", async () => {
  // A channel with threads is redrawn from its store on a Done; one without keeps its rows and
  // re-derives them. Both have to leave the reader looking at the run that took their message.
  for (const threaded of [true, false]) {
    const page = newPage();
    const messages = joiningChannel(page);
    page.threadingSupported = threaded;
    await signIn(page);
    await showDiscord(page, messages);
    await chooseReadMode(page, "Fold read");
    const area = page.el("scroll-area");
    area.scrollTop += rowWithId(page, messages[3].id).getBoundingClientRect().top;
    assert.equal(rowWithId(page, messages[3].id).getBoundingClientRect().top, 0, "the fixture cannot put the reader there");
    const below = rowWithId(page, messages[6].id).getBoundingClientRect().top;
    await doneButton(rowWithId(page, messages[3].id)).click();
    await page.settle();
    const line = rowWithId(page, messages[1].id);
    assert.equal(readRunLineOf(line).textContent, "… 5 read messages …", `threaded ${threaded}: the runs did not join`);
    assert.equal(line.getBoundingClientRect().top, 0,
      `threaded ${threaded}: the line that took the reader's message is not where the message was`);
    assert.ok(rowWithId(page, messages[6].id).getBoundingClientRect().top < below,
      `threaded ${threaded}: what follows did not come up into the room the run gave back`);
  }
});

test("Fold read and gathered replies: a reply below a run's line is not drawn as joined to it, and a gathered stack stays whole", async () => {
  // A reply whose message is behind a line points at nothing drawn: its arrow is the disconnected one.
  const page = newPage();
  page.dealtWith.add("1000000000000000001");
  page.dealtWith.add("1000000000000000002");
  await signIn(page);
  await showDiscord(page, [
    message({ id: "1000000000000000001", content: "is the runner wedged?" }),
    message({ id: "1000000000000000002", content: "and the queue?" }),
    message({ id: "1000000000000000003", content: "restarted it", reply_to: "1000000000000000001", ...ALICE }),
    message({ id: "1000000000000000004", content: "draining now", reply_to: "1000000000000000003", ...ALICE }),
    message({ id: "1000000000000000005", content: "thanks" }),
  ]);
  await chooseReadMode(page, "Fold read");
  assert.equal(rowWithId(page, "1000000000000000001").getAttribute("data-read-run"), "head");
  assert.equal(arrowStyle(rowWithId(page, "1000000000000000003")), "disconnected",
    "the reply's arrow joins the line as though the line were the message it answers");
  assert.equal(arrowStyle(rowWithId(page, "1000000000000000004")), "adjacent", "an arrow between drawn rows changed");
  await chooseReadMode(page, "Hide read");
  await chooseReadMode(page, "Show read");
  assert.equal(arrowStyle(rowWithId(page, "1000000000000000003")), "disconnected");

  // Replies the reader gathered under their message are read together, read or not.
  const gathered = await scatteredPage();
  for (const id of ["597", "598", "599", "602", "603"]) gathered.dealtWith.add(id);
  await reReadChannel(gathered);
  await gatherChip(gathered, "601").click();
  await gathered.settle();
  assert.deepStrictEqual(shownIds(gathered), GATHERED);
  await chooseReadMode(gathered, "Fold read");
  assert.equal(runCountWords(readRunLineOf(rowWithId(gathered, "597"))), "… 3 read messages …",
    "the read messages outside the stack were not collapsed");
  for (const id of ["602", "603"]) {
    assert.equal(rowWithId(gathered, id).hasAttribute("data-read-run"), false, `gathered reply ${id} was collapsed`);
  }
  assert.deepStrictEqual(stackPlaces(gathered).map(([id]) => id), ["601", "602", "603", "604", "606", "607"],
    "Fold read broke the gathered stack");
});

test("ON MAIN a root whose thread holds unread replies says how many, beside N replies; none says nothing", async () => {
  const page = newPage();
  const messages = inboxChannel(page);
  await signIn(page);
  await showDiscord(page, messages);
  // In All the replies are rows of the list, and speak for themselves.
  assert.ok(page.el("discord-log").children.every((li) => !unreadChip(page, li.getAttribute("data-id"))),
    "All drew an unread count");
  const reads = page.timelineCalls.length;
  await pickThread(page, "main");
  assert.equal(page.timelineCalls.length, reads, "counting the replies cost Main a read");
  assert.deepStrictEqual(shownIds(page), [1, 2, 5, 10, 7, 12].map(inboxId));
  // The owner's own reply (04) is read, as his words are; the placeholder (11) is read automatically.
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: null, 7: "2 unread" });
  const chip = unreadChip(page, inboxId(7));
  assert.equal(chip.tagName, "button");
  assert.equal(chip.getAttribute("aria-label"), "Open this thread at its first unread reply, 2 unread replies");
  assert.equal(unreadChip(page, inboxId(2)).getAttribute("aria-label"),
    "Open this thread at its first unread reply, 1 unread reply");
  assert.equal(chip.getAttribute("title"), "Open this thread at its first unread reply");
  // Beside N replies, after it, as the last thing on the row: "3 replies  2 unread".
  const root = rowWithId(page, inboxId(7));
  const order = root.children.map((node) => node.className);
  assert.deepStrictEqual(order.slice(-2), ["thread-replies", "thread-unread"], `not beside N replies: ${order}`);
  assert.equal(threadButton(root).textContent, "3 replies");
  assert.equal(threadBadge(root).textContent, "Thread(3)", "the way into the thread changed its words");
  assert.equal(root.getAttribute("data-unread-replies"), "2");
  assert.equal(rowWithId(page, inboxId(5)).hasAttribute("data-unread-replies"), false);
  // The root is the owner's own words, read, and is drawn so.
  assert.equal(root.getAttribute("data-archived"), "true", "the root is no longer drawn as read");
  // The cue is the accent, as the jump to news below wears it, and is not faded with its read root.
  assert.match(cssBlock(".thread-unread"), /color:\s*var\(--accent\)/);
  assert.match(cssBlock(".thread-unread"), /border:\s*1px solid var\(--accent\)/);
  assert.match(cssBlock("#discord-log li.discord-message[data-unread-replies] > .thread-unread:not(.thread-strip):not(.reply-jump):not(.reply-bridge):not(.reply-ungather)"),
    /z-index:\s*1[\s\S]*filter:\s*none/);
  assert.match(cssBlock("#discord-log li.discord-message[data-unread-replies] > .thread-replies"), /display:\s*inline-block/);
  // `#549 own-read-stored`: turning the setting off changes no count; his reply's read state is kept.
  await page.el("mark-own-read").setChecked(false);
  await page.settle();
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: null, 7: "2 unread" });
  // ...and back in All, no count again.
  await page.el("mark-own-read").setChecked(true);
  await page.settle();
  await pickThread(page, "flat");
  assert.equal(unreadChip(page, inboxId(7)), null, "the count outlived Main");
});

test("HIDE READ keeps a root whose thread holds unread replies, drawn as read; COLLAPSE READ never folds it", async () => {
  const page = newPage();
  await inboxOnMain(page, inboxChannel(page));
  await chooseReadMode(page, "Fold read");
  // Read, all of them, but 02 and 07 are kept: only 05 and 10 run together, and 01 is a run of one.
  assert.equal(rowWithId(page, inboxId(5)).getAttribute("data-read-run"), "head");
  assert.equal(readRunLineOf(rowWithId(page, inboxId(5))).textContent, "… 2 read messages …");
  for (const n of [1, 2, 7]) {
    assert.equal(rowWithId(page, inboxId(n)).hasAttribute("data-read-run"), false, `${inboxId(n)} was collapsed`);
  }
  await chooseReadMode(page, "Hide read");
  assert.deepStrictEqual(shownIds(page), [2, 7, 12].map(inboxId), "Hide read hid a root with unread replies");
  assert.equal(rowWithId(page, inboxId(2)).getAttribute("data-archived"), "true", "the kept root is drawn as unread");
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 7: "2 unread" });
  // `#549 own-read-stored`: turning the setting off puts none of his words back.
  await page.el("mark-own-read").setChecked(false);
  await page.settle();
  assert.deepStrictEqual(shownIds(page), [2, 7, 12].map(inboxId), "turning the setting off put his read words back");
});

test("DONE HERE: a reply marked Done in All or in its thread takes its count down on Main, and Hide read lets the root go", async () => {
  const page = newPage();
  await inboxOnMain(page, inboxChannel(page));
  await chooseReadMode(page, "Hide read");
  assert.deepStrictEqual(shownIds(page), [2, 7, 12].map(inboxId));
  // In All, the agent's answer to question A, dealt with.
  await pickThread(page, "flat");
  await doneButton(rowWithId(page, inboxId(3))).click();
  await page.settle();
  assert.ok(page.dismissCalls.some((body) => (body.messages || []).includes(inboxId(3))), "the Done was not sent");
  const reads = page.timelineCalls.length;
  await pickThread(page, "main");
  assert.equal(page.timelineCalls.length, reads, "coming back to Main read the channel");
  assert.deepStrictEqual(shownIds(page), [7, 12].map(inboxId), "question A stayed with nothing unread under it");
  // In its thread, one of question C's two answers: back on Main the count is one.
  await unreadChip(page, inboxId(7)).click();
  await page.settle();
  assert.equal(page.el("thread-select").value, `thread:${INBOX.c}`, "the count did not open its thread");
  await doneButton(rowWithId(page, inboxId(8))).click();
  await page.settle();
  await pickThread(page, "main");
  assert.deepStrictEqual(unreadSaid(page), { 7: "1 unread" });
  // Put back from All: question A is back with it.
  await pickThread(page, "flat");
  await chooseReadMode(page, "Show read");
  await doneButton(rowWithId(page, inboxId(3))).click();
  await page.settle();
  await pickThread(page, "main");
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: null, 7: "1 unread" });
});

test("DONE ELSEWHERE: Main's own read says which replies were dealt with or put back on another device", async () => {
  const page = newPage();
  await inboxOnMain(page, inboxChannel(page));
  await chooseReadMode(page, "Hide read");
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 7: "2 unread" });
  const views = () => page.timelineCalls.map((call) => new URL(call, "http://fixture.test").searchParams.get("view"));
  const before = views().length;
  page.dealtWith.add(inboxId(3));
  page.dealtWith.add(inboxId(9));
  await reReadChannel(page);
  assert.deepStrictEqual(views().slice(before), ["main"], "learning it read more than Main");
  assert.deepStrictEqual(shownIds(page), [7, 12].map(inboxId), "a root whose reply was dealt with elsewhere stayed");
  assert.deepStrictEqual(unreadSaid(page), { 7: "1 unread" });
  // Put back elsewhere: the list is complete from its oldest row up, so the absence says so.
  page.dealtWith.delete(inboxId(3));
  await reReadChannel(page);
  assert.deepStrictEqual(shownIds(page), [2, 7, 12].map(inboxId));
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 7: "1 unread" });
  // The control: a server that sends only the rows' own marks cannot tell Main.
  const served = page.timeline;
  page.timeline = async (path) => {
    const answer = await served(path);
    const body = JSON.parse(await answer.text());
    delete body.reply_dismissals;
    return json(200, body);
  };
  page.dealtWith.add(inboxId(3));
  await reReadChannel(page);
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 7: "1 unread" },
    "without reply_dismissals the read changed the count, so this proves nothing");
});

test("A REPLY ARRIVING counts on its root at once and brings it back under Hide read; the owner's own adds no unread", async () => {
  const page = newPage();
  const messages = inboxChannel(page);
  await inboxOnMain(page, messages);
  await chooseReadMode(page, "Hide read");
  assert.deepStrictEqual(shownIds(page), [2, 7, 12].map(inboxId));
  // The read an arrival asks for is held: the count is the arrival's own doing, at once.
  const reads = gate(page.timeline);
  page.timeline = reads.respond;
  const b = { ...messages[4].thread, is_root: false };
  const answer = message({ id: inboxId(13), content: "one page failed to render after all", thread: b, ...CODER });
  page.messages = [...page.messages, answer];
  await deliver(page, page.stream(), sseMessage(answer));
  assert.deepStrictEqual(shownIds(page), [2, 5, 7, 12].map(inboxId), "the arrival did not bring question B back");
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: "1 unread", 7: "2 unread" });
  // Held now: two replies, though the root was last read with one.
  assert.equal(threadBadge(rowWithId(page, inboxId(5))).textContent, "Thread(2)");
  assert.equal(threadButton(rowWithId(page, inboxId(5))).textContent, "2 replies");
  // The owner answers in question C's thread from another device: one more reply, no more unread.
  const c = { ...messages[7].thread, is_root: false };
  const own = message({ id: inboxId(14), content: "thanks, ship it", thread: c, ...OWN });
  page.messages = [...page.messages, own];
  await deliver(page, page.stream(), sseMessage(own));
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: "1 unread", 7: "2 unread" });
  assert.equal(threadButton(rowWithId(page, inboxId(7))).textContent, "4 replies");
  reads.open();
  await page.settle();
  await page.settle();
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: "1 unread", 7: "2 unread" }, "the read behind the arrivals changed the counts");
});

test("FROM THE COUNT, the thread opens on its first unread reply, unfolded, in every read mode", async () => {
  for (const mode of ["Show read", "Fold read", "Hide read"]) {
    const page = newPage();
    const messages = runThread(page, { tail: RUN_TAIL });
    await signIn(page);
    await showDiscord(page, messages);
    await pickThread(page, "main");
    await chooseReadMode(page, mode);
    // As the Fold read entry test has it: the root, the line and the start of the answer fit.
    if (mode === "Fold read") page.el("scroll-area").clientHeight = 700;
    // The agent's answer and the two after it; the owner's own and the swiped ones are read.
    const chip = unreadChip(page, runId(0));
    assert.ok(chip, `${mode}: the owner's question shows no unread count`);
    assert.equal(chip.textContent, "3 unread", mode);
    await chip.click();
    await page.settle();
    assert.equal(page.el("thread-select").value, `thread:${RUN_THREAD}`, `${mode}: the count did not open the thread`);
    const answer = rowWithId(page, runId(500));
    assert.equal(answer.getAttribute("data-collapsed"), "false", `${mode}: the first unread reply is folded`);
    if (mode === "Fold read") {
      // The root at the head, the read run one line under it, and the answer below in view.
      const root = rowWithId(page, runId(0));
      assert.ok(atTheHead(page, root), `${mode}: the root is at ${root.getBoundingClientRect().top}px, not the head`);
      assert.equal(readRunLineOf(rowWithId(page, runId(1))).textContent, "… 12 read messages …");
      const top = answer.getBoundingClientRect().top;
      assert.ok(top > root.getBoundingClientRect().top && top < page.el("scroll-area").clientHeight,
        `${mode}: the first unread reply is at ${top}px, out of view`);
    } else {
      assert.ok(atTheHead(page, answer), `${mode}: the first unread reply landed at ${answer.getBoundingClientRect().top}px`);
    }
  }
});

test("A THREAD NOT ALL HELD counts what is held, as at least that many, and exactly once its thread is read", async () => {
  // Reopened in Main with no snapshot: only Main was read, so the page holds no reply at all — and
  // the read of All it asks for behind Main, for these counts, is still on its way (`holdCountsRead`;
  // landed, it makes them exact at once: see "A COLD OPEN IN MAIN" below).
  const first = newPage();
  await inboxOnMain(first, inboxChannel(first));
  first.storage.delete(MESSAGE_CACHE_KEY);
  const arrange = (p) => {
    inboxChannel(p);
    holdCountsRead(p);
  };
  const page = reloadWith(first.storage, inboxChannel(newPage()), arrange);
  await reopenedChannel(page);
  await page.settle();
  assert.equal(page.el("thread-select").value, "main");
  assert.deepStrictEqual(page.timelineCalls.map((call) => new URL(call, "http://fixture.test").searchParams.get("view")),
    ["main"], "the reopen read more than Main");
  assert.equal(page.countsReads.length, 1, "Main's read did not ask for All behind it");
  // Nothing held, but Main's read says each root has replies, and nothing has said they are read:
  // each says how many the page has not brought, and Hide read keeps them all — question B too,
  // whose one reply is Done, because keeping a read root errs the safe way for an inbox and hiding an
  // answered one is the failure this is for. (It was: before review, this said nothing and Hide read
  // kept only 12, hiding questions A and C with their unread answers.)
  assert.deepStrictEqual(unreadSaid(page), { 2: "2 new", 5: "1 new", 7: "3 new" });
  assert.equal(unreadChip(page, inboxId(5)).getAttribute("aria-label"),
    "Open this thread at its first unread reply, 1 new reply not loaded yet");
  await chooseReadMode(page, "Hide read");
  assert.deepStrictEqual(shownIds(page), [2, 5, 7, 12].map(inboxId), "Hide read hid a root whose replies are not known to be read");
  // One reply of question C's three comes in on the stream: held, unread, and the rest unknown.
  const reply = page.messages.find((m) => m.id === inboxId(9));
  await deliver(page, page.stream(), sseMessage(reply));
  assert.deepStrictEqual(shownIds(page), [2, 5, 7, 12].map(inboxId));
  const chip = unreadChip(page, inboxId(7));
  assert.equal(chip.textContent, "1+ unread", "a count of what is held claimed to be the whole");
  assert.equal(chip.getAttribute("aria-label"), "Open this thread at its first unread reply, at least 1 unread reply");
  // Its thread read for itself holds every reply: the count is exact from then on.
  await chip.click();
  await page.settle();
  assert.equal(page.el("thread-select").value, `thread:${INBOX.c}`);
  await pickThread(page, "main");
  assert.deepStrictEqual(unreadSaid(page), { 2: "2 new", 5: "1 new", 7: "2 unread" });
  // And All, read once, makes every thread exact: B's one reply is Done, and B goes.
  await pickThread(page, "flat");
  await page.settle();
  await pickThread(page, "main");
  assert.deepStrictEqual(shownIds(page), [2, 7, 12].map(inboxId));
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 7: "2 unread" });
});

/** Which views the page has read, in order. */
const viewsRead = (page) => page.timelineCalls.map((call) => new URL(call, "http://fixture.test").searchParams.get("view"));

/**
 * The agent answers question B again and question C a third time, and nothing brings the answers:
 * the stream has not brought them (Slack's brings a reply a poll late, `#307 slack-reply-live`), so
 * only Main's read sees them, in its roots' counts.
 * Answers the two new replies.
 */
function answeredUnseen(page, messages) {
  const b = { ...messages[4].thread, reply_count: 2 };
  const c = { ...messages[7].thread, reply_count: 4 };
  const newB = message({ id: inboxId(13), content: "one page failed after all", thread: { ...b, is_root: false }, ...CODER });
  const newC = message({ id: inboxId(14), content: "the fix merged", thread: { ...c, is_root: false }, ...CODER });
  page.messages = [...messages.map((m) => (m.id === inboxId(5) ? { ...m, thread: b } : m.id === inboxId(7) ? { ...m, thread: c } : m)),
    newB, newC];
  page.threads = page.threads.map((t) => (t.id === b.id ? { ...t, reply_count: 2 } : t.id === c.id ? { ...t, reply_count: 4 } : t));
  return [newB, newC];
}

test("A REPLY ONLY MAIN'S READ REPORTS, which nothing brought, counts as new on its root and keeps it; exact once All brings it", async () => {
  const page = newPage();
  const messages = inboxChannel(page);
  await inboxOnMain(page, messages);
  await chooseReadMode(page, "Hide read");
  assert.deepStrictEqual(shownIds(page), [2, 7, 12].map(inboxId));
  const [newB] = answeredUnseen(page, messages);
  const before = viewsRead(page).length;
  await reReadChannel(page);
  assert.deepStrictEqual(viewsRead(page).slice(before), ["main"], "learning of the answers read more than Main");
  // B's one held reply is Done, and Main says it has two: one the page has not brought, which nothing
  // says is read. C holds two unread and is reported a fourth: at least two unread, not exactly two.
  assert.deepStrictEqual(shownIds(page), [2, 5, 7, 12].map(inboxId), "Hide read hid a root with an answer it had not brought");
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: "1 new", 7: "2+ unread" });
  assert.equal(unreadChip(page, inboxId(5)).getAttribute("aria-label"),
    "Open this thread at its first unread reply, 1 new reply not loaded yet");
  assert.equal(unreadChip(page, inboxId(7)).getAttribute("aria-label"),
    "Open this thread at its first unread reply, at least 2 unread replies");
  const root = rowWithId(page, inboxId(5));
  assert.equal(threadBadge(root).textContent, "Thread(2)");
  assert.equal(threadButton(root).textContent, "2 replies");
  assert.equal(threadBadge(rowWithId(page, inboxId(7))).textContent, "Thread(4)");
  // Fold read does not fold it into a run either: 10 is a run of one now, beside it.
  await chooseReadMode(page, "Fold read");
  assert.equal(root.hasAttribute("data-read-run"), false, "Fold read folded a root with an answer it had not brought");
  assert.equal(rowWithId(page, inboxId(5)).getAttribute("data-unread-replies"), "1");
  // All, read, brings both answers: the counts are exact, with no read of Main.
  await pickThread(page, "flat");
  await reReadChannel(page);
  const reads = page.timelineCalls.length;
  await pickThread(page, "main");
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: "1 unread", 7: "3 unread" });
  assert.deepStrictEqual(page.timelineCalls.slice(reads), [], "Main was read for what All's read had answered");
  // Dealt with in All, B's answer takes its root with it under Hide read.
  await pickThread(page, "flat");
  await chooseReadMode(page, "Show read");
  await doneButton(rowWithId(page, newB.id)).click();
  await page.settle();
  await pickThread(page, "main");
  await chooseReadMode(page, "Hide read");
  assert.deepStrictEqual(shownIds(page), [2, 7, 12].map(inboxId));
});

test("SLACK-SHAPED: the owner's question whose first answer only Main's poll reports is kept, with nothing on the stream", async () => {
  // Slack ids, and no thread record on the question until it has a reply, as Slack sends one.
  const page = newPage();
  const threadIds = { a: "1700000000.000200", b: "1700000000.000500", c: "1700000000.000700" };
  const messages = inboxChannel(page, threadIds);
  const asked = message({ id: inboxId(15), content: "can you check the nightly?", ...OWN });
  await inboxOnMain(page, [...messages, asked]);
  await chooseReadMode(page, "Hide read");
  assert.deepStrictEqual(shownIds(page), [2, 7, 12].map(inboxId), "the owner's own question is read, and hidden");
  const thread = { id: "1700000000.001500", root_message_id: asked.id, reply_count: 1, reply_count_exact: true };
  const answer = message({ id: inboxId(16), content: "nightly is red: a flaky test", thread: { ...thread, is_root: false }, ...CODER });
  page.messages = [...messages, { ...asked, thread: { ...thread, is_root: true } }, answer];
  await reReadChannel(page);
  assert.deepStrictEqual(shownIds(page), [2, 7, 12, 15].map(inboxId), "the answered question stayed hidden");
  assert.equal(unreadChip(page, asked.id).textContent, "1 new");
  // The owner reads it in its thread; back on Main it is read, and goes.
  await unreadChip(page, asked.id).click();
  await page.settle();
  assert.equal(page.el("thread-select").value, `thread:${thread.id}`);
  // All reaches the thread, but its read was before the answer: the thread is read, not drawn from
  // All without it, and the reader lands on the answer.
  assert.equal(viewsRead(page).at(-1), "thread", "the thread was drawn from All's read, which has not the answer");
  const landed = rowWithId(page, answer.id);
  assert.ok(landed, "the answer the reader tapped for is not in the thread");
  assert.notEqual(landed.getAttribute("data-collapsed"), "true", "the answer the reader tapped for is folded");
  const top = landed.getBoundingClientRect().top;
  assert.ok(top >= floatingLine(page) && top < page.el("scroll-area").clientHeight,
    `the answer the reader tapped for is at ${top}px, out of view`);
  await doneButton(rowWithId(page, answer.id)).click();
  await page.settle();
  await pickThread(page, "main");
  assert.deepStrictEqual(shownIds(page), [2, 7, 12].map(inboxId));
});

/**
 * All's newest page as the newest `newest` messages, with older history behind it, and each step back
 * as the `back` before that: so a thread can be older than anything All has read. A cursor is how
 * many messages from the newest the step starts at.
 */
function pagedAll(page, newest, back = Infinity) {
  const serve = page.timeline;
  page.timeline = async (path, options) => {
    const url = new URL(path, "http://fixture.test");
    const answer = await serve(path, options);
    if (url.searchParams.get("view") !== "flat") return answer;
    const body = JSON.parse(await answer.text());
    const from = Number(url.searchParams.get("before") || 0);
    const rest = from ? body.messages.slice(0, -from) : body.messages;
    const size = from ? back : newest;
    const messages = rest.slice(-size);
    const more = rest.length > size;
    const on = new Set(messages.map((m) => String(m.id)));
    return json(200, { ...body, messages, returned: messages.length, has_more: more,
      next_before: more ? String(from + size) : null, dismissed: body.dismissed.filter((id) => on.has(id)) });
  };
}

test("A THREAD OLDER THAN ALL REACHES does not count its old replies as new; a new one is, and a step back keeps the count right", async () => {
  // An old question of the owner's, long answered (one Done, one his own); another of his, older than
  // All reaches too, with no answer yet and so, as Slack sends it, no thread; then a day of updates.
  const page = newPage();
  page.threadingSupported = true;
  const old = { id: "spaces/A/threads/old", root_message_id: inboxId(20), is_root: true, reply_count: 2, reply_count_exact: true };
  const under = { ...old, is_root: false };
  const messages = [
    message({ id: inboxId(20), content: "why is the queue slow?", thread: old, ...OWN }),
    message({ id: inboxId(21), content: "a stuck consumer; restarted", thread: under, ...CODER }),
    message({ id: inboxId(22), content: "thanks", thread: under, ...OWN }),
    message({ id: inboxId(25), content: "and the disk alarm?", ...OWN }),
    ...[30, 31, 32, 33, 34].map((n) => message({ id: inboxId(n), content: `update ${n}`, ...CODER })),
  ];
  page.dealtWith.add(inboxId(21));
  page.threads = [{ id: old.id, root: messages[0], title: "queue", reply_count: 2, reply_count_exact: true,
    updated_at: messages[2].timestamp }];
  pagedAll(page, 4, 6);
  await inboxOnMain(page, messages);
  // Main's own read brings the old questions: two replies to the first, neither held, both older
  // than All's page; none to the second.
  await reReadChannel(page);
  await chooseReadMode(page, "Hide read");
  assert.deepStrictEqual(shownIds(page), [30, 31, 32, 33, 34].map(inboxId), "an old answered question was kept as new");
  await chooseReadMode(page, "Show read");
  assert.equal(unreadChip(page, inboxId(20)), null, "old replies the page never brought were counted as new");
  // The same after a reload, from what the device kept.
  const reopened = reloadWith(page.storage, page.messages, (p) => {
    p.threadingSupported = true;
    p.dealtWith = page.dealtWith;
    p.threads = page.threads;
    pagedAll(p, 4, 6);
  });
  await reopenedChannel(reopened);
  await reReadChannel(reopened);
  assert.equal(unreadChip(reopened, inboxId(20)), null, "the reload forgot which replies were old");
  // The agent adds to the first and answers the second, and nothing brings either reply: one new
  // each. The second had no reply when the page last held it, so none of its can be old.
  const grown = { ...old, reply_count: 3 };
  const disk = { id: "spaces/A/threads/disk", root_message_id: inboxId(25), is_root: true, reply_count: 1, reply_count_exact: true };
  const late = message({ id: inboxId(40), content: "it is slow again", thread: under, ...CODER });
  const diskAnswer = message({ id: inboxId(41), content: "the disk alarm was a full log volume", thread: { ...disk, is_root: false }, ...CODER });
  const withAnswers = (first) => [{ ...messages[0], thread: first }, messages[1], messages[2], { ...messages[3], thread: disk },
    ...messages.slice(4), late, diskAnswer];
  reopened.messages = withAnswers(grown);
  await reReadChannel(reopened);
  assert.equal(unreadChip(reopened, inboxId(20)).textContent, "1 new");
  assert.equal(unreadChip(reopened, inboxId(25)).textContent, "1 new", "the first answer to an old question was read as old");
  // All's read brings both: held, unread, and the first question's two old ones still not held.
  await pickThread(reopened, "flat");
  await reReadChannel(reopened);
  await pickThread(reopened, "main");
  assert.equal(unreadChip(reopened, inboxId(20)).textContent, "1+ unread");
  assert.equal(unreadChip(reopened, inboxId(25)).textContent, "1 unread");
  // A step back in All brings the old ones, though not the question itself: all three held, the
  // count exact.
  await pickThread(reopened, "flat");
  await reopened.el("load-older").click();
  await reopened.settle();
  assert.ok(rowWithId(reopened, inboxId(21)), "the step back did not bring the old replies");
  assert.equal(reopened.el("load-older").hidden, false,
    "the step back reached the question, which would say its count itself");
  await pickThread(reopened, "main");
  assert.equal(unreadChip(reopened, inboxId(20)).textContent, "1 unread");
  // Dealt with, then answered once more, with nothing to bring it: one new — the two the step back
  // brought are no longer counted as missing for being old.
  await pickThread(reopened, "flat");
  await doneButton(rowWithId(reopened, late.id)).click();
  await reopened.settle();
  await pickThread(reopened, "main");
  assert.equal(unreadChip(reopened, inboxId(20)), null);
  reopened.messages = [...withAnswers({ ...old, reply_count: 4 }),
    message({ id: inboxId(42), content: "slow once more", thread: under, ...CODER })];
  await reReadChannel(reopened);
  assert.equal(unreadChip(reopened, inboxId(20)).textContent, "1 new", "a step back's replies were still counted as missing");
});

test("ALL'S PAGE SHORT OF A THREAD'S REPLIES, as Slack's gives the newest of each, counts nothing new for what it left out", async () => {
  const page = newPage();
  const messages = inboxChannel(page);
  // C's second answer is dealt with, and All's page leaves out its first: held, C has only what is read.
  page.dealtWith.add(inboxId(9));
  const serve = page.timeline;
  page.timeline = async (path, options) => {
    const answer = await serve(path, options);
    if (new URL(path, "http://fixture.test").searchParams.get("view") !== "flat") return answer;
    const body = JSON.parse(await answer.text());
    const kept = body.messages.filter((m) => m.id !== inboxId(8));
    return json(200, { ...body, messages: kept, returned: kept.length,
      notice: "Some threads' replies are not in All: it brings the replies of the 10 most active threads among " +
        "each 50 top-level messages, and the newest 50 of each; open a thread to read all of it.",
      dismissed: body.dismissed.filter((id) => id !== inboxId(8)) });
  };
  await inboxOnMain(page, messages);
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: null, 7: null },
    "a reply All's page left out was counted as one posted since");
  await chooseReadMode(page, "Hide read");
  await reReadChannel(page);
  assert.deepStrictEqual(shownIds(page), [2, 12].map(inboxId));
});

test("AN OLD REPLY DELETED takes the count of old replies down with it, so the next new one still counts", async () => {
  const page = newPage();
  page.threadingSupported = true;
  const old = { id: "spaces/A/threads/old", root_message_id: inboxId(20), is_root: true, reply_count: 2, reply_count_exact: true };
  const under = { ...old, is_root: false };
  const messages = [
    message({ id: inboxId(20), content: "why is the queue slow?", thread: old, ...OWN }),
    message({ id: inboxId(21), content: "a stuck consumer", thread: under, ...CODER }),
    message({ id: inboxId(22), content: "restarted it", thread: under, ...CODER }),
    ...[30, 31, 32, 33, 34].map((n) => message({ id: inboxId(n), content: `update ${n}`, ...CODER })),
  ];
  pagedAll(page, 4);
  await inboxOnMain(page, messages);
  await reReadChannel(page);
  assert.equal(unreadChip(page, inboxId(20)), null);
  // One of the two old answers is deleted upstream: the root says one reply now.
  const shrunk = { ...old, reply_count: 1 };
  page.messages = [{ ...messages[0], thread: shrunk }, ...messages.slice(2)];
  await reReadChannel(page);
  assert.equal(unreadChip(page, inboxId(20)), null);
  // Then a new answer, which nothing brings: it is the one reply past what is old.
  const late = message({ id: inboxId(40), content: "slow again", thread: under, ...CODER });
  page.messages = [{ ...messages[0], thread: { ...old, reply_count: 2 } }, ...messages.slice(2), late];
  await reReadChannel(page);
  assert.equal(unreadChip(page, inboxId(20)).textContent, "1 new", "a deleted old reply hid a new one");
});

test("DONE IN ALL WHILE ALL'S READ IS ON ITS WAY: the row stays dealt with when the read, sent before, lands", async () => {
  const page = newPage();
  const messages = inboxChannel(page);
  await signIn(page);
  await showDiscord(page, messages);
  const serve = page.timeline;
  let land = () => {};
  page.timeline = (path, options) => {
    const answered = serve(path, options);
    page.timeline = serve;
    return new Promise((resolve) => { land = () => resolve(answered); });
  };
  page.expireTimers(DISCORD_POLL_MS);
  await page.settle();
  const calls = page.timelineCalls.length;
  await doneButton(rowWithId(page, inboxId(12))).click();
  await page.settle();
  assert.equal(rowWithId(page, inboxId(12)).getAttribute("data-archived"), "true");
  land();
  await page.settle();
  await page.settle();
  assert.equal(page.timelineCalls.length, calls, "a later read answered for the Done, so this proves nothing");
  assert.equal(rowWithId(page, inboxId(12)).getAttribute("data-archived"), "true",
    "a read sent before the Done put the row back");
});

test("DONE HERE WHILE MAIN'S READ IS ON ITS WAY: the read, sent before, does not count the reply as unread again", async () => {
  const page = newPage();
  const messages = inboxChannel(page);
  page.pinned.set(inboxId(8), storedPin(messages[8]));
  page.servePinsRevision = true;
  page.pinsRevision = 1;
  await inboxOnMain(page, messages);
  // Main's poll goes out, and its answer is held on the way back: as the server had it then.
  const serve = page.timeline;
  let land = () => {};
  let sentWithout = false;
  page.timeline = (path, options) => {
    const answered = serve(path, options);
    sentWithout = !page.dealtWith.has(inboxId(8));
    page.timeline = serve;
    return new Promise((resolve) => { land = () => resolve(answered); });
  };
  page.expireTimers(DISCORD_POLL_MS);
  await page.settle();
  const calls = page.timelineCalls.length;
  // Meanwhile the reader deals with C's first answer from the Pinned list.
  await showPinned(page);
  await doneButton(page.el("pinned-log").children[0]).click();
  await page.settle();
  assert.ok(page.dealtWith.has(inboxId(8)), "the Done was not stored");
  await page.el("pinned-filter").click();
  await page.settle();
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: null, 7: "1 unread" });
  land();
  await page.settle();
  await page.settle();
  assert.ok(sentWithout, "the read was sent after the Done, so this proves nothing");
  assert.equal(page.timelineCalls.length, calls, "a later read answered for the Done, so this proves nothing");
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: null, 7: "1 unread" },
    "a read sent before the Done counted the reply as unread again");
  // A read sent after it speaks for it as ever: put back on another device, it is unread again.
  page.dealtWith.delete(inboxId(8));
  await reReadChannel(page);
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: null, 7: "2 unread" });
});

test("A DONE WHOSE WRITE FAILS puts back only what it changed, not what a read brought meanwhile", async () => {
  const page = newPage();
  const messages = inboxChannel(page);
  page.pinned.set(inboxId(8), storedPin(messages[8]));
  page.servePinsRevision = true;
  page.pinsRevision = 1;
  await inboxOnMain(page, messages);
  let fail = () => {};
  page.inboxWriteHook = () => new Promise((resolve) => {
    fail = () => resolve(json(500, { error: "store_failed", detail: "the store could not be written" }));
  });
  await showPinned(page);
  const done = doneButton(page.el("pinned-log").children[0]).click();
  await page.settle();
  await page.el("pinned-filter").click();
  await page.settle();
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: null, 7: "1 unread" }, "the Done waited for its write");
  // While it is on its way, A's answer is dealt with on another device, and Main's poll says so —
  // and, the write not having reached the server, nothing of C's.
  page.dealtWith.add(inboxId(3));
  await reReadChannel(page);
  assert.deepStrictEqual(unreadSaid(page), { 2: null, 5: null, 7: "1 unread" }, "a read undid a Done still on its way");
  fail();
  await Promise.resolve(done).catch(() => {});
  await page.settle();
  await page.settle();
  assert.deepStrictEqual(unreadSaid(page), { 2: null, 5: null, 7: "2 unread" },
    "the failed Done put back more than it changed, or kept what it did");
});

test("EVERY PROVIDER'S THREADS: Google Chat, Slack and Discord thread ids count and keep their roots alike", async () => {
  const shapes = {
    "Google Chat": INBOX,
    Slack: { a: "1700000000.000200", b: "1700000000.000500", c: "1700000000.000700" },
    Discord: { a: "1300000000000000002", b: "1300000000000000005", c: "1300000000000000007" },
  };
  for (const [provider, threadIds] of Object.entries(shapes)) {
    const page = newPage();
    await inboxOnMain(page, inboxChannel(page, threadIds));
    assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: null, 7: "2 unread" }, provider);
    await chooseReadMode(page, "Hide read");
    assert.deepStrictEqual(shownIds(page), [2, 7, 12].map(inboxId), provider);
  }
});

test("AN ALL READ LANDING BEHIND MAIN brings its replies and their read state to Main's counts, with no read of Main", async () => {
  const page = newPage();
  const messages = inboxChannel(page);
  await signIn(page);
  await showDiscord(page, messages);
  const serve = page.timeline;
  let release = () => {};
  const held = new Promise((resolve) => { release = resolve; });
  let holding = true;
  page.timeline = async (path, options) => {
    if (holding) {
      holding = false;
      await held;
    }
    return serve(path, options);
  };
  // Meanwhile: question B gets a new answer, and one of C's is dealt with on another device.
  const b = { ...messages[4].thread, is_root: false };
  page.messages = [...messages, message({ id: inboxId(13), content: "one page failed after all", thread: b, ...CODER })];
  page.dealtWith.add(inboxId(8));
  page.setClock(page.clock() + 7 * 60 * 1000);
  page.expireTimers(DISCORD_POLL_MS);
  await page.settle();
  assert.equal(new URL(page.timelineCalls.at(-1), "http://fixture.test").searchParams.get("view"), "flat",
    "the poll of All is not on the wire");
  const reads = page.timelineCalls.length;
  await pickThread(page, "main");
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: null, 7: "2 unread" });
  release();
  await page.settle();
  await page.settle();
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: "1 unread", 7: "1 unread" },
    "Main did not count what All's read brought");
  assert.deepStrictEqual(page.timelineCalls.slice(reads), [], "Main was read for what All's read had answered");
});

test("DONE FROM THE PINNED LIST on Main: a pinned reply dealt with there counts as read on its root at once", async () => {
  const page = newPage();
  const messages = inboxChannel(page);
  page.pinned.set(inboxId(8), storedPin(messages[8]));
  page.servePinsRevision = true;
  page.pinsRevision = 1;
  await inboxOnMain(page, messages);
  await showPinned(page);
  assert.deepStrictEqual(pinnedIds(page), [inboxId(8)]);
  const reads = page.timelineCalls.length;
  await doneButton(page.el("pinned-log").children[0]).click();
  await page.settle();
  assert.ok(page.dismissCalls.some((body) => (body.messages || []).includes(inboxId(8))), "the Done was not sent");
  await page.el("pinned-filter").click();
  await page.settle();
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: null, 7: "1 unread" },
    "a reply dealt with in the Pinned list still counted as unread");
  assert.deepStrictEqual(page.timelineCalls.slice(reads), [], "the count waited for a read");
});

// --- `#222 unread-replies`, after the post-land review -------------------------------------------

const SLACK_IDS = { a: "1700000000.000200", b: "1700000000.000500", c: "1700000000.000700" };

test("SLACK'S BOUNDED ALL, leaving out a thread's replies, does not take them out of its root's count", async () => {
  const page = newPage();
  const messages = inboxChannel(page, SLACK_IDS);
  await inboxOnMain(page, messages);
  await chooseReadMode(page, "Hide read");
  assert.deepStrictEqual(shownIds(page), [2, 7, 12].map(inboxId));
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 7: "2 unread" });
  // Ten other threads are busier than question A's now: All's next page carries none of A's replies,
  // and says it leaves some out.
  allLeavingOut(page, [SLACK_IDS.a]);
  await pickThread(page, "flat");
  await reReadChannel(page);
  assert.ok(rowWithId(page, inboxId(3)), "All dropped an unread reply its page only left out");
  await pickThread(page, "main");
  // Nothing was dealt with: 03, the agent's answer to A, is as unread as it was.
  assert.ok(!page.dealtWith.has(inboxId(3)));
  assert.deepStrictEqual(shownIds(page), [2, 7, 12].map(inboxId),
    "Hide read hid question A, whose agent answer is unread, after a look at All");
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 7: "2 unread" });
  await reReadChannel(page);
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 7: "2 unread" }, "Main's next read lost A's count");

  // THE CONTROL: a page that says nothing of leaving anything out answers for every reply of a root it
  // carries, so one it lacks is gone upstream — here every reply of A, as the page has it.
  const plain = newPage();
  await inboxOnMain(plain, inboxChannel(plain, SLACK_IDS));
  allLeavingOut(plain, [SLACK_IDS.a], null);
  await pickThread(plain, "flat");
  await reReadChannel(plain);
  assert.equal(rowWithId(plain, inboxId(3)), undefined, "a reply an unbounded page lacks was kept as though it were there");
  await pickThread(plain, "main");
  assert.equal(unreadChip(plain, inboxId(2)), null);
  // ...and a bounded page whose root says its thread has no replies answers for every one held: A's
  // were deleted upstream.
  const emptied = newPage();
  await inboxOnMain(emptied, inboxChannel(emptied, SLACK_IDS));
  allLeavingOut(emptied, []);
  emptied.messages = emptied.messages.filter((m) => m.id !== inboxId(3) && m.id !== inboxId(4))
    .map((m) => (m.id === inboxId(2) ? { ...m, thread: { ...m.thread, reply_count: 0 } } : m));
  await pickThread(emptied, "flat");
  await reReadChannel(emptied);
  assert.equal(rowWithId(emptied, inboxId(3)), undefined, "a reply deleted upstream was kept");
  await pickThread(emptied, "main");
  assert.equal(unreadChip(emptied, inboxId(2)), null);
});

test("A REPLY ONLY MAIN'S COUNT KNOWS OF stays \"1 new\" when a bounded All page passes over its thread, and counts once one carries it", async () => {
  const page = newPage();
  const messages = inboxChannel(page, SLACK_IDS);
  await inboxOnMain(page, messages);
  await chooseReadMode(page, "Hide read");
  const [newB] = answeredUnseen(page, messages);
  await reReadChannel(page);
  assert.equal(unreadChip(page, inboxId(5)).textContent, "1 new");
  const left = [SLACK_IDS.b];
  allLeavingOut(page, left);
  await pickThread(page, "flat");
  await reReadChannel(page);
  assert.equal(rowWithId(page, newB.id), undefined, "the fixture's All carried the answer after all");
  await pickThread(page, "main");
  assert.equal(unreadChip(page, inboxId(5)) && unreadChip(page, inboxId(5)).textContent, "1 new",
    "question B's new answer, which nothing has brought, is no longer counted");
  await reReadChannel(page);
  assert.ok(rowWithId(page, inboxId(5)), "Hide read hid question B with its new answer unread");
  assert.equal(unreadChip(page, inboxId(5)).textContent, "1 new");
  // B's thread is among All's ten again: the answer is brought, and counted as held and unread.
  left.length = 0;
  await pickThread(page, "flat");
  await reReadChannel(page);
  await pickThread(page, "main");
  assert.equal(unreadChip(page, inboxId(5)).textContent, "1 unread");
});

test("A STEP BACK in Slack's All that brings an old question without its replies counts them as old, and a new one as new", async () => {
  const page = newPage();
  page.threadingSupported = true;
  const old = { id: "1700000000.002000", root_message_id: inboxId(20), is_root: true, reply_count: 2, reply_count_exact: true };
  const under = { ...old, is_root: false };
  const messages = [
    message({ id: inboxId(20), content: "why is the queue slow?", thread: old, ...OWN }),
    message({ id: inboxId(21), content: "a stuck consumer; restarted", thread: under, ...CODER }),
    message({ id: inboxId(22), content: "thanks", thread: under, ...OWN }),
    ...[30, 31, 32, 33, 34].map((n) => message({ id: inboxId(n), content: `update ${n}`, ...CODER })),
  ];
  page.dealtWith.add(inboxId(21));
  page.threads = [{ id: old.id, root: messages[0], title: "queue", reply_count: 2, reply_count_exact: true,
    updated_at: messages[2].timestamp }];
  // Slack's All: the newest page is the newest messages; the step back is the old question, its thread
  // not among the ten Slack reads replies for.
  const serve = page.timeline;
  page.timeline = async (path, options) => {
    const url = new URL(path, "http://fixture.test");
    const answer = await serve(path, options);
    if (url.searchParams.get("view") !== "flat") return answer;
    const body = JSON.parse(await answer.text());
    const back = url.searchParams.get("before");
    const rows = back ? body.messages.filter((m) => m.id === inboxId(20))
      : body.messages.filter((m) => Number(m.id.slice(-2)) >= 30);
    const on = new Set(rows.map((m) => m.id));
    return json(200, { ...body, messages: rows, returned: rows.length, has_more: !back, next_before: back ? null : "older",
      notice: back ? BOUND_NOTICE : null, dismissed: body.dismissed.filter((id) => on.has(id)) });
  };
  await signIn(page);
  await showDiscord(page, messages);
  await page.el("load-older").click();
  await page.settle();
  assert.ok(rowWithId(page, inboxId(20)), "the step back did not bring the old question");
  assert.equal(rowWithId(page, inboxId(21)), undefined, "the step back brought the old question's replies");
  await pickThread(page, "main");
  await reReadChannel(page);
  await reReadChannel(page);
  assert.equal(unreadChip(page, inboxId(20)), null,
    `an old question, its two replies read, says "${unreadChip(page, inboxId(20)) && unreadChip(page, inboxId(20)).textContent}"`);
  // The agent answers it again, and nothing brings the answer: that one is new.
  page.messages = [{ ...messages[0], thread: { ...old, reply_count: 3 } }, ...messages.slice(1),
    message({ id: inboxId(40), content: "slow again", thread: under, ...CODER })];
  await reReadChannel(page);
  assert.equal(unreadChip(page, inboxId(20)) && unreadChip(page, inboxId(20)).textContent, "1 new");
});

/**
 * In Show read: question B answered at length and dealt with in its thread, which this page holds
 * from that visit, left scrolled to its top; then the agent answers B again, at length, which only
 * Main's read reports. Long, so each answer is folded until a landing opens it, and the thread is
 * taller than the screen. Answers the new reply.
 */
async function answeredAgainAfterVisit(page, messages) {
  await chooseReadMode(page, "Show read");
  const [short] = answeredUnseen(page, messages);
  const newB = { ...short, content: longMessage("one page failed after all").repeat(3) };
  page.messages = page.messages.map((m) => (m.id === newB.id ? newB : m));
  await reReadChannel(page);
  await unreadChip(page, inboxId(5)).click();
  await page.settle();
  assert.equal(viewsRead(page).at(-1), "thread");
  assert.equal(rowWithId(page, newB.id).getAttribute("data-collapsed"), "false", "the first visit did not land on the answer");
  await doneButton(rowWithId(page, newB.id)).click();
  await page.settle();
  page.el("scroll-area").scrollTop = 0;
  await pickThread(page, "main");
  assert.equal(unreadChip(page, inboxId(5)), null, "question B, all dealt with, still says it has news");
  const b3 = { ...messages[4].thread, reply_count: 3 };
  const again = message({ id: inboxId(17), content: longMessage("the docs build broke again").repeat(3),
    thread: { ...b3, is_root: false }, ...CODER });
  page.messages = [...page.messages.map((m) => (m.id === inboxId(5) ? { ...m, thread: b3 } : m)), again];
  page.threads = page.threads.map((t) => (t.id === b3.id ? { ...t, reply_count: 3 } : t));
  return again;
}

/** Whether `row` starts on screen, under the floating line: where a landing on it puts it. */
const landedInView = (page, row) => {
  const top = row.getBoundingClientRect().top;
  return top >= floatingLine(page) && top < page.el("scroll-area").clientHeight;
};

/** Hold every read of a thread until `open()`. */
function holdThreadReads(page) {
  const serve = page.timeline;
  const held = gate(serve);
  page.timeline = (path, options) => (viewOf(path) === "thread" ? held.respond(path, options) : serve(path, options));
  return held;
}

test("THE TAP ON \"1 new\" FOR A THREAD HELD FROM EARLIER draws it at once, reads it behind, and lands on the reply", async () => {
  const page = newPage();
  const messages = inboxChannel(page, SLACK_IDS);
  await inboxOnMain(page, messages);
  const again = await answeredAgainAfterVisit(page, messages);
  await reReadChannel(page);
  assert.equal(unreadChip(page, inboxId(5)).textContent, "1 new");
  const reads = viewsRead(page).length;
  const held = holdThreadReads(page);
  // Not awaited: the tap's own promise settles with the read it starts, which is held.
  unreadChip(page, inboxId(5)).click();
  await page.settle();
  // Up at once from what the page held, with the read on its way behind it.
  assert.equal(page.el("thread-select").value, `thread:${SLACK_IDS.b}`);
  assert.deepStrictEqual(shownIds(page), [5, 6, 13].map(inboxId), "the thread waited for its read with nothing up");
  assert.equal(page.el("channel-loading").hidden, true, "the loading line stood over the held rows");
  assert.deepStrictEqual(viewsRead(page).slice(reads), ["thread"],
    "the thread held from earlier was not read for the reply its root reports");
  held.open();
  await page.settle();
  await page.settle();
  const landed = rowWithId(page, again.id);
  assert.ok(landed, "the reply the reader tapped \"1 new\" for is not in the thread");
  assert.equal(landed.getAttribute("data-collapsed"), "false", "the reply tapped for is folded: nothing landed on it");
  assert.ok(landedInView(page, landed), `the reply tapped for is at ${landed.getBoundingClientRect().top}px, out of view`);
  // Back on Main, B is read again only when the reply is.
  await doneButton(landed).click();
  await page.settle();
  await pickThread(page, "main");
  await chooseReadMode(page, "Hide read");
  assert.equal(rowWithId(page, inboxId(5)), undefined);
});

test("...AFTER A RELOAD, the tap on \"1 new\" for a thread the device's copy covers reads it behind that cover too", async () => {
  const page = newPage();
  const messages = inboxChannel(page, SLACK_IDS);
  await inboxOnMain(page, messages);
  const again = await answeredAgainAfterVisit(page, messages);
  await page.settle();
  const reopened = reloadWith(page.storage, page.messages, (p) => {
    p.threadingSupported = true;
    p.dealtWith = page.dealtWith;
    p.threads = page.threads;
  });
  await reopenedChannel(reopened);
  await reopened.settle();
  assert.equal(reopened.el("thread-select").value, "main");
  assert.equal(unreadChip(reopened, inboxId(5)) && unreadChip(reopened, inboxId(5)).textContent, "1 new");
  const reads = viewsRead(reopened).length;
  await unreadChip(reopened, inboxId(5)).click();
  await reopened.settle();
  await reopened.settle();
  assert.deepStrictEqual(viewsRead(reopened).slice(reads), ["thread"], "the saved cover was taken for the thread");
  const landed = rowWithId(reopened, again.id);
  assert.ok(landed, "the reply the reader tapped \"1 new\" for is not in the thread");
  assert.equal(landed.getAttribute("data-collapsed"), "false", "the reply tapped for is folded: nothing landed on it");
  assert.ok(landedInView(reopened, landed), `the reply tapped for is at ${landed.getBoundingClientRect().top}px, out of view`);

  // THE CONTROL: a thread whose root reports nothing the cover lacks is drawn from it with no read.
  await pickThread(reopened, "main");
  await reReadChannel(reopened);
  const before = viewsRead(reopened).length;
  await pickThread(reopened, `thread:${SLACK_IDS.a}`);
  await reopened.settle();
  assert.deepStrictEqual(viewsRead(reopened).slice(before), [], "a thread All answers for was read on entry");
});

test("...WITH AN UNREAD REPLY HELD, the tap lands on it at once, and the read behind keeps it there", async () => {
  const page = newPage();
  const messages = inboxChannel(page, SLACK_IDS);
  await inboxOnMain(page, messages);
  // In Hide read the thread is short, its two unread answers: the landing leaves the list at its end,
  // where a read that follows the newest line would carry the reader past the landing.
  await chooseReadMode(page, "Hide read");
  // Question C's thread, visited and left with its two answers unread.
  await unreadChip(page, inboxId(7)).click();
  await page.settle();
  await pickThread(page, "main");
  // The new answer is long: following the newest line down to it would take the landing off screen.
  const [, short] = answeredUnseen(page, messages);
  const newC = { ...short, content: longMessage("the fix merged").repeat(4) };
  page.messages = page.messages.map((m) => (m.id === newC.id ? newC : m));
  await reReadChannel(page);
  assert.equal(unreadChip(page, inboxId(7)).textContent, "2+ unread");
  const held = holdThreadReads(page);
  unreadChip(page, inboxId(7)).click();
  await page.settle();
  const first = rowWithId(page, inboxId(8));
  assert.ok(landedInView(page, first), "the tap did not land on the first unread reply it held");
  const top = first.getBoundingClientRect().top;
  held.open();
  await page.settle();
  await page.settle();
  assert.ok(rowWithId(page, newC.id), "the read behind did not bring the new reply");
  assert.equal(rowWithId(page, inboxId(8)).getBoundingClientRect().top, top, "the read moved the reader off the landing");
});

test("A REPLY DELETED UPSTREAM after its thread was read stops counting: the root's count read since is the thread's", async () => {
  const page = newPage();
  const messages = inboxChannel(page, SLACK_IDS);
  await inboxOnMain(page, messages);
  await chooseReadMode(page, "Hide read");
  const [, newC] = answeredUnseen(page, messages);
  await reReadChannel(page);
  assert.equal(unreadChip(page, inboxId(7)).textContent, "2+ unread");
  await unreadChip(page, inboxId(7)).click();
  await page.settle();
  assert.equal(viewsRead(page).at(-1), "thread");
  for (const id of [inboxId(8), inboxId(9), newC.id]) {
    await doneButton(rowWithId(page, id)).click();
    await page.settle();
  }
  await pickThread(page, "main");
  assert.equal(rowWithId(page, inboxId(7)), undefined, "question C, all dealt with, stayed");
  // The newest answer is deleted upstream: C has three replies again, as its root says to All and Main.
  const c3 = { ...messages[7].thread, reply_count: 3 };
  page.messages = page.messages.filter((m) => m.id !== newC.id).map((m) => (m.id === inboxId(7) ? { ...m, thread: c3 } : m));
  page.threads = page.threads.map((t) => (t.id === c3.id ? { ...t, reply_count: 3 } : t));
  await pickThread(page, "flat");
  await reReadChannel(page);
  assert.equal(rowWithId(page, newC.id), undefined, "All kept the deleted reply");
  await pickThread(page, "main");
  await reReadChannel(page);
  assert.equal(unreadChip(page, inboxId(7)), null,
    `question C, every reply dealt with, says "${unreadChip(page, inboxId(7)) && unreadChip(page, inboxId(7)).textContent}"`);
  assert.equal(rowWithId(page, inboxId(7)), undefined, "Hide read keeps question C for a deleted reply");
  assert.equal(threadBadge(rowWithId(page, inboxId(2))).textContent, "Thread(2)");
});

/** Question A, B and C's Main, reopened with no copy on the device; `arrange` arranges its network. */
async function coldInboxInMain(arrange = null) {
  const first = newPage();
  await inboxOnMain(first, inboxChannel(first));
  first.storage.delete(MESSAGE_CACHE_KEY);
  const page = reloadWith(first.storage, inboxChannel(newPage()), (p) => {
    inboxChannel(p);
    if (arrange) arrange(p);
  });
  await reopenedChannel(page);
  await page.settle();
  assert.equal(page.el("thread-select").value, "main");
  return page;
}

test("A COLD OPEN IN MAIN reads All once, behind Main, and the counts are exact: through polls, a reload, and Done", async () => {
  const page = await coldInboxInMain();
  // Main first, drawn from its own read; then All, behind it, for the replies Main's counts report.
  assert.deepStrictEqual(viewsRead(page), ["main", "flat"], "the cold open did not read All behind Main, once");
  assert.equal(page.el("channel-loading").hidden, true);
  // B's one reply is Done: nothing to say. Before, with no read of All, every root said "N new".
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: null, 7: "2 unread" });
  await chooseReadMode(page, "Hide read");
  assert.deepStrictEqual(shownIds(page), [2, 7, 12].map(inboxId));
  for (let i = 0; i < 3; i += 1) await reReadChannel(page);
  assert.deepStrictEqual(viewsRead(page), ["main", "flat", "main", "main", "main"], "Main's polls read All again");
  // A reload: the device's copy holds All's read, so Main alone; and past the interval, a poll with
  // nothing outrunning the store reads no All either.
  const again = reloadWith(page.storage, page.messages, (p) => inboxChannel(p));
  await reopenedChannel(again);
  await again.settle();
  again.setClock(again.clock() + 10 * 60 * 1000);
  await reReadChannel(again);
  assert.deepStrictEqual(viewsRead(again), ["main", "main"], "a reload with All held read it again");
  assert.deepStrictEqual(unreadSaid(again), { 2: "1 unread", 7: "2 unread" });
  // Done on question A's answer, in its thread: A goes.
  await unreadChip(again, inboxId(2)).click();
  await again.settle();
  await doneButton(rowWithId(again, inboxId(3))).click();
  await again.settle();
  await pickThread(again, "main");
  assert.deepStrictEqual(shownIds(again), [7, 12].map(inboxId));
});

test("...AND IF THAT READ OF ALL FAILS, the roots stay kept as \"N new\", and Main asks again only once the interval is up", async () => {
  let failing = true;
  const page = await coldInboxInMain((p) => {
    const serve = p.timeline;
    p.timeline = (path, options) => (failing && viewOf(path) === "flat"
      ? json(502, { error: "discord_error", detail: "the provider is down" }) : serve(path, options));
  });
  assert.deepStrictEqual(viewsRead(page), ["main", "flat"]);
  assert.equal(page.el("error").hidden, true, "a read nobody asked to see raised the error banner");
  assert.deepStrictEqual(unreadSaid(page), { 2: "2 new", 5: "1 new", 7: "3 new" });
  await chooseReadMode(page, "Hide read");
  assert.deepStrictEqual(shownIds(page), [2, 5, 7, 12].map(inboxId), "a failed read of All let answered roots go");
  failing = false;
  await reReadChannel(page);
  await reReadChannel(page);
  assert.deepStrictEqual(viewsRead(page), ["main", "flat", "main", "main"], "a failed read of All was asked again at once");
  page.setClock(page.clock() + COUNTS_ALL_READ_MS);
  await reReadChannel(page);
  assert.deepStrictEqual(viewsRead(page).slice(4), ["main", "flat"], "the interval up, Main did not ask for All again");
  assert.deepStrictEqual(shownIds(page), [2, 7, 12].map(inboxId));
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 7: "2 unread" });
});

test("THE READ OF ALL FOR MAIN'S COUNTS IS BOUNDED: not while All is fresh, not for a count it already saw, again for one that grew", async () => {
  const page = newPage();
  const messages = inboxChannel(page, SLACK_IDS);
  await inboxOnMain(page, messages);
  answeredUnseen(page, messages);
  // All was read at the open, a moment ago: Main's own read says B and C have news, and reads no All.
  await reReadChannel(page);
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: "1 new", 7: "2+ unread" });
  let reads = viewsRead(page).length;
  assert.deepStrictEqual(viewsRead(page).slice(-1), ["main"]);
  // Past the interval, the next read of Main reads All behind it. Slack's page leaves out B's replies.
  allLeavingOut(page, [SLACK_IDS.b]);
  page.setClock(page.clock() + COUNTS_ALL_READ_MS);
  await reReadChannel(page);
  assert.deepStrictEqual(viewsRead(page).slice(reads), ["main", "flat"]);
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: "1 new", 7: "3 unread" }, "All's read was not counted");
  // Another interval on, B is as Main last saw it: All, which does not carry it, is not read for it again.
  page.setClock(page.clock() + COUNTS_ALL_READ_MS);
  reads = viewsRead(page).length;
  await reReadChannel(page);
  assert.deepStrictEqual(viewsRead(page).slice(reads), ["main"], "All was read again for a count it had already seen");
  // B is answered once more: news, and the interval is up.
  const b3 = { ...messages[4].thread, reply_count: 3 };
  page.messages = [...page.messages.map((m) => (m.id === inboxId(5) ? { ...m, thread: b3 } : m)),
    message({ id: inboxId(18), content: "and the index page", thread: { ...b3, is_root: false }, ...CODER })];
  reads = viewsRead(page).length;
  await reReadChannel(page);
  assert.deepStrictEqual(viewsRead(page).slice(reads), ["main", "flat"], "a count that grew did not read All");
  assert.equal(unreadChip(page, inboxId(5)).textContent, "2 new");
  // Hidden, the page reads nothing it was not asked for. A hidden page polls nothing (`#226
  // page-energy-profile`), so the read of Main that finds the news is one already on the wire when
  // the page is hidden, and lands after.
  page.setClock(page.clock() + COUNTS_ALL_READ_MS);
  page.messages = page.messages.map((m) => (m.id === inboxId(5) ? { ...m, thread: { ...b3, reply_count: 4 } } : m));
  const answer = page.timeline;
  const held = gate(answer);
  page.timeline = (path) => held.respond(path);
  reads = viewsRead(page).length;
  await reReadChannel(page);
  await page.setVisibility("hidden");
  page.timeline = answer;
  held.open();
  await page.settle();
  await page.settle();
  assert.deepStrictEqual(viewsRead(page).slice(reads), ["main"], "the read of Main behind the hide did not land");
  assert.ok(!viewsRead(page).slice(reads).includes("flat"), "a hidden page read All for its counts");
});

test("A ROOT HIDE READ KEEPS SHOWS ITS CHIP, drawn from a copy that has no thread record yet", async () => {
  // Slack's question has no thread until its first answer. The Threads list, read when the reader
  // touches the picker, knows of the answer before Main's next read does.
  const page = newPage();
  const messages = inboxChannel(page, SLACK_IDS);
  const asked = message({ id: inboxId(15), content: "can you check the nightly?", ...OWN });
  await inboxOnMain(page, [...messages, asked]);
  await chooseReadMode(page, "Hide read");
  assert.equal(rowWithId(page, asked.id), undefined, "the owner's own question is read, and hidden");
  const thread = { id: "1700000000.001500", root_message_id: asked.id, is_root: true, reply_count: 1, reply_count_exact: true };
  page.threads = [...page.threads, { id: thread.id, root: { ...asked, thread }, title: "nightly", reply_count: 1,
    reply_count_exact: true, updated_at: asked.timestamp }];
  page.setClock(page.clock() + 5 * 60 * 1000);
  await page.el("thread-select").dispatch("pointerdown");
  await page.settle();
  assert.ok(viewsRead(page).includes("threads"), "the Threads list was not read");
  const root = rowWithId(page, asked.id);
  assert.ok(root, "Hide read hid the question the Threads list says is answered");
  assert.equal(unreadChip(page, asked.id) && unreadChip(page, asked.id).textContent, "1 new",
    "Hide read kept the question with nothing to say why");
});

test("THE UNREAD COUNT AND N REPLIES never break inside themselves on a narrow row", () => {
  assert.match(cssBlock(".thread-unread"), /white-space:\s*nowrap/);
  assert.match(cssBlock(".thread-replies"), /white-space:\s*nowrap/);
});

// --- `#222 unread-replies`, after the second review ----------------------------------------------

/**
 * Question C's two agent answers (08, 09) deleted upstream: C has one reply left, the placeholder
 * (11), as its root and its summary now say. `threadIds` as `inboxChannel` was given.
 */
function answersDeleted(page, threadIds = INBOX) {
  page.messages = page.messages.filter((m) => m.id !== inboxId(8) && m.id !== inboxId(9))
    .map((m) => (m.id === inboxId(7) ? { ...m, thread: { ...m.thread, reply_count: 1 } } : m));
  page.threads = page.threads.map((t) => (t.id === threadIds.c ? { ...t, reply_count: 1 } : t));
}

test("ALL'S PAGE CARRYING A THREAD WHOLE answers for every reply of it: replies deleted upstream stop counting", async () => {
  // A page that says nothing of leaving anything out carries question C's root, and of its thread
  // only the placeholder: the agent's two answers are gone upstream. They are not kept as unread.
  const page = newPage();
  await inboxOnMain(page, inboxChannel(page));
  await chooseReadMode(page, "Hide read");
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 7: "2 unread" });
  answersDeleted(page);
  await pickThread(page, "flat");
  await reReadChannel(page);
  assert.equal(rowWithId(page, inboxId(8)), undefined, "All kept a reply deleted upstream");
  assert.equal(rowWithId(page, inboxId(9)), undefined, "All kept a reply deleted upstream");
  await pickThread(page, "main");
  await reReadChannel(page);
  assert.equal(unreadChip(page, inboxId(7)), null,
    `question C counts replies that are gone: ${JSON.stringify(unreadSaid(page))}`);
  assert.deepStrictEqual(shownIds(page), [2, 12].map(inboxId), "Hide read kept question C for replies that are gone");

  // ...and so does one whose root gives only an estimate of the count, as Discord's own threads do.
  const estimated = newPage();
  const shaped = inboxChannel(estimated).map((m) => (m.thread && m.thread.id === INBOX.c
    ? { ...m, thread: { ...m.thread, reply_count_exact: false } } : m));
  await inboxOnMain(estimated, shaped);
  estimated.messages = shaped.filter((m) => m.id !== inboxId(8) && m.id !== inboxId(9));
  await pickThread(estimated, "flat");
  await reReadChannel(estimated);
  assert.equal(rowWithId(estimated, inboxId(8)), undefined, "a page carrying C's root kept a reply deleted upstream");
  await pickThread(estimated, "main");
  assert.equal(unreadChip(estimated, inboxId(7)), null,
    `question C counts replies that are gone: ${JSON.stringify(unreadSaid(estimated))}`);

  // Slack's page, which says it leaves some threads out, carrying as many of C's replies as C's root
  // reports: the same.
  const slack = newPage();
  await inboxOnMain(slack, inboxChannel(slack, SLACK_IDS));
  await chooseReadMode(slack, "Hide read");
  allLeavingOut(slack, []);
  answersDeleted(slack, SLACK_IDS);
  await pickThread(slack, "flat");
  await reReadChannel(slack);
  assert.equal(rowWithId(slack, inboxId(8)), undefined, "a bounded page carrying C whole kept a reply deleted upstream");
  await pickThread(slack, "main");
  assert.equal(unreadChip(slack, inboxId(7)), null,
    `question C counts replies that are gone: ${JSON.stringify(unreadSaid(slack))}`);

  // THE CONTROL: a bounded page carrying fewer of C's replies than its root reports — the newest
  // fifty of a long thread — answers for them only from the oldest it carries: the older answer held
  // stays, unread, and counted.
  const long = newPage();
  await inboxOnMain(long, inboxChannel(long, SLACK_IDS));
  await chooseReadMode(long, "Hide read");
  const serve = long.timeline;
  long.timeline = async (path, options) => {
    const answer = await serve(path, options);
    if (viewOf(path) !== "flat") return answer;
    const body = JSON.parse(await answer.text());
    const kept = body.messages.filter((m) => m.id !== inboxId(8));
    return json(200, { ...body, messages: kept, returned: kept.length, notice: BOUND_NOTICE,
      dismissed: body.dismissed.filter((id) => id !== inboxId(8)) });
  };
  await pickThread(long, "flat");
  await reReadChannel(long);
  assert.ok(rowWithId(long, inboxId(8)), "a reply older than any the bounded page carries was dropped as deleted");
  await pickThread(long, "main");
  assert.deepStrictEqual(unreadSaid(long), { 2: "1 unread", 7: "2 unread" });
});

/**
 * `#549 own-read-stored`. Question B is the owner's own words, so on `page`'s server it starts read.
 * The tests that press Done on its root to settle its passed-over reply need it as one he has not
 * marked read — first seen while "Mark my own messages as read" was off, or put back since — which is
 * what this decides it as.
 */
function questionBNotRead(page) {
  page.ownDecided.add(inboxId(5));
  return page;
}

/**
 * Questions A, B and C on Slack, reopened in Main with no copy on the device (B not marked read,
 * unless `bNotRead` is false: see `questionBNotRead`), where All's page
 * passes over question B's thread — ten others are busier — and says so. B's one reply is Done on
 * the server, which this device cannot know without reading it. `left` is what All's page leaves out
 * (`allLeavingOut`), for a test to change later.
 */
async function coldSlackPastTheTen(left = [SLACK_IDS.b], bNotRead = true) {
  const first = newPage();
  if (bNotRead) questionBNotRead(first);
  await inboxOnMain(first, inboxChannel(first, SLACK_IDS));
  first.storage.delete(MESSAGE_CACHE_KEY);
  const page = reloadWith(first.storage, inboxChannel(newPage(), SLACK_IDS), (p) => {
    inboxChannel(p, SLACK_IDS);
    if (bNotRead) questionBNotRead(p);
    allLeavingOut(p, left);
  });
  await reopenedChannel(page);
  await page.settle();
  assert.deepStrictEqual(viewsRead(page), ["main", "flat"], "the cold open did not read All behind Main, once");
  return page;
}

test("A SLACK THREAD PAST ALL'S TEN, on a cold open, says \"N new\" until Done on its root settles it; one posted after is new", async () => {
  const page = await coldSlackPastTheTen();
  // A and C are exact. B's reply is not known: counted as new, and B kept, rather than hidden.
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: "1 new", 7: "2 unread" });
  await chooseReadMode(page, "Hide read");
  assert.deepStrictEqual(shownIds(page), [2, 5, 7, 12].map(inboxId));
  // Nothing brings it while nothing happens in B: a look at All, Main's polls, twenty minutes. Nor
  // is All read again for it.
  await pickThread(page, "flat");
  await page.settle();
  await pickThread(page, "main");
  for (let i = 0; i < 3; i += 1) await reReadChannel(page);
  page.setClock(page.clock() + 20 * 60 * 1000);
  await reReadChannel(page);
  await reReadChannel(page);
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: "1 new", 7: "2 unread" });
  assert.equal(viewsRead(page).filter((view) => view === "flat").length, 1, "All was read again for a thread it passes over");
  // ...nor a reload, from the device's copy.
  const left = [SLACK_IDS.b];
  const reload = (from) => reloadWith(from.storage, from.messages, (p) => {
    inboxChannel(p, SLACK_IDS);
    for (const id of from.dealtWith) p.dealtWith.add(id);
    for (const id of from.ownDecided) p.ownDecided.add(id);
    allLeavingOut(p, left);
  });
  const mid = reload(page);
  await reopenedChannel(mid);
  await mid.settle();
  assert.deepStrictEqual(viewsRead(mid), ["main"]);
  assert.equal(mid.el("todo-filter").getAttribute("data-read-mode"), "hide");
  assert.deepStrictEqual(shownIds(mid), [2, 5, 7, 12].map(inboxId));
  assert.equal(unreadChip(mid, inboxId(5)).textContent, "1 new");
  // Done on B's root, its write failing: nothing changes.
  mid.inboxWriteHook = async () => json(502, { error: "store_error", detail: "the store is read-only" });
  await doneButton(rowWithId(mid, inboxId(5))).click().catch(() => {});
  await mid.settle();
  assert.deepStrictEqual(shownIds(mid), [2, 5, 7, 12].map(inboxId), "a Done that failed settled question B");
  assert.equal(unreadChip(mid, inboxId(5)).textContent, "1 new");
  // Done on B's root: the reader has dealt with it. Its reply, unknown, is settled, and B goes.
  mid.inboxWriteHook = null;
  await doneButton(rowWithId(mid, inboxId(5))).click();
  await mid.settle();
  assert.deepStrictEqual(shownIds(mid), [2, 7, 12].map(inboxId), "Done on question B's root did not settle its unknown reply");
  await reReadChannel(mid);
  assert.deepStrictEqual(shownIds(mid), [2, 7, 12].map(inboxId), "Main's next read brought question B back");
  // ...through a reload, with no read of All.
  const again = reload(mid);
  await reopenedChannel(again);
  await again.settle();
  assert.deepStrictEqual(viewsRead(again), ["main"]);
  assert.deepStrictEqual(shownIds(again), [2, 7, 12].map(inboxId), "the reload brought question B back");
  // The agent answers B again, which only Main's count reports: that one is new, and B is back.
  const b2 = { ...again.messages[4].thread, reply_count: 2 };
  again.messages = [...again.messages.map((m) => (m.id === inboxId(5) ? { ...m, thread: b2 } : m)),
    message({ id: inboxId(13), content: "the docs build broke again", thread: { ...b2, is_root: false }, ...CODER })];
  await reReadChannel(again);
  assert.equal(unreadChip(again, inboxId(5)) && unreadChip(again, inboxId(5)).textContent, "1 new",
    "an answer posted after the Done was taken as settled");
  assert.ok(rowWithId(again, inboxId(5)), "Hide read hid question B with a new answer nothing has brought");
  // B's first reply is put back on another device, and B's thread is among All's ten again: a read of
  // All places both replies, the settled one included, as they are — both unread.
  again.dealtWith.delete(inboxId(6));
  left.length = 0;
  await pickThread(again, "flat");
  await reReadChannel(again);
  await pickThread(again, "main");
  assert.equal(unreadChip(again, inboxId(5)) && unreadChip(again, inboxId(5)).textContent, "2 unread",
    "a read of All bringing B's replies did not place the one Done had settled");

  // THE CONTROL: Done on a root whose thread a read has placed settles nothing — what its count says
  // beyond that read was posted since, so Done through a list never hides an answer nothing brought.
  const warm = questionBNotRead(newPage());
  const messages = inboxChannel(warm, SLACK_IDS);
  await inboxOnMain(warm, messages);
  await chooseReadMode(warm, "Hide read");
  answeredUnseen(warm, messages);
  await reReadChannel(warm);
  assert.equal(unreadChip(warm, inboxId(5)).textContent, "1 new");
  await doneButton(rowWithId(warm, inboxId(5))).click();
  await warm.settle();
  assert.equal(unreadChip(warm, inboxId(5)) && unreadChip(warm, inboxId(5)).textContent, "1 new",
    "Done on the root hid an answer posted since");
  assert.ok(rowWithId(warm, inboxId(5)), "Hide read hid question B with a new answer nothing has brought");
});

test("...AND ENTERING THAT THREAD READS IT, rather than drawing it from All's cover with its reply missing", async () => {
  const page = await coldSlackPastTheTen();
  await doneButton(rowWithId(page, inboxId(5))).click();
  await page.settle();
  assert.equal(unreadChip(page, inboxId(5)), null);
  const reads = viewsRead(page).length;
  await threadBadge(rowWithId(page, inboxId(5))).click();
  await page.settle();
  await page.settle();
  assert.equal(page.el("thread-select").value, `thread:${SLACK_IDS.b}`);
  assert.deepStrictEqual(viewsRead(page).slice(reads), ["thread"], "a thread All's page passed over was drawn from All's cover");
  assert.ok(rowWithId(page, inboxId(6)), "the thread was drawn without its reply");
});

test("THE READ OF ALL FOR MAIN'S COUNTS, answering after the reader's own read of All, is not folded over it", async () => {
  // A cold open in Main, and the read of All behind it is slow. The reader opens All meanwhile, and
  // their read brings a message posted since; then the slow answer comes, from before it.
  let release = () => {};
  const page = await coldInboxInMain((p) => {
    const serve = p.timeline;
    let first = true;
    p.timeline = (path, options) => {
      if (viewOf(path) !== "flat" || !first) return serve(path, options);
      first = false;
      // As the server had it when asked.
      const answer = serve(path, options);
      return new Promise((resolve) => {
        release = () => resolve(answer);
      });
    };
  });
  assert.deepStrictEqual(viewsRead(page), ["main", "flat"]);
  const late = message({ id: inboxId(16), content: "the canary is green", ...CODER });
  page.messages = [...page.messages, late];
  page.setClock(page.clock() + 1000);
  await pickThread(page, "flat");
  await page.settle();
  assert.ok(rowWithId(page, late.id), "the reader's read of All did not bring the new message");
  release();
  await page.settle();
  await page.settle();
  assert.ok(rowWithId(page, late.id), "the older answer took out, as deleted, a message the reader's newer read brought");
  await pickThread(page, "main");
  assert.ok(rowWithId(page, late.id), "Main lost the message too");
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: null, 7: "2 unread" });
});

// --- `#222 unread-replies`, after the third review -----------------------------------------------

/** What question B's root says on Main of its thread's unread replies: its chip's words, or null. */
const saidB = (page) => (unreadChip(page, inboxId(5)) ? unreadChip(page, inboxId(5)).textContent : null);

test("UNDO OF A DONE THAT SETTLED a passed-over thread's unknown reply puts its \"1 new\" back, and Main's reads keep it", async () => {
  const page = await coldSlackPastTheTen();
  await chooseReadMode(page, "Hide read");
  assert.deepStrictEqual(shownIds(page), [2, 5, 7, 12].map(inboxId));
  assert.equal(saidB(page), "1 new");
  await doneButton(rowWithId(page, inboxId(5))).click();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), [2, 7, 12].map(inboxId), "Done on question B's root did not settle it");
  assert.equal(page.el("undo-dismiss").hidden, false, "no undo offered");
  await page.el("undo-dismiss").click();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), [2, 5, 7, 12].map(inboxId), "Undo of Done left question B hidden under Hide read");
  assert.equal(saidB(page), "1 new", "Undo of Done did not put question B's \"1 new\" back");
  await reReadChannel(page);
  assert.equal(saidB(page), "1 new", "Main's next read settled question B again");
  // Done again settles it again: the Undo left nothing of the first behind.
  await doneButton(rowWithId(page, inboxId(5))).click();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), [2, 7, 12].map(inboxId), "a second Done did not settle question B");

  // Undo of "Mark read through here" on Main, which settled B with the rest of the list, the same.
  const through = await coldSlackPastTheTen();
  await chooseReadMode(through, "Hide read");
  const newest = rowWithId(through, inboxId(12));
  await rowMoreButton(newest).click();
  await readThroughButton(newest).click();
  await through.settle();
  // A's and C's agent answers are unread, and keep their roots; B's unknown reply is settled.
  assert.deepStrictEqual(shownIds(through), [2, 7].map(inboxId));
  await through.el("undo-dismiss").click();
  await through.settle();
  assert.deepStrictEqual(shownIds(through), [2, 5, 7, 12].map(inboxId), "Undo of Mark read through here left question B hidden");
  assert.equal(saidB(through), "1 new");

  // THE CONTROL: Undo of a Done that settled nothing — A's root, whose answer the page holds unread —
  // changes nothing of A's count.
  const held = await coldSlackPastTheTen();
  await chooseReadMode(held, "Hide read");
  await doneButton(rowWithId(held, inboxId(2))).click();
  await held.settle();
  assert.deepStrictEqual(unreadSaid(held), { 2: "1 unread", 5: "1 new", 7: "2 unread" }, "Done on a root marked its held reply");
  await held.el("undo-dismiss").click();
  await held.settle();
  assert.deepStrictEqual(unreadSaid(held), { 2: "1 unread", 5: "1 new", 7: "2 unread" });
});

test("PUTTING THE ROOT BACK takes back what its Done settled, after a reload too; a put back that fails takes nothing back", async () => {
  const page = await coldSlackPastTheTen();
  await doneButton(rowWithId(page, inboxId(5))).click();
  await page.settle();
  assert.equal(saidB(page), null);
  const reopened = reloadWith(page.storage, page.messages, (p) => {
    inboxChannel(p, SLACK_IDS);
    for (const id of page.dealtWith) p.dealtWith.add(id);
    for (const id of page.ownDecided) p.ownDecided.add(id);
    allLeavingOut(p, [SLACK_IDS.b]);
  });
  await reopenedChannel(reopened);
  await reopened.settle();
  assert.deepStrictEqual(viewsRead(reopened), ["main"]);
  assert.equal(saidB(reopened), null, "the reload took the Done's settling back");
  await chooseReadMode(reopened, "Show read");
  assert.equal(doneButton(rowWithId(reopened, inboxId(5))).textContent, "Unarchive");
  // The put back's write fails: nothing changes.
  reopened.inboxWriteHook = async (kind) => (kind === "restore"
    ? json(502, { error: "store_error", detail: "the store is read-only" }) : undefined);
  await doneButton(rowWithId(reopened, inboxId(5))).click().catch(() => {});
  await reopened.settle();
  assert.equal(saidB(reopened), null, "a put back that failed took the settling back");
  await chooseReadMode(reopened, "Hide read");
  assert.equal(rowWithId(reopened, inboxId(5)), undefined, "a put back that failed brought question B back");
  // It goes through: B is as it was before the Done, its reply unknown and counted as new.
  reopened.inboxWriteHook = null;
  await chooseReadMode(reopened, "Show read");
  await doneButton(rowWithId(reopened, inboxId(5))).click();
  await reopened.settle();
  assert.equal(saidB(reopened), "1 new", "putting question B's root back left its reply settled");
  await chooseReadMode(reopened, "Hide read");
  assert.ok(rowWithId(reopened, inboxId(5)), "Hide read hid question B, put back with its reply unknown");
  await reReadChannel(reopened);
  assert.equal(saidB(reopened), "1 new");

  // THE CONTROL: once a read has placed B's reply, the gap is that read's, and putting the root back
  // leaves it: the reply is held, and Done on the server, so B has nothing to say.
  const left = [SLACK_IDS.b];
  const placed = await coldSlackPastTheTen(left);
  await chooseReadMode(placed, "Hide read");
  await doneButton(rowWithId(placed, inboxId(5))).click();
  await placed.settle();
  assert.equal(rowWithId(placed, inboxId(5)), undefined);
  // B's thread is among All's ten again.
  left.length = 0;
  await chooseReadMode(placed, "Show read");
  await pickThread(placed, "flat");
  await reReadChannel(placed);
  assert.ok(rowWithId(placed, inboxId(6)), "All did not bring question B's reply");
  await pickThread(placed, "main");
  await chooseReadMode(placed, "Hide read");
  await placed.el("undo-dismiss").click();
  await placed.settle();
  assert.equal(saidB(placed), null, `question B, its one reply Done, says "${saidB(placed)}"`);
  assert.equal(rowWithId(placed, inboxId(5)), undefined);
});

test("DONE IN ALL on a root whose thread All passes over settles nothing: Main, where its \"1 new\" is drawn, is where Done settles it", async () => {
  const page = await coldSlackPastTheTen();
  // Show read: in All, question B's root is the owner's own words, read, and Hide read hides it.
  await pickThread(page, "flat");
  await page.settle();
  await doneButton(rowWithId(page, inboxId(5))).click();
  await page.settle();
  await pickThread(page, "main");
  await chooseReadMode(page, "Hide read");
  assert.equal(saidB(page), "1 new", "Done in All, where nothing says B has a new reply, settled it");
  assert.ok(rowWithId(page, inboxId(5)), "Hide read hid question B after a Done in All");
  // The tap on it reads the thread, which places the reply: it is Done on the server, so B goes.
  await unreadChip(page, inboxId(5)).click();
  await page.settle();
  await page.settle();
  assert.equal(viewsRead(page).at(-1), "thread");
  await pickThread(page, "main");
  assert.equal(rowWithId(page, inboxId(5)), undefined);
});

test("A STEP BACK IN SLACK'S ALL bringing a passed-over root without its replies does not hide its unread answer", async () => {
  const page = newPage();
  const messages = inboxChannel(page, SLACK_IDS);
  // B's one answer is unread on the server.
  page.dealtWith.delete(inboxId(6));
  const later = [30, 31, 32, 33, 34].map((n) => message({ id: inboxId(n), content: `update ${n}`, ...CODER }));
  const laterIds = new Set(later.map((m) => m.id));
  for (const m of later) page.dealtWith.add(m.id);
  let slid = false;
  const serve = page.timeline;
  page.timeline = async (path, options) => {
    const url = new URL(path, "http://fixture.test");
    const answer = await serve(path, options);
    if (url.searchParams.get("view") !== "flat") return answer;
    const body = JSON.parse(await answer.text());
    const back = url.searchParams.get("before");
    // Slack's All past the ten for question B: its root on the page, none of its replies.
    let rows = body.messages.filter((m) => !(m.thread && m.thread.id === SLACK_IDS.b && !m.thread.is_root));
    if (!slid && back) rows = [];
    // Later, enough newer messages that B's root is off All's newest page, and on the step back.
    if (slid) rows = rows.filter((m) => laterIds.has(m.id) === !back);
    const on = new Set(rows.map((m) => m.id));
    return json(200, { ...body, messages: rows, returned: rows.length, has_more: !back, next_before: back ? null : "older",
      notice: BOUND_NOTICE, dismissed: body.dismissed.filter((id) => on.has(id)) });
  };
  await inboxOnMain(page, messages);
  await chooseReadMode(page, "Hide read");
  await reReadChannel(page);
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: "1 new", 7: "2 unread" });
  page.messages = [...page.messages, ...later];
  slid = true;
  await chooseReadMode(page, "Show read");
  await pickThread(page, "flat");
  await reReadChannel(page);
  await page.el("load-older").click();
  await page.settle();
  assert.ok(rowWithId(page, inboxId(5)), "the step back did not bring question B's root");
  assert.equal(rowWithId(page, inboxId(6)), undefined, "the step back brought B's reply");
  await pickThread(page, "main");
  await chooseReadMode(page, "Hide read");
  await reReadChannel(page);
  assert.ok(rowWithId(page, inboxId(5)),
    "Hide read hid question B, whose agent answer is unread and was never brought, after a step back in All");
  assert.equal(saidB(page), "1 new");
});

test("A RELOAD WHOSE DEVICE COPY WAS CUT past a passed-over root keeps its unread answer counted through Main's read", async () => {
  const page = newPage();
  const messages = inboxChannel(page, SLACK_IDS);
  page.dealtWith.delete(inboxId(6));
  const later = Array.from({ length: 125 }, (_, i) => message({ id: inboxId(100 + i), content: `update ${i}`, ...CODER }));
  for (const m of later) page.dealtWith.add(m.id);
  allLeavingOut(page, [SLACK_IDS.b]);
  await inboxOnMain(page, [...messages, ...later]);
  await chooseReadMode(page, "Hide read");
  await reReadChannel(page);
  assert.equal(saidB(page), "1 new");
  const reopened = reloadWith(page.storage, page.messages, (p) => {
    inboxChannel(p, SLACK_IDS);
    p.dealtWith = page.dealtWith;
    allLeavingOut(p, [SLACK_IDS.b]);
  });
  await reopenedChannel(reopened);
  await reopened.settle();
  await reReadChannel(reopened);
  assert.equal(reopened.el("todo-filter").getAttribute("data-read-mode"), "hide");
  assert.ok(rowWithId(reopened, inboxId(5)),
    `Hide read hid question B after a reload, its unread answer never brought; says ${JSON.stringify(unreadSaid(reopened))}`);
  assert.equal(saidB(reopened), "1 new");
});

/** The device's saved copy of the one timeline channel, to edit as an older version would have saved it. */
function editSavedChannel(page, edit) {
  const cache = JSON.parse(page.storage.get(MESSAGE_CACHE_KEY));
  const entry = Object.values(cache.scopes).find((scope) => scope.mode === "timeline");
  edit(entry);
  page.storage.set(MESSAGE_CACHE_KEY, JSON.stringify(cache));
}

test("A DEVICE COPY SAVED BY AN EARLIER VERSION has its gaps placed afresh: B says \"1 new\", and Done on its root settles it", async () => {
  // Saved before passed-over threads were kept: B's gap set to nothing, as Main's read did once All
  // reached B's root, so B said "1 new" for good and Done on it did not clear it.
  const saved = async (edit) => {
    const page = await coldSlackPastTheTen();
    await chooseReadMode(page, "Hide read");
    await reReadChannel(page);
    editSavedChannel(page, (entry) => {
      assert.deepStrictEqual(entry.passedOver, [SLACK_IDS.b]);
      assert.equal(entry.replyGaps[SLACK_IDS.b], undefined);
      edit(entry);
    });
    const reopened = reloadWith(page.storage, page.messages, (p) => {
      inboxChannel(p, SLACK_IDS);
      for (const id of page.dealtWith) p.dealtWith.add(id);
      for (const id of page.ownDecided) p.ownDecided.add(id);
    for (const id of page.ownDecided) p.ownDecided.add(id);
      allLeavingOut(p, [SLACK_IDS.b]);
    });
    await reopenedChannel(reopened);
    await reopened.settle();
    return reopened;
  };
  const before = await saved((entry) => {
    delete entry.passedOver;
    delete entry.settledByDone;
    entry.replyGaps[SLACK_IDS.b] = 0;
  });
  assert.equal(before.el("todo-filter").getAttribute("data-read-mode"), "hide");
  assert.equal(saidB(before), "1 new");
  await doneButton(rowWithId(before, inboxId(5))).click();
  await before.settle();
  await reReadChannel(before);
  assert.equal(rowWithId(before, inboxId(5)), undefined,
    "Done on question B's root did not clear it, on a device copy saved before passed-over threads were kept");
  assert.deepStrictEqual(unreadSaid(before), { 2: "1 unread", 7: "2 unread" });
  // The cost, once: All read again behind Main's first read.
  assert.deepStrictEqual(viewsRead(before), ["main", "flat", "main"], "All was not read again behind Main to place the gaps");

  // Saved by the version before this one: B's gap set to its whole count by Main's read after the
  // copy was cut, hiding B's answer.
  const cut = await saved((entry) => {
    delete entry.settledByDone;
    entry.replyGaps[SLACK_IDS.b] = 1;
  });
  assert.ok(rowWithId(cut, inboxId(5)), "Hide read hid question B, on a copy whose gap for it was its whole count");
  assert.equal(saidB(cut), "1 new");
  assert.deepStrictEqual(viewsRead(cut), ["main", "flat"]);

  // THE CONTROL: a copy this version saved keeps its gaps, and reads no All for them.
  const current = await saved(() => {});
  assert.deepStrictEqual(viewsRead(current), ["main"], "a copy this version saved was not trusted");
  assert.deepStrictEqual(unreadSaid(current), { 2: "1 unread", 5: "1 new", 7: "2 unread" });
});

test("UNREAD REPLIES CUT FROM THE DEVICE'S COPY still count after a reload, as new, and keep their roots; read ones cut count as old", async () => {
  const page = newPage();
  const messages = inboxChannel(page);
  const later = Array.from({ length: 125 }, (_, i) => message({ id: inboxId(100 + i), content: `update ${i}`, ...CODER }));
  for (const m of later) page.dealtWith.add(m.id);
  await inboxOnMain(page, [...messages, ...later]);
  await chooseReadMode(page, "Hide read");
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 7: "2 unread" });
  editSavedChannel(page, (entry) => {
    assert.equal(entry.messages.length, 120);
    assert.ok(!entry.messages.some((m) => m.thread), "the copy kept a reply: nothing is cut");
  });
  const reopened = reloadWith(page.storage, page.messages, (p) => {
    inboxChannel(p);
    p.dealtWith = page.dealtWith;
  });
  await reopenedChannel(reopened);
  await reopened.settle();
  assert.deepStrictEqual(viewsRead(reopened), ["main"]);
  assert.equal(reopened.el("todo-filter").getAttribute("data-read-mode"), "hide");
  // A's agent answer and C's two are unread: no longer held, they count as new, and keep A and C.
  // B's one reply was Done, A's other is the owner's own, C's third a placeholder: cut, those are old.
  assert.deepStrictEqual(unreadSaid(reopened), { 2: "1 new", 7: "2 new" }, "unread agent answers cut from the copy vanished");
  assert.deepStrictEqual(shownIds(reopened).slice(0, 2), [2, 7].map(inboxId));
  // Nor is Done on the root a way to lose them: they are known to be unread.
  await doneButton(rowWithId(reopened, inboxId(7))).click();
  await reopened.settle();
  assert.equal(unreadChip(reopened, inboxId(7)) && unreadChip(reopened, inboxId(7)).textContent, "2 new");
  // A tap reads the thread: the replies are held again, and the count exact.
  await unreadChip(reopened, inboxId(7)).click();
  await reopened.settle();
  await reopened.settle();
  assert.ok(rowWithId(reopened, inboxId(8)), "the thread's read did not bring C's answers");
  await pickThread(reopened, "main");
  assert.deepStrictEqual(unreadSaid(reopened), { 2: "1 new", 7: "2 unread" });
});

/**
 * Questions A, B and C on Slack, reopened in Main from the device's copy `store`, while All fails to
 * read whenever `failing()` says so; All's page passes over question B's thread when it does read.
 */
async function reopenedWithAllFailing(store, messages, dealtWith, failing, ownDecided = []) {
  const page = reloadWith(store, messages, (p) => {
    inboxChannel(p, SLACK_IDS);
    p.messages = messages;
    for (const id of dealtWith) p.dealtWith.add(id);
    for (const id of ownDecided) p.ownDecided.add(id);
    allLeavingOut(p, [SLACK_IDS.b]);
    const serve = p.timeline;
    p.timeline = (path, options) => (failing() && viewOf(path) === "flat"
      ? json(502, { error: "discord_error", detail: "the provider is down" }) : serve(path, options));
  });
  await reopenedChannel(page);
  await page.settle();
  return page;
}

test("UNDO AFTER THE SETTLED THREAD WAS LOWERED OR PASSED OVER by later reads still takes the settling back, and Done settles it again", async () => {
  // A cold open whose read of All fails: no read has placed any thread, and Done on B settles it.
  const first = questionBNotRead(newPage());
  await inboxOnMain(first, inboxChannel(first, SLACK_IDS));
  first.storage.delete(MESSAGE_CACHE_KEY);
  let failing = true;
  const page = await reopenedWithAllFailing(first.storage, first.messages, first.dealtWith, () => failing, first.ownDecided);
  assert.deepStrictEqual(viewsRead(page), ["main", "flat"]);
  assert.deepStrictEqual(unreadSaid(page), { 2: "2 new", 5: "1 new", 7: "3 new" });
  await chooseReadMode(page, "Hide read");
  await doneButton(rowWithId(page, inboxId(5))).click();
  await page.settle();
  assert.equal(rowWithId(page, inboxId(5)), undefined);
  // The read of All goes through, past the interval, and passes B over.
  failing = false;
  page.setClock(page.clock() + COUNTS_ALL_READ_MS);
  await reReadChannel(page);
  assert.deepStrictEqual(viewsRead(page).slice(-2), ["main", "flat"]);
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 7: "2 unread" });
  await page.el("undo-dismiss").click();
  await page.settle();
  assert.equal(saidB(page), "1 new");
  // Main's read, All now reaching B's root, does not make B's unknown reply a known one Done cannot settle.
  await reReadChannel(page);
  assert.equal(saidB(page), "1 new");
  await doneButton(rowWithId(page, inboxId(5))).click();
  await page.settle();
  assert.equal(rowWithId(page, inboxId(5)), undefined, "Done after the Undo did not settle question B again");

  // A settled gap a read only lowers — a reply deleted upstream — is still the Done's to take back.
  const lowered = await coldSlackPastTheTen();
  answeredUnseen(lowered, lowered.messages);
  await reReadChannel(lowered);
  await chooseReadMode(lowered, "Hide read");
  assert.equal(saidB(lowered), "2 new");
  await doneButton(rowWithId(lowered, inboxId(5))).click();
  await lowered.settle();
  assert.equal(rowWithId(lowered, inboxId(5)), undefined);
  lowered.messages = lowered.messages.filter((m) => m.id !== inboxId(13))
    .map((m) => (m.id === inboxId(5) ? { ...m, thread: { ...m.thread, reply_count: 1 } } : m));
  await reReadChannel(lowered);
  assert.equal(rowWithId(lowered, inboxId(5)), undefined);
  await lowered.el("undo-dismiss").click();
  await lowered.settle();
  assert.equal(saidB(lowered), "1 new", "Undo after a read lowered the settled gap left question B settled");
});

test("A SETTLED THREAD'S READ REPLIES CUT FROM THE DEVICE'S COPY move with what Done settled, so putting the root back still takes it back", async () => {
  // B's first reply held and Done, its second never brought; then a copy saved by an earlier version,
  // whose gaps are dropped, reopened while All fails: B's gap is unknown, and Done on B settles it.
  const first = questionBNotRead(newPage());
  const messages = inboxChannel(first, SLACK_IDS);
  await inboxOnMain(first, messages);
  answeredUnseen(first, messages);
  await reReadChannel(first);
  editSavedChannel(first, (entry) => {
    delete entry.settledByDone;
  });
  const page = await reopenedWithAllFailing(first.storage, first.messages, first.dealtWith, () => true, first.ownDecided);
  assert.equal(saidB(page), "1 new");
  await doneButton(rowWithId(page, inboxId(5))).click();
  await page.settle();
  assert.equal(saidB(page), null);
  // Then enough updates that the copy is cut past B's thread, its Done reply included.
  const later = Array.from({ length: 125 }, (_, i) => message({ id: inboxId(100 + i), content: `update ${i}`, ...CODER }));
  for (const m of later) page.dealtWith.add(m.id);
  page.messages = [...page.messages, ...later];
  await reReadChannel(page);
  editSavedChannel(page, (entry) => {
    assert.ok(!entry.messages.some((m) => m.id === inboxId(6)), "the copy was not cut past B's reply");
  });
  const again = await reopenedWithAllFailing(page.storage, page.messages, page.dealtWith, () => true);
  await chooseReadMode(again, "Hide read");
  assert.equal(rowWithId(again, inboxId(5)), undefined, "the reload took the Done's settling back");
  await chooseReadMode(again, "Show read");
  await doneButton(rowWithId(again, inboxId(5))).click();
  await again.settle();
  await chooseReadMode(again, "Hide read");
  assert.ok(rowWithId(again, inboxId(5)), "putting question B's root back, after the copy was cut, left it settled");
  assert.ok(saidB(again), "question B is back with nothing to say why");
});

// --- clearing unread in the chat service's own app ----------------------------------------------
//
// `#232 upstream-mark-read`. The server marks every message it receives in a channel read in the
// chat service's own app, for a channel the owner turns that on for. Settings is where he does, so
// these are about what the box offers, what it warns, what reaches the server, and what it says the
// server did.

const CLEAR_PATH = `/api/v1/channels/${CHANNEL.id}/upstream-clear`;

/** A signed-in page on Settings, whose server can clear unread in Google Chat. */
test("`#602 history-settings-and-limits`: the channel box says how the channel's history stands, saves its own mode, and asks for it next", async () => {
  const walking = {
    channel_id: CHANNEL.id, server_mode: "full", mode: "full", settable: true, messages: 12400,
    oldest_ms: Date.UTC(2026, 3, 1, 12), walking: true, reached_ms: Date.UTC(2026, 3, 1, 12),
    complete: false, cycle_pages: 140, pages_per_hour: 240.5, eta_s: 5400,
    now_ms: Date.UTC(2026, 9, 27, 22, 0), requests_per_minute: 20, requests_last_minute: 7,
  };
  const historyPage = async (script = SCRIPT, view = walking) => {
    const page = newPage(new Map(), script);
    page.messageCache = messageCacheStatus();
    page.history.set(CHANNEL.id, view);
    await signIn(page);
    await page.el("open-settings").click();
    await page.settle();
    return page;
  };
  const page = await historyPage();
  assert.equal(page.el("channel-history").hidden, false, "the box hides the channel's history");
  assert.deepEqual(page.historyCalls, [`GET ${CHANNEL.id}`]);
  assert.ok(markupHolds("channel-settings", "channel-history"), "the history is not in the channel box");
  const line = page.el("channel-history-progress").textContent;
  assert.match(line, /^12,?400 messages saved, back to /);
  assert.match(line, /Reading older messages, back to .+ so far at 240\.5 pages an hour\. About 2 hours to go\.$/);
  assert.equal(page.el("channel-history-default").textContent, "Server default (whole history)");
  assert.equal(page.el("channel-history-mode").value, "default");
  assert.equal(page.el("channel-history-save").disabled, true, "Save offered with nothing to save");
  assert.equal(page.el("channel-history-prioritize").hidden, false);
  // Its own mode: a deliberate Save, of the mode alone.
  page.el("channel-history-mode").value = "recent";
  await page.el("channel-history-mode").dispatch("change");
  assert.equal(page.historyCalls.length, 1, "choosing a mode saved it by itself");
  assert.equal(page.el("channel-history-save").disabled, false);
  await page.el("channel-history-save").click();
  await page.settle();
  assert.equal(page.historyCalls.at(-1), `PUT ${CHANNEL.id} {"mode":"recent"}`);
  assert.equal(page.el("channel-history-mode").value, "recent");
  assert.equal(page.el("channel-history-save").disabled, true);
  assert.equal(page.el("channel-history-state").textContent, "Saved.");
  // Read older messages next: the queue, and the button steps aside while it is at the front.
  await page.el("channel-history-prioritize").click();
  await page.settle();
  assert.equal(page.historyCalls.at(-1), `POST ${CHANNEL.id} prioritize`);
  assert.match(page.el("channel-history-progress").textContent, /Asked for: it is read next\./);
  assert.equal(page.el("channel-history-prioritize").hidden, true);
  // While it is at the front the box reads it again soon, and less often otherwise.
  assert.equal(page.expireTimers(5000) > 0, true, "the queued channel is not read again");
  await page.settle();
  assert.equal(page.historyCalls.at(-1), `GET ${CHANNEL.id}`);
  // Nothing about it is kept on this device: it is the server's.
  assert.deepEqual([...page.storage.keys()].filter((key) => /history/i.test(key)), []);
  // A complete channel offers nothing further back, and a pause says why and for how long.
  const whole = await historyPage(SCRIPT, {
    ...walking, walking: false, complete: true, complete_ms: Date.UTC(2026, 9, 27, 20), eta_s: undefined,
    held_reason: "rate_limited", held_until_ms: Date.UTC(2026, 9, 27, 22, 3),
  });
  assert.equal(whole.el("channel-history-prioritize").hidden, true);
  assert.match(whole.el("channel-history-progress").textContent,
    /Complete: every message back to the channel's beginning\. Paused for 3 minutes: the chat service asked for fewer requests\.$/);
  // The negative control: a box never offered shows and reads nothing, so the checks above saw the box.
  const blind = await historyPage(brokenScript(
    "  return !allChannelsChosen() && Boolean(knownChannel(id)) && Boolean(serverCache && serverCache.available);\n",
    "  return false;\n"));
  assert.equal(blind.el("channel-history").hidden, true, "the negative control showed the box anyway");
  assert.deepEqual(blind.historyCalls, [], "the negative control read the server anyway, so the check above proves nothing");
});

/** One push, as the server reports it. */
const clearAttempt = (fields = {}) => ({
  at_ms: Date.UTC(2026, 9, 11, 10, 43),
  message_id: "1110000000000000300",
  message_at_ms: Date.UTC(2026, 9, 11, 10, 42),
  result: "advanced",
  target: "2026-10-11T10:42:00Z",
  previous: "2026-10-11T09:00:00Z",
  ...fields,
});

test("CLEARING UNREAD is not offered by a server whose chat service cannot, and nothing is asked", async () => {
  const page = newPage();
  await signIn(page);
  await page.el("open-settings").click();
  await page.settle();
  assert.equal(page.el("upstream-clear").hidden, true, "offered by a server that cannot");
  assert.deepEqual(page.requests.filter((request) => request.includes("/upstream-clear")), [],
    "the page asked about a setting the server does not offer");
});

test("...nor to a read-scope token, which could not change it", async () => {
  const page = await clearingPage((page) => { page.tokenScope = "read"; });
  assert.equal(page.el("upstream-clear").hidden, true);
  assert.deepEqual(page.upstreamClearCalls, []);
});

test("...nor for a channel whose own chat service cannot, when another one can", async () => {
  const space = { id: "1110000000000000201", label: "chat space", writable: true, provider: "gchat" };
  const team = { id: "C0123ABCDE", label: "slack team", writable: true, provider: "slack" };
  const provider = (key, name, clear) => ({
    key, name, threading_supported: false, live_delivery: "push", live_poll_seconds: 0,
    channel_registration_supported: false, channel_discovery_supported: false,
    upstream_read_mark_supported: clear, upstream_clear_supported: clear || undefined,
    self_author_id: null, owner_author_id: null,
  });
  const page = await clearingPage((page) => {
    page.channels = [team, space];
    page.providers = [provider("gchat", "Google Chat", true), provider("slack", "Slack", false)];
  });
  // `#355 provider-grouped-channels`. With two services Settings lists one service's channels at a
  // time, opening on the service of the channel being read: Google Chat's, the bar's first group. A
  // channel of another service is reached by choosing its service first, as a reader would.
  assert.equal(page.el("settings-service").value, "gchat");
  assert.equal(page.el("settings-channel").value, space.id);
  assert.equal(page.el("upstream-clear").hidden, false, "not offered for the channel whose service can");
  assert.equal(page.el("upstream-clear-provider").textContent, "Google Chat");
  page.el("settings-service").value = "slack";
  await page.el("settings-service").dispatch("change");
  await page.settle();
  assert.equal(page.el("settings-channel").value, team.id);
  assert.equal(page.el("upstream-clear").hidden, true, "offered for a channel whose service cannot");
  page.el("settings-service").value = "gchat";
  await page.el("settings-service").dispatch("change");
  await page.settle();
  assert.equal(page.el("settings-channel").value, space.id);
  assert.equal(page.el("upstream-clear").hidden, false, "not offered for the channel whose service can");
  assert.equal(page.el("upstream-clear-provider").textContent, "Google Chat");
  assert.deepEqual(page.upstreamClearCalls, [`GET ${space.id}`]);
});

test("THE WARNING is in the markup beside the choice, never hidden, and says what it does to people's messages", async () => {
  assert.equal(PAGE_ELEMENTS.get("upstream-clear-warning").hidden, false);
  const warning = (/<p id="upstream-clear-warning"[^>]*>([\s\S]*?)<\/p>/.exec(HTML) ||
    assert.fail("no warning in web/voice.html"))[1].replace(/<[^>]+>/g, " ").replace(/\s+/g, " ");
  assert.match(warning, /EVERY message in this channel read/);
  assert.match(warning, /including messages from people/);
  // The service's marker covers the whole channel, so the first push clears the backlog too. An
  // owner turning it on to keep his unread messages from people must be told, here, that they go.
  assert.match(warning,
    /The first message vibe-talk receives after it is on also marks everything already unread there read, people's messages too\./);
  assert.match(warning, /vibe-talk's own unread and Done state are not changed/);
  assert.ok(markupHolds("upstream-clear", "upstream-clear-warning"));
  assert.ok(markupHolds("upstream-clear", "upstream-clear-mode"));
  assert.match(
    (/<select id="upstream-clear-mode"[^>]*>/.exec(HTML) || [""])[0],
    /aria-describedby="upstream-clear-warning upstream-clear-threads upstream-clear-state"/,
    "the choice is not described by its warning, so a screen reader reaches it without one"
  );
  const page = await clearingPage();
  assert.equal(page.el("upstream-clear-warning-provider").textContent, "Google Chat",
    "the warning does not name the service it marks messages read in");
});

test("THREAD REPLIES: Settings says plainly, beside the choice, that they stay unread there", async () => {
  // `#253 upstream-clear-polish`. Nothing available can mark a thread read; only the space's main
  // marker moves. Said on its own line, never folded into the warning where it reads as a detail.
  assert.equal(PAGE_ELEMENTS.get("upstream-clear-threads").hidden, false);
  assert.ok(markupHolds("upstream-clear", "upstream-clear-threads"));
  const page = await clearingPage();
  const provider = page.el("upstream-clear-threads-provider").textContent;
  assert.equal(provider, "Google Chat", "the line does not name the service whose threads stay unread");
  const line = (/<p id="upstream-clear-threads"[^>]*>([\s\S]*?)<\/p>/.exec(HTML) ||
    assert.fail("no thread line in web/voice.html"))[1]
    .replace(/<span id="upstream-clear-threads-provider">[^<]*<\/span>/, provider)
    .replace(/\s+/g, " ").trim();
  assert.equal(line, "Thread replies stay unread in Google Chat; only the main conversation is cleared.");
});

test("the box shows the server's own state for the channel: off, and the server's words for it", async () => {
  const page = await clearingPage();
  assert.equal(page.el("upstream-clear").hidden, false);
  assert.deepEqual(page.upstreamClearCalls, [`GET ${CHANNEL.id}`]);
  assert.equal(page.el("upstream-clear-mode").value, "off");
  assert.equal(page.el("upstream-clear-push").hidden, true, "Mark read now offered while off");
  assert.equal(page.el("upstream-clear-save").disabled, true, "Save offered with nothing to save");
  assert.match(page.el("upstream-clear-state").textContent, /^Off\. Nothing here is marked read in Google Chat\.$/);
  // Nothing about it is kept on this device: it is the server's.
  assert.deepEqual([...page.storage.keys()].filter((key) => /upstream|clear/i.test(key)), []);
});

test("TURNING IT ON is a deliberate Save of the mode alone, and draws what the server stored", async () => {
  const page = await clearingPage();
  page.el("upstream-clear-mode").value = "on";
  await page.el("upstream-clear-mode").dispatch("change");
  assert.deepEqual(page.upstreamClearCalls, [`GET ${CHANNEL.id}`], "choosing a mode saved it by itself");
  // `#253 upstream-clear-polish`. No second step appears: no checkbox, and no button until it is saved.
  assert.equal(page.el("upstream-clear-push").hidden, true, "Mark read now offered before On was saved");
  assert.equal(page.el("upstream-clear-save").disabled, false);
  await page.el("upstream-clear-save").click();
  await page.settle();
  assert.equal(page.upstreamClearCalls.at(-1), `PUT ${CHANNEL.id} {"mode":"on"}`);
  assert.equal(page.el("upstream-clear-mode").value, "on");
  assert.equal(page.el("upstream-clear-save").disabled, true);
  assert.match(page.el("upstream-clear-state").textContent,
    /^On\. When the next message arrives here, it and everything before it are marked read in Google Chat\.$/);
  assert.equal(page.el("upstream-clear-push").hidden, false, "Mark read now not offered once On is saved");
  assert.equal(page.el("upstream-clear-push").textContent, "Mark read now");
  assert.equal(page.expireTimers(4000), 0, "a save asked again, with nothing pushed to ask about");
});

test("NO CHECKBOX, no second phase: the box holds one choice, Save, and the one-shot button", async () => {
  // The owner: "when I check that and save a NEW checkbox appears". There is no checkbox to appear.
  const box = (/<div id="upstream-clear"[^>]*>([\s\S]*?)<\/div>\s*<\/section>/.exec(HTML) ||
    assert.fail("no clear-unread box in web/voice.html"))[1];
  assert.doesNotMatch(box, /type="checkbox"/);
  assert.equal(PAGE_ELEMENTS.has("upstream-clear-now"), false);
  assert.equal(PAGE_ELEMENTS.has("upstream-clear-now-label"), false);
  assert.match(SCRIPT_CODE, /el\("upstream-clear-push"\)/);
  assert.doesNotMatch(SCRIPT_CODE, /clear_now|upstream-clear-now/);
  assert.equal(PAGE_ELEMENTS.get("upstream-clear-push").tag, "button");
  assert.ok(markupHolds("upstream-clear", "upstream-clear-push"));
});

test("MARK READ NOW is offered while On, as TRY IT NOW while a dry run, and never while Off or unsaved", async () => {
  const cases = [
    ["off", true, null],
    ["on", false, "Mark read now"],
    ["dry_run", false, "Try it now"],
  ];
  for (const [mode, hidden, label] of cases) {
    const page = await clearingPage((page) => {
      page.upstreamClear.set(CHANNEL.id, {
        provider_name: "Google Chat", supported: true, mode, failures: 0, pending: false,
      });
    });
    assert.equal(page.el("upstream-clear-push").hidden, hidden, `${mode}: hidden is wrong`);
    if (label) assert.equal(page.el("upstream-clear-push").textContent, label, `${mode}: wrong label`);
  }
  // An unsaved choice hides it: the button acts on the saved mode, and beside an unsaved Off it would
  // mark messages read in a channel being turned off.
  const page = await clearingPage((page) => {
    page.upstreamClear.set(CHANNEL.id, {
      provider_name: "Google Chat", supported: true, mode: "on", failures: 0, pending: false,
    });
  });
  page.el("upstream-clear-mode").value = "off";
  await page.el("upstream-clear-mode").dispatch("change");
  assert.equal(page.el("upstream-clear-push").hidden, true, "offered beside an unsaved Off");
  page.el("upstream-clear-mode").value = "on";
  await page.el("upstream-clear-mode").dispatch("change");
  assert.equal(page.el("upstream-clear-push").hidden, false);
  // ...and a channel the service refused offers nothing to press.
  const refused = await clearingPage((page) => {
    page.upstreamClear.set(CHANNEL.id, {
      provider_name: "Google Chat", supported: true, mode: "on", failures: 0, pending: false,
      unavailable: "this channel is one thread of a larger space",
    });
  });
  assert.equal(refused.el("upstream-clear-push").hidden, true, "offered for a channel the service refused");
});

test("ONE PRESS IS ONE PUSH: disabled while it is in flight, progress and then the result in the status line", async () => {
  const page = await clearingPage((page) => {
    page.upstreamClear.set(CHANNEL.id, {
      provider_name: "Google Chat", supported: true, mode: "on", failures: 0, pending: false,
    });
  });
  page.holdUpstreamClearNow = true;
  const push = page.el("upstream-clear-push");
  const pressed = push.click();
  await page.settle();
  assert.equal(push.disabled, true, "the button can be pressed again while the first press is waiting");
  assert.equal(page.el("upstream-clear-state").textContent, "Marking the newest message read in Google Chat…");
  // A tap that reaches it anyway (a disabled button in a real browser fires nothing) sends nothing.
  await push.click();
  await page.settle();
  assert.deepEqual(page.upstreamClearCalls, [`GET ${CHANNEL.id}`, `POST ${CHANNEL.id} now`],
    "a second press sent a second push");
  assert.equal(page.el("upstream-clear-mode").value, "on", "pressing it changed the choice");
  page.holdUpstreamClearNow = false;
  page.pendingUpstreamClearNow.splice(0).forEach((release) => release());
  await pressed;
  await page.settle();
  assert.equal(push.disabled, false, "the button stayed disabled after the answer");
  assert.match(page.el("upstream-clear-state").textContent,
    /^Done: marked read in Google Chat through the message from .+\.$/);
  // Nothing was saved: the mode is the server's, untouched, and no PUT went out.
  assert.equal(page.upstreamClearCalls.filter((call) => call.startsWith("PUT")).length, 0);
  // Pressed again, one more push.
  await push.click();
  await page.settle();
  assert.equal(page.upstreamClearCalls.filter((call) => call.endsWith(" now")).length, 2);
});

test("TRY IT NOW in a dry run says what it would mark and that nothing was written", async () => {
  const page = await clearingPage((page) => {
    page.upstreamClear.set(CHANNEL.id, {
      provider_name: "Google Chat", supported: true, mode: "dry_run", failures: 0, pending: false,
    });
  });
  page.holdUpstreamClearNow = true;
  const pressed = page.el("upstream-clear-push").click();
  await page.settle();
  assert.equal(page.el("upstream-clear-state").textContent, "Trying it on the newest message in Google Chat…");
  page.holdUpstreamClearNow = false;
  page.pendingUpstreamClearNow.splice(0).forEach((release) => release());
  await pressed;
  await page.settle();
  assert.match(page.el("upstream-clear-state").textContent,
    /^Dry run: would mark it read in Google Chat through the message from .*\. Nothing was written\.$/);
});

test("the status line says what a press came to: nothing newer, a retry wait, still waiting, and a refusal", async () => {
  const on = { provider_name: "Google Chat", supported: true, mode: "on", failures: 0, pending: false };
  const failed = (error) => clearAttempt({ result: "failed", target: undefined, previous: undefined, error });
  const cases = [
    [(held) => ({ outcome: "covered", state: held }),
      /^Nothing newer to mark: the newest message here is already marked read in Google Chat\.$/],
    [(held) => ({ outcome: "no_message", state: held }), /^There is no message here to mark read yet\.$/],
    [(held) => ({
      outcome: "retry_waiting",
      state: { ...held, failures: 1, pending: true, retry_at_ms: Date.UTC(2026, 9, 11, 11, 0),
        last: clearAttempt({ result: "failed", target: undefined, previous: undefined,
          error: "Google Chat RATE LIMIT: quota exceeded" }) },
    }), /^Not sent yet: the last try failed \(Google Chat RATE LIMIT: quota exceeded\), so the newest message goes with the next one, at .+\.$/],
    // `#253 upstream-clear-polish`. The press's own push failed: said as a failure, never "Done".
    [(held) => ({
      outcome: "failed",
      attempt: failed("Google Chat returned HTTP 502: mark-read failed"),
      state: { ...held, failures: 1, pending: true, retry_at_ms: Date.UTC(2026, 9, 11, 11, 0),
        last: failed("Google Chat returned HTTP 502: mark-read failed") },
    }), /^Not done: the push failed: Google Chat returned HTTP 502: mark-read failed\. It is tried again by itself at .+\.$/],
    [(held) => ({ outcome: "pushed", attempt: clearAttempt({ result: "already_read" }),
      state: { ...held, last: clearAttempt({ result: "already_read" }) } }),
      /^Done: Google Chat already had it read through the message from .+, so nothing needed moving\.$/],
    // What the press came to is its own attempt, not whatever `state.last` holds by the time the
    // answer is read: here a later push has already failed, and the press's own push succeeded.
    [(held) => ({
      outcome: "pushed",
      attempt: clearAttempt(),
      state: { ...held, failures: 1, retry_at_ms: Date.UTC(2026, 9, 11, 11, 0), pushed: clearAttempt(),
        last: failed("Google Chat RATE LIMIT: quota exceeded") },
    }), /^Done: marked read in Google Chat through the message from .+\.$/],
    // "pushed" with no attempt to say it from claims nothing: the line is the channel's state.
    [(held) => ({ outcome: "pushed", state: { ...held, last: failed("Google Chat returned HTTP 502") } }),
      /^Failing since .*: Google Chat returned HTTP 502\./],
    [(held) => ({ outcome: "skipped", attempt: clearAttempt({ result: "skipped" }),
      state: { ...held, last: clearAttempt({ result: "skipped" }) } }),
      /^Nothing was marked: the bridge no longer holds the newest message\. The next message that arrives is marked\.$/],
    // Another device turned it off while the push was on its way.
    [(held) => ({ outcome: "changed", state: { ...held, mode: "off" } }),
      /^Not finished: this channel's setting changed while the push was on its way\. Off\. Nothing here is marked read in Google Chat\.$/],
    [(held) => ({ outcome: "unavailable", state: { ...held, unavailable: "this channel is one thread of a larger space" } }),
      /^Not available for this channel: this channel is one thread of a larger space\.$/],
  ];
  for (const [answer, said] of cases) {
    const page = await clearingPage((page) => {
      page.upstreamClear.set(CHANNEL.id, { ...on });
      page.upstreamClearNow = answer;
    });
    await page.el("upstream-clear-push").click();
    await page.settle();
    assert.match(page.el("upstream-clear-state").textContent, said);
    assert.equal(page.expireTimers(4000), 0, "asked again after an answer that needed no second look");
  }

  // Refused (another device turned it off meanwhile): said, and the button can be pressed again.
  const refused = await clearingPage((page) => { page.upstreamClear.set(CHANNEL.id, { ...on }); });
  refused.upstreamClearFailure = { status: 400, error: "refused", detail: "clearing unread is off for this channel" };
  await refused.el("upstream-clear-push").click();
  await refused.settle();
  assert.match(refused.el("upstream-clear-state").textContent, /^Not done: clearing unread is off for this channel/);
  assert.equal(refused.el("upstream-clear-push").disabled, false, "a refused press cannot be tried again");
});

/**
 * A clear box on a channel that is on, whose "Mark read now" the server answers `waiting`: the
 * push still on its way, the last attempt recorded being `before`. `#253 upstream-clear-polish`.
 */
async function waitingPress(before = null) {
  const on = { provider_name: "Google Chat", supported: true, mode: "on", failures: 0, pending: false,
    ...(before ? { last: before, pushed: before } : {}) };
  const page = await clearingPage((page) => {
    page.upstreamClear.set(CHANNEL.id, { ...on });
    page.upstreamClearNow = (held) => ({ outcome: "waiting", state: { ...held, pending: true } });
  });
  await page.el("upstream-clear-push").click();
  await page.settle();
  assert.match(page.el("upstream-clear-state").textContent, /^Still waiting for Google Chat to answer\./);
  return { page, on };
}

/** Let the box's next re-read happen, and say how many re-reads were armed for it. */
async function recheck(page) {
  const fired = page.expireTimers(4000);
  await page.settle();
  return fired;
}

test("STILL WAITING: the box asks again every few seconds until the press's own push answers it", async () => {
  // `#253 upstream-clear-polish`. One re-read was not enough: a bridge slower than four seconds
  // left the line describing the state before the push, for good.
  const older = clearAttempt({ at_ms: Date.UTC(2026, 9, 11, 9, 30), message_at_ms: Date.UTC(2026, 9, 11, 9, 29),
    message_id: "1110000000000000100" });
  const { page, on } = await waitingPress(older);
  const gets = () => page.upstreamClearCalls.filter((call) => call.startsWith("GET")).length;
  const before = gets();
  // Nothing new yet: asked, still waiting, and asked again.
  assert.equal(await recheck(page), 1, "nothing was armed to ask again");
  assert.equal(gets(), before + 1);
  assert.match(page.el("upstream-clear-state").textContent, /^Still waiting for Google Chat to answer\./);
  // An OLDER message's push landing does not answer the press, which is for a newer message.
  const between = clearAttempt({ at_ms: Date.UTC(2026, 9, 11, 10, 0), message_at_ms: Date.UTC(2026, 9, 11, 9, 59),
    message_id: "1110000000000000200" });
  page.upstreamClear.set(CHANNEL.id, { ...on, last: between, pushed: between, pending: true });
  assert.equal(await recheck(page), 1, "an older push's success ended the asking");
  assert.match(page.el("upstream-clear-state").textContent, /^Still waiting for Google Chat to answer\./);
  // The press's own push lands: said from that attempt, and the asking stops.
  page.upstreamClear.set(CHANNEL.id, { ...on, last: clearAttempt(), pushed: clearAttempt() });
  assert.equal(await recheck(page), 1);
  assert.match(page.el("upstream-clear-state").textContent,
    /^Done: marked read in Google Chat through the message from .+\.$/);
  assert.equal(await recheck(page), 0, "asked again after the press was answered");
});

test("...a press still waiting is said as failed, as waiting for a retry, or as changed, by what lands", async () => {
  const failed = (fields = {}) => clearAttempt({ result: "failed", target: undefined, previous: undefined,
    error: "Google Chat RATE LIMIT: quota exceeded", ...fields });
  // Its own push failed.
  {
    const { page, on } = await waitingPress();
    page.upstreamClear.set(CHANNEL.id, { ...on, failures: 1, pending: true,
      retry_at_ms: Date.UTC(2026, 9, 11, 11, 0), last: failed() });
    await recheck(page);
    assert.match(page.el("upstream-clear-state").textContent,
      /^Not done: the push failed: Google Chat RATE LIMIT: quota exceeded\. It is tried again by itself at .+\.$/);
    assert.equal(await recheck(page), 0);
  }
  // An older push failed after the press: the pressed message waits for the retry.
  {
    const { page, on } = await waitingPress();
    page.upstreamClear.set(CHANNEL.id, { ...on, failures: 1, pending: true,
      retry_at_ms: Date.UTC(2026, 9, 11, 11, 0),
      last: failed({ message_at_ms: Date.UTC(2026, 9, 11, 9, 0), message_id: "1110000000000000100" }) });
    await recheck(page);
    assert.match(page.el("upstream-clear-state").textContent,
      /^Not sent yet: the last try failed \(Google Chat RATE LIMIT: quota exceeded\), so the newest message goes with the next one, at .+\.$/);
    assert.equal(await recheck(page), 0);
  }
  // Another device turned it off meanwhile.
  {
    const { page, on } = await waitingPress();
    page.upstreamClear.set(CHANNEL.id, { ...on, mode: "off" });
    await recheck(page);
    assert.match(page.el("upstream-clear-state").textContent,
      /^Not finished: this channel's setting changed while the push was on its way\. Off\./);
    assert.equal(page.el("upstream-clear-mode").value, "off", "the box kept showing On");
    assert.equal(await recheck(page), 0);
  }
});

test("...and stops asking after thirty seconds, saying the push is still unanswered", async () => {
  const { page } = await waitingPress();
  const gets = () => page.upstreamClearCalls.filter((call) => call.startsWith("GET")).length;
  const before = gets();
  let asked = 0;
  while (await recheck(page)) asked += 1;
  assert.equal(asked, 7, "not asked every four seconds for thirty seconds");
  assert.equal(gets(), before + 7);
  assert.equal(page.el("upstream-clear-state").textContent,
    "Google Chat has not answered yet. The server carries on without this page; open Settings again later to see what it came to.");
  // ...and a read that fails along the way is asked again, not taken as an answer.
  const flaky = (await waitingPress()).page;
  flaky.upstreamClearFailure = { status: 502, error: "chat_error", detail: "bad gateway" };
  assert.equal(await recheck(flaky), 1);
  assert.match(flaky.el("upstream-clear-state").textContent, /^Still waiting for Google Chat to answer\./);
  assert.equal(await recheck(flaky), 1, "a failed re-read ended the asking");
});

test("...and only while Settings shows that channel: leaving it, saving, or pressing again stops the asking", async () => {
  // Leaving Settings.
  {
    const { page } = await waitingPress();
    await page.el("close-settings").click();
    await page.settle();
    const calls = page.upstreamClearCalls.length;
    await recheck(page);
    assert.equal(page.upstreamClearCalls.length, calls, "asked with Settings closed");
    assert.equal(await recheck(page), 0, "kept asking with Settings closed");
  }
  // Saving another mode: the owner's newest word, not the press, is what the line says.
  {
    const { page } = await waitingPress();
    page.el("upstream-clear-mode").value = "dry_run";
    await page.el("upstream-clear-mode").dispatch("change");
    await page.el("upstream-clear-save").click();
    await page.settle();
    assert.equal(await recheck(page), 0, "kept asking about the press after a save");
    assert.match(page.el("upstream-clear-state").textContent, /^Dry run\./);
  }
  // Pressing again: one press is asked about at a time.
  {
    const { page } = await waitingPress();
    await page.el("upstream-clear-push").click();
    await page.settle();
    assert.equal(await recheck(page), 1, "two presses were asked about at once");
  }
});

test("...and a mode chosen while the box asks stays chosen, with Save offered, until it is saved", async () => {
  // `#253 upstream-clear-polish`. Each re-read put the saved mode back in the select: a choice made
  // in the thirty seconds of asking went back to On, and Save went grey under the owner's hand.
  const { page, on } = await waitingPress();
  page.el("upstream-clear-mode").value = "off";
  await page.el("upstream-clear-mode").dispatch("change");
  assert.equal(page.el("upstream-clear-save").disabled, false);
  // Nothing new yet: asked, still waiting, and the choice left alone.
  assert.equal(await recheck(page), 1);
  assert.match(page.el("upstream-clear-state").textContent, /^Still waiting for Google Chat to answer\./);
  assert.equal(page.el("upstream-clear-mode").value, "off", "a re-read took back a mode chosen and not saved");
  assert.equal(page.el("upstream-clear-save").disabled, false, "a re-read left the unsaved choice nothing to save");
  assert.equal(page.el("upstream-clear-push").hidden, true, "Mark read now offered beside an unsaved Off");
  // The press's own push lands: said, and the choice still left alone.
  page.upstreamClear.set(CHANNEL.id, { ...on, last: clearAttempt(), pushed: clearAttempt() });
  assert.equal(await recheck(page), 1);
  assert.match(page.el("upstream-clear-state").textContent, /^Done: marked read in Google Chat through the message from .+\.$/);
  assert.equal(page.el("upstream-clear-mode").value, "off", "the answering read took back the unsaved choice");
  assert.equal(page.el("upstream-clear-save").disabled, false);
  // Saved, it is the saved mode...
  await page.el("upstream-clear-save").click();
  await page.settle();
  assert.equal(page.upstreamClearCalls.at(-1), `PUT ${CHANNEL.id} {"mode":"off"}`);
  assert.equal(page.el("upstream-clear-mode").value, "off");
  assert.equal(page.el("upstream-clear-save").disabled, true);
  // ...and the select follows the server again: another device's change is shown on the next read.
  page.upstreamClear.set(CHANNEL.id, { ...on, mode: "dry_run" });
  await page.el("close-settings").click();
  await page.el("open-settings").click();
  await page.settle();
  assert.equal(page.el("upstream-clear-mode").value, "dry_run", "a saved choice stopped the select following the server");
  assert.equal(page.el("upstream-clear-save").disabled, true);
});

test("...turning it off sends off and nothing else, and a failed save says so and keeps the choice", async () => {
  const page = await clearingPage((page) => {
    page.upstreamClear.set(CHANNEL.id, {
      provider_name: "Google Chat", supported: true, mode: "on", failures: 0, pending: false,
      pushed: clearAttempt(), last: clearAttempt(), previous_cursor: "2026-10-11T09:00:00Z",
    });
  });
  page.el("upstream-clear-mode").value = "off";
  await page.el("upstream-clear-mode").dispatch("change");
  assert.equal(page.el("upstream-clear-push").hidden, true, "Mark read now offered when turning it off");
  page.upstreamClearFailure = { status: 503, error: "storage_not_configured", detail: "set storage.path" };
  await page.el("upstream-clear-save").click();
  await page.settle();
  assert.match(page.el("upstream-clear-state").textContent, /^Not saved: set storage\.path/);
  assert.equal(page.el("upstream-clear-mode").value, "off", "the choice was thrown away by a failed save");
  assert.equal(page.el("upstream-clear-save").disabled, false, "a failed save cannot be tried again");
  page.upstreamClearFailure = null;
  await page.el("upstream-clear-save").click();
  await page.settle();
  assert.equal(page.upstreamClearCalls.at(-1), `PUT ${CHANNEL.id} {"mode":"off"}`);
  assert.match(page.el("upstream-clear-state").textContent, /^Off\. Earlier, vibe-talk marked it read/);
  assert.equal(page.expireTimers(4000), 0, "asked again after turning it off");
});

test("the status line says a dry run wrote nothing, a failure and its retry, and a channel it cannot clear", async () => {
  const cases = [
    [{ mode: "dry_run", last: clearAttempt({ result: "dry_run" }) },
      /^Dry run: would mark it read in Google Chat through the message from .*\. Nothing was written\.$/],
    [{ mode: "on", failures: 2, retry_at_ms: Date.UTC(2026, 9, 11, 11, 0),
      last: clearAttempt({ result: "failed", target: undefined, previous: undefined,
        error: "Google Chat returned HTTP 502: mark-read failed" }) },
      /^Failing since .*: Google Chat returned HTTP 502: mark-read failed\. Trying again at /],
    [{ mode: "on", unavailable: "this channel is one thread of a larger space" },
      /^Not available for this channel: this channel is one thread of a larger space\.$/],
    [{ mode: "on", pending: true },
      /^On\. The newest message, and everything before it, is marked read in Google Chat shortly\.$/],
    [{ mode: "on" },
      /^On\. When the next message arrives here, it and everything before it are marked read in Google Chat\.$/],
  ];
  for (const [state, said] of cases) {
    const page = await clearingPage((page) => {
      page.upstreamClear.set(CHANNEL.id, {
        provider_name: "Google Chat", supported: true, failures: 0, pending: false, ...state,
      });
    });
    assert.equal(page.el("upstream-clear-mode").value, state.mode);
    assert.match(page.el("upstream-clear-state").textContent, said);
  }
});

test("opening Settings again reads the server's state again, and a read failure is said, not guessed", async () => {
  const page = await clearingPage();
  await page.el("close-settings").click();
  page.upstreamClearFailure = { status: 502, error: "chat_error", detail: "the bridge is restarting" };
  await page.el("open-settings").click();
  await page.settle();
  assert.deepEqual(page.upstreamClearCalls, [`GET ${CHANNEL.id}`, `GET ${CHANNEL.id}`]);
  assert.match(page.el("upstream-clear-state").textContent, /^Could not read this setting: the bridge is restarting/);
});

test("`#240 reading-bookmark`: b and Shift+B are in the table of keys, so Help, aria-keyshortcuts and the tooltips say them", async () => {
  const page = await keyboardPage();
  assert.equal(page.el("jump-bookmark").getAttribute("aria-keyshortcuts"), "B",
    "the chip does not say its keys to a screen reader");
  const item = rowMoreMenu(row(page, 0)).children.find((node) => node.className === "row-bookmark-button");
  assert.equal(item.getAttribute("aria-keyshortcuts"), "Shift+B", "the ⋯ item does not say its key");
  const keys = page.el("keyboard-keys").text();
  assert.match(keys, /bBack to your bookmark/, "the list of keys does not name b");
  assert.doesNotMatch(keys, /b or m/, "the list of keys still gives the bookmark m (`#538 c-key-copy`: the message box's)");
  assert.match(keys, /Shift\+B/, "the list of keys does not name Shift+B");
  assert.match(keys, /Marks nothing Done/, "the list does not say setting the bookmark marks nothing Done");
  // Single keys: never while typing in a box, and off with the switch.
  const box = page.el("channel-compose-text");
  await press(page, box, "B", { shiftKey: true });
  await press(page, box, "b");
  assert.deepStrictEqual(page.bookmarkCalls, [], "a key typed into the message box moved the bookmark");
  await page.el("single-keys").setChecked(false);
  await page.settle();
  await press(page, page.el("scroll-area"), "ArrowDown");
  await pressHere(page, "B", { shiftKey: true });
  assert.deepStrictEqual(page.bookmarkCalls, [], "Shift+B fired with single-key shortcuts off");
  assert.equal(page.el("jump-bookmark").hasAttribute("aria-keyshortcuts"), false);
});

test("`#240 reading-bookmark`: the chip points toward the line, goes when the line is on screen, and is never over the call", async () => {
  const page = await keyboardPageAtTop(tallChannel(30));
  await bookmarkRow(page, "15");
  const area = page.el("scroll-area");
  const chip = page.el("jump-bookmark");
  area.scrollTop = 0;
  area.dispatch("scroll");
  await page.settle();
  assert.equal(chip.hidden, false, "the chip is not offered with the line below the screen");
  assert.equal(chip.getAttribute("data-direction"), "down");
  assert.equal(chip.getAttribute("aria-label"), "Jump to your bookmark, below");
  area.scrollTop = area.scrollHeight;
  await area.dispatch("scroll");
  assert.equal(chip.getAttribute("data-direction"), "up", "the chip points away from the line");
  await chip.click();
  await page.settle();
  assert.equal(chip.hidden, true, "the chip stayed after taking the reader to the line");
  assert.equal(selectedId(page), "15", "the chip did not select the message under the line");
  area.scrollTop = area.scrollHeight;
  await area.dispatch("scroll");
  assert.equal(chip.hidden, false);
  await page.el("view-switch").click();
  await page.settle();
  assert.equal(chip.hidden, true, "the chip is offered over the call's transcript");
});

test("`#240 reading-bookmark`: Done through here at or below the line slides it below that message; above it, the line stays", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog(6));
  await bookmarkRow(page, bid(1));
  await throughHere(page, bid(3));
  assert.deepStrictEqual(page.dismissCalls.map((call) => call.messages), [[bid(0), bid(1), bid(2), bid(3)]]);
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(4), place: "above" }, "the line did not land below the last message marked");
  const slide = page.bookmarkCalls[page.bookmarkCalls.length - 1];
  assert.equal(slide.body.message_id, bid(4));
  assert.equal(slide.ifRevision, 1, "the slide was not conditional");
  // A line below the message the act stops at stays where it is: nothing above it was unread.
  const other = newPage();
  await signIn(other);
  await showDiscord(other, backlog(6));
  await bookmarkRow(other, bid(4));
  const calls = other.bookmarkCalls.length;
  await throughHere(other, bid(2));
  assert.deepStrictEqual(bookmarkAt(other), { id: bid(4), place: "above" }, "a Done above the line moved it");
  assert.equal(other.bookmarkCalls.length, calls);
  // Shift+E is not this act any more (`#295 shift-e-read-collapse`): it marks the one message, and the
  // line moves as e's Done moves it, from the message right under it to the next one still to read.
  const keys = newPage();
  await signIn(keys);
  await showDiscord(keys, backlog(6));
  keys.el("scroll-area").scrollTop = 0;
  await bookmarkRow(keys, bid(1));
  await press(keys, keys.el("scroll-area"), "ArrowDown");
  await pressHere(keys, "j");
  assert.equal(selectedId(keys), bid(1));
  await pressHere(keys, "E", { shiftKey: true });
  assert.deepStrictEqual(keys.dismissCalls.map((call) => call.messages), [[bid(1)]],
    "Shift+E marked more than the message it was pressed on");
  assert.deepStrictEqual(bookmarkAt(keys), { id: bid(2), place: "above" }, "Shift+E did not slide the line as e does");
  assert.equal(selectedId(keys), bid(1), "Shift+E moved the selection off the message");
});

test("`#240 reading-bookmark`: e moves the bookmark and the selection together, and z takes both back", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog(4));
  page.el("scroll-area").scrollTop = 0;
  await bookmarkRow(page, bid(1));
  await press(page, page.el("scroll-area"), "ArrowDown");
  await pressHere(page, "j");
  assert.equal(selectedId(page), bid(1));
  await pressHere(page, "e");
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(2), place: "above" });
  assert.equal(selectedId(page), bid(2), "e left the selection behind the bookmark");
  const moved = page.bookmark.revision;
  await pressHere(page, "z");
  await page.settle();
  assert.deepStrictEqual(page.restoreCalls.map((call) => call.messages), [[bid(1)]]);
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(1), place: "above" }, "Undo left the bookmark where the Done moved it");
  const back = page.bookmarkCalls[page.bookmarkCalls.length - 1];
  assert.deepStrictEqual([back.body.message_id, back.ifRevision], [bid(1), moved], "the undo of the slide was not conditional");
  // A bookmark moved since the Done is left alone by its undo.
  await doneButton(channelRow(page, bid(1))).click();
  await page.settle();
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(2), place: "above" });
  await bookmarkRow(page, bid(3));
  const calls = page.bookmarkCalls.length;
  await page.el("undo-dismiss").click();
  await page.settle();
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(3), place: "above" }, "Undo undid a move made after the Done");
  assert.equal(page.bookmarkCalls.length, calls);
});

test("`#240 reading-bookmark`: a bookmark write that fails is taken back and said; a Done that fails takes its slide back unsent", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog(3));
  page.bookmarkFailure = { status: 500, error: "storage_error", detail: "the disk is full" };
  await bookmarkRow(page, bid(1));
  assert.equal(bookmarkAt(page), null, "a bookmark the server refused stayed on screen");
  assert.equal(statusText(page), "The bookmark was not saved.");
  assert.match(page.el("error").text(), /could not be saved: the disk is full/, "the reason was not said");
  page.bookmarkFailure = null;
  await bookmarkRow(page, bid(1));
  const calls = page.bookmarkCalls.length;
  page.inboxWriteHook = async (kind) => (kind === "dismiss"
    ? json(502, { error: "store_error", detail: "the store is read-only" }) : undefined);
  await doneButton(channelRow(page, bid(1))).click();
  await page.settle();
  page.inboxWriteHook = null;
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(1), place: "above" }, "the slide outlived the Done that failed");
  assert.equal(page.bookmarkCalls.length, calls, "the slide of a Done that failed was sent anyway");
});

test("`#240 reading-bookmark`: a slide that lost to the other device takes where the server says the bookmark is", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog(5));
  await bookmarkRow(page, bid(1));
  // The owner moved it on his phone; this page has not read since.
  page.moveBookmarkElsewhere({ message_id: bid(4), place: "above", timestamp: page.messages[4].timestamp });
  await doneButton(channelRow(page, bid(1))).click();
  await page.settle();
  const slide = page.bookmarkCalls[page.bookmarkCalls.length - 1];
  assert.equal(slide.ifRevision, 1);
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(4), place: "above" }, "the refused slide was kept over the other device's move");
  assert.equal(page.bookmark.anchor.message_id, bid(4), "the page overwrote the other device's move");
  assert.equal(statusText(page), "Bookmark moved on another device.");
});

test("`#240 reading-bookmark`: the next read shows a move made on the other device; a read sent before this page's own move does not take it back", async () => {
  const page = await servedBookmarkPage(backlog(5));
  assert.equal(bookmarkAt(page), null);
  page.moveBookmarkElsewhere({ message_id: bid(3), place: "above", timestamp: page.messages[3].timestamp });
  await reReadChannel(page);
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(3), place: "above" }, "a read carrying another device's move drew nothing");
  // This page moves it, and a read lands while that write is still on its way.
  page.holdBookmarkWrites = true;
  await bookmarkRow(page, bid(1));
  await reReadChannel(page);
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(1), place: "above" }, "a read from before the move took it back");
  page.holdBookmarkWrites = false;
  for (const release of page.pendingBookmarkWrites.splice(0)) release();
  await page.settle();
  await reReadChannel(page);
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(1), place: "above" });
  assert.equal(page.bookmark.anchor.message_id, bid(1));
});

test("`#240 reading-bookmark`: the old place marker is moved to the server once, then dropped; a channel with a bookmark keeps its own", async () => {
  const store = new Map([["vibe-talk.voice.place-marker", bid(2)]]);
  const page = newPage(store, SCRIPT, (p) => {
    p.serveBookmark = true;
  });
  await signIn(page);
  await showDiscord(page, backlog(4));
  await page.settle();
  assert.deepStrictEqual(page.bookmarkCalls.map((call) => [call.method, call.body.message_id, call.ifRevision]),
    [["PUT", bid(2), 0]], "the old place was not moved to the server, conditionally on there being none");
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(2), place: "above" });
  assert.equal(store.has("vibe-talk.voice.place-marker"), false, "the browser's copy was kept");
  const kept = new Map([["vibe-talk.voice.place-marker", bid(2)]]);
  const other = newPage(kept, SCRIPT, (p) => {
    p.serveBookmark = true;
    p.bookmark = { revision: 3, anchor: { message_id: bid(0), place: "above", timestamp: "2026-08-19T04:00:00.000Z" } };
  });
  await signIn(other);
  await showDiscord(other, backlog(4));
  await other.settle();
  assert.deepStrictEqual(other.bookmarkCalls, [], "the old place overwrote the bookmark the server has");
  assert.deepStrictEqual(bookmarkAt(other), { id: bid(0), place: "above" });
  assert.equal(kept.has("vibe-talk.voice.place-marker"), false);
});

test("`#240 reading-bookmark`: an old place marker whose message is never read again is dropped after a bounded number of reads", async () => {
  const store = new Map([["vibe-talk.voice.place-marker", "8999999999999999999"]]);
  const page = newPage(store, SCRIPT, (p) => {
    p.serveBookmark = true;
  });
  await signIn(page);
  await showDiscord(page, backlog(4));
  for (let read = 0; read < 3; read += 1) await reReadChannel(page);
  assert.equal(store.get("vibe-talk.voice.place-marker"), "8999999999999999999",
    "the old place was dropped before it had been looked for long enough");
  for (let read = 0; read < sourceConstant("LEGACY_MARKER_READS"); read += 1) await reReadChannel(page);
  assert.equal(store.has("vibe-talk.voice.place-marker"), false, "an old place no read brings is looked for forever");
  // ...and its message turning up later moves nothing: it was given up on.
  page.messages = [...page.messages, message({ id: "8999999999999999999", content: "the old place" })];
  await reReadChannel(page);
  assert.deepStrictEqual(page.bookmarkCalls, [], "an old place given up on was moved to the server");
  assert.equal(bookmarkAt(page), null);
});

test("`#240 reading-bookmark`: the device's copy of a channel keeps the bookmark, so a page opened before any read draws the line, and the first read says where it is now", async () => {
  for (const threads of [false, true]) {
    const mode = threads ? "timeline mode" : "page mode";
    const first = newPage(new Map(), SCRIPT, (p) => {
      p.serveBookmark = true;
      p.threadingSupported = threads;
    });
    await signIn(first);
    await showDiscord(first, backlog(5));
    await bookmarkRow(first, bid(2));
    let reads = null;
    const page = reloadPage(first.storage, (p) => {
      p.serveBookmark = true;
      p.threadingSupported = threads;
      p.messages = first.messages;
      p.bookmark = { ...first.bookmark };
      const route = threads ? "timeline" : "channelPage";
      reads = gate(p[route]);
      p[route] = reads.respond;
    });
    // Synchronously, from the device: nothing has answered.
    assert.deepStrictEqual(shownIds(page), [0, 1, 2, 3, 4].map(bid), `${mode}: the saved rows were not drawn`);
    assert.deepStrictEqual(bookmarkAt(page), { id: bid(2), place: "above" }, `${mode}: a page opened from the device's copy drew no line`);
    await page.settle();
    assert.deepStrictEqual(bookmarkAt(page), { id: bid(2), place: "above" }, `${mode}: sign-in took the saved line away`);
    // Moved on the other device meanwhile: the first read says so, over the saved copy.
    page.bookmark = { revision: page.bookmark.revision + 1,
      anchor: { message_id: bid(4), place: "above", timestamp: page.messages[4].timestamp } };
    reads.open();
    await page.settle();
    await page.settle();
    assert.deepStrictEqual(bookmarkAt(page), { id: bid(4), place: "above" }, `${mode}: the saved line outlived the read`);
    assert.deepStrictEqual(page.bookmarkCalls, [], `${mode}: drawing the saved line wrote the bookmark`);
  }
});

test("`#240 reading-bookmark`: Main places the line by time for a thread reply, All above it, its thread too, and another thread draws none", async () => {
  const page = await threadPage();
  assert.equal(page.el("thread-select").value, "flat");
  await bookmarkRow(page, "202");
  assert.deepStrictEqual(bookmarkAt(page), { id: "202", place: "above" }, "All did not draw the line above the reply");
  assert.equal(page.bookmarkCalls[0].body.thread_id, "spaces/A/threads/one & two", "the anchor lost its thread");
  await pickThread(page, "main");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"]);
  assert.deepStrictEqual(bookmarkAt(page), { id: "203", place: "above" }, "Main did not place the reply's line by time");
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  assert.deepStrictEqual(bookmarkAt(page), { id: "202", place: "above" }, "the reply's own thread drew no line");
  await press(page, page.el("thread-back"), "Escape");
  await page.settle();
  await threadBadge(rowWithId(page, "203")).click();
  await page.settle();
  assert.equal(bookmarkAt(page), null, "another thread drew the line");
  assert.equal(page.el("jump-bookmark").hidden, true, "the chip offers a line this list does not draw");
  await press(page, page.el("scroll-area"), "b");
  assert.equal(statusText(page), "Your bookmark is not in this thread.", "b did not say why it did nothing");
});

test("`#240 reading-bookmark`: under Fold read the line goes above a run it heads or after one it is inside; under Hide read, to the next message shown", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog(6));
  for (const n of [1, 2, 3]) page.dealtWith.add(bid(n));
  await reReadChannel(page);
  await bookmarkRow(page, bid(2));
  const mode = () => page.el("todo-filter").getAttribute("data-read-mode");
  await page.el("todo-filter").click();
  await page.settle();
  assert.equal(mode(), "fold");
  assert.equal(channelRow(page, bid(1)).getAttribute("data-read-run"), "head", "the fixture made no run");
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(4), place: "above" },
    "a line inside a collapsed run did not drop to the next message shown");
  await page.el("todo-filter").click();
  await page.settle();
  assert.equal(mode(), "hide");
  assert.deepStrictEqual(shownIds(page), [bid(0), bid(4), bid(5)]);
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(4), place: "above" }, "Hide read did not drop the line to the next message shown");
  assert.equal(page.bookmark.anchor.message_id, bid(2), "drawing the line elsewhere moved the bookmark");
  await page.el("todo-filter").click();
  await page.settle();
  assert.equal(mode(), "show");
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(2), place: "above" }, "Show read did not draw the line on its own message");
  await bookmarkRow(page, bid(1));
  await page.el("todo-filter").click();
  await page.settle();
  assert.equal(mode(), "fold");
  assert.equal(channelRow(page, bid(1)).getAttribute("data-read-run"), "head");
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(1), place: "above" }, "the line is not above the run it heads");
});

test("`#240 reading-bookmark`: under Hide read, a bookmark on a read message older than every message shown goes above the first, and one older than the window draws nothing", async () => {
  // Without threads, Hide read is the server's to-do list: the messages of its recent window not
  // dealt with. A read message that list leaves out is still in the window when it is no older than
  // the window's oldest message, and then every message between it and the list is read too.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog(6));
  await bookmarkRow(page, bid(1));
  for (const n of [0, 1, 2, 3]) page.dealtWith.add(bid(n));
  await chooseReadMode(page, "Hide read");
  assert.deepStrictEqual(shownIds(page), [bid(4), bid(5)]);
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(4), place: "above" },
    "a bookmark on a read message older than the list drew no line under Hide read");
  await press(page, page.el("scroll-area"), "b");
  assert.equal(selectedId(page), bid(4), "b did not go to the line");
  assert.equal(page.bookmark.anchor.message_id, bid(1), "drawing the line elsewhere moved the bookmark");
  // A window that no longer reaches the bookmark's message cannot say what lies between: no line.
  page.todoWindowFrom = page.messages[2].timestamp;
  await reReadChannel(page);
  assert.equal(bookmarkAt(page), null, "a line was drawn for a bookmark older than the window");
  await press(page, page.el("scroll-area"), "b");
  assert.equal(statusText(page), "Not in this list — Hide read is on.");
});

test("`#240 reading-bookmark`: Shift+B on a thread's card, in the Threads list or on the right, says there are no bookmarks and writes nothing", async () => {
  // A card is a thread, not a message, and holds none: there is nothing to put the line above. Since
  // `#584 thread-view-bookmark-chip` the key says so, where it used to be left to the browser in silence.
  const page = await threadPage();
  await page.el("channel-view-threads").click();
  await page.settle();
  const card = page.el("thread-list").children[0];
  await card.descendants().find((node) => node.className === "thread-open").focus();
  let event = await press(page, focused(page), "B", { shiftKey: true });
  assert.equal(event.defaulted, false, "Shift+B on a card in the Threads list was left to the browser");
  assert.equal(statusText(page), "No bookmarks in thread view.", "Shift+B on a card said something else");
  const two = await threadPage();
  await twoColumns(two);
  const side = two.el("side-cards").children[0];
  side.focus();
  event = await press(two, side, "B", { shiftKey: true });
  assert.equal(event.defaulted, false, "Shift+B on a card on the right was left to the browser");
  assert.equal(statusText(two), "No bookmarks in thread view.");
  assert.deepStrictEqual([...page.bookmarkCalls, ...two.bookmarkCalls], [], "Shift+B on a card wrote a bookmark");
});

test("`#240 reading-bookmark`: in two columns the left draws the line, the right only on the anchor's thread, and b goes to the left", async () => {
  const { page } = await twoColumnServer();
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  assert.deepStrictEqual(sideIds(page), ["201", "202"]);
  await bookmarkRow(page, "202", "side-log");
  assert.deepStrictEqual(bookmarkAt(page, "side-log"), { id: "202", place: "above" }, "the thread on the right drew no line");
  assert.deepStrictEqual(bookmarkAt(page), { id: "203", place: "above" }, "Main on the left did not place the line by time");
  await threadBadge(rowWithId(page, "203")).click();
  await page.settle();
  assert.deepStrictEqual(sideIds(page), ["203", "204"]);
  assert.equal(bookmarkAt(page, "side-log"), null, "the right column drew the line on a thread that does not hold it");
  await press(page, page.el("side-scroll"), "b");
  assert.equal(selectedId(page), "203", "b from the right column did not go to the left column's line");
});

/** Every X on a line in `list`, drawn or not: one row carries the line at most. */
const bookmarkClears = (page, list = "discord-log") =>
  page.el(list).descendants().filter((node) => node.className === "bookmark-clear");

/** The saved copy of the channel on this device, as a page opened offline would draw it. */
const savedScope = (page) => {
  const raw = page.storage.get(MESSAGE_CACHE_KEY);
  const scopes = raw ? JSON.parse(raw).scopes : {};
  return Object.values(scopes)[0] || null;
};

test("`#420 bookmark-clear`: the X on the line takes the bookmark away, here, on the server and in the saved copy, and the reader stays put", async () => {
  const page = await servedBookmarkPage(backlog(6));
  assert.deepStrictEqual(bookmarkClears(page), [], "an X is drawn with no bookmark");
  await bookmarkRow(page, bid(2));
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(2), place: "above" });
  assert.equal(savedScope(page).bookmark.anchor.message_id, bid(2), "the premise: the device keeps the bookmark");
  const x = bookmarkClearOf(page);
  assert.ok(x, "the line carries no X");
  assert.equal(x.tagName.toLowerCase(), "button");
  assert.equal(x.getAttribute("type"), "button");
  assert.equal(x.getAttribute("aria-label"), "Remove bookmark", "the X has no name for a screen reader");
  assert.equal(x.getAttribute("title"), "Remove bookmark", "the X has no tooltip");
  // The word is the separator; the X beside it, never inside one, whose children a screen reader skips.
  const line = bookmarkAt(page, "discord-log", true);
  const word = line.children.find((node) => node.getAttribute("role") === "separator");
  assert.ok(word && word.getAttribute("aria-label") === "Bookmark", "the line's word is not a separator named Bookmark");
  assert.equal(line.getAttribute("role"), null, "the X is inside a separator");

  const area = page.el("scroll-area");
  area.scrollTop = Math.round((area.scrollHeight - area.clientHeight) / 2);
  const scrolled = area.scrollTop;
  const lined = rowHoldingId(page, bid(2));
  const top = lined.getBoundingClientRect().top;
  const calls = page.bookmarkCalls.length;
  const folded = rowStates(page);
  const stopped = await tapBookmarkClear(page);

  assert.equal(stopped, true, "the X's tap went on to the row, which folds it or reads it aloud");
  assert.equal(bookmarkAt(page), null, "the X left the line");
  assert.deepStrictEqual(bookmarkClears(page), [], "the X stayed with no bookmark");
  assert.deepStrictEqual(page.bookmarkCalls.slice(calls).map((call) => [call.method, call.ifRevision]), [["DELETE", undefined]],
    "the removal was not written to the server, unconditionally, once");
  assert.equal(page.bookmark.anchor, null, "the server still holds the bookmark");
  assert.equal(savedScope(page).bookmark.anchor, null, "the device's saved copy still draws the line offline");
  assert.equal(area.scrollTop, scrolled, "taking the line away moved the list");
  assert.equal(rowHoldingId(page, bid(2)).getBoundingClientRect().top, top, "the message under the line moved");
  assert.deepStrictEqual(rowStates(page), folded, "the X's tap folded, selected or marked a row");
  assert.deepStrictEqual(page.dismissCalls, [], "removing the bookmark marked something Done");
  assert.match(statusText(page), /Bookmark removed/);
  // b now has nowhere to go, and says how to set one.
  await page.el("jump-bookmark").click();
  await page.settle();
  assert.equal(page.el("jump-bookmark").hidden, true);
});

test("`#420 bookmark-clear`: Undo puts the bookmark back, conditionally, for a few seconds, never over another device's", async () => {
  const page = await servedBookmarkPage(backlog(6));
  await bookmarkRow(page, bid(3));
  const undo = page.el("undo-bookmark");
  assert.equal(undo.hidden, true, "Undo is offered with nothing removed");
  await tapBookmarkClear(page);
  assert.equal(undo.hidden, false, "no Undo after the X");
  const revision = page.bookmark.revision;
  await undo.click();
  await page.settle();
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(3), place: "above" }, "Undo did not put the line back where it was");
  assert.equal(page.bookmark.anchor && page.bookmark.anchor.message_id, bid(3), "the server did not get it back");
  const back = page.bookmarkCalls[page.bookmarkCalls.length - 1];
  assert.deepStrictEqual([back.method, back.body.message_id, back.ifRevision], ["PUT", bid(3), revision],
    "Undo was not written conditionally on the removal's revision");
  assert.equal(undo.hidden, true, "Undo stayed up after it was used");
  assert.ok(bookmarkClearOf(page), "the line came back without its X");

  // The chip goes by itself after BOOKMARK_UNDO_MS.
  await tapBookmarkClear(page);
  assert.equal(undo.hidden, false);
  assert.equal(page.expireTimers(sourceConstant("BOOKMARK_UNDO_MS")), 1, "no timer was armed to take Undo away");
  await page.settle();
  assert.equal(undo.hidden, true, "Undo stayed up past its few seconds");
  assert.equal(bookmarkAt(page), null);

  // Another device set one meanwhile: Undo keeps that one.
  await bookmarkRow(page, bid(1));
  await tapBookmarkClear(page);
  page.moveBookmarkElsewhere({ message_id: bid(4), place: "above", timestamp: page.messages[4].timestamp });
  const calls = page.bookmarkCalls.length;
  await undo.click();
  await page.settle();
  assert.equal(page.bookmark.anchor && page.bookmark.anchor.message_id, bid(4), "Undo moved a bookmark set on another device");
  assert.equal(page.bookmarkCalls.length, calls + 1);
  assert.equal(page.bookmarkCalls[calls].ifRevision !== undefined, true, "Undo was written unconditionally");
});

test("`#420 bookmark-clear`: Undo pressed while the removal is still on its way is written after it, on the revision it answered", async () => {
  const page = await servedBookmarkPage(backlog(6));
  await bookmarkRow(page, bid(2));
  page.holdBookmarkWrites = true;
  await tapBookmarkClear(page);
  assert.equal(bookmarkAt(page), null);
  await page.el("undo-bookmark").click();
  await page.settle();
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(2), place: "above" }, "Undo did not draw the line at once");
  page.holdBookmarkWrites = false;
  for (const release of page.pendingBookmarkWrites.splice(0)) release();
  await page.settle();
  await page.settle();
  const [removal, back] = page.bookmarkCalls.slice(-2);
  assert.equal(removal.method, "DELETE");
  assert.deepStrictEqual([back.method, back.body.message_id, back.ifRevision], ["PUT", bid(2), page.bookmark.revision - 1],
    "Undo was not written on the revision the removal answered");
  assert.equal(page.bookmark.anchor && page.bookmark.anchor.message_id, bid(2), "the server did not end with the bookmark back");
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(2), place: "above" }, "the removal's answer took the line away again");
});

test("`#420 bookmark-clear`: Undo is offered only in the removal's channel, goes with a new bookmark, and with the credential", async () => {
  const page = await servedBookmarkPage(backlog(6), (p) => {
    p.channels = [{ ...CHANNEL }, { ...OTHER_CHANNEL }];
  });
  const undo = page.el("undo-bookmark");
  await bookmarkRow(page, bid(2));
  await tapBookmarkClear(page);
  assert.equal(undo.hidden, false);
  await readChannel(page, OTHER_CHANNEL.id);
  assert.equal(undo.hidden, true, "another channel offers to put this channel's bookmark back");
  await readChannel(page, CHANNEL.id);
  assert.equal(undo.hidden, false, "back in its channel within the few seconds, Undo is gone");

  // A bookmark set since is the one kept, and Undo goes with the setting.
  await bookmarkRow(page, bid(4));
  assert.equal(undo.hidden, true, "Undo stayed up over a bookmark set since");
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(4), place: "above" });

  // Another credential: nothing of the first one's is put back under it.
  await tapBookmarkClear(page);
  assert.equal(undo.hidden, false);
  page.el("api-token").value = "write-token-bbbbbbbbbbbbbbbb";
  await page.el("save-token").click();
  await page.settle();
  assert.equal(undo.hidden, true, "another credential is offered the first one's Undo");
});

test("`#420 bookmark-clear`: a removal that fails after Undo was pressed says nothing: the line shown is Undo's", async () => {
  const page = await servedBookmarkPage(backlog(6));
  await bookmarkRow(page, bid(2));
  page.holdBookmarkWrites = true;
  await tapBookmarkClear(page);
  await page.el("undo-bookmark").click();
  await page.settle();
  page.bookmarkFailure = { status: 503, error: "store_unavailable", detail: "the store is restarting" };
  const [removal] = page.pendingBookmarkWrites.splice(0, 1);
  removal();
  await page.settle();
  assert.doesNotMatch(page.el("status").textContent, /not saved/, "a failure Undo already took back was said");
  page.bookmarkFailure = null;
  page.holdBookmarkWrites = false;
  for (const release of page.pendingBookmarkWrites.splice(0)) release();
  await page.settle();
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(2), place: "above" });
  assert.equal(page.bookmark.anchor && page.bookmark.anchor.message_id, bid(2));
});

test("`#420 bookmark-clear`: a removal the server refuses comes back, with no Undo; Shift+B's removal offers Undo too, and Help says the X", async () => {
  const page = await servedBookmarkPage(backlog(6));
  await bookmarkRow(page, bid(2));
  page.bookmarkFailure = { status: 503, error: "store_unavailable", detail: "the store is restarting" };
  await tapBookmarkClear(page);
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(2), place: "above" }, "a refused removal left the line gone");
  assert.equal(page.el("undo-bookmark").hidden, true, "Undo is offered for a removal that did not happen");
  page.bookmarkFailure = null;

  const keys = await keyboardPageAtTop(tallChannel(8));
  await bookmarkRow(keys, "3");
  await press(keys, keys.el("scroll-area"), "ArrowDown");
  await pressHere(keys, "j");
  await pressHere(keys, "j");
  assert.equal(selectedId(keys), "3");
  await pressHere(keys, "B", { shiftKey: true });
  await keys.settle();
  assert.equal(bookmarkAt(keys), null, "Shift+B on the message under the line did not take it away");
  assert.equal(keys.el("undo-bookmark").hidden, false, "Shift+B's removal offers no Undo");
  assert.match(keys.el("keyboard-keys").text(), /the X on its line/, "Help does not say the line's X does the same");
});

test("`#420 bookmark-clear`: the X on the right column's line takes the bookmark away in both columns", async () => {
  const { page } = await twoColumnServer();
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  await bookmarkRow(page, "202", "side-log");
  assert.deepStrictEqual(bookmarkAt(page, "side-log"), { id: "202", place: "above" });
  assert.ok(bookmarkAt(page), "the premise: the left column draws the line too");
  assert.equal(await tapBookmarkClear(page, "side-log"), true, "the X's tap went on to the row");
  assert.equal(bookmarkAt(page, "side-log"), null, "the right column kept its line");
  assert.equal(bookmarkAt(page), null, "the left column kept its line");
  assert.equal(page.bookmark.anchor, null);
  assert.equal(page.el("undo-bookmark").hidden, false, "no Undo after the right column's X");
});

test("`#420 bookmark-clear`: the X is a touch target in the line's room, the one part of the line that takes a tap", () => {
  const x = cssBlock("#discord-log .bookmark-clear");
  assert.match(x, /width:\s*max\(24px, 1\.75rem\)/, "the X is narrower than a 24px touch target");
  assert.match(x, /height:\s*max\(24px, 1\.75rem\)/, "the X is shorter than a 24px touch target");
  assert.match(x, /pointer-events:\s*auto/, "the line's pointer-events: none reaches the X");
  assert.match(x, /order:\s*1/, "the X is not at the line's end, after its right-hand rule");
  assert.match(cssBlock("#discord-log .bookmark-line"), /pointer-events:\s*none/, "the line itself takes taps");
  for (const list of ["side-log", "side-cards"]) {
    assert.match(cssBlock(`#${list} .bookmark-clear`), /pointer-events:\s*auto/, `the X in #${list} is not drawn`);
  }
});

// --- a filter changed: the selected message stays where it is (#243 filter-keeps-selection) ----------
//
// The owner, 2026-10-12: "jump to my last pinned message: go to search, filter for pinned, select the
// message. But then as soon as I clear the filter, the screen jumps and I don't have the message I just
// found. The rule here is that when you adjust the filter, the highlighted message MUST stay on the
// screen. Ideally it stays on the screen in the same Y location without a "jump."" Each kind of filter
// change below — the search text cleared, the search closed by Escape or the glass, Pinned, Links, the
// kinds of link, the read modes — is measured by the selected message's id and how far below the top of
// the list's box it stands, before and after, within a pixel. tests/filter_place_browser.py does the
// owner's own sequence in Chromium at a desk's size and a phone's.

const FILTER_WALK_PAGES = sourceConstant("FILTER_WALK_PAGES");

/** Scroll `scroller` so that message `id` of `list` stands `y` px below its top. */
function parkAt(page, id, y, list = "discord-log", scroller = "scroll-area") {
  page.el(scroller).scrollTop += yOf(page, id, list, scroller) - y;
  assert.ok(Math.abs(yOf(page, id, list, scroller) - y) <= 1, `message ${id} cannot stand ${y}px down: the list ends`);
}

test("`#243 filter-keeps-selection`: a message selected among a search's matches stays where it is when the text is cleared", async () => {
  const run = async (script) => {
    const page = newPage(new Map(), script);
    await signIn(page);
    await showDiscord(page, placeChannel());
    await search(page, "deploy");
    await page.settle();
    parkAt(page, "25", 150);
    await clickRow(page, "25");
    const y = yOf(page, "25");
    const top = page.el("scroll-area").scrollTop;
    // The field's own clear button: the focus is in the field, and the text goes all at once.
    page.el("search-field").focus();
    await page.el("search-field").setValue("");
    await page.settle();
    return { page, y, top };
  };
  const { page, y, top } = await run(SCRIPT);
  keptAt(page, "25", y, "clearing the search text");
  assert.ok(page.el("scroll-area").scrollTop - top > SCROLL_VIEWPORT_PX,
    "the fixture brought back no screenful of rows above the message, so nothing could have moved it");
  assert.deepStrictEqual(litIds(page), ["25"], "the message is not lit as the selection once the search went");
  assert.equal(focused(page), page.el("search-field"), "clearing the text took the focus out of the field");
  // THE CONTROL: without the placing, the same offset in the whole list is somewhere else entirely.
  const unplaced = await run(brokenScript(
    "        if (!landed || currentFilterHold() !== null) placeFilterAnchor(anchor, wasEmptied);",
    "        void anchor;"));
  assert.ok(Math.abs(yOf(unplaced.page, "25") - unplaced.y) > 1, "the test cannot see the message jump");
});

test("`#243 filter-keeps-selection`: closing the search by Escape or by the glass leaves the selected message where it is, and Escape gives it the focus", async () => {
  for (const close of ["Escape", "the glass"]) {
    const page = newPage();
    await signIn(page);
    await showDiscord(page, placeChannel());
    await search(page, "deploy");
    await page.settle();
    parkAt(page, "30", 180);
    await clickRow(page, "30");
    const y = yOf(page, "30");
    if (close === "Escape") await press(page, page.el("search-field"), "Escape");
    else await page.el("search-toggle").click();
    await page.settle();
    assert.equal(page.el("search-toggle").getAttribute("aria-pressed"), "false", `${close} did not close the search`);
    keptAt(page, "30", y, `closing the search by ${close}`);
    assert.deepStrictEqual(litIds(page), ["30"], `closed by ${close}, the message is not lit as the selection`);
    if (close === "Escape") assert.equal(selectedId(page), "30", "Escape did not give the focus to the selected message");
  }
});

test("`#243 filter-keeps-selection`: THE OWNER'S SEQUENCE — search, Pinned, a pin chosen, the filter cleared — leaves that message where it was", async () => {
  for (const [how, clear] of [["touch", "Pinned"], ["mouse", "Escape"], ["mouse", "the glass"]]) {
    const { page, messages } = pinnedPlacePage();
    await signIn(page);
    await showDiscord(page, messages);
    await showPinned(page);
    assert.deepStrictEqual(pinnedIds(page), ["3", "12", "27", "33", "38"], "the fixture's pins are not the ones listed");
    // The pin the reader was after, on screen whole (from review: it was chosen above the list's box),
    // and chosen as a thumb or a mouse chooses it.
    parkAt(page, "27", 120, "pinned-log");
    const pin = placeRow(page, "27", "pinned-log");
    assert.ok(pin.getBoundingClientRect().bottom - page.el("scroll-area").getBoundingClientRect().top < SCROLL_VIEWPORT_PX,
      "the pin chosen is not on screen whole");
    if (how === "touch") await tapRow(page, "27", "pinned-log");
    else await clickRow(page, "27", "pinned-log");
    const y = yOf(page, "27", "pinned-log");
    if (clear === "Pinned") await page.el("pinned-filter").click();
    else if (clear === "Escape") await press(page, page.el("search-field"), "Escape");
    else await page.el("search-toggle").click();
    await page.settle();
    const shape = `chosen by ${how}, cleared by ${clear}`;
    assert.equal(page.el("pinned-log").hidden, true, `${shape}: the Pinned list is still up`);
    keptAt(page, "27", y, shape);
    if (how === "touch") {
      assert.deepStrictEqual(litIds(page), [], `${shape}: a message a tap chose is lit`);
      // ...and the selection came with it: the next key starts from it.
      await press(page, page.el("scroll-area"), "ArrowDown");
      assert.equal(selectedId(page), "28", `${shape}: ↓ did not start from the pin chosen in the Pinned list`);
    } else {
      assert.deepStrictEqual(litIds(page), ["27"], `${shape}: the pin chosen is not lit in the channel's list`);
      if (clear === "Escape") assert.equal(selectedId(page), "27", `${shape}: the focus is not on the pin chosen`);
    }
  }
});

test("`#243 filter-keeps-selection`: turned on, Pinned keeps a selected pin where it was, and the nearest pin in place of a message that is not one", async () => {
  const { page, messages } = pinnedPlacePage();
  await signIn(page);
  await showDiscord(page, messages);
  await search(page, "");
  await page.settle();
  // A pin selected in the channel: it is where it was in the Pinned list.
  parkAt(page, "33", 200);
  await clickRow(page, "33");
  let y = yOf(page, "33");
  await page.el("pinned-filter").click();
  await page.settle();
  keptAt(page, "33", y, "turning Pinned on", "pinned-log");
  assert.deepStrictEqual(litIds(page), ["33"], "the pin selected in the channel is not lit in the Pinned list");
  await page.el("pinned-filter").click();
  await page.settle();
  keptAt(page, "33", y, "turning Pinned off again");
  // A message that is not pinned, chosen by a tap: the pin nearest it stands where it stood, and the
  // status line says so. Turned off without scrolling, the message is back where it was.
  parkAt(page, "26", 160);
  await tapRow(page, "26");
  y = yOf(page, "26");
  await page.el("pinned-filter").click();
  await page.settle();
  keptAt(page, "27", y, "turning Pinned on over a message that is not pinned", "pinned-log");
  assert.equal(statusText(page), "Not pinned — the nearest pin is here.");
  await page.el("pinned-filter").click();
  await page.settle();
  keptAt(page, "26", y, "turning Pinned on and off without a scroll");
});

test("`#243 filter-keeps-selection`: Links on and off keeps the selected message where it is; one without a link is kept by the nearest with one, and comes back", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, placeChannel());
  await showLinks(page);
  // Under Links: a message with a link, selected, and Links turned off.
  parkAt(page, "21", 170);
  await clickRow(page, "21");
  let y = yOf(page, "21");
  await page.el("links-filter").click();
  await page.settle();
  keptAt(page, "21", y, "turning Links off");
  assert.deepStrictEqual(litIds(page), ["21"]);
  await page.el("links-filter").click();
  await page.settle();
  keptAt(page, "21", y, "turning Links on");
  await page.el("links-filter").click();
  await page.settle();
  // THE HIDDEN ANCHOR. Message 20 has no link: under Links the next message shown, 21, stands where it
  // stood, and the status line says why. A tap chose it, so the selection stays with it, to come back.
  parkAt(page, "20", 140);
  await tapRow(page, "20");
  y = yOf(page, "20");
  await page.el("links-filter").click();
  await page.settle();
  keptAt(page, "21", y, "turning Links on over a message without a link");
  assert.equal(statusText(page), "Filtered out — the nearest is here.");
  await page.el("links-filter").click();
  await page.settle();
  keptAt(page, "20", y, "turning Links on and off without a scroll");
  await press(page, page.el("scroll-area"), "ArrowDown");
  assert.equal(selectedId(page), "21", "the selection did not stay with the message Links hid");
});

test("`#243 filter-keeps-selection`: PRs, Commits and Actions keep the selected link where it is, and a pull request hidden by PRs comes back where it was", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, placeChannel());
  await showLinks(page);
  // A document's link stays through every kind turned off and on.
  parkAt(page, "27", 190);
  await clickRow(page, "27");
  const y = yOf(page, "27");
  for (const [kind, why] of [["pr", "PRs off"], ["commit", "Commits off"], ["action", "Actions off"],
    ["pr", "PRs on"], ["commit", "Commits on"], ["action", "Actions on"]]) {
    await tapKind(page, kind);
    keptAt(page, "27", y, why);
  }
  assert.deepStrictEqual(litIds(page), ["27"]);
  // A pull request, selected, and PRs off: the nearest message still shown takes its place. Back on,
  // the pull request is where it was.
  parkAt(page, "24", 150);
  await tapRow(page, "24");
  const at = yOf(page, "24");
  await tapKind(page, "pr");
  assert.equal(placeRow(page, "24").hasClass("search-hidden"), true, "the fixture's pull request is still shown");
  assert.equal(statusText(page), "Filtered out — the nearest is here.");
  const standIn = page.el("discord-log").children.find((li) => !li.hasClass("search-hidden") &&
    Math.abs(li.getBoundingClientRect().top - at) <= 1);
  assert.ok(standIn, "no message shown stands where the pull request stood");
  assert.ok(["21", "27"].includes(standIn.getAttribute("data-id")),
    `the message standing in is ${standIn.getAttribute("data-id")}, not the nearest one shown`);
  await tapKind(page, "pr");
  keptAt(page, "24", at, "PRs off and on again without a scroll");
});

test("`#243 filter-keeps-selection`: Pinned turned on at the newest line before the pins are read says so from the head of the list, and the pins land at the newest", async () => {
  const run = async (script) => {
    const page = newPage(new Map(), script);
    const messages = placeChannel();
    for (const id of ["2", "5", "9", "14", "18", "22", "26", "30", "34", "37", "39"]) {
      page.pinned.set(id, storedPin(messages[Number(id) - 1]));
    }
    await signIn(page);
    await showDiscord(page, messages);
    await search(page, "");
    await page.settle();
    // The composer below every list keeps it overflowing a phone with its rows gone (see the emptied
    // list's test above): a tall line under the box stands in for it.
    page.el("channel-compose-state").textContent = "a line under the message box. ".repeat(40);
    const area = page.el("scroll-area");
    area.scrollTop = area.scrollHeight;
    assert.ok(atBottomOf(area), "the fixture's reader is not on the newest line");
    // No read has carried the pins' revision, so the filter reads them now: "Loading…" first.
    await page.el("pinned-filter").click();
    const loading = { top: area.scrollTop, said: page.el("search-empty").textContent };
    for (let i = 0; i < 3; i += 1) await page.settle();
    return { page, area, loading };
  };
  const { page, area, loading } = await run(SCRIPT);
  assert.equal(loading.said, "Loading this channel's pinned messages…");
  assert.equal(loading.top, 0, "the sentence saying the pins are loading was not shown from the head of the list");
  assert.equal(pinnedIds(page).length, 11, "the pins did not land");
  assert.ok(atBottomOf(area), "the pins landed with the reader at the oldest of them");
  // THE CONTROL: without the newest line kept for the pins, they land under a reader at their oldest.
  const lost = await run(brokenScript("    filterState().hold = anchor\n      ? {", "    filterState().hold = anchor && !anchor.newest\n      ? {"));
  assert.ok(!atBottomOf(lost.area), "the test cannot see the pins land at their oldest");
});

/** Thirty messages from the agent, of two heights: 1, 2, 5, 6, 9, 10 and so on dealt with, the rest not. */
function readPlaceChannel(page) {
  return Array.from({ length: 30 }, (_unused, i) => {
    const n = i + 1;
    const id = String(n);
    if (n % 4 === 1 || n % 4 === 2) page.dealtWith.add(id);
    return message({ id, content: n % 3 === 0 ? longMessage(`update ${n}`) : `update ${n}: ${"the state of it. ".repeat(2)}`,
      ...CODER });
  });
}

test("`#243 filter-keeps-selection`: Show read, Fold read and Hide read each keep the selected message where it is — behind its run's line when it is collapsed", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, readPlaceChannel(page));
  // An unread message, selected, through the whole cycle: Fold read folds the runs above it into
  // lines, Hide read reads only what is left, and Show read reads the channel again.
  parkAt(page, "19", 200);
  await clickRow(page, "19");
  const y = yOf(page, "19");
  const top = page.el("scroll-area").scrollTop;
  await chooseReadMode(page, "Fold read");
  keptAt(page, "19", y, "Fold read");
  assert.ok(collapsedRows(page).length > 0, "Fold read collapsed nothing above the message");
  await chooseReadMode(page, "Hide read");
  await page.settle();
  keptAt(page, "19", y, "Hide read");
  assert.ok(top - page.el("scroll-area").scrollTop > SCROLL_VIEWPORT_PX / 2,
    "Hide read took too little away above the message for a jump to show");
  await chooseReadMode(page, "Show read");
  await page.settle();
  keptAt(page, "19", y, "Show read");
  assert.deepStrictEqual(litIds(page), ["19"], "the message is not lit as the selection after the cycle");
  // A read message inside a run: under Fold read the run's line stands where it stood, and is lit.
  parkAt(page, "18", 160);
  await clickRow(page, "18");
  const runAt = yOf(page, "18");
  await chooseReadMode(page, "Fold read");
  assert.equal(placeRow(page, "18").getAttribute("data-read-run"), "member", "the fixture's message did not join a run");
  keptAt(page, "17", runAt, "the line of the run the selected message joined");
  assert.deepStrictEqual(litIds(page), ["17"], "the run's line is not lit for the selected message inside it");
  // Hide read takes it away: the nearest message left stands there, and the status line says why.
  await chooseReadMode(page, "Hide read");
  await page.settle();
  assert.equal(placeRow(page, "18"), undefined, "Hide read left a read message in the list");
  assert.equal(statusText(page), "Hide read hides it — nearest is here.");
  const standIn = page.el("discord-log").children.find((li) => Math.abs(li.getBoundingClientRect().top - runAt) <= 1);
  assert.ok(standIn && ["19", "16"].includes(standIn.getAttribute("data-id")),
    `the message standing in for 18 is ${standIn ? standIn.getAttribute("data-id") : "none"}, not the nearest one left`);
});

test("`#243 filter-keeps-selection`: a pin older than anything loaded is walked back to when the filter goes, with a line saying so, and put where it was", async () => {
  const page = newPage();
  await signIn(page);
  // 995..1018 in pages of six: the newest page is 1013..1018, and 997 is three pages back.
  const all = pagedChannel(page, { steps: 4, size: 6,
    content: (i) => (i % 2 ? longMessage(`message ${i}`) : `message ${i}: ${"short and plain. ".repeat(2)}`) });
  for (const held of [all[2], all[20]]) page.pinned.set(held.id, storedPin(held));
  await showDiscord(page, []);
  assert.equal(placeRow(page, "997"), undefined, "the old pin is loaded already");
  // Turned on before the pins were read: they land behind the filter's "Loading…".
  await showPinned(page);
  assert.deepStrictEqual(pinnedIds(page), ["997", "1015"]);
  // At the head of the Pinned list nothing steps back behind it: older history adds no pin to it.
  page.el("scroll-area").scrollTop = 0;
  await page.el("scroll-area").dispatch("scroll");
  await page.settle();
  assert.equal(olderReads(page).length, 0, "the head of the Pinned list read an older page of the channel behind it");
  await tapRow(page, "997", "pinned-log");
  const y = yOf(page, "997", "pinned-log");
  // What the step-back line says while each older page is read.
  const lines = [];
  const serve = page.channelPage;
  page.channelPage = (path) => {
    if (/[?&]before=/.test(String(path))) lines.push(page.el("load-older").textContent);
    return serve(path);
  };
  await page.el("pinned-filter").click();
  for (let i = 0; i < 4 * FILTER_WALK_PAGES; i += 1) await page.settle();
  assert.equal(olderReads(page).length, 3, `the walk read ${olderReads(page).length} older pages, not the three to the pin`);
  assert.deepStrictEqual(lines, Array(3).fill("Loading earlier messages…"), "the step-back line did not say what the walk was doing");
  keptAt(page, "997", y, "the old pin, walked back to");
  assert.equal(statusText(page), "Here is the selected message.");
  assert.equal(page.el("load-older").textContent, "Older messages", "the line still says it is loading");
  await press(page, page.el("scroll-area"), "ArrowDown");
  assert.equal(selectedId(page), "998", "the old pin did not come back as the selection");
});

test("`#243 filter-keeps-selection`: the walk back is bounded: a pin further back than it reads leaves the oldest loaded message in its place, and says so", async () => {
  const page = newPage();
  await signIn(page);
  const all = pagedChannel(page, { steps: FILTER_WALK_PAGES + 3, size: 2,
    content: (i) => `message ${i}: ${"short and plain. ".repeat(3)}` });
  page.pinned.set(all[0].id, storedPin(all[0]));
  page.pinned.set(all[all.length - 1].id, storedPin(all[all.length - 1]));
  await showDiscord(page, []);
  await showPinned(page);
  await tapRow(page, all[0].id, "pinned-log");
  const y = yOf(page, all[0].id, "pinned-log");
  await page.el("pinned-filter").click();
  for (let i = 0; i < 4 * (FILTER_WALK_PAGES + 3); i += 1) await page.settle();
  assert.equal(olderReads(page).length, FILTER_WALK_PAGES, "the walk back was not bounded");
  assert.equal(placeRow(page, all[0].id), undefined);
  const oldest = page.el("discord-log").children.find((li) => li.hasAttribute("data-id"));
  keptAt(page, oldest.getAttribute("data-id"), y, "the oldest message loaded, in the old pin's place");
  assert.equal(statusText(page), `Over ${FILTER_WALK_PAGES} pages back — oldest is here.`);
});

/** A channel with one long thread: twenty messages on Main, 305 its root, and twenty replies in it. */
function columnPlaceChannel(page) {
  const thread = { id: "spaces/A/threads/long", root_message_id: "305", is_root: true, reply_count: 20,
    reply_count_exact: true };
  const messages = Array.from({ length: 40 }, (_unused, i) => {
    const n = 300 + i;
    const words = n % 3 === 0 ? `alpha ${n}` : `message ${n}`;
    const content = n % 2 ? longMessage(words) : `${words}: ${"a line or so of it. ".repeat(2)}`;
    const own = n === 305 ? thread : n >= 320 ? { ...thread, is_root: false } : null;
    return message({ id: String(n), content, ...(own ? { thread: own } : {}) });
  });
  page.threadingSupported = true;
  page.threads = [{ id: thread.id, root: messages[5], title: "The long discussion", reply_count: 20,
    reply_count_exact: true, updated_at: messages[39].timestamp }];
  return messages;
}

test("`#243 filter-keeps-selection`: in two columns the search keeps each column's selected message where it is", async () => {
  const page = newPage();
  const messages = columnPlaceChannel(page);
  await signIn(page);
  await showDiscord(page, messages);
  await twoColumns(page);
  await threadBadge(rowWithId(page, "305")).click();
  await page.settle();
  assert.ok(sideIds(page).includes("330"), "the thread did not open on the right");
  await search(page, "alpha");
  await page.settle();
  // One message selected in each column: the left's first, then the right's, which is the one lit.
  parkAt(page, "312", 150);
  await clickRow(page, "312");
  parkAt(page, "330", 180, "side-log", "side-scroll");
  await clickRow(page, "330", "side-log", "side-scroll");
  const left = yOf(page, "312");
  const right = yOf(page, "330", "side-log", "side-scroll");
  const tops = [page.el("scroll-area").scrollTop, page.el("side-scroll").scrollTop];
  await page.el("search-field").setValue("");
  await page.settle();
  keptAt(page, "312", left, "the left column, the search cleared");
  keptAt(page, "330", right, "the right column, the search cleared", "side-log", "side-scroll");
  assert.ok(page.el("scroll-area").scrollTop - tops[0] > SCROLL_VIEWPORT_PX / 2 &&
    page.el("side-scroll").scrollTop - tops[1] > SCROLL_VIEWPORT_PX / 2,
    "the fixture brought too few rows back above the selected messages for a jump to show");
  assert.deepStrictEqual(litRows(page), ["side-log:330"], "the right column's message is not the one lit");
});

test("`#243 filter-keeps-selection`: and nothing else moves it afterwards — not a read bringing a new message, while the reader stays", async () => {
  const run = async (script) => {
    const { page, messages } = pinnedPlacePage(["12", "27", "40"], script);
    await signIn(page);
    await showDiscord(page, messages);
    await showPinned(page);
    // The newest message, pinned and chosen: in the channel's list it is at the newest line.
    await clickRow(page, "40", "pinned-log");
    await page.el("pinned-filter").click();
    await page.settle();
    return page;
  };
  const page = await run(SCRIPT);
  const area = page.el("scroll-area");
  assert.ok(atBottomOf(area), "the fixture's pin is not at the newest line once the filter goes");
  const y = yOf(page, "40");
  // A poll brings a message below it. On the newest line, a reader is followed down to new messages —
  // but not one whose message a filter change has just put there.
  page.messages = [...page.messages, message({ id: "41", content: longMessage("arrived") })];
  await reReadChannel(page);
  keptAt(page, "40", y, "a read bringing a new message");
  assert.ok(jumpSaysArrived(page), "the new message below is not offered");
  // Once the reader scrolls, the newest line is followed again as it always was.
  area.scrollTop = area.scrollHeight;
  await area.dispatch("scroll");
  page.messages = [...page.messages, message({ id: "42", content: longMessage("arrived again") })];
  await reReadChannel(page);
  assert.ok(atBottomOf(area), "a reader on the newest line was not followed once they had scrolled");
  // THE CONTROL: with the hold taken out, the read follows the newest line and the message moves.
  const followed = await run(SCRIPT
    .replace("  return atBottom(area) && heldFilterRow() === null;", "  return atBottom(area);")
    .replace("  if (how.keepPosition && keepFilterHold()) {", "  if (false) {"));
  const before = yOf(followed, "40");
  followed.messages = [...followed.messages, message({ id: "41", content: longMessage("arrived") })];
  await reReadChannel(followed);
  assert.ok(Math.abs(yOf(followed, "40") - before) > 1, "the test cannot see a read move the message");
});

/**
 * Where pin 30 stands: far enough down that message 28, two rows above it, is on screen whole. 310px
 * since `#244 elision-word-count`, whose line under a folded message made 28 a line taller.
 */
const PIN_THIRTY_Y = 310;

/** The pinned place channel with pin 30 at `PIN_THIRTY_Y` in the Pinned list, chosen there by the mouse, and Pinned turned off. */
async function pinThirtyKept(script = SCRIPT) {
  const { page, messages } = pinnedPlacePage(["16", "18", "20", "22", "24", "26", "27", "30", "33", "38"], script);
  await signIn(page);
  await showDiscord(page, messages);
  await showPinned(page);
  parkAt(page, "30", PIN_THIRTY_Y, "pinned-log");
  await clickRow(page, "30", "pinned-log");
  await page.el("pinned-filter").click();
  await page.settle();
  keptAt(page, "30", PIN_THIRTY_Y, "turning Pinned off over the pin chosen");
  return page;
}

test("`#243 filter-keeps-selection`: the reader's own act ends what the change held — a fold opened just above the kept message stays where the reader saw it", async () => {
  // From review: the hold lasted until the reader scrolled, and stayed every `preservingScroll`'s
  // anchor meanwhile. Message 28 is on screen whole just above the pin kept; opening its fold kept the
  // pin still and pushed 28's first 140px above the screen.
  const run = async (script, pressFirst) => {
    const page = await pinThirtyKept(script);
    const row = placeRow(page, "28");
    const before = yOf(page, "28");
    const height = row.height();
    assert.ok(before >= 0 && before + height <= PIN_THIRTY_Y, `message 28 is not on screen whole above the pin (${before}px, ${height}px tall)`);
    // A mouse's press comes first in a browser; a key on the focused fold is the click alone.
    if (pressFirst) await page.documentEvent("pointerdown", { target: foldButton(row), pointerType: "mouse" });
    await foldButton(row).click();
    await page.settle();
    assert.ok(row.height() > height, "the fold did not open");
    return { page, before };
  };
  for (const pressFirst of [false, true]) {
    const { page, before } = await run(SCRIPT, pressFirst);
    keptAt(page, "28", before, `the message whose fold the reader opened${pressFirst ? ", pressed first" : ""}`);
  }
  // The press alone ends the hold, the fold's own act aside...
  const pressed = await run(brokenScript(
    "    readerActed();\n    preservingScroll(() => setFolded(entry, !isFolded(entry)));",
    "    preservingScroll(() => setFolded(entry, !isFolded(entry)));"), true);
  keptAt(pressed.page, "28", pressed.before, "the message whose fold the reader pressed and opened");
  // THE CONTROL: with neither, the message the reader opened is pushed up by the held pin.
  const held = await run(brokenScript(
    "    readerActed();\n    preservingScroll(() => setFolded(entry, !isFolded(entry)));",
    "    preservingScroll(() => setFolded(entry, !isFolded(entry)));"), false);
  assert.ok(yOf(held.page, "28") < held.before - 1, "the test cannot see the held pin move the message opened");
});

test("`#243 filter-keeps-selection`: after the reader acts in the list — a press, a key, the mouse choosing another message, Collapse all — the newest line is followed again", async () => {
  const kept = async (script) => {
    const { page, messages } = pinnedPlacePage(["12", "27", "40"], script);
    await signIn(page);
    await showDiscord(page, messages);
    await showPinned(page);
    await clickRow(page, "40", "pinned-log");
    await page.el("pinned-filter").click();
    await page.settle();
    assert.ok(atBottomOf(page.el("scroll-area")), "the fixture's pin is not at the newest line once the filter goes");
    return page;
  };
  const acts = {
    "a tap in the list": (page) => page.documentEvent("pointerdown", { target: placeRow(page, "39"), pointerType: "touch" }),
    "↑ choosing the message above": (page) => press(page, page.el("scroll-area"), "ArrowUp"),
    "the mouse moving onto another message": (page) => mouseTo(page, placeRow(page, "39"), 120, yOf(page, "39") + 5),
    "Collapse all": (page) => page.el("collapse-all").click(),
  };
  let n = 41;
  for (const [act, run] of Object.entries(acts)) {
    const page = await kept(SCRIPT);
    await run(page);
    await page.settle();
    page.messages = [...page.messages, message({ id: String(n), content: longMessage(`arrived after ${act}`) })];
    n += 1;
    await reReadChannel(page);
    assert.ok(atBottomOf(page.el("scroll-area")), `after ${act}, a reader on the newest line was not followed down`);
  }
  // THE CONTROL: with the reader's act not ending the hold, the same tap leaves the new message unfollowed.
  const page = await kept(brokenScript("function readerActed() {\n  filterState().hold = null;", "function readerActed() {\n  return;"));
  await acts["a tap in the list"](page);
  page.messages = [...page.messages, message({ id: String(n), content: longMessage("arrived") })];
  await reReadChannel(page);
  assert.ok(!atBottomOf(page.el("scroll-area")), "the test cannot see the hold outlast the reader's act");
});

/** The `.body` of a channel row, and the top of each message's block in it, below the top of `scroller`'s box. */
function blockTops(page, row, scroller = "scroll-area") {
  const body = row.descendants().find((node) => String(node.className).split(/\s+/).includes("body"));
  assert.ok(body, "the row has no body");
  const top = page.el(scroller).getBoundingClientRect().top;
  return { body, tops: body.children.map((block) => block.getBoundingClientRect().top - top) };
}

test("`#243 filter-keeps-selection`: a pin that is the second part of a combined row stands where it stood in the Pinned list, the part above it above that", async () => {
  const run = async (script) => {
    const page = newPage(new Map(), script);
    const messages = placeChannel();
    // 30 sent two seconds after 29, by the same account: the channel draws the two as one row.
    messages[29] = { ...messages[29], timestamp: new Date(Date.parse(messages[28].timestamp) + 2000).toISOString() };
    for (const id of ["12", "30", "38"]) page.pinned.set(id, storedPin(messages[Number(id) - 1]));
    page.servePinsRevision = true;
    page.pinsRevision = 4;
    await signIn(page);
    await showDiscord(page, messages);
    assert.equal(placeRow(page, "30").getAttribute("data-ids"), "29 30", "the fixture's pin is not the second part of a combined row");
    await showPinned(page);
    parkAt(page, "30", 220, "pinned-log");
    await clickRow(page, "30", "pinned-log");
    // Where the pin's text starts in the Pinned list, where it is a row of its own.
    const [text] = blockTops(page, placeRow(page, "30", "pinned-log")).tops;
    await page.el("pinned-filter").click();
    await page.settle();
    return { page, text };
  };
  const { page, text } = await run(SCRIPT);
  const row = placeRow(page, "30");
  const { body, tops } = blockTops(page, row);
  assert.equal(body.hasClass("clamped"), false, "the combined row is folded, and its second part out of sight");
  assert.equal(tops.length, 2, "the combined row does not draw its two messages as two blocks");
  assert.ok(Math.abs(tops[1] - text) <= 1, `the pin's text moved from ${text.toFixed(1)}px to ${tops[1].toFixed(1)}px down`);
  assert.ok(tops[0] < text - 1, "the part above the pin is not above where the pin stood");
  assert.deepStrictEqual(litIds(page), ["29"], "the combined row holding the pin is not lit as the selection");
  // THE CONTROL: placed by the row's top, the pin lands lower by the part above it.
  const lower = await run(brokenScript("messageTop(placed, row ? id : null)", "messageTop(placed, null)"));
  const fell = blockTops(lower.page, placeRow(lower.page, "30")).tops[1];
  assert.ok(fell > lower.text + 1, "the test cannot see the pin land lower by the part above it");
});

test("`#243 filter-keeps-selection`: a reader who scrolls while the walk back reads a page is left where they scrolled, and a jump ends the walk and its line", async () => {
  const walking = async (script, during) => {
    const page = newPage(new Map(), script);
    await signIn(page);
    // 995..1026 in pages of eight, every message tall: the list scrolls at every step.
    const all = pagedChannel(page, { steps: 4, size: 8, content: (i) => longMessage(`message ${i}`) });
    for (const held of [all[2], all[30]]) page.pinned.set(held.id, storedPin(held));
    await showDiscord(page, []);
    await showPinned(page);
    await tapRow(page, "997", "pinned-log");
    const y = yOf(page, "997", "pinned-log");
    const serve = page.channelPage;
    let seen = 0;
    page.channelPage = (path) => {
      if (/[?&]before=/.test(String(path)) && seen++ === 0) during(page);
      return serve(path);
    };
    await page.el("pinned-filter").click();
    for (let i = 0; i < 4 * FILTER_WALK_PAGES; i += 1) await page.settle();
    return { page, y };
  };
  const area = (page) => page.el("scroll-area");
  const walkSaid = (page) => !page.el("status-line").hidden && page.el("status").textContent === "Loading earlier messages…";
  // The reader scrolls down 150px while the first older page is read.
  const scroll = (page) => { area(page).scrollTop += 150; };
  const left = await walking(SCRIPT, scroll);
  assert.equal(olderReads(left.page).length, 1, "the walk went on reading after the reader scrolled");
  assert.ok(area(left.page).scrollTop > 0, "the reader was pulled back to the head of the list");
  assert.equal(placeRow(left.page, "997"), undefined, "the walk read on to the pin");
  assert.equal(walkSaid(left.page), false, "the status line still says the walk is loading");
  assert.equal(left.page.el("load-older").textContent, "Older messages");
  // The Newest chip pressed meanwhile: the reader is at the newest line, and the walk's line is gone.
  const jumped = await walking(SCRIPT, (page) => { void page.el("jump-newest").click(); });
  assert.equal(olderReads(jumped.page).length, 1, "the walk went on reading after the jump");
  assert.ok(atBottomOf(area(jumped.page)), "the jump did not leave the reader at the newest line");
  assert.equal(walkSaid(jumped.page), false, "the status line still says the walk is loading after the jump");
  // THE CONTROL: without the walk seeing the scroll, the reader is pulled back and the walk reads on.
  const pulled = await walking(brokenScript(
    "      if (top && top.parentNode && Math.abs(top.getBoundingClientRect().top - was) > 1) {",
    "      if (false) {"), scroll);
  assert.ok(olderReads(pulled.page).length > 1, "the test cannot see the walk read on past the reader's scroll");
  keptAt(pulled.page, "997", pulled.y, "the pin the walk pulled the reader back to");
});

// --- a reply marks read consistently, whichever way it was started (`#300 reply-marks-read`) ------
//
// The owner, 2026-10-14: Reply on a message and opening its thread to write in it both make a reply to
// the message before, and they marked read differently. The rule: the first reply that starts a thread
// marks its root, whether started from Reply or from the thread; a later message in an existing thread
// marks nothing on its own; a targeted Reply marks the message it answers. Marking read is the page's
// own: Done, and the provider's marker where it has one, asked `after_reply` once the send is confirmed.

/** A page on `freshMessageData`'s channel whose provider has a read marker the server can move. */
const markerPage = (script = SCRIPT) =>
  freshMessagePage((page) => { page.upstreamReadMarkSupported = true; }, script);

/** Write `text` in the box of the column on screen (`side` for the right one) and send it. */
async function writeInThread(page, text, side = false) {
  page.el(side ? "side-compose-text" : "channel-compose-text").value = text;
  await page.el(side ? "side-send" : "channel-send").click();
  await page.settle();
}

/** The rule's own line in `sendChannelMessage`, taken out for a negative control. */
const STARTS_THREAD = "    marksRead: rootThisStarts(channel, selection().threadId),\n";

test("`#300 reply-marks-read`: the first reply written in a thread nobody has answered marks its root read, here and at the provider; the next marks nothing", async () => {
  const { page, data } = await markerPage();
  await pickThread(page, "main");
  await threadBadge(rowOf(page, "210")).click();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["210"], "the premise is the empty thread, opened from its grey Thread");
  await writeInThread(page, "the first reply, which starts the thread");
  assert.deepStrictEqual(page.repliesPosted.map((call) => sentWithoutKey(call.body)),
    [{ text: "the first reply, which starts the thread", thread_id: data.lone.id }], "the reply went elsewhere");
  assert.deepStrictEqual(page.dismissCalls, [{ messages: ["210"] }], "the root of the thread it started is not Done");
  assert.deepStrictEqual(page.upstreamReadCalls, [{ message_id: "210", after_reply: true }],
    "the provider's marker was not asked to move through the root, as a reply's mark");
  assert.equal(rowOf(page, "210").getAttribute("data-archived"), "true", "the root is not drawn read");
  assert.equal(doneButton(rowOf(page, "210")).text(), "Unarchive");
  assert.match(page.el("status").textContent, /^Marked read here and in Discord\./);
  assert.equal(page.el("undo-dismiss").hidden, false, "the root's Done offers no Undo");

  // The thread has its reply now: the next message in its box is a later one, and marks nothing.
  await writeInThread(page, "a second thought");
  assert.equal(page.repliesPosted.length, 2);
  assert.equal(page.dismissCalls.length, 1, "a later message in the thread marked something Done");
  assert.equal(page.upstreamReadCalls.length, 1, "a later message in the thread asked the provider again");

  // THE CONTROL, which is the owner's report: without the rule the first reply in the thread marks
  // nothing, where Reply on the same message would have.
  const { page: control } = await markerPage(brokenScript(STARTS_THREAD, "    marksRead: null,\n"));
  await pickThread(control, "main");
  await threadBadge(rowOf(control, "210")).click();
  await control.settle();
  await writeInThread(control, "the first reply, which starts the thread");
  assert.equal(control.repliesPosted.length, 1);
  assert.deepStrictEqual(control.dismissCalls, [], "with the rule taken out the root still went read, so this test cannot see it");
  assert.deepStrictEqual(control.upstreamReadCalls, []);
});

test("`#300 reply-marks-read`: in two columns the first reply in the right column's new thread marks its root in both columns", async () => {
  const { page, data } = await markerPage();
  await twoColumns(page);
  await threadBadge(rowWithId(page, "210")).click();
  await page.settle();
  assert.deepStrictEqual(sideIds(page), ["210"], "the premise is the empty thread on the right");
  await writeInThread(page, "the first reply, from the right", true);
  assert.deepStrictEqual(sentWithoutKey(page.repliesPosted.at(-1).body), { text: "the first reply, from the right", thread_id: data.lone.id });
  assert.deepStrictEqual(page.dismissCalls, [{ messages: ["210"] }]);
  assert.deepStrictEqual(page.upstreamReadCalls, [{ message_id: "210", after_reply: true }]);
  assert.equal(rowWithId(page, "210").getAttribute("data-archived"), "true", "Main does not draw the root read");
  assert.equal(sideRow(page, "210").getAttribute("data-archived"), "true", "the right column does not draw the root read");
});

test("`#300 reply-marks-read`: two messages written at once in a thread nobody has answered — only the first marks its root", async () => {
  const run = async (script) => {
    const { page } = await markerPage(script);
    await pickThread(page, "main");
    await threadBadge(rowOf(page, "210")).click();
    await page.settle();
    const finish = delayedReply(page);
    page.el("channel-compose-text").value = "one";
    const first = page.el("channel-send").click();
    page.el("channel-compose-text").value = "two";
    const second = page.el("channel-send").click();
    await page.settle();
    threadedService(page);
    const posted = message({ id: "9400000000000000009", content: "one",
      thread: { ...freshMessageData().lone, is_root: false, reply_count: null, reply_count_exact: false } });
    page.messages = [...page.messages, posted];
    finish(json(200, { posted }));
    await Promise.all([first, second]);
    await page.settle();
    await page.settle();
    return page;
  };
  const page = await run(SCRIPT);
  assert.deepStrictEqual(page.repliesPosted.map((call) => call.body.text), ["one", "two"], "the premise is two sends");
  assert.deepStrictEqual(page.dismissCalls, [{ messages: ["210"] }]);
  assert.deepStrictEqual(page.upstreamReadCalls, [{ message_id: "210", after_reply: true }],
    "the second message, a later one, asked the provider to mark the root again");
  // THE CONTROL: without the outbox counted, the second was taken for the first as well.
  const control = await run(brokenScript(
    "    if (entry.channel === String(channel) && (entry.threadId === key || entry.continueIn === key)) return null;\n", ""));
  assert.equal(control.upstreamReadCalls.length, 2, "with the outbox not counted only one asked, so this test cannot see it");
});

test("`#300 reply-marks-read`: in a thread that has replies its box marks nothing, and Reply inside it marks the message it answers", async () => {
  const { page } = await markerPage();
  await pickThread(page, "thread:spaces/A/threads/one & two");
  assert.deepStrictEqual(shownIds(page), ["201", "202"]);
  await writeInThread(page, "a later message in an answered thread");
  assert.equal(page.repliesPosted.length, 1);
  assert.deepStrictEqual(page.dismissCalls, [], "a message in an answered thread's box marked something Done");
  assert.deepStrictEqual(page.upstreamReadCalls, [], "a message in an answered thread's box marked something at the provider");
  await replyAndSend(page, "202", "agreed, merging");
  assert.deepStrictEqual(sentWithoutKey(page.repliesPosted.at(-1).body),
    { text: "agreed, merging", reply_to: "202", thread_id: "spaces/A/threads/one & two" });
  assert.deepStrictEqual(page.dismissCalls, [{ messages: ["202"] }], "Reply in the thread did not mark what it answered");
  assert.deepStrictEqual(page.upstreamReadCalls, [{ message_id: "202", after_reply: true }]);
  assert.equal(rowOf(page, "201").getAttribute("data-archived"), "false", "Reply on a reply marked its root");
});

test("`#300 reply-marks-read`: a Reply's mark at the provider follows its Done, only once the reply is sent, and only where the provider has a marker", async () => {
  const target = "1000000000000000003";
  const run = async (marker, script = SCRIPT) => {
    const page = newPage(new Map(), script);
    page.upstreamReadMarkSupported = marker;
    await signIn(page);
    await showDiscord(page, conversation());
    return page;
  };
  const page = await run(true);
  const refused = page.replyResponse;
  page.replyResponse = errorResponse(400, "bad_request", "the chat service refused the message");
  await replyAndSend(page, target, "refused");
  assert.deepStrictEqual([page.dismissCalls, page.upstreamReadCalls], [[], []], "a reply that was not sent marked something");
  page.replyResponse = refused;
  await outgoingRetry(outgoingRows(page)[0]).click();
  await page.settle();
  assert.deepStrictEqual(page.dismissCalls, [{ messages: [target] }]);
  assert.deepStrictEqual(page.upstreamReadCalls, [{ message_id: target, after_reply: true }]);
  const dismissed = page.requests.findIndex((one) => one.endsWith("/dismiss"));
  const marked = page.requests.findIndex((one) => one.endsWith("/upstream-read"));
  assert.ok(dismissed >= 0 && marked > dismissed, "the provider was asked before the Done was saved");
  assert.match(page.el("status").textContent, /^Marked read here and in Discord\./,
    "the status line still says the message was not marked in the chat service");

  // The provider refusing the mark for a while: the Done stands, and the status line, not the error
  // banner, says the chat service was not marked.
  page.upstreamReadResponse = () => json(503, { error: "upstream_unavailable", detail: "Google Chat returned HTTP 429" });
  await replyAndSend(page, "1000000000000000001", "and this one");
  assert.deepStrictEqual(page.dismissCalls.at(-1), { messages: ["1000000000000000001"] });
  assert.equal(page.upstreamReadCalls.length, 2);
  assert.match(page.el("status").textContent, /^Marked read here, not in Discord: /,
    "a mark the provider refused was not said, or was said as a success");
  assert.equal(page.el("error").hidden, true, `a refused mark put up the error banner: ${page.el("error").textContent}`);
  assert.equal(rowWithId(page, "1000000000000000001").getAttribute("data-archived"), "true", "a refused mark took the Done back");

  // No marker at the provider: Done, and nothing asked of it.
  const plain = await run(false);
  await replyAndSend(plain, target, "on it");
  assert.deepStrictEqual(plain.dismissCalls, [{ messages: [target] }]);
  assert.deepStrictEqual(plain.upstreamReadCalls, [], "a provider with no marker was asked to move it");
  // THE CONTROL: the provider's word is what keeps the page from asking.
  const control = await run(false, brokenScript("  if (!channelMarksUpstream(channel)) return none;\n", ""));
  await replyAndSend(control, target, "on it");
  assert.equal(control.upstreamReadCalls.length, 1, "with the check taken out nothing was asked, so this test cannot see it");
});

test("`#300 reply-marks-read`: a Reply in a channel that is one thread of a larger space is Done, and the service's marker, which covers the whole space, is not asked", async () => {
  // The finding: every reply moved the provider's marker for the whole space, so on a registration
  // narrowed to one thread each Reply marked every earlier message of the space read in the service's
  // own app, messages vibe-talk never received. The timeline names such a channel's conversation as
  // its `thread` (`#44 thread-scope-main-view`), and the page holds it as `channelStore.current().scope`.
  const run = async (script = SCRIPT) => {
    const page = newPage(new Map(), script);
    page.upstreamReadMarkSupported = true;
    const data = scopedData();
    serveScoped(page, data.summary);
    await signIn(page);
    await showDiscord(page, data.messages.slice(0, 3));
    return page;
  };
  const page = await run();
  await replyAndSend(page, "302", "answering in the conversation");
  assert.equal(page.repliesPosted.at(-1).body.reply_to, "302", "the premise is a reply sent");
  assert.deepStrictEqual(page.dismissCalls, [{ messages: ["302"] }], "the reply's Done did not stand");
  assert.deepStrictEqual(page.upstreamReadCalls, [],
    "a reply in one thread of a larger space asked to move the whole space's read marker");
  assert.doesNotMatch(page.el("status").textContent, /Marked read here and in/,
    "the status line says the chat service was marked");
  assert.equal(rowOf(page, "302").getAttribute("data-archived"), "true", "the answered row is not drawn read");

  // THE CONTROL: without the page's own check it asks, and only the server would stand in the way.
  const control = await run(brokenScript("  if (channelIsOneThread(channel)) return", "  if (false) return"));
  await replyAndSend(control, "302", "answering in the conversation");
  assert.deepStrictEqual(control.upstreamReadCalls, [{ message_id: "302", after_reply: true }],
    "with the check taken out nothing was asked, so this test cannot see it");
});

test("`#300 reply-marks-read`: a reply's mark the server did not ask of the service is said as not marked there, with the reason", async () => {
  const target = "1000000000000000003";
  const run = async (script = SCRIPT) => {
    const page = newPage(new Map(), script);
    page.upstreamReadMarkSupported = true;
    await signIn(page);
    await showDiscord(page, conversation());
    return page;
  };
  const notAsked = (outcome) => (body) => json(200, { channel: CHANNEL, through: body.message_id, outcome,
    upstream_read_notice: "The source chat provider was not asked to move its read cursor." });
  const page = await run();
  page.upstreamReadResponse = notAsked("thread_channel");
  await replyAndSend(page, target, "on it");
  assert.deepStrictEqual(page.upstreamReadCalls, [{ message_id: target, after_reply: true }], "the premise is the server asked");
  assert.match(page.el("status").textContent,
    /^Marked read here, not in Discord: this channel is one thread of a larger space there, and its read marker covers the whole space\./,
    "a mark the server did not ask of the service was said as made");
  assert.equal(page.el("error").hidden, true, `a mark not asked put up the error banner: ${page.el("error").textContent}`);
  assert.equal(rowWithId(page, target).getAttribute("data-archived"), "true", "the Done did not stand");

  page.upstreamReadResponse = notAsked("scope_unknown");
  await replyAndSend(page, "1000000000000000001", "and this");
  assert.match(page.el("status").textContent,
    /^Marked read here, not in Discord: it cannot say whether its read marker covers more than this channel\./);

  // THE CONTROL: without the outcome read, the status line claims the service was marked.
  const control = await run(brokenScript("    const why = serviceMarkNotAsked(outcome);", "    const why = null;"));
  control.upstreamReadResponse = notAsked("thread_channel");
  await replyAndSend(control, target, "on it");
  assert.match(control.el("status").textContent, /^Marked read here and in Discord\./,
    "with the outcome ignored the status line still said why, so this test cannot see it");
});

test("`#300 reply-marks-read`: Quote reply, and Reply docked in either column, mark what they answer as Reply does", async () => {
  const page = await quotePage((p) => { p.upstreamReadMarkSupported = true; });
  const first = rowWithId(page, "1000000000000000001");
  await page.selectText(wordsIn(first), null, "the first message");
  await replyButton(first).click();
  await page.el("reply-text").setValue("> the first message\n\nit is wedged");
  await page.el("reply-send").click();
  await page.settle();
  assert.equal(page.repliesPosted.at(-1).body.reply_to, "1000000000000000001", "the premise is a quote reply");
  assert.deepStrictEqual(page.dismissCalls, [{ messages: ["1000000000000000001"] }]);
  assert.deepStrictEqual(page.upstreamReadCalls, [{ message_id: "1000000000000000001", after_reply: true }]);

  const { page: two } = await twoColumnServer((p) => { p.upstreamReadMarkSupported = true; });
  await threadBadge(rowWithId(two, "201")).click();
  await two.settle();
  await replyAndSend(two, "202", "agreed", "side-log");
  await replyAndSend(two, "200", "noted");
  assert.deepStrictEqual(two.dismissCalls, [{ messages: ["202"] }, { messages: ["200"] }]);
  assert.deepStrictEqual(two.upstreamReadCalls,
    [{ message_id: "202", after_reply: true }, { message_id: "200", after_reply: true }],
    "a reply docked in a column was not marked at the provider as Reply is");
});

test("`#300 reply-marks-read`: an outbox entry saved by an earlier page marks what it answers once sent; one saved marking nothing marks nothing", async () => {
  const target = "1000000000000000003";
  const saved = (id, fields) => ({ id, channel: CHANNEL.id, text: `saved ${id}`, remaining: `saved ${id}`,
    state: "failed", threadId: null, replyTo: null, createdAt: 1, postedCount: 0, parts: [], observedParts: [],
    detail: "refused", ...fields });
  const store = new Map([["vibe-talk.outgoing-messages", JSON.stringify([
    saved("outgoing-older-page", { replyTo: target }),
    saved("outgoing-marks-nothing", { marksRead: null, threadId: "spaces/A/threads/elsewhere" }),
  ])]]);
  const page = newPage(store);
  page.upstreamReadMarkSupported = true;
  await signIn(page);
  await showDiscord(page, conversation());
  assert.equal(outgoingRows(page).length, 2, "the premise is two saved sends");
  await outgoingRetry(outgoingRows(page)[0]).click();
  await page.settle();
  assert.deepStrictEqual(page.dismissCalls, [{ messages: [target] }], "an entry an earlier page saved no longer marks what it answers");
  assert.deepStrictEqual(page.upstreamReadCalls, [{ message_id: target, after_reply: true }]);
  const rest = outgoingRows(page).find((row) => outgoingRetry(row));
  await outgoingRetry(rest).click();
  await page.settle();
  assert.equal(page.repliesPosted.length, 2, "the premise is both sent");
  assert.equal(page.dismissCalls.length, 1, "an entry saved marking nothing marked something");
  assert.equal(page.upstreamReadCalls.length, 1);
  // And a new one is saved with what it marks.
  await replyAndSend(page, "1000000000000000001", "fresh");
  assert.ok(savedOutgoing(page).some((entry) => entry.marksRead === "1000000000000000001"),
    "the outbox does not keep what a send marks read across a reload");
});

test("`#300 reply-marks-read`: a voice reply confirmed on the card is marked by the server, and the page writes no second mark", async () => {
  const page = newPage();
  page.upstreamReadMarkSupported = true;
  await proposedDuringACall(page);
  await page.el("post-confirm-send").click();
  await page.settle();
  assert.equal(page.proposalCommits.length, 1, "the premise is a committed voice reply");
  assert.deepStrictEqual(page.dismissCalls, [], "the page wrote a Done the server already wrote for the voice reply");
  assert.deepStrictEqual(page.upstreamReadCalls, [], "the page asked the provider for a mark the server already asked");
});

test("`#251 author-filter`: the author filter sits left of Links, only while the bar is open, in the house style, and says each state", async () => {
  assertMarkupContains("search-float", "author-filter");
  const bar = HTML_CODE.slice(HTML_CODE.indexOf('id="search-float"'));
  const at = (id) => bar.indexOf(`id="${id}"`);
  assert.ok(at("search-count") < at("author-filter") && at("author-filter") < at("links-filter"),
    "the author filter is not between the count and Links");
  assert.equal(bar.slice(bar.indexOf("</button>", at("author-filter")) + "</button>".length, at("links-filter")).trim(),
    "<button", "something stands between the author filter and Links");
  // An icon per state, each in the house style, and each named for its state.
  const button = bar.slice(at("author-filter"), bar.indexOf("</button>", at("author-filter")));
  for (const mode of ["everyone", "mine", "theirs"]) {
    const from = button.indexOf(`data-author="${mode}" viewBox`);
    assert.ok(from > 0, `the author filter has no icon for ${mode}`);
    const svg = button.slice(button.lastIndexOf("<svg", from), button.indexOf(">", from));
    for (const attribute of ['class="icon author-icon"', 'stroke-width="1.9"', 'stroke-linecap="round"', 'aria-hidden="true"']) {
      assert.ok(svg.includes(attribute), `the ${mode} icon does not share ${attribute} with the pin`);
    }
    assert.match(cssBlock(`#author-filter[data-author="${mode}"] .author-icon[data-author="${mode}"]`), /display:\s*block/,
      `the ${mode} icon is never drawn`);
  }
  assert.match(cssBlock("#author-filter .author-icon"), /display:\s*none/, "every state's icon is drawn at once");
  // The same square as Links and Pinned, drawn by the same rules, abutting Links.
  for (const selector of ["#author-filter", "#author-filter::before", "#author-filter .icon"]) {
    assert.deepStrictEqual(cssRules(CSS, selector), cssRules(CSS, selector.replace("author", "pinned")).slice(0, 1),
      `${selector} is not drawn by the rule that draws Pinned`);
  }
  assert.match(cssBlock("#author-filter + #links-filter"), /margin-left:\s*calc\(-1 \* var\(--bar-gap\)\)/);
  // ...except on a narrow phone, where a third square on the line would take the field's room: there it
  // hangs under the bar's left end, and the list STARTS below it — its top edge moved down to the
  // disc's foot and the room at its head given back — so a scrolled list runs under nothing.
  const narrow = mediaBody("(max-width: 400px)");
  assert.match(narrow, /#screen-main\[data-searching\] #author-filter\s*\{[^}]*position:\s*absolute[^}]*top:\s*calc\(100% \+ 1px \+ var\(--kinds-gap\)\)/,
    "on a narrow phone the author filter does not hang under the bar");
  const inset = /#screen-main\[data-searching\] #scroll-area\s*\{([^}]*)\}/.exec(narrow);
  assert.ok(inset, "on a narrow phone the list is not placed under the author filter");
  assert.match(inset[1], /--hang-foot:\s*calc\(var\(--float-line\) \+ var\(--search-bar\) \/ 2 \+ var\(--kinds-gap\) \+ var\(--toggle-hit\)\)/,
    "the list's top is not the author filter's foot");
  assert.match(inset[1], /margin-top:\s*var\(--hang-foot\)/, "the list does not start below the author filter");
  assert.match(inset[1], /padding-top:\s*0\.35rem/, "the room at the list's head is not given back, so every row moves");
  assert.match(inset[1], /mask-image:\s*linear-gradient\(to bottom, transparent 0, #000 0\.35rem\)/,
    "the fade under the header reaches into the first row");
  const kindsInset = /#screen-main\[data-searching\]\[data-link-kinds\] #scroll-area\s*\{([^}]*)\}/.exec(narrow);
  assert.ok(kindsInset && /2 \* \(var\(--kinds-gap\) \+ var\(--toggle-hit\)\)/.test(kindsInset[1]) &&
    /padding-top:\s*0\.35rem/.test(kindsInset[1]), "with the kinds of link up the list does not start under the disc below them");
  assert.match(cssBlock("#author-filter[aria-pressed=\"true\"]"), /color: var\(--accent\)/, "a filtering state does not say so");

  const page = newPage();
  await signIn(page);
  await showDiscord(page, authorChannel());
  const filter = page.el("author-filter");
  assert.equal(filter.hidden, true, "the filter is offered before the search bar is open");
  await page.el("search-toggle").click();
  assert.equal(filter.hidden, false, "the open bar has no author filter");
  const said = [];
  for (let presses = 0; presses < 3; presses += 1) {
    await filter.click();
    await page.settle();
    said.push([...authorState(page), filter.getAttribute("aria-label")]);
  }
  assert.deepStrictEqual(said, [
    ["mine", "true", "Only your messages. Press for only everyone else's"],
    ["theirs", "true", "Only everyone else's messages. Press for everyone's"],
    ["everyone", "false", "Everyone's messages. Press for only yours"],
  ], "a press does not go round Everyone, Mine and Theirs, saying each");
  // Over the call view too, where Pinned is not offered; and gone with the bar.
  await filter.click();
  await page.el("view-switch").click();
  await page.settle();
  assert.equal(filter.hidden, false, "the author filter is not offered over the call view");
  assert.deepStrictEqual(authorState(page), ["mine", "true"], "switching views changed whose messages are shown");
  await page.el("view-switch").click();
  await page.settle();
  await page.el("search-toggle").click();
  await page.settle();
  assert.deepStrictEqual(authorState(page), ["everyone", "false"], "the author filter outlived the bar");
  assert.equal(filter.hidden, true);
  assert.equal(unfiltered(page, "discord-log").length, 12, "a row stayed hidden after the filter went");
  await chooseAuthor(page, "theirs");
  await page.el("search-field").dispatch("keydown", { key: "Escape" });
  await page.settle();
  assert.deepStrictEqual(authorState(page), ["everyone", "false"], "Escape left the author filter on");
});

test("`#520 search-whose-toggle-hint`: the author filter's tooltip and title name what it does and the state it is in, per state", async () => {
  // The tooltip is drawn off the button on hover and on keyboard focus, and floats (no layout shift).
  assert.match(cssBlock("#author-filter .author-tip"), /position:\s*absolute/, "the tooltip does not float");
  assert.match(cssBlock("#author-filter .author-tip"), /display:\s*none/,
    "the tooltip is drawn while nobody is asking for it");
  assert.match(cssBlock("#author-filter:hover .author-tip"), /display:\s*block/,
    "the tooltip never shows on hover");
  assert.match(cssBlock("#author-filter:focus-visible .author-tip"), /display:\s*block/,
    "the tooltip never shows on keyboard focus");
  assert.match(cssBlock("#author-filter .author-tip"), /pointer-events:\s*none/, "the tooltip takes presses");
  const page = newPage();
  await signIn(page);
  await showDiscord(page, authorChannel());
  await page.el("search-toggle").click();
  await page.settle();
  const filter = page.el("author-filter");
  const tip = page.el("author-filter-tip");
  assert.ok(/<button id="author-filter"[^>]*aria-describedby="author-filter-tip"/.test(HTML_CODE),
    "the button does not name its tooltip");
  const said = [];
  for (let presses = 0; presses < 3; presses += 1) {
    await filter.click();
    await page.settle();
    said.push([filter.getAttribute("data-author"), filter.getAttribute("title"), tip.textContent]);
  }
  assert.deepStrictEqual(said, [
    ["mine", "Whose messages: My messages (only yours). Press to show only everyone else's.",
      "Whose messages: My messages (only yours). Press to show only everyone else's."],
    ["theirs", "Whose messages: Their messages (only everyone else's). Press to show everyone's.",
      "Whose messages: Their messages (only everyone else's). Press to show everyone's."],
    ["everyone", "Whose messages: Both (everyone's). Press to show only yours.",
      "Whose messages: Both (everyone's). Press to show only yours."],
  ], "the tooltip and title do not name the control and its state in each state");
});

test("`#520 search-whose-toggle-hint`: a press leaves a chip under the author filter naming the state it landed on, then it goes", async () => {
  // The chip wears the page's chip dress, floats under the button, and does not animate under
  // reduced motion.
  assert.match(cssBlock("#author-filter .author-chip"), /position:\s*absolute/, "the chip does not float under the button");
  assert.match(cssBlock("#author-filter .author-chip"), /top:\s*calc\(100%/, "the chip is not under the button");
  const reduced = [...CSS.matchAll(/@media \(prefers-reduced-motion: reduce\) \{([\s\S]*?)\n\}/g)]
    .some((m) => m[1].includes(".author-chip") && /transition:\s*none/.test(m[1]));
  assert.ok(reduced, "the chip still animates under reduced motion");
  const page = newPage();
  await signIn(page);
  await showDiscord(page, authorChannel());
  const chip = page.el("author-filter-chip");
  assert.equal(chip.hidden, true, "the chip is up before any press");
  assert.match(HTML_CODE, /id="author-filter-chip" class="chip author-chip"/,
    "the chip does not wear the page's chip style");
  await page.el("search-toggle").click();
  await page.settle();
  const landed = [];
  for (let presses = 0; presses < 3; presses += 1) {
    await page.el("author-filter").click();
    await page.settle();
    landed.push([chip.hidden, chip.textContent]);
  }
  assert.deepStrictEqual(landed, [
    [false, "My messages"],
    [false, "Their messages"],
    [false, "Both"],
  ], "a press does not leave the chip naming the state it landed on");
  // It fades after ~1.5 s and is then gone; a fresh press restarts the wait.
  assert.equal(page.expireTimers(1500), 1, "no timer was armed to fade the chip");
  assert.ok(chip.className.split(/\s+/).includes("author-chip-out"), "the chip does not fade");
  assert.equal(page.expireTimers(400), 1, "no timer was armed to take the faded chip away");
  assert.equal(chip.hidden, true, "the chip never went away");
  await page.el("author-filter").click();
  await page.settle();
  assert.equal(chip.hidden, false, "a fresh press does not bring the chip back");
  assert.equal(chip.textContent, "My messages");
});

test("`#251 author-filter`: Mine with nothing typed is every message of the owner's loaded, Theirs every other, counted", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, authorChannel());
  await chooseAuthor(page, "mine");
  assert.deepStrictEqual(passing(page), ["3", "6", "9", "12"], "Mine is not exactly the owner's messages");
  assert.equal(page.el("search-count").textContent, "4 of 12 loaded", "Mine with nothing typed says no count");
  assert.equal(page.el("search-empty").hidden, true);
  await chooseAuthor(page, "theirs");
  assert.deepStrictEqual(passing(page), ["1", "2", "4", "5", "7", "8", "10", "11"], "Theirs is not every other message");
  assert.equal(page.el("search-count").textContent, "8 of 12 loaded");
  // A person is "them" as much as an agent is.
  page.messages = [...authorChannel(), message({ id: "13", content: "a person", ...ALICE })];
  await reReadChannel(page);
  await page.settle();
  assert.ok(passing(page).includes("13"), "Theirs left out a message from another person");
  await chooseAuthor(page, "mine");
  assert.ok(!passing(page).includes("13"));
  // A channel where none is the owner's says so, in words that say the list is only what is loaded.
  page.messages = authorChannel().filter((m) => m.author_id !== OWN.author_id);
  await reReadChannel(page);
  await page.settle();
  assert.deepStrictEqual(passing(page), []);
  assert.equal(page.el("search-empty").hidden, false, "an empty Mine says nothing about why");
  assert.equal(page.el("search-empty").textContent, "None of the messages loaded so far are yours.");
  // THE CONTROL: without the author test in the pass, Mine filters nothing.
  const unwired = newPage(new Map(), brokenScript(
    "      const hit = said && (!links || shown > 0) && authorPasses(row, author);",
    "      const hit = said && (!links || shown > 0);"));
  await signIn(unwired);
  await showDiscord(unwired, authorChannel());
  await chooseAuthor(unwired, "mine");
  assert.equal(passing(unwired).length, 12, "the test cannot see Mine take anything away");
});

test("`#251 author-filter`: ANDed with the search text, Links and Pinned, the count speaking for all of them", async () => {
  const page = newPage();
  const messages = authorChannel();
  for (const n of [3, 4, 6]) page.pinned.set(String(n), storedPin(messages[n - 1], n));
  page.servePinsRevision = true;
  page.pinsRevision = 3;
  await signIn(page);
  await showDiscord(page, messages);
  await chooseAuthor(page, "mine");
  await page.el("search-field").setValue("deploy");
  assert.deepStrictEqual(passing(page), ["12"], "the text and Mine are not both required");
  assert.equal(page.el("search-count").textContent, "1 of 12 loaded");
  await page.el("search-field").setValue("step 4");
  assert.deepStrictEqual(passing(page), []);
  assert.equal(page.el("search-empty").textContent, "None of the messages of yours loaded so far match that search.");
  await chooseAuthor(page, "theirs");
  assert.deepStrictEqual(passing(page), ["4"]);
  await page.el("search-field").setValue("");
  // Links: the links of the owner's messages, and of everyone else's.
  await page.el("links-filter").click();
  await page.settle();
  assert.deepStrictEqual(passing(page), ["7"], "Links and Theirs are not both required");
  await chooseAuthor(page, "mine");
  assert.deepStrictEqual(passing(page), ["6"], "Links and Mine are not both required");
  assert.equal(page.el("search-count").textContent, "1 of 12 loaded");
  await page.el("links-filter").click();
  await page.settle();
  // Pinned: the owner's pins, and then everyone else's.
  await page.el("pinned-filter").click();
  await page.settle();
  await page.settle();
  assert.deepStrictEqual(passing(page, "pinned-log"), ["3", "6"], "Pinned and Mine are not both required");
  assert.equal(page.el("search-count").textContent, "2 of 3 pinned");
  await chooseAuthor(page, "theirs");
  assert.deepStrictEqual(passing(page, "pinned-log"), ["4"]);
  assert.equal(page.el("search-count").textContent, "1 of 3 pinned");
  await page.el("search-field").setValue("message");
  assert.deepStrictEqual(passing(page, "pinned-log"), []);
  assert.equal(page.el("search-empty").textContent, "None of this channel's pinned messages from anyone else match that search.");
});

test("`#251 author-filter`: it chooses rows, so Fold read's runs open under it, and Hide read says why Mine is empty when the owner's own count as read", async () => {
  const page = newPage();
  await signIn(page);
  // The agent's messages read, the owner's own read by the setting: everything but 10 and 11 is read.
  const messages = authorChannel();
  for (const m of messages) if (!["10", "11"].includes(m.id) && m.author_id !== OWN.author_id) page.dealtWith.add(m.id);
  await showDiscord(page, messages);
  await chooseReadMode(page, "Fold read");
  assert.ok(collapsedRows(page).length > 0, "the fixture collapsed nothing");
  await chooseAuthor(page, "mine");
  assert.deepStrictEqual(collapsedRows(page), [], "a run stayed collapsed under the author filter");
  assert.deepStrictEqual(passing(page), ["3", "6", "9", "12"]);
  await chooseAuthor(page, "everyone");
  assert.ok(collapsedRows(page).length > 0, "the runs did not collapse again once the filter was off");
  // Hide read with the owner's messages counted as read: Mine is empty, and says why and how out.
  await chooseReadMode(page, "Hide read");
  await page.settle();
  await chooseAuthor(page, "mine");
  assert.deepStrictEqual(passing(page), []);
  assert.equal(page.el("search-empty").hidden, false);
  assert.equal(page.el("search-empty").textContent,
    "Your own messages start read, so Hide read leaves them out — press h for Show read.");
  await chooseAuthor(page, "theirs");
  assert.deepStrictEqual(passing(page), ["10", "11"], "Hide read and Theirs are not both applied");
});

test("`#251 author-filter`: every change keeps the selected message where it is, and one Theirs hides is held by the nearest shown", async () => {
  const run = async (script) => {
    const page = newPage(new Map(), script);
    await signIn(page);
    await showDiscord(page, placeChannel().map((m, i) => ({ ...m, ...((i + 1) % 3 === 0 ? OWN : CODER) })));
    await search(page, "");
    await page.settle();
    parkAt(page, "24", 170);
    await clickRow(page, "24");
    const y = yOf(page, "24");
    await page.el("author-filter").click();
    await page.settle();
    return { page, y };
  };
  const { page, y } = await run(SCRIPT);
  assert.deepStrictEqual(authorState(page)[0], "mine");
  keptAt(page, "24", y, "Mine on");
  assert.deepStrictEqual(litIds(page), ["24"]);
  // THE CONTROL: the same change, not placed, moves the message.
  const unplaced = await run(brokenScript("  keepingFilterPlace(() => liftingHideRead(() => {\n    authorFilter = next;",
    "  ((change) => change())(() => liftingHideRead(() => {\n    authorFilter = next;"));
  assert.ok(Math.abs(yOf(unplaced.page, "24") - unplaced.y) > 1, "the test cannot see the message jump");
  // Theirs hides it: the nearest message shown stands where it stood, and the status line says so.
  await page.el("author-filter").click();
  await page.settle();
  const stand = page.el("discord-log").children.find((li) => rowShownHere(li) && Math.abs(yOf(page, li.getAttribute("data-id")) - y) <= 1);
  assert.ok(stand && ["25", "23"].includes(stand.getAttribute("data-id")),
    `the message standing in for 24 is ${stand ? stand.getAttribute("data-id") : "none"}`);
  assert.equal(statusText(page), "Filtered out — the nearest is here.");
  // ...and Everyone, without a scroll, puts it back where it was.
  await page.el("author-filter").click();
  await page.settle();
  keptAt(page, "24", y, "Everyone again, after Theirs hid it");
});

/** Whether a row of a list is shown: neither hidden nor filtered out. */
const rowShownHere = (li) => !li.hidden && !li.hasClass("search-hidden");

test("`#251 author-filter`: Shift+M goes round the states with the bar that holds them, and gives the list back the focus", async () => {
  const page = await keyboardPage();
  await press(page, page.el("scroll-area"), "M", { shiftKey: true });
  assert.deepStrictEqual(authorState(page), ["mine", "true"], "Shift+M did not show only the owner's messages");
  assert.equal(page.el("search-field").hidden, false, "a filter came on with the bar that holds it shut");
  assert.equal(focused(page), page.el("scroll-area"), `after Shift+M the focus is on ${focusName(page)}`);
  await press(page, page.el("scroll-area"), "M", { shiftKey: true });
  assert.deepStrictEqual(authorState(page), ["theirs", "true"]);
  await press(page, page.el("scroll-area"), "M", { shiftKey: true });
  assert.deepStrictEqual(authorState(page), ["everyone", "false"]);
  // m alone is the message box's (`#538 c-key-copy`), never the author filter.
  await press(page, page.el("scroll-area"), "m");
  assert.deepStrictEqual(authorState(page), ["everyone", "false"], "a plain m changed the author filter");
  assert.ok(KEYMAP_SOURCE_HAS("author-filter"), "the list of keys does not name the author filter's key");
});

/** Whether the page's table of keys names a control. */
const KEYMAP_SOURCE_HAS = (id) => SCRIPT_CODE.slice(SCRIPT_CODE.indexOf("const KEYMAP = ["),
  SCRIPT_CODE.indexOf("];", SCRIPT_CODE.indexOf("const KEYMAP = ["))).includes(`controls: ["${id}"]`);

test("`#251 author-filter`: over the call view Mine is what you said and Theirs what the agent said", async () => {
  const page = newPage();
  await signIn(page);
  await startTalking(page);
  youSay(page, "is the runner wedged");
  assistantSays(page, "it restarted at noon");
  youSay(page, "thanks");
  await page.settle();
  await chooseAuthor(page, "mine");
  assert.deepStrictEqual(unfiltered(page, "transcript").map((li) => li.className), ["mine", "mine"]);
  assert.equal(page.el("search-count").textContent, "2 of 3 loaded");
  await chooseAuthor(page, "theirs");
  assert.deepStrictEqual(unfiltered(page, "transcript").map((li) => li.className), ["theirs"]);
  assistantSays(page, "anything else?");
  await page.settle();
  assert.equal(unfiltered(page, "transcript").length, 2, "a turn arriving under Theirs arrived unfiltered");
});

test("`#251 author-filter`: in two columns it filters both, and the count speaks for both", async () => {
  const page = newPage();
  const data = threadData();
  // The first thread's answer, and the second thread's root, are the owner's.
  data.messages[2] = { ...data.messages[2], ...OWN };
  data.messages[3] = { ...data.messages[3], ...OWN };
  page.threadingSupported = true;
  page.threads = data.threads;
  await signIn(page);
  await showDiscord(page, data.messages);
  await twoColumns(page);
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  await chooseAuthor(page, "mine");
  const visible = (list) => unfiltered(page, list).map((li) => li.getAttribute("data-id"));
  assert.deepStrictEqual(visible("side-log"), ["202"], "Mine did not reach the right column");
  assert.deepStrictEqual(visible("discord-log"), ["203"], "Mine did not reach the left column");
  assert.equal(page.el("search-count").textContent, `2 of ${messageRows(page, "discord-log").length + 2} loaded`,
    "the count does not speak for both columns");
  for (const list of ["discord-log", "side-log"]) {
    const rows = messageRows(page, list);
    assert.ok(rows.length > 0, `#${list} holds no messages`);
    for (const li of rows) {
      assert.equal(rowShownHere(li), li.getAttribute("data-who") === "me", `#${list} shows a row Mine does not keep, or hides one it does`);
    }
  }
});

// --- `#584 thread-view-bookmark-chip` ----------------------------------------------------------------
//
// The owner, 2026-10-27: in the Threads view b and Shift+B said "Back at your bookmark", where no bookmark
// is drawn: b worked in Main, out of sight behind the thread list. Every way to the bookmark says so there.

test("`#584 thread-view-bookmark-chip`: over the thread list b, Shift+B and the chip say there are no bookmarks, and touch none", async () => {
  const { page } = await repliesPage({}, false);
  // A bookmark set in All, where it is drawn.
  await press(page, page.el("scroll-area"), "ArrowDown");
  await pressHere(page, "B", { shiftKey: true });
  assert.equal(statusText(page), "Bookmarked. b comes back here.");
  const calls = page.bookmarkCalls.length;
  await pickThread(page, "threads");
  assert.equal(page.el("screen-main").getAttribute("data-columns"), "list");
  const cards = page.el("side-scroll");
  for (const [key, fields] of [["b", {}], ["B", { shiftKey: true }]]) {
    page.el("status").textContent = "";
    const event = await press(page, cards, key, fields);
    assert.equal(event.defaulted, false, `${key} over the thread list was left to the browser`);
    assert.equal(statusText(page), "No bookmarks in thread view.", `${key} over the thread list said something else`);
  }
  // On a card the keys have ringed, too: never silent, never "Bookmarked".
  await press(page, cards, "j");
  page.el("status").textContent = "";
  await press(page, cards, "B", { shiftKey: true });
  assert.equal(statusText(page), "No bookmarks in thread view.", "Shift+B on a card said something else");
  assert.equal(page.el("jump-bookmark").hidden, true, "the Bookmark chip was offered over the thread list");
  assert.equal(page.bookmarkCalls.length, calls, "a bookmark key over the thread list moved the bookmark");
  // Back in All, b goes back to it as ever.
  await pickThread(page, "flat");
  await press(page, page.el("scroll-area"), "b");
  assert.equal(statusText(page), "Back at your bookmark.");

  // In two columns the cards are on the right and Main, with its line, on the left: Shift+B with the keys
  // in the cards says so; b goes to Main's line, which is on screen.
  const two = await repliesPage({});
  await press(two.page, two.page.el("scroll-area"), "ArrowDown");
  await pressHere(two.page, "B", { shiftKey: true });
  await press(two.page, two.page.el("scroll-area"), "]");
  await press(two.page, two.page.el("side-scroll"), "j");
  await press(two.page, two.page.el("side-scroll"), "B", { shiftKey: true });
  assert.equal(statusText(two.page), "No bookmarks in thread view.", "Shift+B on a card in two columns said something else");
  await press(two.page, two.page.el("side-scroll"), "b");
  assert.equal(statusText(two.page), "Back at your bookmark.", "b in two columns did not go to Main's line");
});
