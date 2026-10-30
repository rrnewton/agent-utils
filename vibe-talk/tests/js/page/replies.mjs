// The app page's tests: replies: following one up, gathering them under a message, replying in place, and drafts.
//
// One feature's file of the suite tests/js/voice_page.test.mjs runs, against the fixture in
// ./fixture.mjs. Run with the rest of the suite, or alone: node --test tests/js/page/replies.mjs.

import assert from "node:assert/strict";

import { ALICE, BASE_CSS, CHANNEL, CODER, CSS, CSS_CODE, DESKTOP_QUERY, DISCORD_POLL_MS,
  FakeElement, GATHERED, HOLD_MS, HTML_CODE, LINE_PX, MESSAGE_CACHE_KEY, MESSAGE_DAY, OWN, OWNER,
  PULL_WAIT_MS, REPLY_DOUBLE_TAP_MS, REPLY_JUMP_PAGES, RETRY_FIRST_MS, RUN_TAIL, SCATTERED, SCRIPT,
  SCRIPT_CODE, SEND_UNDO_MS, SLOW_SEND, arrowStyle, assertMarkupContains, atBottomOf, backlog,
  bodyOf, bookmarkAt, bookmarkRow, bridgeReply, brokenModule, brokenScript, channelRow,
  chooseReadMode, conversation, cssBlock, cssBlockIn, cssRules, delayedReply, deliver,
  dismissedItems, dismissedOpenerOf, distinctReplies, doneButton, draftMarkerOf, enterRunThread,
  errorResponse, failedSend, focusName, focused, forwardPage, freshMessagePage, freshness, gate,
  gatherChip, gutterChild, helpEntry, holdCountsRead, inPlaceRows, inReadingMode, inTwoColumns,
  inlineAnchors, json, keyboardPage, keyedChannel, keyless, longMessage, message, metaOf, mouseOnto,
  newPage, nodeWithinFixture, offline, olderReads, openReplyOn, outgoingActions, outgoingDismiss,
  outgoingEdit, outgoingRetry, outgoingRows, outgoingSpinner, pagedChannel, pagedTimeline,
  parityPage, pickThread, pollOnce, posted, postedWhere, press, pressHere, providerDown, putBackOf,
  questionFarAbove, quotePage, reReadChannel, readButton, readsSince, reloadPage, reloadWith,
  reopenedChannel, replyArrowOf, replyBoxAt, replyButton, replyColumnsPage, replyInStream,
  replyQuietly, retryWait, retryingFirst, row, rowOf, rowWithId, runId, runThread, savedDismissed,
  savedOutgoing, savedScopes, savedUiState, scatteredPage, scatteredThread, scopeKey, search,
  selectedId, sendBehind, sendChannel, sentOk, sentWithoutKey, settleSend, showDiscord, showPinned,
  shownIds, sideIds, sideRow, signIn, sourceConstant, sourceString, splitPost, sseDelete,
  sseMessage, stackPlaces, startTalking, statusText, storedPin, tagOf, takesASide, tallColumns,
  tapArrow, test, threadBadge, threadButton, threadData, threadOptions, threadPage, timelineReads,
  tokenValues, turnSummariesOn, turnTodoOn, twoColumnServer, twoColumns, unpackedRow,
  withLiveChannel, wordsIn } from "./fixture.mjs";

const REPLY_LANDED_MS = sourceConstant("REPLY_LANDED_MS");

test("the arrow has a gutter of its own: one touch target, on every reply, whoever sent it", () => {
  // The page suite cannot lay anything out; the real geometry — the arrow inside the gutter, clear
  // of the row's text and buttons at 412 and 360 pixels — is checked in Chromium by
  // tests/offline_cache_browser.py. What can be checked here is that the rules which produce it
  // are the ones that say so.
  const gutter = tokenValues(CSS_CODE, "--reply-gutter");
  assert.deepStrictEqual(gutter, ["2.75rem"], "the gutter is not one 44px touch target");
  const reply = cssBlock('#discord-log li.discord-message[data-is-reply="true"][data-who]');
  assert.match(reply, /margin-left:\s*var\(--reply-gutter\)/, "a reply leaves no room for its arrow");
  assert.match(reply, /min-height:\s*var\(--reply-gutter\)/, "a short reply lets its arrow hang over the next row");
  // The agent's tile has a left side like every row since `#213 row-side-borders` (which the test of
  // that name pins), and it is the edge the arrow comes out of: nothing on a reply takes it away.
  assert.equal(takesASide(cssBlock('#discord-log li.discord-message[data-is-reply="true"][data-who="coder"]')),
    null, "a reply from the agent has no left side");
  const arrow = cssBlock("#discord-log .reply-jump");
  assert.match(arrow, /position:\s*absolute/);
  assert.match(arrow, /right:\s*100%/, "the arrow is not anchored to the box's left side");
  assert.match(arrow, /width:\s*var\(--reply-gutter\)/, "the target is narrower than the gutter");
  assert.match(arrow, /height:\s*var\(--reply-gutter\)/, "the target is shorter than a fingertip");
  assert.match(arrow, /color:\s*var\(--accent\)/, "the arrow is not in the accent, so it is not legible on dark");
  // The owner's own rows are inset further than the gutter on a desk; a reply keeps the larger.
  assert.match(cssBlockIn("(min-width: 900px) and (pointer: fine)",
    '#discord-log li.discord-message[data-is-reply="true"][data-who="me"]'),
  /margin-left:\s*max\(var\(--reply-gutter\)/, "a reply of the owner's loses his side of the column on a desk");
  // The landing is lit, and lit over whatever the row is, and still for a reader who asked for no motion.
  assert.match(cssBlock('#discord-log li.discord-message[data-landed="true"]::after'), /--accent/);
  assert.match(cssBlock('#discord-log li.discord-message[data-landed="true"]'), /opacity:\s*1/,
    "the answered row stays dimmed while it is the one being pointed at");
});

test("...and the arrow does not recede with its row: a reply's state is drawn on its box", () => {
  // A row's `opacity` and `filter` take everything inside it with them, and the arrow is inside
  // the row. The replies that recede are the ones the owner meets most — his own, read by default;
  // ones he swiped Done; ones somebody answered — and on those the arrow came out grey, at about
  // 2:1 against the page. Chromium measures the drawn pixels (tests/offline_cache_browser.py);
  // what is checked here is the arrangement that produces them.
  const own = /(?<![-\w])/.source;
  const reply = cssBlock('#discord-log li.discord-message[data-is-reply="true"][data-who]');
  assert.match(reply, new RegExp(`${own}opacity:\\s*1\\b`), "a reply is still faded as a whole row, arrow and all");
  assert.match(reply, new RegExp(`${own}filter:\\s*none`), "a reply is still greyed as a whole row, arrow and all");
  // Each state SAYS how far a row recedes, rather than fading the row itself, so a reply can draw it
  // elsewhere; every other row draws it as before, on the whole row.
  // `#256 reply-marks-read`: being answered no longer recedes on its own (a reply marks the message Done).
  for (const [state, amount] of [["data-archived", "0.42"], ["data-noise", "0.32"]]) {
    const rule = cssBlock(`#discord-log li.discord-message[${state}="true"]`);
    assert.match(rule, new RegExp(`--row-opacity:\\s*${amount.replace(".", "\\.")}`), `${state} no longer says how far it recedes`);
    assert.doesNotMatch(rule, new RegExp(`${own}opacity:`), `${state} fades the whole row again, a reply's arrow with it`);
  }
  assert.match(cssBlock("#discord-log li.discord-message"), /opacity:\s*var\(--row-opacity, 1\)/,
    "a row that is not a reply no longer recedes at all");
  // The reply's box: veiled in the page's colour by as much as the row would have faded, its
  // content greyed part by part with the arrow left out, and its tint drained under both.
  const veil = cssBlock('#discord-log li.discord-message[data-is-reply="true"]::after');
  assert.match(veil, /color-mix\(in srgb, var\(--bg\) calc\(\(1 - var\(--row-opacity, 1\)\) \* 100%\)/,
    "a reply's box no longer fades by its state's amount");
  assert.match(veil, /pointer-events:\s*none/, "the veil takes the taps meant for the reply's buttons");
  assert.match(cssBlock('#discord-log li.discord-message[data-is-reply="true"] > :not(.thread-strip):not(.reply-jump):not(.reply-bridge):not(.reply-ungather)'),
    /filter:\s*var\(--row-filter, none\)/, "a reply's content is not greyed with its state");
  assert.match(reply, /background-blend-mode:\s*saturation/, "a reply's tint is not drained with its state");
  // A lit reply is at full strength, and the landing's own wash takes the pseudo-element: the same
  // weight, so the later declaration wins.
  assert.ok(CSS.indexOf('li.discord-message[data-is-reply="true"]::after {') <
    CSS.indexOf('li.discord-message[data-landed="true"]::after {'),
  "a reply's veil is declared after the landing, so a lit reply loses its light");
});

test("TAPPING THE ARROW brings the message it answers to the head of the list, lit", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, questionFarAbove());
  const area = page.el("scroll-area");
  const question = rowWithId(page, "1000000000000000001");
  const reply = rowWithId(page, "1000000000000000099");
  assert.ok(area.scrollHeight > area.clientHeight * 3, "the channel does not overflow, so this proves nothing");
  assert.ok(question.getBoundingClientRect().bottom < 0, "the question was already on screen");

  await tapArrow(page, reply);

  const top = question.getBoundingClientRect().top;
  assert.ok(top >= 0 && top <= 3 * LINE_PX, `the question landed at ${top}px, not at the head of the list`);
  assert.equal(question.getAttribute("data-landed"), "true", "nothing says which row the jump was for");
  assert.match(statusText(page), /Here is the message it answers/);
  // Lit for a moment, then put out.
  assert.equal(page.expireTimers(REPLY_LANDED_MS), 1, "the light was never timed to go out");
  assert.equal(question.getAttribute("data-landed"), null, "the landing stayed lit");
});

test("...and the reply stays one tap away: Back to reply, while the reply is off screen", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, questionFarAbove());
  const area = page.el("scroll-area");
  const reply = rowWithId(page, "1000000000000000099");
  assert.equal(page.el("back-to-reply").hidden, true, "a way back is offered before anybody went anywhere");

  await tapArrow(page, reply);
  assert.equal(page.el("back-to-reply").hidden, false,
    "the jump left the reply eight messages down with no way back");

  await page.el("back-to-reply").click();
  const box = reply.getBoundingClientRect();
  assert.ok(box.top < area.clientHeight && box.bottom > 0,
    `the reply is not on screen after Back to reply: ${box.top}`);
  assert.equal(reply.getAttribute("data-landed"), "true", "the way back does not say where it landed either");
  assert.equal(page.el("back-to-reply").hidden, true, "the way back outlived being used");

  // ...and it also goes when the reader scrolls back to the reply on their own.
  await tapArrow(page, reply);
  assert.equal(page.el("back-to-reply").hidden, false);
  area.scrollTop = area.scrollHeight;
  await area.dispatch("scroll");
  assert.equal(page.el("back-to-reply").hidden, true, "the chip offers to go where the reader already is");
});

test("...with no way back offered when the reply is still on screen after the jump", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [
    message({ id: "1000000000000000001", content: "is the runner wedged?" }),
    message({ id: "1000000000000000002", content: "restarted it", reply_to: "1000000000000000001" }),
  ]);
  await tapArrow(page, row(page, 1));
  assert.equal(row(page, 0).getAttribute("data-landed"), "true");
  assert.equal(page.el("back-to-reply").hidden, true, "a chip offers a way back to a reply that never left");
});

test("THE TAP IS THE ARROW'S: it neither folds the reply nor reads it aloud", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, questionFarAbove());
  const reply = rowWithId(page, "1000000000000000099");
  assert.equal(reply.getAttribute("data-collapsed"), "true", "the reply is not foldable, so this proves nothing");

  await tapArrow(page, reply);
  assert.equal(reply.getAttribute("data-collapsed"), "true", "tapping the arrow also opened the reply");

  // In reading mode a tap on a row is a request to hear it. Not this tap.
  await readButton(page).click();
  await page.settle();
  await tapArrow(page, reply);
  assert.deepStrictEqual(page.speakCalls, [], "tapping the arrow started reading the reply aloud");
  assert.ok(rows.every((li) => li.getAttribute("data-pending") !== "true"), "a read was queued by the arrow");
  assert.equal(rowWithId(page, "1000000000000000001").getAttribute("data-landed"), "true",
    "in reading mode the arrow stopped jumping");
});

test("A COMBINED ROW holds the message: the jump lands on that row", async () => {
  // Combining is drawing, not data: the reply answers ONE message, and the row it is drawn in is
  // whichever row holds it — here the second half of a post split in two.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [
    ...splitPost(),
    message({
      id: "1000000000000000003", author: "alice", author_id: "1000000000000000005", author_is_bot: false,
      content: "which runner?", reply_to: "1000000000000000002", timestamp: "2026-08-19T04:40:00.000Z",
    }),
  ]);
  assert.equal(page.el("discord-log").children.length, 2, "the split post was not drawn as one row");

  await tapArrow(page, row(page, 1));
  assert.equal(row(page, 0).getAttribute("data-landed"), "true", "a message inside a combined row was not found");
  assert.match(statusText(page), /Here is the message it answers/);
});

test("NOT LOADED YET: the tap walks back through older pages until the message is there", async () => {
  const page = newPage();
  await signIn(page);
  const all = pagedChannel(page, { steps: 4, size: 4 });
  // The newest message answers one three pages further back.
  all[all.length - 1].reply_to = all[2].id;
  await showDiscord(page, []);
  assert.equal(rowWithId(page, all[2].id), undefined, "the question was loaded already");

  await tapArrow(page, row(page, page.el("discord-log").children.length - 1));

  assert.equal(olderReads(page).length, 3,
    `it read ${olderReads(page).length} older pages, not the three it needed`);
  const question = rowWithId(page, all[2].id);
  assert.ok(question, "the walk did not bring the question in");
  assert.equal(question.getAttribute("data-landed"), "true", "the walk found it and did not go there");
  assert.match(statusText(page), /Here is the message it answers/);
});

test("...a BOUNDED walk, which then says the message is further back than that", async () => {
  // A message deleted upstream is never found. An unbounded walk would read the channel to its
  // first message looking for it.
  const page = newPage();
  await signIn(page);
  const all = pagedChannel(page, { steps: 3 * REPLY_JUMP_PAGES, size: 2 });
  all[all.length - 1].reply_to = "990";
  await showDiscord(page, []);

  await tapArrow(page, row(page, page.el("discord-log").children.length - 1));
  assert.equal(olderReads(page).length, REPLY_JUMP_PAGES, "the walk back was not bounded");
  assert.equal(statusText(page), `Further back than ${REPLY_JUMP_PAGES} pages — scroll up.`);
  assert.ok(!page.el("discord-log").children.some((li) => li.getAttribute("data-landed") === "true"),
    "a row was lit for a message that was never found");
});

test("...and a step back already on the wire is waited for, not counted as a page it never read", async () => {
  // The reader's own walk back — the button, or arriving at the top of the list — may be under way
  // when the arrow is tapped. Turned away, the tap's walk would count that page without reading it
  // and look for its message among rows that had not arrived.
  const page = newPage();
  await signIn(page);
  const all = pagedChannel(page, { steps: 3, size: 4 });
  all[all.length - 1].reply_to = all[1].id;
  await showDiscord(page, []);
  const serve = page.channelPage;
  let release = null;
  const held = new Promise((resolve) => { release = resolve; });
  let holding = true;
  page.channelPage = async (path) => {
    if (holding && /[?&]before=/.test(String(path))) {
      holding = false;
      await held;
    }
    return serve(path);
  };

  const stepping = page.el("load-older").click();
  const tapping = tapArrow(page, row(page, page.el("discord-log").children.length - 1));
  release();
  await stepping;
  await tapping;

  assert.equal(olderReads(page).length, 2,
    `${olderReads(page).length} older reads, not the reader's one and the tap's one`);
  assert.equal(rowWithId(page, all[1].id)?.getAttribute("data-landed"), "true",
    "the message two pages back was not reached");
});

test("...and NOT IN THE HISTORY AT ALL is said as that, not as a tap that did nothing", async () => {
  const page = newPage();
  await signIn(page);
  const all = pagedChannel(page, { steps: 2, size: 3 });
  all[all.length - 1].reply_to = "990";
  await showDiscord(page, []);
  await tapArrow(page, row(page, page.el("discord-log").children.length - 1));
  assert.equal(olderReads(page).length, 1, "the walk went on past the beginning of the channel");
  assert.equal(statusText(page), "Not in this channel's history.");

  // A server that does not say whether there is more is not walked, and is not told "deleted".
  const quiet = newPage();
  await signIn(quiet);
  await showDiscord(quiet, [
    message({ id: "1000000000000000002", content: "restarted it", reply_to: "999999999999999999" }),
  ]);
  await tapArrow(quiet, row(quiet, 0));
  assert.equal(statusText(quiet), "Older than what is loaded.");
});

test("A SEARCH that hides the message is closed, so the row can be shown", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [
    message({ id: "1000000000000000001", content: "is the runner wedged?" }),
    message({ id: "1000000000000000002", content: "restarted it", reply_to: "1000000000000000001" }),
  ]);
  await search(page, "restarted");
  assert.ok(row(page, 0).hasClass("search-hidden"), "the search did not hide the question");

  await tapArrow(page, row(page, 1));
  assert.ok(!row(page, 0).hasClass("search-hidden"), "the jump went to a row the search is hiding");
  assert.equal(page.el("search-field").hidden, true, "the search was cleared and left open");
  assert.equal(row(page, 0).getAttribute("data-landed"), "true");
});

test("IN ALL, a thread reply answering the main channel jumps within All", async () => {
  const page = newPage();
  const data = threadData();
  // The second thread's answer answers the main channel's announcement.
  data.messages[4].reply_to = "200";
  page.threadingSupported = true;
  page.threads = data.threads;
  await signIn(page);
  await showDiscord(page, data.messages);
  assert.equal(page.el("thread-select").value, "flat");
  const reads = page.timelineCalls.length;

  await tapArrow(page, rowWithId(page, "204"));
  assert.equal(rowWithId(page, "200").getAttribute("data-landed"), "true");
  assert.equal(page.el("thread-select").value, "flat", "a jump inside All changed the view");
  assert.equal(page.timelineCalls.length, reads, "a jump to a message already on screen went to the network");
});

test("IN MAIN, a reply to a message Main does not show opens All and lands there", async () => {
  // Main shows thread roots and nothing inside a thread. All is the one view holding every message,
  // so it is where "not found" means not in the channel — and it holds the reply too, so the way
  // back is still the chip.
  const page = newPage();
  const data = threadData();
  // The second thread's ROOT, which Main shows, answers the first thread's answer, which it does not.
  data.messages[3].reply_to = "202";
  page.threadingSupported = true;
  page.threads = data.threads;
  await signIn(page);
  await showDiscord(page, data.messages);
  await pickThread(page, "main");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "Main is not what this test assumes");

  await tapArrow(page, rowWithId(page, "203"));
  assert.equal(page.el("thread-select").value, "flat", "the jump did not open All");
  assert.equal(rowWithId(page, "202").getAttribute("data-landed"), "true",
    "All opened and the message was not shown");
  assert.match(statusText(page), /Here is the message it answers/);
});

test("IN A THREAD, a reply to the main channel opens All and lands there", async () => {
  const page = newPage();
  const data = threadData();
  data.messages[4].reply_to = "200";
  page.threadingSupported = true;
  page.threads = data.threads;
  await signIn(page);
  await showDiscord(page, data.messages);
  await pickThread(page, `thread:${data.threads[1].id}`);
  assert.deepStrictEqual(shownIds(page), ["203", "204"], "the thread is not what this test assumes");

  await tapArrow(page, rowWithId(page, "204"));
  assert.equal(page.el("thread-select").value, "flat", "the jump did not leave the thread for All");
  assert.equal(rowWithId(page, "200").getAttribute("data-landed"), "true");
});

test("HIDE READ hiding the message is said, not walked past", async () => {
  const page = newPage();
  page.threadingSupported = true;
  page.dealtWith.add("301");
  await signIn(page);
  await showDiscord(page, [
    message({ id: "301", content: "is the runner wedged?" }),
    message({ id: "302", content: "restarted it", reply_to: "301" }),
  ]);
  await turnTodoOn(page);
  assert.deepStrictEqual(shownIds(page), ["302"]);
  const reads = page.timelineCalls.length;

  await tapArrow(page, rowWithId(page, "302"));
  assert.equal(statusText(page), "Hide read is hiding that message.");
  assert.equal(page.timelineCalls.length, reads, "it went looking for a message it already held");
});

test("...and when it is All that turns up the message Hide read is hiding, that is said too", async () => {
  // From Main, a thread root answering a reply inside another thread: Main does not show the
  // answer, so the tap opens All — where Hide read hides it as well. Said as that, rather than
  // walked past to "older than what is loaded".
  const page = newPage();
  const data = threadData();
  data.messages[3].reply_to = "202";
  page.threadingSupported = true;
  page.threads = data.threads;
  page.dealtWith.add("202");
  await signIn(page);
  await showDiscord(page, data.messages);
  await turnTodoOn(page);
  await pickThread(page, "main");
  assert.ok(shownIds(page).includes("203"), "the reply is not on screen in Main");

  await tapArrow(page, rowWithId(page, "203"));
  assert.equal(page.el("thread-select").value, "flat", "the jump did not open All");
  assert.equal(statusText(page), "Hide read is hiding that message.");
  assert.ok(!timelineReads(page).some((path) => /[?&]before=/.test(path)),
    "it walked back looking for a message it already held");
});

test("...nor is a finger on the arrow the start of a swipe or a press-and-hold on the reply", async () => {
  // The row follows a finger sideways to archive, and opens its details under one that rests. A
  // finger on the arrow is pressing the arrow. A hold there would also leave the row's
  // click-swallowing flag set behind a click the arrow keeps to itself, and the reader's next tap
  // on any row would do nothing.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, questionFarAbove());
  const reply = rowWithId(page, "1000000000000000099");
  const mark = replyArrowOf(reply).children[0];
  const commit = sourceConstant("SWIPE_COMMIT_PX") + 10;
  const touch = (x) => ({ pointerId: 1, pointerType: "touch", clientX: x, clientY: 10, target: mark });

  await reply.dispatch("pointerdown", touch(10));
  assert.equal(page.expireTimers(HOLD_MS), 0, "a finger resting on the arrow armed the row's press-and-hold");
  await reply.dispatch("pointermove", touch(10 + commit));
  assert.ok(!reply.style.transform, "the row followed a finger that came down on its arrow");
  await reply.dispatch("pointerup", touch(10 + commit));
  await page.settle();
  assert.deepEqual(page.dismissCalls, [], "a drag that began on the arrow archived the reply");
  assert.ok(!reply.descendants().some((node) => node.hasClass("msg-details")), "the arrow opened the row's details");

  await tapArrow(page, reply);
  // The reader's next tap on the reply's own text is still a tap on the reply: it opens it.
  await reply.dispatch("click", { target: reply });
  assert.equal(reply.getAttribute("data-collapsed"), "false", "the tap after the arrow was swallowed");
});

test("AN ARROW TO THE ROW DIRECTLY ABOVE is the adjacent one; any other reply's is disconnected", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [
    message({ id: "1000000000000000001", content: "is the runner wedged?" }),
    message({ id: "1000000000000000002", content: "restarted it", reply_to: "1000000000000000001", ...ALICE }),
    message({ id: "1000000000000000003", content: "and the queue?" }),
    message({ id: "1000000000000000004", content: "draining now", reply_to: "1000000000000000001", ...ALICE }),
    message({ id: "1000000000000000005", content: "answers something never loaded", reply_to: "999" }),
  ]);
  assert.equal(arrowStyle(rowWithId(page, "1000000000000000002")), "adjacent",
    "a reply directly under the message it answers does not draw the arrow that meets it");
  assert.equal(arrowStyle(rowWithId(page, "1000000000000000004")), "disconnected",
    "a reply two rows under what it answers draws the arrow that says it answers the row above");
  assert.equal(arrowStyle(rowWithId(page, "1000000000000000005")), "disconnected");
});

test("...where the row above holds it as a constituent of a combined row", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [
    ...splitPost(),
    message({ id: "1000000000000000003", content: "which runner?", reply_to: "1000000000000000002",
      timestamp: "2026-08-19T04:40:00.000Z", ...ALICE }),
  ]);
  assert.equal(page.el("discord-log").children.length, 2, "the split post was not drawn as one row");
  assert.equal(arrowStyle(row(page, 1)), "adjacent", "the second half of a combined row is not the row above");
});

test("...where the row above is the previous one the reader can SEE, and decided again as that changes", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [
    message({ id: "1000000000000000001", content: "is the runner wedged?" }),
    message({ id: "1000000000000000002", content: "the weather is nice" }),
    message({ id: "1000000000000000003", content: "restarted the runner", reply_to: "1000000000000000001", ...ALICE }),
  ]);
  const reply = rowWithId(page, "1000000000000000003");
  assert.equal(arrowStyle(reply), "disconnected");
  // A search that hides the row between: the message it answers is directly above it on screen.
  await search(page, "runner");
  assert.ok(rowWithId(page, "1000000000000000002").hasClass("search-hidden"), "the search did not hide the row between");
  assert.equal(arrowStyle(reply), "adjacent", "a row the search hides still counts as the row above");
  // ...and one that hides the message it answers: nothing above it is what it answers.
  await search(page, "restarted");
  assert.equal(arrowStyle(reply), "disconnected", "a hidden message is still met by the arrow");
  await page.el("search-toggle").click();
  assert.equal(arrowStyle(reply), "disconnected", "closing the search left the arrow as it was under it");
});

test("...and Hide read, which does not draw a read row at all, leaves the message directly above", async () => {
  const page = newPage();
  page.threadingSupported = true;
  page.dealtWith.add("302");
  await signIn(page);
  await showDiscord(page, [
    message({ id: "301", content: "is the runner wedged?" }),
    message({ id: "302", content: "already dealt with" }),
    message({ id: "303", content: "restarted it", reply_to: "301", ...ALICE }),
  ]);
  assert.equal(arrowStyle(rowWithId(page, "303")), "disconnected");
  await turnTodoOn(page);
  assert.deepStrictEqual(shownIds(page), ["301", "303"]);
  assert.equal(arrowStyle(rowWithId(page, "303")), "adjacent", "Hide read left the arrow pointing past an empty place");
});

test("...and an older page arriving above can bring in the message a first row answers", async () => {
  const page = newPage();
  await signIn(page);
  const all = pagedChannel(page, { steps: 2, size: 3 });
  all[3].reply_to = all[2].id;
  await showDiscord(page, []);
  assert.equal(row(page, 0).getAttribute("data-id"), all[3].id, "the reply is not the first row loaded");
  assert.equal(arrowStyle(row(page, 0)), "disconnected", "nothing is above the first row");
  await page.el("load-older").click();
  for (let i = 0; i < 4; i += 1) await page.settle();
  assert.equal(rowWithId(page, all[2].id), row(page, 2), "the older page did not land above");
  assert.equal(arrowStyle(rowWithId(page, all[3].id)), "adjacent", "the arrow was not decided again");
});

test("WHERE THE ADJACENT ARROW MEETS THE ROW ABOVE is measured: its foot, moved under its edge, or its side", async () => {
  // The fixture lays nothing out sideways, so the boxes are placed by hand: the lefts a phone gives
  // the agent's full-width tile, the owner's inset row, a desk's owner row far right of the gutter,
  // and an edge too close to the box for either. The rem here is the fixture's 16px.
  const page = newPage(new Map(), SCRIPT, (p) => p.enableResizeObserver());
  await signIn(page);
  await showDiscord(page, [
    message({ id: "1000000000000000001", content: "is the runner wedged?" }),
    message({ id: "1000000000000000002", content: "restarted it", reply_to: "1000000000000000001", ...ALICE }),
  ]);
  const list = page.el("discord-log");
  list.clientWidth = 380;
  const place = (li, left) => {
    li.getBoundingClientRect = () => ({ ...FakeElement.prototype.getBoundingClientRect.call(li), left, right: left + 300 });
  };
  const reply = row(page, 1);
  const arrow = replyArrowOf(reply);
  const measured = (parentLeft) => {
    place(row(page, 0), parentLeft);
    place(reply, 44);
    page.resized(list);
    page.expireTimers(0);
    return [arrow.getAttribute("data-reply-reach"), arrow.style.getPropertyValue("--reply-shift") || null,
      arrow.style.getPropertyValue("--reply-run") || null];
  };
  assert.deepStrictEqual(measured(-12), ["below", "0.00px", null], "a row spanning the gutter is not met at its foot");
  assert.deepStrictEqual(measured(22), ["below", "6.80px", null],
    "an inset row's foot is met on its rounded corner rather than under its edge");
  assert.deepStrictEqual(measured(96), ["side", null, "71.20px"], "a row starting past the box is met at nothing");
  assert.deepStrictEqual(measured(36), ["below", "8.80px", null], "an edge too close for a turn is not met as near as the shaft goes");
});

test("both arrows are drawn as the issue asks, in the stylesheet", () => {
  // DISCONNECTED: out of the middle of the box's side, then forty-five degrees.
  assert.match(cssBlock("#discord-log .reply-jump"), /top:\s*calc\(50% - var\(--reply-gutter\) \/ 2\)/,
    "the arrow to a message elsewhere does not leave the middle of the box's side");
  assert.match(cssBlock('#discord-log .reply-jump[data-reply-style="disconnected"] .reply-jump-mark::before'),
    /transform:\s*rotate\(45deg\)/, "the run is not at forty-five degrees");
  // ADJACENT: at the top, risen by exactly the gap between two rows — the margin under a row and this
  // row's own top border, both stated in the base sheet.
  const adjacent = cssBlock('#discord-log .reply-jump[data-reply-style="adjacent"]');
  assert.match(adjacent, /top:\s*0/);
  assert.match(adjacent, /--reply-rise:\s*calc\(0\.5rem \+ 1px\)/, "the rise is not the gap between two rows");
  const rowRule = cssRules(BASE_CSS, ".messages li").join("\n");
  assert.match(rowRule, /margin-bottom:\s*0\.5rem/, "the gap the rise crosses has changed under it");
  assert.match(rowRule, /border:\s*1px solid/, "the border the rise crosses has changed under it");
  const mark = cssBlock('#discord-log .reply-jump[data-reply-style="adjacent"] .reply-jump-mark');
  assert.match(mark, /top:\s*calc\(0\.45rem - var\(--reply-rise\)\)/);
  assert.match(cssBlock('#discord-log .reply-jump[data-reply-style="adjacent"] .reply-jump-mark::after'),
    /top:\s*-0\.45rem/, "the head's point is not where the shaft's rise ends");
  // The shaft is where web/voice.js measures it from.
  const shaft = Number(/^const REPLY_SHAFT_REM = ([\d.]+);$/m.exec(SCRIPT_CODE)?.[1]);
  assert.match(mark, new RegExp(`width:\\s*calc\\(${String(shaft).replace(".", "\\.")}rem \\+`),
    "the shaft is drawn somewhere other than where it is measured from");
  assert.match(cssBlock('#discord-log .reply-jump[data-reply-reach="side"] .reply-jump-mark::after'),
    new RegExp(`right:\\s*calc\\(${String(shaft).replace(".", "\\.")}rem - var\\(--reply-run, 0px\\)\\)`),
    "the turn to the row's side does not run as far as measured");
  // The head the adjacent arrow draws outside its square — in the gap, or beside the row above — is the
  // arrow's to press, not whatever is under it: the page, or the row above's own arrow, which jumps
  // somewhere else. Every head is the drawing's `::after`, and it takes presses though the rest of the
  // drawing passes them to the square; no rule for a head takes that back. offline_cache_browser.py
  // presses each head in Chromium.
  assert.match(cssBlock("#discord-log .reply-jump-mark"), /pointer-events:\s*none/);
  assert.match(cssBlock("#discord-log .reply-jump-mark::after"), /pointer-events:\s*auto/,
    "a press on an arrow's head goes through it to whatever is under it");
  const heads = [...CSS_CODE.matchAll(/([^{}]*\.reply-jump-mark::after[^{}]*)\{([^}]*)\}/g)];
  assert.ok(heads.length >= 4, "the heads are not where this test looks for them");
  for (const [, selector, body] of heads) {
    assert.doesNotMatch(body, /pointer-events:\s*none/, `${selector.trim()} passes presses on the head through`);
  }
});

test("a long post's first part, a root by then, is one row with its continuation in its thread (#463)", async () => {
  // Slack posts a long message as its first part in the channel and the rest as replies in that
  // part's thread, about three seconds apart. The root used to stand alone, so the post read as
  // separate messages. A root still never joins the row above it (705 after 704, two seconds
  // apart), nor does a reply in another thread (707), nor another author's reply (706).
  const at = (seconds) => new Date(MESSAGE_DAY.getTime() + seconds * 1000).toISOString();
  const post = { id: "1792765680.753759", root_message_id: "701", is_root: true, reply_count: 3,
    reply_count_exact: true };
  const other = { id: "1792765690.000100", root_message_id: "704", is_root: true, reply_count: 1,
    reply_count_exact: true };
  const messages = [
    message({ id: "701", content: "state replication model", thread: post, timestamp: at(0), ...OWNER }),
    message({ id: "702", content: "part two", thread: { ...post, is_root: false }, timestamp: at(3), ...OWNER }),
    message({ id: "703", content: "part three", thread: { ...post, is_root: false }, timestamp: at(6), ...OWNER }),
    message({ id: "704", content: "a new thread", thread: other, timestamp: at(8), ...OWNER }),
    // Slack names every message on the channel's own timeline the root of its own thread.
    message({ id: "705", content: "in the channel", timestamp: at(9), ...OWNER,
      thread: { id: "1792765691.000100", root_message_id: "705", is_root: true, reply_count: 0,
        reply_count_exact: true } }),
    message({ id: "706", content: "an answer", thread: { ...post, is_root: false }, timestamp: at(10), ...ALICE }),
    message({ id: "707", content: "the other thread", thread: { ...other, is_root: false }, timestamp: at(11),
      ...OWNER }),
  ];
  const page = newPage();
  page.threadingSupported = true;
  page.threads = [
    { id: post.id, root: messages[0], title: "Replication", reply_count: 3, reply_count_exact: true,
      updated_at: messages[5].timestamp },
    { id: other.id, root: messages[3], title: "Other", reply_count: 1, reply_count_exact: true,
      updated_at: messages[6].timestamp },
  ];
  await signIn(page);
  await showDiscord(page, messages);
  assert.equal(page.el("thread-select").value, "flat", "the channel did not open in All");
  assert.deepStrictEqual(page.el("discord-log").children.map((li) => li.getAttribute("data-ids")),
    ["701 702 703", "704", "705", "706", "707"]);
  const first = page.el("discord-log").children[0];
  assert.match(bodyOf(first).text(), /state replication model\s*part two\s*part three/,
    "the combined row does not show the whole post in order");
});

test("a root's summary on Main is not the summary of the long post it heads in All (#463)", async () => {
  // The same root heads two different rows: itself on Main, and the whole post with its parts in
  // All. Each row is summarised for the text it draws, so neither view's answer is shown in the
  // other, whichever view asked first.
  const at = (seconds) => new Date(MESSAGE_DAY.getTime() + seconds * 1000).toISOString();
  const post = { id: "1792765680.753759", root_message_id: "801", is_root: true, reply_count: 2,
    reply_count_exact: true };
  const messages = [
    message({ id: "801", content: longMessage("part one"), thread: post, timestamp: at(0), ...OWNER }),
    message({ id: "802", content: longMessage("part two"), thread: { ...post, is_root: false }, timestamp: at(3),
      ...OWNER }),
    message({ id: "803", content: longMessage("part three"), thread: { ...post, is_root: false },
      timestamp: at(6), ...OWNER }),
  ];
  const page = newPage();
  page.threadingSupported = true;
  page.threads = [{ id: post.id, root: messages[0], title: "Long post", reply_count: 2, reply_count_exact: true,
    updated_at: messages[2].timestamp }];
  await signIn(page);
  await showDiscord(page, messages);
  assert.deepStrictEqual(page.el("discord-log").children.map((li) => li.getAttribute("data-ids")),
    ["801 802 803"], "the fixture's post is not one row in All");
  await turnSummariesOn(page);
  assert.deepStrictEqual(page.summaryAsks.map((ask) => [ask.message, ask.with]), [["801", ["802", "803"]]],
    "All did not ask about the whole post");
  const picker = page.el("thread-select");
  picker.value = "main";
  await picker.dispatch("change");
  await page.settle();
  assert.deepStrictEqual(page.el("discord-log").children.map((li) => li.getAttribute("data-ids")), ["801"],
    "Main does not draw the root alone");
  // A scroll asks about what is on screen, as it does for any row (`requestVisibleSummaries`).
  await page.el("scroll-area").dispatch("scroll");
  await page.settle();
  assert.deepStrictEqual(page.summaryAsks.map((ask) => [ask.message, ask.with]),
    [["801", ["802", "803"]], ["801", []]], "Main reused the whole post's summary for its root alone");
});

/** One tap on a reply's arrow, and nothing waited for. */
async function clickArrow(arrow, li) {
  let stopped = false;
  const event = { target: arrow, stopPropagation: () => { stopped = true; } };
  await arrow.dispatch("click", event);
  if (!stopped) await li.dispatch("click", event);
}

/**
 * Two taps on a reply's arrow, inside the window that makes them one double tap; then, unless `wait`
 * is false, the reader waits, and the moment after them in which more taps are ignored passes — as a
 * first tap's jump would go, if one were wrongly still waiting, and the test would see it land.
 */
async function doubleTapArrow(page, li, { wait = true } = {}) {
  const arrow = replyArrowOf(li);
  assert.ok(arrow, "the row has no arrow to tap");
  for (let i = 0; i < 2; i += 1) await clickArrow(arrow, li);
  for (let i = 0; i < 4 * REPLY_JUMP_PAGES; i += 1) await page.settle();
  if (!wait) return;
  page.expireTimers(REPLY_DOUBLE_TAP_MS);
  for (let i = 0; i < 4 * REPLY_JUMP_PAGES; i += 1) await page.settle();
}

/** Put the row holding `id` `offset` pixels down the list's viewport, and say where it is. */
function parkRow(page, id, offset = 120) {
  const area = page.el("scroll-area");
  area.scrollTop += rowWithId(page, id).getBoundingClientRect().top - offset;
  return rowWithId(page, id).getBoundingClientRect().top;
}

test("THE THREAD CHIP reads Thread(N), and in Main is on the roots, the way into a thread", async () => {
  const page = await threadPage();
  assert.equal(threadBadge(rowWithId(page, "201")).textContent, "Thread(1)");
  assert.equal(threadBadge(rowWithId(page, "202")).textContent, "Thread(1)");
  assert.equal(threadBadge(rowWithId(page, "201")).getAttribute("aria-label"), "Open this thread, 1 reply");
  await pickThread(page, "main");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"]);
  assert.equal(threadBadge(rowWithId(page, "200")), undefined, "a message in no thread offers one");
  assert.equal(threadBadge(rowWithId(page, "201")).textContent, "Thread(1)",
    "Main's root no longer offers the way into its thread now that N replies gathers");
  // The picker names threads by their facts and name, never "Thread(N)": the heading matches it.
  assert.ok(threadOptions(page).every(([, label]) => !/Thread\(/.test(label)), "the picker took up the chip's form");
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  assert.equal(page.el("thread-heading").hidden, false, "the chip in Main did not open the thread");
});

test("N REPLIES NO LONGER OPENS THE THREAD: it gathers the replies under the message, in All", async () => {
  const page = await scatteredPage();
  const chip = gatherChip(page, "601");
  assert.equal(chip.textContent, "2 replies");
  assert.equal(chip.getAttribute("aria-pressed"), "false");
  const threadReads = () => page.timelineCalls.filter((path) => /view=thread/.test(path)).length;
  const reads = threadReads();
  await chip.click();
  await page.settle();
  assert.equal(page.el("thread-heading").hidden, true, "N replies opened the thread view");
  assert.equal(page.el("thread-select").value, "flat");
  assert.equal(threadReads(), reads, "N replies read the thread");
  // EVERY reply to it, by pointer or by thread, and the reply to a reply, under it in time order;
  // nothing lost; nothing else moved.
  assert.deepStrictEqual(shownIds(page), GATHERED);
  assert.deepStrictEqual([...shownIds(page)].sort(), [...SCATTERED].sort(), "gathering lost or added a message");
  assert.deepStrictEqual(stackPlaces(page), [["601", "parent"], ["602", "first"], ["603", "middle"],
    ["604", "middle"], ["606", "middle"], ["607", "last"]]);
  assert.equal(gatherChip(page, "601").getAttribute("aria-pressed"), "true", "the chip does not say it gathered them");
  assert.equal(statusText(page), "Gathered 5 replies under it.");
  assert.deepEqual(page.dismissCalls, [], "gathering marked something read");
});

test("THE BRIDGE: each gathered reply has its part of it, the first the X, and none an arrow drawn", async () => {
  const page = await scatteredPage();
  await gatherChip(page, "601").click();
  await page.settle();
  for (const id of ["602", "603", "604", "606", "607"]) {
    const li = rowWithId(page, id);
    assert.ok(gutterChild(li, "reply-bridge"), `${id} has no part of the bridge`);
    assert.equal(gutterChild(li, "reply-bridge").getAttribute("aria-hidden"), "true");
    // The stack hangs one gutter in from the parent, which is the agent's tile.
    assert.equal(li.getAttribute("data-stack-under"), "coder");
  }
  const x = gutterChild(rowWithId(page, "602"), "reply-ungather");
  assert.ok(x, "there is no way to put the replies back");
  assert.equal(x.tagName, "button");
  assert.equal(x.getAttribute("aria-label"), "Show replies in time order");
  assert.equal(x.text(), "", "the X is a typed glyph");
  assert.ok(["603", "604", "606", "607"].every((id) => !gutterChild(rowWithId(page, id), "reply-ungather")),
    "more than one X");
  assert.ok(["601", "605"].every((id) => !gutterChild(rowWithId(page, id), "reply-bridge")),
    "a row outside the stack has a part of the bridge");
  // A gathered reply's arrow is not drawn — the bridge stands in for it.
  assert.match(cssBlock("#discord-log li.discord-message[data-stack-under] > .reply-jump"), /display:\s*none/);
  // The stylesheet's half: the X a touch target square, the spine reaching the row above, the last
  // part ending in its own line, and an inset per shape of parent, on a desk as well.
  const square = cssBlock("#discord-log .reply-ungather");
  assert.match(square, /width:\s*var\(--reply-gutter\)/);
  assert.match(square, /height:\s*var\(--reply-gutter\)/);
  assert.match(cssBlock("#discord-log .reply-bridge"), /top:\s*calc\(-1 \* var\(--reply-rise\)\)/);
  assert.match(cssBlock('#discord-log li.discord-message[data-coalesce="last"] > .reply-bridge'), /border-bottom:/);
  for (const shape of ["coder", "me", "third", "reply", "me-reply"]) {
    const selector = `#discord-log li.discord-message[data-stack-under="${shape}"][data-who]`;
    assert.match(cssBlock(selector), /margin-left:/, `no inset for a stack under ${shape}`);
    assert.match(cssBlockIn("(min-width: 900px) and (pointer: fine)", selector), /margin-left:/,
      `no desk inset for a stack under ${shape}`);
  }
});

test("THE PARENT DOES NOT MOVE: gathering, and putting back, keep it where it was on the screen", async () => {
  const page = await scatteredPage();
  const area = page.el("scroll-area");
  const top = parkRow(page, "601");
  assert.ok(area.scrollTop > 0, "the list does not scroll, so this proves nothing");
  await gatherChip(page, "601").click();
  await page.settle();
  assert.equal(rowWithId(page, "601").getBoundingClientRect().top, top, "gathering moved the parent");
  await gutterChild(rowWithId(page, "602"), "reply-ungather").dispatch("click", { stopPropagation() {} });
  assert.deepStrictEqual(shownIds(page), SCATTERED, "the X did not put the replies back in time order");
  assert.deepStrictEqual(stackPlaces(page), [], "the stack outlived the X");
  assert.ok(page.el("discord-log").children.every((li) => !gutterChild(li, "reply-bridge")), "a bridge outlived the X");
  assert.equal(rowWithId(page, "601").getBoundingClientRect().top, top, "putting them back moved the parent");
  assert.equal(statusText(page), "Replies back in time order.");
  assert.equal(gatherChip(page, "601").getAttribute("aria-pressed"), "false");
});

test("...and the chip, pressed, puts them back as the X does", async () => {
  const page = await scatteredPage();
  await gatherChip(page, "601").click();
  await page.settle();
  await gatherChip(page, "601").click();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), SCATTERED, "a second tap on the chip did nothing");
});

test("FROM MAIN, the chip opens All and gathers there, with the parent where it was in Main", async () => {
  const page = await scatteredPage();
  await pickThread(page, "main");
  assert.ok(!shownIds(page).includes("604"), "Main shows the thread's replies, so this proves nothing");
  const top = parkRow(page, "601", 90);
  await gatherChip(page, "601").click();
  for (let i = 0; i < 4; i += 1) await page.settle();
  assert.equal(page.el("thread-select").value, "flat", "the chip did not open All");
  assert.deepStrictEqual(shownIds(page), GATHERED);
  assert.equal(rowWithId(page, "601").getBoundingClientRect().top, top, "the parent jumped when All opened");
});

test("TWO TAPS ON A REPLY'S ARROW gather the replies of what it answers, the tapped reply kept in place", async () => {
  const page = await scatteredPage();
  const area = page.el("scroll-area");
  const top = parkRow(page, "607", 260);
  const before = area.scrollTop;
  await doubleTapArrow(page, rowWithId(page, "607"), { wait: false });
  // One timer at the window's length: the moment in which a third tap is ignored. Two would be that and
  // the first tap's jump, still waiting to go.
  assert.equal(page.expireTimers(REPLY_DOUBLE_TAP_MS), 1, "the first tap's jump is still waiting to go");
  for (let i = 0; i < 4; i += 1) await page.settle();
  assert.deepStrictEqual(shownIds(page), GATHERED, "two taps did not gather the replies");
  assert.ok(page.el("discord-log").children.every((li) => li.getAttribute("data-landed") !== "true"),
    "the first of two taps jumped away");
  assert.ok(area.scrollTop < before, "the reply moved up and the list did not follow it, so this proves nothing");
  assert.equal(rowWithId(page, "607").getBoundingClientRect().top, top, "the tapped reply did not stay where it was");
});

test("...and ONE tap still jumps, once the window for a second has passed, and not before", async () => {
  const page = await scatteredPage();
  const reply = rowWithId(page, "607");
  const arrow = replyArrowOf(reply);
  await arrow.dispatch("click", { target: arrow, stopPropagation() {} });
  await page.settle();
  assert.equal(rowWithId(page, "601").getAttribute("data-landed"), null, "the tap jumped before a second could come");
  assert.equal(page.expireTimers(REPLY_DOUBLE_TAP_MS), 1);
  await page.settle();
  assert.equal(rowWithId(page, "601").getAttribute("data-landed"), "true", "the single tap never jumped");
  assert.deepStrictEqual(shownIds(page), SCATTERED, "a single tap gathered");
});

test("A PROVIDER WITHOUT THREADS gathers by the reply pointer, from two taps on an arrow", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [
    message({ id: "1000000000000000001", content: "is the runner wedged?" }),
    message({ id: "1000000000000000002", content: "the weather is nice" }),
    message({ id: "1000000000000000003", content: "restarted it", reply_to: "1000000000000000001", ...ALICE }),
    message({ id: "1000000000000000004", content: "lunch?" }),
    message({ id: "1000000000000000005", content: "and the queue drains", reply_to: "1000000000000000001", ...OWNER }),
  ]);
  await doubleTapArrow(page, rowWithId(page, "1000000000000000005"));
  assert.deepStrictEqual(shownIds(page), ["1000000000000000001", "1000000000000000003", "1000000000000000005",
    "1000000000000000002", "1000000000000000004"]);
  // A message arriving live that answers it joins the stack; one that does not stays at the foot.
  const stream = page.stream();
  await deliver(page, stream, sseMessage(message({ id: "1000000000000000006", content: "one more", reply_to: "1000000000000000001" })));
  await deliver(page, stream, sseMessage(message({ id: "1000000000000000007", content: "unrelated" })));
  assert.deepStrictEqual(shownIds(page), ["1000000000000000001", "1000000000000000003", "1000000000000000005",
    "1000000000000000006", "1000000000000000002", "1000000000000000004", "1000000000000000007"],
  "a live reply did not join the stack");
  // ...and what a reload draws, saved meanwhile, is in time order: a reload draws nothing gathered.
  const saved = savedScopes(page)[scopeKey(CHANNEL.id)].messages.map((m) => m.id);
  assert.deepStrictEqual(saved, [...saved].sort(), "the saved snapshot is in the gathered order");
});

test("GATHERED REPLIES OUTLIVE A REFRESH and a live reply joins them; a change of view or channel puts them back", async () => {
  const page = await scatteredPage();
  await gatherChip(page, "601").click();
  await page.settle();
  // A poll re-reads All and draws it in time order; it is drawn gathered.
  page.expireTimers(DISCORD_POLL_MS);
  for (let i = 0; i < 4; i += 1) await page.settle();
  assert.deepStrictEqual(shownIds(page), GATHERED, "a refresh scattered the replies");
  // A live reply in the thread joins the stack, at its end.
  const late = message({ id: "610", content: "and green again", thread: { id: "spaces/A/threads/build",
    root_message_id: "601", is_root: false, reply_count: 2, reply_count_exact: true } });
  page.messages = [...page.messages, late];
  page.stream().push(sseMessage(late));
  for (let i = 0; i < 4; i += 1) await page.settle();
  assert.deepStrictEqual(shownIds(page), [...GATHERED.slice(0, 10), "610", ...GATHERED.slice(10)],
    "a live reply to the parent did not join the stack");
  // Main and back: the replies are back in time order, in Main and in All.
  await pickThread(page, "main");
  await pickThread(page, "flat");
  assert.deepStrictEqual(stackPlaces(page), [], "a change of view kept the stack");
  assert.deepStrictEqual(shownIds(page), [...SCATTERED, "610"], "All came back in the gathered order");
});

test("A FILTER THAT TAKES ROWS AWAY puts the replies back; Hide read leaves the stack its unread replies", async () => {
  const page = await scatteredPage();
  await gatherChip(page, "601").click();
  await page.settle();
  await search(page, "runner");
  assert.deepStrictEqual(stackPlaces(page), [], "a search left a bridge to rows it hides");
  assert.deepStrictEqual(shownIds(page), SCATTERED);
  await page.el("search-toggle").click();
  // Asking to gather with the search open closes it, as the jump does for a row the search hides.
  await search(page, "runner");
  await gatherChip(page, "601").click();
  await page.settle();
  assert.equal(page.el("search-field").hidden, true, "the search was left over the replies it hides");
  assert.deepStrictEqual(shownIds(page), GATHERED);
  // Hide read: 604 is read, and the stack is what is left of it.
  page.dealtWith.add("604");
  await turnTodoOn(page);
  page.expireTimers(DISCORD_POLL_MS);
  for (let i = 0; i < 4; i += 1) await page.settle();
  assert.deepStrictEqual(shownIds(page), GATHERED.filter((id) => id !== "604"), "Hide read did not compose with the stack");
  assert.deepStrictEqual(stackPlaces(page).map(([id]) => id), ["601", "602", "603", "606", "607"]);
});

test("WHAT IS NOT GATHERED IS SAID: a thread's count above what is loaded, and Hide read", async () => {
  const page = await scatteredPage(3);
  await gatherChip(page, "601").click();
  await page.settle();
  assert.equal(statusText(page), "Gathered 5 replies; 1 not loaded.");
  const hidden = await scatteredPage();
  hidden.dealtWith.add("604");
  await turnTodoOn(hidden);
  hidden.expireTimers(DISCORD_POLL_MS);
  for (let i = 0; i < 4; i += 1) await hidden.settle();
  assert.ok(!shownIds(hidden).includes("604"), "Hide read is not hiding the read reply");
  await gatherChip(hidden, "601").click();
  await hidden.settle();
  assert.equal(statusText(hidden), "Gathered 4; Hide read hides 1 more.");
});

test("TWO STACKS NEVER: gathering another conversation puts the first's back", async () => {
  const page = await scatteredPage();
  await gatherChip(page, "601").click();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), GATHERED);
  // 611 answers 600, a message of another conversation: its double tap gathers that one instead.
  const late = message({ id: "611", content: "about the step before", reply_to: "600" });
  page.messages = [...page.messages, late];
  page.stream().push(sseMessage(late));
  for (let i = 0; i < 4; i += 1) await page.settle();
  assert.deepStrictEqual(shownIds(page), [...GATHERED, "611"], "the live message is not where this test assumes");
  await doubleTapArrow(page, rowWithId(page, "611"));
  assert.deepStrictEqual(shownIds(page), [...SCATTERED.slice(0, 4), "611", ...SCATTERED.slice(4)],
    "601's replies were left gathered as well");
  assert.deepStrictEqual(stackPlaces(page), [["600", "parent"], ["611", "only"]]);
  assert.equal(gatherChip(page, "601").getAttribute("aria-pressed"), "false");
});

/**
 * The owner's two trees, 2026-10-08: A <- B <- C, where C answers B, and A <- B, A <- C, where both
 * answer A. 1 is A; 3 is B; 5 is C; 2, 4 and 6 are other messages between and after them.
 */
function twoTrees(chain) {
  return [
    message({ id: "1000000000000000001", content: "is the runner wedged?" }),
    message({ id: "1000000000000000002", content: "the weather is nice" }),
    message({ id: "1000000000000000003", content: "restarted it", reply_to: "1000000000000000001", ...ALICE }),
    message({ id: "1000000000000000004", content: "lunch?" }),
    message({ id: "1000000000000000005", content: "the queue drains too",
      reply_to: chain ? "1000000000000000003" : "1000000000000000001", ...OWNER }),
    message({ id: "1000000000000000006", content: "back at two" }),
  ];
}

const TREE_GATHERED = ["1000000000000000001", "1000000000000000003", "1000000000000000005",
  "1000000000000000002", "1000000000000000004", "1000000000000000006"];

test("REPLIES TO REPLIES: A <- B <- C and A <- B, A <- C both stack B and C under A, in time order", async () => {
  for (const chain of [true, false]) {
    const tree = chain ? "A <- B <- C" : "A <- B, A <- C";
    const page = newPage();
    await signIn(page);
    await showDiscord(page, twoTrees(chain));
    // Two taps on C's arrow: in the chain C answers B, and the stack is still the whole of A's.
    const top = parkRow(page, "1000000000000000005", 200);
    await doubleTapArrow(page, rowWithId(page, "1000000000000000005"));
    assert.deepStrictEqual(shownIds(page), TREE_GATHERED, `${tree}: B and C are not under A in time order`);
    assert.deepStrictEqual(stackPlaces(page), [["1000000000000000001", "parent"], ["1000000000000000003", "first"],
      ["1000000000000000005", "last"]], `${tree}: the stack is not one bridge from A to B and C`);
    assert.equal(rowWithId(page, "1000000000000000005").getBoundingClientRect().top, top,
      `${tree}: the tapped reply did not stay where it was`);
    assert.equal(statusText(page), "Gathered 2 replies under it.");
    // B's arrow, two taps, and A's N replies would say the same: one stack for the conversation.
    await gutterChild(rowWithId(page, "1000000000000000003"), "reply-ungather").dispatch("click", { stopPropagation() {} });
    await doubleTapArrow(page, rowWithId(page, "1000000000000000003"));
    assert.deepStrictEqual(shownIds(page), TREE_GATHERED, `${tree}: B's arrow gathered something else`);
  }
});

test("...and ONE tap on C's arrow still jumps to its ACTUAL parent: B in the chain, A in the other tree", async () => {
  for (const chain of [true, false]) {
    const page = newPage();
    await signIn(page);
    await showDiscord(page, twoTrees(chain));
    await tapArrow(page, rowWithId(page, "1000000000000000005"));
    const landed = page.el("discord-log").children.filter((li) => li.getAttribute("data-landed") === "true")
      .map((li) => li.getAttribute("data-id"));
    assert.deepStrictEqual(landed, [chain ? "1000000000000000003" : "1000000000000000001"],
      `${chain ? "A <- B <- C" : "A <- B, A <- C"}: one tap on C's arrow went to the head of the conversation`);
    assert.deepStrictEqual(stackPlaces(page), [], "one tap gathered");
  }
});

test("TWO TAPS ON A REPLY TO A REPLY gather its conversation's head, the topmost message the list holds", async () => {
  // 603 answers 602, which answers 601: two taps on 603's arrow stack 601's whole conversation, not
  // 602's piece of it, and 603 stays where it was.
  const page = await scatteredPage();
  const top = parkRow(page, "603", 200);
  await doubleTapArrow(page, rowWithId(page, "603"));
  assert.deepStrictEqual(shownIds(page), GATHERED, "two taps on a reply to a reply gathered a piece of the conversation");
  assert.equal(stackPlaces(page)[0][0], "601");
  assert.equal(gatherChip(page, "601").getAttribute("aria-pressed"), "true",
    "the stack two taps drew is not the one 601's N replies says it holds");
  assert.equal(rowWithId(page, "603").getBoundingClientRect().top, top, "the tapped reply did not stay where it was");
});

test("A CYCLE IN THE POINTERS ends: no loop, and a stack under the message the tapped arrow names", async () => {
  // 1 and 3 answer each other, 5 answers 3, and 6 answers itself: no person can send these, and a
  // provider's data could still say them.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [
    message({ id: "1000000000000000001", content: "is the runner wedged?", reply_to: "1000000000000000003" }),
    message({ id: "1000000000000000002", content: "the weather is nice" }),
    message({ id: "1000000000000000003", content: "restarted it", reply_to: "1000000000000000001", ...ALICE }),
    message({ id: "1000000000000000004", content: "lunch?" }),
    message({ id: "1000000000000000005", content: "the queue drains", reply_to: "1000000000000000003", ...OWNER }),
    message({ id: "1000000000000000006", content: "a note to myself", reply_to: "1000000000000000006" }),
  ]);
  // 5's arrow: up from 3 to 1, and 1's pointer comes back round to 3. 1 is the head, and its group is
  // 3 and 5, each once.
  await doubleTapArrow(page, rowWithId(page, "1000000000000000005"));
  assert.deepStrictEqual(shownIds(page), ["1000000000000000001", "1000000000000000003", "1000000000000000005",
    "1000000000000000002", "1000000000000000004", "1000000000000000006"]);
  assert.deepStrictEqual(stackPlaces(page).map(([id]) => id),
    ["1000000000000000001", "1000000000000000003", "1000000000000000005"]);
  // 3's arrow names 1, and the walk up does not come back round through 3 itself: the same stack.
  await doubleTapArrow(page, rowWithId(page, "1000000000000000003"));
  assert.deepStrictEqual(stackPlaces(page).map(([id]) => id),
    ["1000000000000000001", "1000000000000000003", "1000000000000000005"]);
  // A message that answers itself has no replies to gather, and says so.
  await doubleTapArrow(page, rowWithId(page, "1000000000000000006"));
  assert.equal(statusText(page), "No replies to it are loaded.");
});

test("A MISSING PARENT ends the walk up: the head is the topmost message loaded", async () => {
  // 1 answers a message never loaded; 3 answers 1, and 5 answers 3; 6 answers another message never
  // loaded, so it is in no conversation here.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [
    message({ id: "1000000000000000001", content: "is the runner wedged?", reply_to: "999" }),
    message({ id: "1000000000000000002", content: "the weather is nice" }),
    message({ id: "1000000000000000003", content: "restarted it", reply_to: "1000000000000000001", ...ALICE }),
    message({ id: "1000000000000000004", content: "lunch?" }),
    message({ id: "1000000000000000005", content: "the queue drains", reply_to: "1000000000000000003", ...OWNER }),
    message({ id: "1000000000000000006", content: "same here", reply_to: "998" }),
  ]);
  // Up from 3 to 1, whose own message is not loaded: 1 is the head.
  await doubleTapArrow(page, rowWithId(page, "1000000000000000005"));
  assert.deepStrictEqual(stackPlaces(page), [["1000000000000000001", "parent"], ["1000000000000000003", "first"],
    ["1000000000000000005", "last"]]);
  assert.deepStrictEqual(shownIds(page), ["1000000000000000001", "1000000000000000003", "1000000000000000005",
    "1000000000000000002", "1000000000000000004", "1000000000000000006"]);
  // Two taps on an arrow whose message is not loaded at all look for it, as one tap does, and say so.
  await doubleTapArrow(page, rowWithId(page, "1000000000000000006"));
  assert.equal(statusText(page), "Older than what is loaded.");
  assert.equal(stackPlaces(page)[0][0], "1000000000000000001", "a message not found put the stack away");
});

test("...and a link through a message Hide read leaves out still joins what is drawn either side of it", async () => {
  // 302 is read, so Hide read does not draw it; 304 answers it, and it answers 301. Two taps on 304's
  // arrow pass through 302 to 301, and 304 is gathered under it, with 302 said as hidden.
  const page = newPage();
  page.threadingSupported = true;
  page.dealtWith.add("302");
  await signIn(page);
  await showDiscord(page, [
    message({ id: "300", content: "the weather is nice" }),
    message({ id: "301", content: "is the runner wedged?" }),
    message({ id: "302", content: "restarted it", reply_to: "301", ...ALICE }),
    message({ id: "303", content: "lunch?" }),
    message({ id: "304", content: "the queue drains", reply_to: "302", ...OWNER }),
  ]);
  await turnTodoOn(page);
  assert.deepStrictEqual(shownIds(page), ["300", "301", "303", "304"], "Hide read is drawing the read reply");
  await doubleTapArrow(page, rowWithId(page, "304"));
  assert.deepStrictEqual(shownIds(page), ["300", "301", "304", "303"]);
  assert.deepStrictEqual(stackPlaces(page), [["301", "parent"], ["304", "only"]],
    "the conversation stopped at the message Hide read leaves out");
  assert.equal(statusText(page), "Gathered 1; Hide read hides 1 more.");
});

test("COMBINING inside a stack: two quick replies from one author are one gathered row", async () => {
  const page = newPage();
  await signIn(page);
  const quick = (id, seconds, overrides) => message({ id, timestamp: `2026-08-19T05:00:${seconds}.000Z`, ...overrides });
  await showDiscord(page, [
    quick("1000000000000000001", "00", { content: "is the runner wedged?" }),
    quick("1000000000000000002", "30", { content: "lunch?", ...ALICE }),
    quick("1000000000000000003", "40", { content: "restarted it", reply_to: "1000000000000000001", ...OWNER }),
    quick("1000000000000000004", "41", { content: "and the queue drains", reply_to: "1000000000000000001", ...OWNER }),
  ]);
  assert.equal(page.el("discord-log").children.length, 3, "the owner's two quick replies were not combined");
  await doubleTapArrow(page, rowWithId(page, "1000000000000000003"));
  assert.deepStrictEqual(page.el("discord-log").children.map((li) => li.getAttribute("data-ids")),
    ["1000000000000000001", "1000000000000000003 1000000000000000004", "1000000000000000002"]);
  assert.deepStrictEqual(stackPlaces(page).map(([, place]) => place), ["parent", "only"]);
});

test("...but nothing newer joins the PARENT'S row: it stays the row time order draws it in", async () => {
  const quick = (id, seconds, overrides) => message({ id, timestamp: `2026-08-19T05:00:${seconds}.000Z`, ...overrides });
  const rows = (page) => page.el("discord-log").children.map((li) => li.getAttribute("data-ids"));
  // 1 and 3 are one author's, seconds apart, and two rows only because 2, which answers 1, is between.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [
    quick("1000000000000000001", "00", { content: "is the runner wedged?", ...ALICE }),
    quick("1000000000000000002", "01", { content: "restarted it", reply_to: "1000000000000000001", ...OWNER }),
    quick("1000000000000000003", "02", { content: "never mind, it reported", ...ALICE }),
  ]);
  assert.deepStrictEqual(rows(page), ["1000000000000000001", "1000000000000000002", "1000000000000000003"]);
  await doubleTapArrow(page, rowWithId(page, "1000000000000000002"));
  assert.deepStrictEqual(rows(page), ["1000000000000000001", "1000000000000000002", "1000000000000000003"],
    "a message newer than the reply joined the parent's row, above the stack and inside the box it hangs from");
  assert.deepStrictEqual(stackPlaces(page), [["1000000000000000001", "parent"], ["1000000000000000002", "only"]]);
  // And what time order drew in the parent's row stays there: the row the reply's arrow met is the row
  // its bridge comes out of, and 4 still does not join it.
  const combined = newPage();
  await signIn(combined);
  await showDiscord(combined, [
    quick("1000000000000000001", "00", { content: "is the runner wedged?", ...ALICE }),
    quick("1000000000000000002", "01", { content: "it has not reported since 03:10", ...ALICE }),
    quick("1000000000000000003", "03", { content: "restarted it", reply_to: "1000000000000000001", ...OWNER }),
    quick("1000000000000000004", "04", { content: "never mind, it reported", ...ALICE }),
  ]);
  const drawn = ["1000000000000000001 1000000000000000002", "1000000000000000003", "1000000000000000004"];
  assert.deepStrictEqual(rows(combined), drawn);
  assert.equal(arrowStyle(rowWithId(combined, "1000000000000000003")), "adjacent");
  await doubleTapArrow(combined, rowWithId(combined, "1000000000000000003"));
  assert.deepStrictEqual(rows(combined), drawn, "gathering redrew the parent's row as another row");
  assert.deepStrictEqual(stackPlaces(combined).map(([, place]) => place), ["parent", "only"]);
});

test("A THIRD TAP just after two is the same gesture: it neither jumps away nor presses the X", async () => {
  const page = await scatteredPage();
  const reply = rowWithId(page, "607");
  const arrow = replyArrowOf(reply);
  await doubleTapArrow(page, reply, { wait: false });
  assert.deepStrictEqual(shownIds(page), GATHERED);
  // The third, on the arrow and on the X that has appeared in the gutter, within the window.
  await clickArrow(arrow, reply);
  const x = gutterChild(rowWithId(page, "602"), "reply-ungather");
  await x.dispatch("click", { stopPropagation() {} });
  for (let i = 0; i < 4; i += 1) await page.settle();
  assert.equal(page.expireTimers(REPLY_DOUBLE_TAP_MS), 1, "a third tap started a jump of its own");
  for (let i = 0; i < 4 * REPLY_JUMP_PAGES; i += 1) await page.settle();
  assert.deepStrictEqual(shownIds(page), GATHERED, "a third tap put the replies straight back");
  assert.ok(page.el("discord-log").children.every((li) => li.getAttribute("data-landed") !== "true"),
    "a third tap jumped away from the replies it had just gathered");
  // Once the moment has passed, the X is the X again.
  await gutterChild(rowWithId(page, "602"), "reply-ungather").dispatch("click", { stopPropagation() {} });
  assert.deepStrictEqual(shownIds(page), SCATTERED, "the X did nothing once the taps had settled");
});

test("A TAP STILL WAITING TO JUMP goes with a change of view or of channel", async () => {
  const page = newPage();
  const data = scatteredThread();
  const second = { id: "1110000000000000002", label: "second", writable: true };
  page.threadingSupported = true;
  page.threads = data.threads;
  page.channels = [{ ...CHANNEL }, second];
  await signIn(page);
  await showDiscord(page, data.messages);
  assert.deepStrictEqual(shownIds(page), SCATTERED);
  const reply = rowWithId(page, "607");
  await clickArrow(replyArrowOf(reply), reply);
  await pickThread(page, "main");
  assert.equal(page.expireTimers(REPLY_DOUBLE_TAP_MS), 0, "the tap taken in All still waits to jump in Main");
  for (let i = 0; i < 4 * REPLY_JUMP_PAGES; i += 1) await page.settle();
  assert.equal(page.el("thread-select").value, "main", "a tap taken in All jumped once Main was open");
  assert.ok(page.el("discord-log").children.every((li) => li.getAttribute("data-landed") !== "true"));
  await pickThread(page, "flat");
  const again = rowWithId(page, "607");
  await clickArrow(replyArrowOf(again), again);
  page.el("discord-channel").value = second.id;
  await page.el("discord-channel").dispatch("change");
  await page.settle();
  assert.equal(page.expireTimers(REPLY_DOUBLE_TAP_MS), 0, "the tap taken in one channel still waits to jump in another");
});

test("DONE IN HIDE READ, taking a row out from between, decides the arrows and the stack again", async () => {
  // A provider without threads, whose Hide read takes a row dealt with out of the list where it is.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [
    message({ id: "1000000000000000001", content: "is the runner wedged?" }),
    message({ id: "1000000000000000002", content: "the weather is nice" }),
    message({ id: "1000000000000000003", content: "restarted it", reply_to: "1000000000000000001", ...ALICE }),
    message({ id: "1000000000000000004", content: "lunch?" }),
    message({ id: "1000000000000000005", content: "and the queue drains", reply_to: "1000000000000000001", ...OWNER }),
  ]);
  await turnTodoOn(page);
  assert.equal(arrowStyle(rowWithId(page, "1000000000000000003")), "disconnected");
  await doneButton(rowWithId(page, "1000000000000000002")).click();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["1000000000000000001", "1000000000000000003", "1000000000000000004",
    "1000000000000000005"]);
  assert.equal(arrowStyle(rowWithId(page, "1000000000000000003")), "adjacent",
    "the reply's arrow still points past the row Done took away");
  // Gathered: a reply dealt with leaves the stack one shorter, and its parent dealt with takes the stack.
  await doubleTapArrow(page, rowWithId(page, "1000000000000000005"));
  assert.deepStrictEqual(stackPlaces(page).map(([, place]) => place), ["parent", "first", "last"]);
  await doneButton(rowWithId(page, "1000000000000000003")).click();
  await page.settle();
  assert.deepStrictEqual(stackPlaces(page), [["1000000000000000001", "parent"], ["1000000000000000005", "only"]],
    "the bridge still draws a reply Done took out of the stack");
  await doneButton(rowWithId(page, "1000000000000000001")).click();
  await page.settle();
  assert.deepStrictEqual(stackPlaces(page), [], "the stack outlived its parent");
  assert.deepStrictEqual(shownIds(page), ["1000000000000000004", "1000000000000000005"],
    "the replies stayed where the stack had them");
});

test("IN ALL, a message not loaded yet is walked back to on the timeline's own cursor", async () => {
  const page = newPage();
  const all = pagedTimeline(page, { steps: 4, size: 4 });
  all[all.length - 1].reply_to = all[3].id;
  await signIn(page);
  await showDiscord(page, []);
  assert.equal(page.el("thread-select").value, "flat");
  assert.equal(rowWithId(page, all[3].id), undefined, "the question was loaded already");

  await tapArrow(page, rowWithId(page, all[all.length - 1].id));
  const older = timelineReads(page).filter((path) => /[?&]before=/.test(path));
  assert.equal(older.length, 3, `it read ${older.length} older pages of All, not the three it needed`);
  assert.ok(older.every((path) => /view=flat/.test(path)), "the walk read some other view than All");
  assert.equal(rowWithId(page, all[3].id)?.getAttribute("data-landed"), "true", "the walk did not land on it");
  assert.match(statusText(page), /Here is the message it answers/);
});

test("IN MAIN, a message the page has never read opens All, and All is walked back for it", async () => {
  // Nothing says which part of the channel an unread message is in, so it is looked for where
  // every message is. That is the view the reader is left in, so it is the one the channel keeps
  // (`#205 channel-view-memory`): the page reopens where the jump left the reader.
  const page = newPage();
  const all = pagedTimeline(page, { steps: 4, size: 4 });
  all[all.length - 1].reply_to = all[6].id;
  await signIn(page);
  await showDiscord(page, []);
  await pickThread(page, "main");
  assert.equal(page.el("thread-select").value, "main");

  await tapArrow(page, rowWithId(page, all[all.length - 1].id));
  assert.equal(page.el("thread-select").value, "flat", "the jump did not open All");
  const older = timelineReads(page).filter((path) => /[?&]before=/.test(path));
  assert.ok(older.length >= 1 && older.every((path) => /view=flat/.test(path)),
    `the walk was not All's: ${older.join(", ")}`);
  assert.equal(rowWithId(page, all[6].id)?.getAttribute("data-landed"), "true", "the walk did not land on it");
  assert.deepStrictEqual(savedUiState(page).channels.map((entry) => entry.channelView), ["flat"],
    "the channel is not kept in the view the jump left the reader in");
});

/**
 * Main, reopened from a session left there on a device that no longer holds that session's
 * snapshot, so All has never been read in this page and is not in the store; the second thread's
 * root, which Main shows, answers the first thread's answer, which only All does. With `polled`, a
 * poll of Main goes on the wire and is held there, so the tap on the arrow comes while it is in
 * flight and All's read is queued behind it. `answer` is what the read of All is answered with.
 */
async function tapInMainWithoutAll({ polled = true, answer = null } = {}) {
  const first = await threadPage("main");
  first.storage.delete(MESSAGE_CACHE_KEY);
  const data = threadData();
  data.messages[3].reply_to = "202";
  const page = reloadWith(first.storage, data.messages, (p) => {
    p.threadingSupported = true;
    p.threads = data.threads;
    holdCountsRead(p);
  });
  await reopenedChannel(page);
  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "Main is not what this test assumes");
  const serve = page.timeline;
  let release = () => {};
  const held = new Promise((resolve) => { release = resolve; });
  let holding = polled;
  page.timeline = async (path, options) => {
    if (holding) {
      holding = false;
      await held;
      return serve(path, options);
    }
    return answer && /view=flat/.test(String(path)) ? answer() : serve(path, options);
  };
  if (polled) {
    page.expireTimers(DISCORD_POLL_MS);
    await page.settle();
    assert.equal(holding, false, "no poll went on the wire, so this proves nothing");
  }
  await tapArrow(page, rowWithId(page, "203"));
  return { page, release };
}

/** Let the held poll land, and whatever was waiting behind it — a read, and a wait on that read. */
async function landHeldPoll(page, release) {
  release();
  for (let i = 0; i < 20; i += 1) {
    await page.settle();
    page.expireTimers(PULL_WAIT_MS);
  }
  await page.settle();
}

test("...and All opened while a read is on the wire is waited for, not taken for an empty history", async () => {
  // All has never been read here, so `changeChannelView` can only queue its read behind the poll's
  // and return with nothing drawn. An empty list then is All NOT READ YET, not All read to its
  // first message: it used to be said as "Not in this channel's history", and the row All brought
  // in a moment later was never landed on.
  const { page, release } = await tapInMainWithoutAll();
  assert.equal(page.el("thread-select").value, "flat", "the jump did not open All");
  assert.notEqual(page.el("status").textContent, "Not in this channel's history.",
    "All not read yet was taken for All read to the beginning of the channel");

  await landHeldPoll(page, release);
  assert.equal(rowWithId(page, "202")?.getAttribute("data-landed"), "true",
    "All's read landed and the jump never went to the message it brought in");
  assert.match(statusText(page), /Here is the message it answers/);
});

test("...and if that read of All fails, the end of the channel is not claimed either", async () => {
  // Queued behind a poll, and failing once it runs.
  const { page, release } = await tapInMainWithoutAll({ answer: providerDown });
  await landHeldPoll(page, release);
  assert.equal(page.el("thread-select").value, "flat");
  assert.equal(statusText(page), "Could not read All — try again.",
    "a read that never landed was reported as the end of the channel's history");

  // ...and read at once, with nothing in its way, and failing: the status line does not go on
  // saying All is being opened.
  const { page: direct } = await tapInMainWithoutAll({ polled: false, answer: providerDown });
  assert.equal(direct.el("thread-select").value, "flat");
  assert.equal(statusText(direct), "Could not read All — try again.",
    "a failed read of All left the status line saying All was on its way");
});

test("A DONE ON THE MESSAGE UNDER THE BOOKMARK slides it down to the next message still to read", async () => {
  // `#240 reading-bookmark`. The owner: "marking the current message as read can auto-bump a
  // bookmark sitting just before that message to after it." Past messages already Done, to the one
  // e selects next, so the bookmark and the selection stay together; conditionally, so a move made
  // on the other device meanwhile is never undone.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog(5));
  const id = (n) => `800000000000000000${n}`;
  await bookmarkRow(page, id(1));
  await doneButton(channelRow(page, id(3))).click();
  await page.settle();
  assert.deepStrictEqual(bookmarkAt(page), { id: id(1), place: "above" }, "a Done elsewhere moved the line");
  const before = page.bookmark.revision;

  await doneButton(channelRow(page, id(1))).click();
  await page.settle();
  assert.deepStrictEqual(bookmarkAt(page), { id: id(2), place: "above" }, "the line did not slide past the Done message");
  const slide = page.bookmarkCalls[page.bookmarkCalls.length - 1];
  assert.deepStrictEqual([slide.method, slide.body.message_id, slide.ifRevision], ["PUT", id(2), before],
    "the slide was not written conditionally on the revision the page held");

  await doneButton(channelRow(page, id(2))).click();
  await page.settle();
  assert.deepStrictEqual(bookmarkAt(page), { id: id(4), place: "above" },
    "the line stopped above a message already Done instead of the next one to read");

  await doneButton(channelRow(page, id(4))).click();
  await page.settle();
  assert.deepStrictEqual(bookmarkAt(page), { id: id(4), place: "below" },
    "with everything after it Done the line did not go below the newest message");
  assert.deepStrictEqual(page.bookmark.anchor && [page.bookmark.anchor.message_id, page.bookmark.anchor.place],
    [id(4), "below"]);
  assert.equal(page.el("jump-bookmark").hidden, true, "the line is on screen, and the chip is offered anyway");
});

test("...a Done anywhere else, putting a message back, and a new message never move it", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog(3));
  const id = (n) => `800000000000000000${n}`;
  await bookmarkRow(page, id(1));
  const calls = page.bookmarkCalls.length;
  await doneButton(channelRow(page, id(0))).click();
  await page.settle();
  await doneButton(channelRow(page, id(0))).click();
  await page.settle();
  assert.equal(page.dealtWith.has(id(0)), false, "the second tap did not put the message back");
  page.messages = [...page.messages, message({ id: "8000000000000000009", content: "just arrived" })];
  await reReadChannel(page);
  assert.deepStrictEqual(bookmarkAt(page), { id: id(1), place: "above" }, "the line moved");
  assert.equal(page.bookmarkCalls.length, calls, "something that is not the owner's own act wrote the bookmark");
});

test("...and below the newest message, an arrival comes in under the line", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog(2));
  const id = (n) => `800000000000000000${n}`;
  await bookmarkRow(page, id(1));
  await doneButton(channelRow(page, id(1))).click();
  await page.settle();
  assert.deepStrictEqual(bookmarkAt(page), { id: id(1), place: "below" });
  const calls = page.bookmarkCalls.length;
  page.messages = [...page.messages, message({ id: "8000000000000000009", content: "something new" })];
  await reReadChannel(page);
  assert.deepStrictEqual(bookmarkAt(page), { id: "8000000000000000009", place: "above" },
    "the new message did not come in under the line");
  assert.equal(page.bookmarkCalls.length, calls, "an arrival wrote the bookmark");
});

test("...and an arrival that joins the newest row, below whose message the line sits, puts the line above that row, never under it", async () => {
  // A row is one box, so the line cannot go between its messages: past the row would put a message
  // still to read above the line, and above it only puts a message already read below the line.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog(2));
  const id = (n) => `800000000000000000${n}`;
  await bookmarkRow(page, id(1));
  await doneButton(channelRow(page, id(1))).click();
  await page.settle();
  assert.deepStrictEqual(bookmarkAt(page), { id: id(1), place: "below" });
  const calls = page.bookmarkCalls.length;
  const last = page.messages[page.messages.length - 1];
  page.messages = [...page.messages, message({ id: "8000000000000000009", content: "and one more thing",
    timestamp: new Date(Date.parse(last.timestamp) + 1500).toISOString() })];
  await reReadChannel(page);
  assert.deepStrictEqual(page.el("discord-log").children.at(-1).getAttribute("data-ids").split(" "),
    [id(1), "8000000000000000009"], "the fixture made no combined row");
  assert.deepStrictEqual(bookmarkAt(page), { id: id(1), place: "above" },
    "the line sits under a row holding a message still to read");
  assert.equal(page.bookmark.anchor.place, "below", "drawing the line elsewhere moved the bookmark");
  assert.equal(page.bookmarkCalls.length, calls, "an arrival wrote the bookmark");
});

test("the bookmark is a line in the accent across the list, in room of its own, that a tap cannot clear", () => {
  const line = cssBlock("#discord-log .bookmark-line");
  assert.match(line, /color:\s*var\(--accent\)/, "the word is not in the accent");
  assert.match(line, /pointer-events:\s*none/, "a stray tap could land on the line");
  assert.match(line, /filter:\s*none !important/, "a receding row would drain the line with it");
  assert.doesNotMatch(line, /-100vw/, "a viewport-wide line brings a horizontal scrollbar with it");
  const rule = cssBlock("#discord-log .bookmark-line::before");
  assert.match(rule, /border-top:\s*2px solid var\(--accent\)/, "the rule is not solid, two pixels, in the accent");
  assert.match(cssBlock('#discord-log li.discord-message[data-bookmark="above"]'), /margin-top:\s*var\(--bookmark-room\)/,
    "the row under the line keeps no room for it");
  assert.match(cssBlock('#discord-log li.discord-message[data-bookmark="above"] > .bookmark-line'), /bottom:\s*calc\(100% \+ 1px\)/);
  // A collapsed run hides everything on its line's row but the run's line — and the bookmark's.
  assert.match(cssBlock('#discord-log li.discord-message[data-read-run="head"] > .bookmark-line'), /display:\s*flex !important/);
  // ...and a row the line is on recedes in its box, as a reply does, so the line keeps its colour.
  assert.match(cssBlock("#discord-log li.discord-message[data-bookmark][data-who]"), /opacity:\s*1/);
  assert.match(cssBlock("#discord-log li.discord-message[data-bookmark]::after"), /--row-veil/);
});

test("THE REPLY REALLY REFERENCES THE MESSAGE IT WAS OPENED ON", async () => {
  // Asserted on the recorded request, never on the interface's own claim of success. `reply_to` is
  // the whole point: it is what makes Discord record a reference, which is what makes the answer a
  // REPLY instead of a loose message that happens to follow.
  const page = newPage();
  await signIn(page);
  await openReplyOn(
    page,
    [message({ id: "111", content: "first" }), message({ id: "222", content: "second" })],
    1
  );

  page.el("reply-text").value = "  looking at it now  ";
  await page.el("reply-text").dispatch("input");
  await page.el("reply-send").click();
  await page.settle();

  assert.equal(page.repliesPosted.length, 1, "the reply was not posted at all");
  const sent = page.repliesPosted[0];
  assert.deepStrictEqual(sentWithoutKey(sent.body), { text: "looking at it now", reply_to: "222" });
  assert.equal(sent.method, "POST");
  assert.equal(sent.contentType, "application/json", "a JSON body went out with no content type");
  assert.ok(
    sent.path.includes(encodeURIComponent(CHANNEL.id)),
    `the reply was posted to ${sent.path}, which does not name the channel being read`
  );
  // And it comes back onto the screen, from the server's own answer.
  assert.equal(page.screen(), "main", "a successful send left the reader on the reply screen");
  const lines = page.el("discord-log").children;
  assert.match(lines[lines.length - 1].text(), /looking at it now/);
});

test("a draft survives leaving without sending, and survives a reload", async () => {
  const store = new Map();
  const page = newPage(store);
  await signIn(page);
  const messages = [message({ id: "111", content: "first" }), message({ id: "222", content: "second" })];
  await openReplyOn(page, messages, 0);

  page.el("reply-text").value = "half a thought about the first one";
  await page.el("reply-text").dispatch("input");
  await page.el("reply-cancel").click();

  // Per MESSAGE, not per page: a global draft would hand what you wrote about one message to a
  // reply to a different one, which is the kind of mistake that gets posted.
  await openReplyOn(page, messages, 1);
  assert.equal(page.el("reply-text").value, "", "the draft leaked onto a different message");
  await page.el("reply-cancel").click();
  await openReplyOn(page, messages, 0);
  assert.equal(page.el("reply-text").value, "half a thought about the first one");
  await page.el("reply-cancel").click();

  // A RELOAD: same storage, brand new execution — back on the channel it was left on.
  const reloaded = reloadWith(store, messages);
  await signIn(reloaded);
  await reopenedChannel(reloaded);
  await openReplyOn(reloaded, messages, 0);
  assert.equal(
    reloaded.el("reply-text").value,
    "half a thought about the first one",
    "the draft did not survive a reload"
  );
});

test("a browser that refuses to store a DRAFT says so, rather than promising a reload", async () => {
  // The read-after-write check was written here and its answer thrown away, which made it a
  // comment: the page checked, learned the draft had not been stored, and told nobody. The test
  // above is exactly the promise being broken — "it survives a reload" — so a reader in private
  // browsing has to be told it will not, in the place they typed it.
  const page = newPage();
  await signIn(page);
  await openReplyOn(page, [message({ id: "777", content: "answer me" })]);
  // Private browsing: setItem is accepted and then does nothing.
  page.storage.set = () => page.storage;

  page.el("reply-text").value = "something I would rather not retype";
  await page.el("reply-text").dispatch("input");

  const said = page.el("reply-state").textContent;
  assert.match(
    said,
    /refused to store/,
    `the page said nothing about a draft it did not keep: ${said}`
  );
  assert.match(said, /reload/);
  // ...and it is still here for THIS screen, which the page must not muddle with the warning.
  assert.equal(page.el("reply-text").value, "something I would rather not retype");

  // The other direction: a browser that really stores it must NOT be accused of losing it, or the
  // warning becomes a decoration everybody learns to ignore.
  delete page.storage.set;
  const kept = newPage();
  await signIn(kept);
  await openReplyOn(kept, [message({ id: "888", content: "answer me too" })]);
  kept.el("reply-text").value = "this one is safe";
  await kept.el("reply-text").dispatch("input");
  assert.equal(kept.el("reply-state").textContent, "", "a stored draft was reported as lost");
});

test("a failed Google Chat send names the backend and retains the message outside the editor", async () => {
  const page = newPage();
  page.chatProviderName = "Google Chat";
  await signIn(page);
  await openReplyOn(page, [message({ id: "333", content: "the one being answered" })]);
  page.replyResponse = errorResponse(502, "chat_error", "Google Chat returned HTTP 502");

  page.el("reply-text").value = "this took a while to write";
  await page.el("reply-text").dispatch("input");
  await page.el("reply-send").click();
  await page.settle();

  assert.equal(page.screen(), "main");
  assert.equal(page.el("reply-text").value, "");
  const held = outgoingRows(page)[0];
  assert.match(held.text(), /this took a while to write/);
  assert.match(held.text(), /Google Chat returned HTTP 502/);
  assert.doesNotMatch(held.text(), /discord/i);
  assert.ok(outgoingRetry(held), "the failed message offers no way to retry");
  assert.equal(savedOutgoing(page)[0].remaining, "this took a while to write",
    "a reload would lose the failed message");
  // A failed local message is distinct from history confirmed by the provider.
  assert.equal(page.el("discord-log").children.length, 1);
});

test("a reply that fails does NOT hang up on you", async () => {
  // Same property `guardQuietly` exists for on the channel read: posting is not the call, and a
  // network blink while typing must not cost the conversation.
  const page = newPage();
  const socket = await startTalking(page);
  await openReplyOn(page, [message({ id: "444", content: "answer me" })]);
  page.replyResponse = errorResponse(502, "discord_error", "nope");

  page.el("reply-text").value = "on it";
  await page.el("reply-text").dispatch("input");
  await page.el("reply-send").click();
  await page.settle();

  assert.equal(page.tracks[0].stops, 0, "a failed reply released the microphone");
  assert.notEqual(socket.readyState, 3, "a failed reply hung up the call");
  assert.equal(page.sockets.length, 1);
  assert.equal(page.el("error").hidden, true, "a reply failure raised the CALL's failure panel");
});

test("the reply's box names the message being answered, with its id", async () => {
  // `#410 inline-reply-editor`. The message itself is its row, where it was on the list, unfolded or
  // not as the reader left it; the box under it says who is answered and the id the answer goes to.
  const page = newPage();
  await signIn(page);
  const long = longMessage("the one being answered");
  await openReplyOn(page, [message({ id: "666777888999000111", author: "build-bot", content: long })]);
  assert.match(page.el("reply-target-meta").textContent, /666777888999000111/);
  assert.match(page.el("reply-target-meta").textContent, /build-bot \(bot\)/);
});

test("a reply from All to a thread reply that arrived live goes into that thread, not the main channel", async () => {
  // The failure of 2026-10-04: a thread reply arrived on the live stream, whose copy carried no
  // thread record, and the read that would have placed it timed out. Reply from All then posted
  // on the main channel, where the answer stood alone as a thread of its own.
  const page = await threadPage();
  await page.el("channel-view-flat").click();
  await page.settle();
  const first = page.messages[1].thread;
  const pushed = message({ id: "206", content: "a later answer in the first discussion" });
  page.messages.push({ ...pushed, thread: { ...first, is_root: false } });
  const standard = page.timeline;
  page.timeline = errorResponse(502, "chat_error", "history read timed out");
  await deliver(page, page.stream(), sseMessage(pushed));
  await page.settle();
  // "slow" since `#195 send-resilience`: a read that timed out says so rather than just "failed".
  assert.ok(["failed", "slow"].includes(freshness(page).getAttribute("data-state")),
    "the read after the arrival did not fail");

  await replyButton(rowOf(page, "206")).click();
  page.timeline = standard;
  await page.el("reply-text").setValue("answering where the question was asked");
  await page.el("reply-send").click();
  await page.settle();
  assert.equal(page.repliesPosted.length, 1);
  assert.deepEqual(sentWithoutKey(page.repliesPosted[0].body), {
    text: "answering where the question was asked", reply_to: "206", thread_id: first.id,
  }, "the reply left its thread for the main channel");
});

test("a reply whose message cannot be placed is not posted anywhere, and Retry places it once the channel reads", async () => {
  const page = await threadPage();
  await page.el("channel-view-flat").click();
  await page.settle();
  const first = page.messages[1].thread;
  const pushed = message({ id: "206", content: "a later answer in the first discussion" });
  page.messages.push({ ...pushed, thread: { ...first, is_root: false } });
  const standard = page.timeline;
  page.timeline = errorResponse(502, "chat_error", "history read timed out");
  await deliver(page, page.stream(), sseMessage(pushed));
  await page.settle();

  await replyButton(rowOf(page, "206")).click();
  await page.el("reply-text").setValue("held until the page knows where");
  await page.el("reply-send").click();
  await page.settle();
  assert.equal(page.repliesPosted.length, 0, "a reply was posted before anyone knew where it belonged");
  const held = outgoingRows(page)[0];
  assert.notEqual(held.getAttribute("data-send-state"), "unconfirmed", "nothing was posted, so nothing is unconfirmed");
  // `#277 send-order`: a read that timed out goes again on its own, which posting nothing cannot
  // post anything twice, and says when.
  assert.equal(held.getAttribute("data-send-state"), "retrying", "a read that timed out was left to the reader");
  assert.match(held.text(), retryingFirst(), "the receipt did not say when it goes again");
  assert.match(held.text(), /thread/i, "the receipt did not say what the page could not find out");
  assert.match(held.text(), /history read timed out/, "the receipt hid why");
  assert.equal(savedOutgoing(page)[0].remaining, "held until the page knows where");

  page.timeline = standard;
  await outgoingRetry(held).click();
  await page.settle();
  assert.deepEqual(sentWithoutKey(page.repliesPosted[0].body), {
    text: "held until the page knows where", reply_to: "206", thread_id: first.id,
  });
});

test("the live echo of a reply sent into a thread does not take the thread away from it", async () => {
  const page = await threadPage();
  await page.el("channel-view-flat").click();
  await page.settle();
  const first = page.messages[1].thread;
  const mine = message({ id: "9301", content: "my answer in the first discussion", reply_to: "202",
    thread: { ...first, is_root: false, reply_count: null, reply_count_exact: false } });
  page.replyResponse = async () => json(200, { posted: mine });
  // No read succeeds from here on, so only what the page keeps can say where the reply is.
  page.timeline = errorResponse(502, "chat_error", "history read timed out");
  await replyButton(rowOf(page, "202")).click();
  await page.el("reply-text").setValue("my answer in the first discussion");
  await page.el("reply-send").click();
  await page.settle();
  assert.deepEqual(sentWithoutKey(page.repliesPosted[0].body), {
    text: "my answer in the first discussion", reply_to: "202", thread_id: first.id,
  });
  // The stream's copy of the same message, which says nothing about threads.
  const { thread: _withheld, ...echo } = mine;
  await deliver(page, page.stream(), sseMessage(echo, { self_posted: true }));
  await page.settle();
  // Main and back to All again draws All from what the page holds, with no read.
  await page.el("channel-view-main").click();
  await page.settle();
  assert.equal(rowOf(page, "9301"), undefined, "a thread reply was drawn on Main");
  await page.el("channel-view-flat").click();
  await page.settle();
  assert.ok(threadBadge(rowOf(page, "9301")), "the echo erased the reply's thread");

  page.replyResponse = async () => json(200, { posted: message({ id: "9302", content: "and a follow-up" }) });
  await replyButton(rowOf(page, "9301")).click();
  await page.el("reply-text").setValue("and a follow-up");
  await page.el("reply-send").click();
  await page.settle();
  assert.deepEqual(sentWithoutKey(page.repliesPosted[1].body), { text: "and a follow-up", reply_to: "9301", thread_id: first.id });
});

test("a read that lands after a newer one was asked for still places the message a reply is opened on", async () => {
  // A read that lands once a newer one has been asked for is folded into the store without
  // redrawing rows whose ids did not change, so the row on screen keeps the stream's copy.
  const page = await threadPage();
  await page.el("channel-view-flat").click();
  await page.settle();
  const first = page.messages[1].thread;
  const pushed = message({ id: "206", content: "a later answer in the first discussion" });
  page.messages.push({ ...pushed, thread: { ...first, is_root: false } });
  const held = gate(page.timeline);
  const failing = errorResponse(502, "chat_error", "history read timed out");
  let reads = 0;
  page.timeline = (path) => (++reads === 1 ? held.respond(path) : failing());
  await deliver(page, page.stream(), sseMessage(pushed));
  const aside = message({ id: "207", content: "an aside on the main channel" });
  page.messages.push(aside);
  await deliver(page, page.stream(), sseMessage(aside));
  held.open();
  await page.settle();
  await page.settle();
  assert.equal(reads, 2, "the newer read was never made");
  assert.equal(threadBadge(rowOf(page, "206")), undefined,
    "the late read redrew the row, so this no longer tests the row's stale copy");

  await replyButton(rowOf(page, "206")).click();
  await page.el("reply-text").setValue("answering from the stale row");
  await page.el("reply-send").click();
  await page.settle();
  assert.deepEqual(sentWithoutKey(page.repliesPosted[0].body), {
    text: "answering from the stale row", reply_to: "206", thread_id: first.id,
  }, "the reply trusted the row's copy over what the store had since read");
});

test("a message the stream left unplaced is still unplaced after a reload, so its reply still waits", async () => {
  const page = await threadPage();
  await page.el("channel-view-flat").click();
  await page.settle();
  const first = page.messages[1].thread;
  const pushed = message({ id: "206", content: "a later answer in the first discussion" });
  page.messages.push({ ...pushed, thread: { ...first, is_root: false } });
  page.timeline = errorResponse(502, "chat_error", "history read timed out");
  await deliver(page, page.stream(), sseMessage(pushed));
  await page.settle();

  const again = reloadPage(page.storage, (p) => {
    p.threadingSupported = true;
    p.threads = page.threads;
    p.messages = page.messages;
    p.timeline = offline;
  });
  await reopenedChannel(again);
  assert.ok(rowOf(again, "206"), "the saved row was not drawn");
  await replyButton(rowOf(again, "206")).click();
  assert.equal(again.el("reply-destination").hidden, false);
  assert.match(again.el("reply-destination").textContent, /not known yet/,
    "after a reload the page took the saved copy's silence for the main channel");
  await again.el("reply-text").setValue("still waiting to know where");
  await again.el("reply-send").click();
  await again.settle();
  assert.equal(again.repliesPosted.length, 0, "the reply was guessed onto the main channel");
});

test("a thread nobody has answered yet is tagged Thread with no number, and grows no replies chip", async () => {
  // A provider can name a thread for a message nobody has answered — every Google Chat message
  // heads one — so a zero is an ordinary count, not a broken one.
  const page = newPage();
  page.threadingSupported = true;
  page.threads = [];
  const lone = { id: "spaces/A/threads/lone", root_message_id: "210", is_root: true,
    reply_count: 0, reply_count_exact: true };
  await signIn(page);
  await showDiscord(page, [message({ id: "210", content: "nobody has answered this", thread: lone })]);
  assert.equal(threadButton(rowOf(page, "210")), undefined, "Main offered a chip for no replies");
  assert.deepStrictEqual(threadOptions(page), [["main", "Main"], ["flat", "All"], ["threads", "Thread list"]],
    "the picker offered a thread with nothing in it to read");

  await page.el("channel-view-flat").click();
  await page.settle();
  const badge = threadBadge(rowOf(page, "210"));
  assert.equal(badge.textContent, "Thread");
  // `#252 thread-button-state`: no count to announce, and what the tag does instead — it starts the
  // thread nobody has answered.
  assert.equal(badge.getAttribute("aria-label"), "Start a thread on this message", "the tag announced a count of nothing");
  assert.equal(threadButton(rowOf(page, "210")), undefined, "All offered a chip for no replies");
  assert.doesNotMatch(page.el("discord-log").text(), /Thread 0|0 replies/);

  // Its tag still opens it, and while it is open the picker names it.
  await badge.click();
  await page.settle();
  assert.equal(page.el("thread-heading").hidden, false);
  assert.ok(threadOptions(page).some(([value]) => value === `thread:${lone.id}`));
});

test("Reply offers a new thread only on a main-channel message the chat service can branch one from", async () => {
  const page = newPage();
  page.threadingSupported = true;
  const data = threadData();
  page.threads = data.threads;
  const lone = { id: "spaces/A/threads/lone", root_message_id: "210", is_root: true,
    reply_count: 0, reply_count_exact: true };
  await signIn(page);
  await showDiscord(page, [...data.messages, message({ id: "210", content: "nobody has answered this", thread: lone })]);
  distinctReplies(page);
  const branchOffered = () => !page.el("reply-branch-row").hidden;
  const send = async (text) => {
    await page.el("reply-text").setValue(text);
    await page.el("reply-send").click();
    await page.settle();
    return sentWithoutKey(page.repliesPosted.at(-1).body);
  };

  // A main-channel message with an empty thread: offered, off, and off means a reply on Main.
  await replyButton(rowOf(page, "210")).click();
  assert.ok(branchOffered(), "no new-thread choice where one is possible");
  assert.equal(page.el("reply-branch").checked, false, "a new thread was the default");
  assert.match(page.el("reply-destination").textContent, /main channel/);
  assert.deepEqual(await send("an answer on the main channel"), { text: "an answer on the main channel", reply_to: "210" });

  // Ticked, the reply starts that message's thread — and the tick does not outlive the reply.
  await replyButton(rowOf(page, "210")).click();
  assert.equal(page.el("reply-branch").checked, false, "the last reply's choice carried over");
  page.el("reply-branch").checked = true;
  await page.el("reply-branch").dispatch("change");
  assert.match(page.el("reply-destination").textContent, /new thread/);
  assert.deepEqual(await send("starting a thread on it"),
    { text: "starting a thread on it", reply_to: "210", thread_id: lone.id });

  // A root whose thread already has replies has nothing new to branch: it is answered on Main.
  await replyButton(rowOf(page, "201")).click();
  assert.equal(branchOffered(), false, "a new thread was offered where one already exists");
  assert.deepEqual(await send("an answer to the root"), { text: "an answer to the root", reply_to: "201" });

  // A message with no thread at all, which this provider says is on the main channel.
  await replyButton(rowOf(page, "200")).click();
  assert.equal(branchOffered(), false);
  assert.equal(page.el("reply-destination").hidden, true, "a plain reply grew a note");
  assert.deepEqual(await send("an answer to the announcement"), { text: "an answer to the announcement", reply_to: "200" });

  // On All, a thread reply is answered in its thread, with nothing to choose.
  await page.el("channel-view-flat").click();
  await page.settle();
  await replyButton(rowOf(page, "202")).click();
  assert.equal(branchOffered(), false);
  assert.match(page.el("reply-destination").textContent, /thread this message belongs to/);
  assert.deepEqual(await send("an answer in the thread"),
    { text: "an answer in the thread", reply_to: "202", thread_id: data.threads[0].id });
});

test("Reply offers no new thread on a root the page has since learnt was answered, whatever its row says", async () => {
  // A row keeps the copy it was drawn from. Drawn while nobody had answered the root, that copy
  // still says 0 replies after the store has read otherwise — and ticking "Start a new thread"
  // would post into the thread that already exists.
  const page = newPage();
  page.threadingSupported = true;
  const data = threadData();
  page.threads = data.threads;
  const lone = { id: "spaces/A/threads/lone", root_message_id: "210", is_root: true,
    reply_count: 0, reply_count_exact: true };
  const root = message({ id: "210", content: "nobody has answered this yet", thread: lone });
  await signIn(page);
  await showDiscord(page, [...data.messages, root]);
  await page.el("channel-view-flat").click();
  await page.settle();

  // Somebody answers it, and the read that says so lands after a newer one was asked for.
  const answered = { ...lone, reply_count: 1 };
  const answer = message({ id: "211", content: "an answer at last" });
  page.messages = [...data.messages, { ...root, thread: answered }, { ...answer, thread: { ...answered, is_root: false } }];
  const held = gate(page.timeline);
  const failing = errorResponse(502, "chat_error", "history read timed out");
  let reads = 0;
  page.timeline = (path) => (++reads === 1 ? held.respond(path) : failing());
  await deliver(page, page.stream(), sseMessage(answer));
  const aside = message({ id: "212", content: "an aside on the main channel" });
  page.messages.push(aside);
  await deliver(page, page.stream(), sseMessage(aside));
  held.open();
  await page.settle();
  await page.settle();
  assert.equal(reads, 2, "the newer read was never made");
  assert.equal(threadButton(rowOf(page, "210")), undefined,
    "the late read redrew the root, so this no longer tests the row's old copy");

  await replyButton(rowOf(page, "210")).click();
  assert.equal(page.el("reply-branch-row").hidden, true, "a new thread was offered on a root that has replies");
});

/** The note under the reply's box: where Send will post (`renderReplyDestination`). */
const replyNote = (page) => page.el("reply-destination").textContent;

// The outbox posts the messages bound for one destination in the order they were sent. A reply
// still being placed has no destination yet: until it has one it may share any, and once it has
// one it is behind whatever was already on its way there.

/**
 * All, holding a message that came in live and that no read has placed since: by default the first
 * discussion's answer `206`, which the channel's own reads put in that thread. Reads fail from the
 * arrival on; `standard` is the fixture's timeline route, for a test to put back.
 */
async function unplacedArrival(fields = {}) {
  const page = await threadPage();
  await page.el("channel-view-flat").click();
  await page.settle();
  const first = page.messages[1].thread;
  const pushed = message({ id: "206", content: "a later answer in the first discussion", ...fields });
  page.messages.push(fields.id ? pushed : { ...pushed, thread: { ...first, is_root: false } });
  const standard = page.timeline;
  page.timeline = errorResponse(502, "chat_error", "history read timed out");
  await deliver(page, page.stream(), sseMessage(pushed));
  await page.settle();
  return { page, first, standard };
}

/** Hold each POST to the reply route until the test answers it, as its own new message. */
function heldReplies(page) {
  const waiting = [];
  page.replyResponse = (_path, options) => new Promise((resolve) => {
    const body = JSON.parse(options.body);
    const sent = waiting.length + 1;
    waiting.push(() => resolve(json(200, {
      posted: message({ id: `940000000000000000${sent}`, content: body.text, reply_to: body.reply_to || null }),
    })));
  });
  return waiting;
}

test("an answer sent into a thread waits for an earlier reply that is still finding out it belongs there", async () => {
  const { page, first, standard } = await unplacedArrival();
  const placing = gate(standard);
  page.timeline = placing.respond;
  distinctReplies(page);
  await replyQuietly(page, "206", "the first answer, to the message that came in live");
  await replyQuietly(page, "202", "the second answer, to a message the page has placed");
  assert.equal(page.repliesPosted.length, 0, "the second answer did not wait for the first to be placed");

  placing.open();
  await page.settle();
  await page.settle();
  assert.deepEqual(postedWhere(page), [
    ["the first answer, to the message that came in live", first.id],
    ["the second answer, to a message the page has placed", first.id],
  ], "the thread received its answers in the wrong order");
});

test("a reply placed in a thread waits behind an answer already on its way there", async () => {
  const { page, first, standard } = await unplacedArrival();
  const answers = heldReplies(page);
  await replyQuietly(page, "202", "an answer already on its way to the thread");
  assert.equal(page.repliesPosted.length, 1);

  page.timeline = standard;
  await replyQuietly(page, "206", "an answer that had to find its thread first");
  await page.settle();
  assert.equal(page.repliesPosted.length, 1,
    "the placed reply overtook the answer queued before it for the same thread");
  const placed = outgoingRows(page).find((row) => /an answer that had to find its thread first/.test(row.text()));
  assert.match(placed ? placed.text() : "", /Waiting to send/, "the reply was not placed and held");

  answers[0]();
  await page.settle();
  await page.settle();
  assert.deepEqual(postedWhere(page), [
    ["an answer already on its way to the thread", first.id],
    ["an answer that had to find its thread first", first.id],
  ]);
  answers[1]();
  await page.settle();
});

test("a message to the main channel waits for a reply to be placed, not for its delivery into a thread", async () => {
  const { page, first, standard } = await unplacedArrival();
  const placing = gate(standard);
  page.timeline = placing.respond;
  const answers = heldReplies(page);
  await replyQuietly(page, "206", "an answer that has to find its thread first");
  await page.el("channel-compose-text").setValue("a note to the main channel");
  page.el("channel-send").click();
  await page.settle();
  assert.equal(page.repliesPosted.length, 0,
    "a message that could have shared the reply's destination did not wait to learn it");

  placing.open();
  await page.settle();
  await page.settle();
  assert.deepEqual(postedWhere(page), [
    ["an answer that has to find its thread first", first.id],
    ["a note to the main channel", null],
  ], "the main channel waited for a delivery into a thread");
  answers[0]();
  answers[1]();
  await page.settle();
});

test("a message to the main channel waits for the delivery of a reply placed on the main channel", async () => {
  const { page, standard } = await unplacedArrival({ id: "207", content: "an aside on the main channel" });
  const placing = gate(standard);
  page.timeline = placing.respond;
  const answers = heldReplies(page);
  await replyQuietly(page, "207", "an answer to the aside");
  await page.el("channel-compose-text").setValue("a note after it");
  page.el("channel-send").click();
  await page.settle();
  placing.open();
  await page.settle();
  await page.settle();
  assert.deepEqual(postedWhere(page), [["an answer to the aside", null]],
    "a later message to the main channel overtook a reply placed there");

  answers[0]();
  await page.settle();
  await page.settle();
  assert.deepEqual(postedWhere(page), [["an answer to the aside", null], ["a note after it", null]]);
  answers[1]();
  await page.settle();
});

test("two replies to messages that came in live are placed by one read of the channel", async () => {
  // The read that places a reply folds in every message the stream left unplaced, and on a slow
  // chat service a second read alongside it is one more to time out.
  const { page, first, standard } = await unplacedArrival();
  const aside = message({ id: "207", content: "an aside on the main channel" });
  page.messages.push(aside);
  await deliver(page, page.stream(), sseMessage(aside));
  await page.settle();
  const placing = gate(standard);
  page.timeline = placing.respond;
  distinctReplies(page);
  const before = page.timelineCalls.length;
  await replyQuietly(page, "206", "an answer in the first discussion");
  await replyQuietly(page, "207", "an answer to the aside");
  placing.open();
  await page.settle();
  await page.settle();
  // Bound for different destinations, so neither waits for the other's delivery.
  assert.deepEqual(postedWhere(page).sort(), [
    ["an answer in the first discussion", first.id],
    ["an answer to the aside", null],
  ]);
  assert.equal(page.timelineCalls.length - before, 1, "each reply read the channel to place itself");
});

// `#277 send-order`. A reply that could not be placed posted nothing, but it gave up no place: Retry
// may still send it anywhere in its channel, so what was written after it there waits for it, until
// it is placed and sent, dismissed, or edited back into Reply. Before this, a main-channel note
// written after it went at once, and the reply, placed on the main channel by Retry, landed after
// the note.

test("`#277 send-order`: a reply that could not be placed holds what was written after it in its channel, and Retry lands it first", async () => {
  const { page, standard } = await unplacedArrival({ id: "207", content: "an aside on the main channel" });
  distinctReplies(page);
  await replyQuietly(page, "207", "A, answering the aside");
  await sendBehind(page, "B, a note after it");
  assert.deepEqual(postedWhere(page), [], "the note went while the reply written before it could still go first");
  const [reply, note] = outgoingRows(page);
  assert.equal(note.getAttribute("data-send-state"), "queued");
  assert.match(note.text(), new RegExp("Waiting to send… “A, answering the aside” goes first, " +
    `and goes again in ${RETRY_FIRST_MS / 1000} s\\.`), "the note did not say what it waits behind");
  assert.deepEqual(outgoingActions(note), ["Edit", "Dismiss"]);

  page.timeline = standard;
  await outgoingRetry(reply).click();
  for (let step = 0; step < 3; step += 1) await page.settle();
  assert.deepEqual(postedWhere(page), [
    ["A, answering the aside", null],
    ["B, a note after it", null],
  ], "the note landed before the reply written ahead of it");
  assert.equal(page.repliesPosted[0].body.reply_to, "207");
  assert.equal(outgoingRows(page).length, 0);
});

test("`#277 send-order`: a reply whose placing read failed for a moment is placed again on its own, and what waits behind it follows", async () => {
  const { page, first, standard } = await unplacedArrival();
  distinctReplies(page);
  await replyQuietly(page, "206", "an answer whose thread could not be read");
  await sendBehind(page, "a note to the main channel after it");
  assert.equal(outgoingRows(page)[0].getAttribute("data-send-state"), "retrying");
  const reads = page.timelineCalls.length;
  // The read fails again: the reply waits longer, and the note with it.
  assert.ok(page.expireTimers(retryWait(0)) >= 1, "the placing read was not asked again");
  await page.settle();
  assert.equal(page.timelineCalls.length, reads + 1, "the placing read was not asked again");
  assert.deepEqual(postedWhere(page), []);
  assert.match(outgoingRows(page)[1].text(), new RegExp(`goes again in ${retryWait(1) / 1000} s\\.`));

  page.timeline = standard;
  assert.ok(page.expireTimers(retryWait(1)) >= 1, "the second try was not armed");
  for (let step = 0; step < 3; step += 1) await page.settle();
  assert.deepEqual(postedWhere(page), [
    ["an answer whose thread could not be read", first.id],
    ["a note to the main channel after it", null],
  ]);
});

test("`#277 send-order`: a reply that cannot be placed at all holds its channel until it is dismissed, and then the next message goes", async () => {
  const { page, standard } = await unplacedArrival({ id: "207", content: "an aside on the main channel" });
  // The channel reads again, but no longer carries the message answered: an answer, not a moment.
  page.messages = page.messages.filter((m) => m.id !== "207");
  page.timeline = standard;
  distinctReplies(page);
  await replyQuietly(page, "207", "an answer to a lost message");
  await sendBehind(page, "a note written after it");
  const [reply, note] = outgoingRows(page);
  assert.equal(reply.getAttribute("data-send-state"), "failed");
  assert.match(reply.text(), /not among the channel's newest messages\. Retry once the channel can be read\./);
  assert.match(note.text(), new RegExp("Waiting to send… “an answer to a lost message” goes first, " +
    "and was not sent: which thread it goes in is not known yet\\. Retry, Edit or Dismiss it to let this one go\\."));
  for (const wait of [0, 1, 2, 3, 4, 5].map(retryWait)) page.expireTimers(wait);
  await page.settle();
  assert.deepEqual(postedWhere(page), [], "the reply was asked again on its own, or the note went past it");

  await outgoingDismiss(reply).click();
  for (let step = 0; step < 3; step += 1) await page.settle();
  assert.deepEqual(postedWhere(page), [["a note written after it", null]], "a Dismiss did not let the note go");
});

test("`#277 send-order`: after a reload a note still waits behind a reply that could not be placed, and goes once Retry has sent it", async () => {
  const { page } = await unplacedArrival({ id: "207", content: "an aside on the main channel" });
  await replyQuietly(page, "207", "an answer to the aside");
  await sendBehind(page, "a note after it");
  assert.equal(page.repliesPosted.length, 0);

  const again = reloadPage(page.storage, (p) => {
    p.threadingSupported = true;
    p.threads = page.threads;
    p.messages = page.messages;
  });
  await reopenedChannel(again);
  for (const wait of [0, 1, 2, 3].map(retryWait)) again.expireTimers(wait);
  await again.settle();
  assert.equal(again.repliesPosted.length, 0, "reopening the app sent a message on its own");
  const [reply, note] = outgoingRows(again);
  assert.equal(reply.getAttribute("data-send-state"), "failed");
  assert.equal(note.getAttribute("data-send-state"), "queued", "the reload let the note stop waiting behind the reply");
  assert.match(note.text(), /“an answer to the aside” goes first, and was not sent/);

  distinctReplies(again);
  await outgoingRetry(reply).click();
  for (let step = 0; step < 3; step += 1) await again.settle();
  assert.deepEqual(postedWhere(again), [["an answer to the aside", null], ["a note after it", null]]);
  assert.equal(again.repliesPosted[0].body.reply_to, "207");
});

test("a page closed while a reply was being placed says nothing was sent, not that delivery is unconfirmed", async () => {
  // As signing out does: only a POST that had started can have arrived. Neither the reply still
  // finding out where it goes nor the note waiting behind it had one. The note still waits behind
  // the reply (`#277 send-order`): Retry may yet send the reply to the main channel, ahead of it.
  const { page, standard } = await unplacedArrival();
  page.timeline = gate(standard).respond;
  await replyQuietly(page, "206", "an answer finding its thread");
  await page.el("channel-compose-text").setValue("a note waiting its turn");
  page.el("channel-send").click();
  await page.settle();
  assert.equal(page.repliesPosted.length, 0);

  const again = reloadWith(new Map(page.storage), []);
  await signIn(again);
  await reopenedChannel(again);
  assert.equal(again.repliesPosted.length, 0, "a reload posted on its own");
  const [reply, note] = outgoingRows(again);
  assert.deepEqual([reply, note].map((row) => row.getAttribute("data-send-state")), ["failed", "queued"]);
  assert.match(reply.text(), /closed before this message was sent/);
  assert.match(note.text(), /Waiting to send… “an answer finding its thread” goes first, and was not sent: which thread it goes in is not known yet\./);
  for (const row of [reply, note]) {
    assert.doesNotMatch(row.text(), /unconfirmed/i, "a message posted nowhere was called possibly delivered");
  }
});

test("a live message arriving before its send acknowledgement appears only once", async () => {
  const page = newPage();
  const stream = await withLiveChannel(page, []);
  const finish = delayedReply(page);
  await page.el("channel-compose-text").setValue("the live echo wins the race");
  const submitted = page.el("channel-send").click();
  await page.settle();
  const posted = message({ id: "9161", content: "the live echo wins the race" });
  await deliver(page, stream, sseMessage(posted));
  assert.equal(shownIds(page).filter((id) => id === "9161").length, 1);
  assert.equal(outgoingRows(page).length, 1,
    "matching text was mistaken for proof that this particular POST succeeded");
  finish(json(200, { posted }));
  await submitted;
  await page.settle();
  assert.equal(shownIds(page).filter((id) => id === "9161").length, 1,
    "the acknowledgement appended a second copy of the live message");
  assert.equal(outgoingRows(page).length, 0);
  await deliver(page, stream, sseMessage(posted));
  assert.equal(shownIds(page).filter((id) => id === "9161").length, 1,
    "a replayed live event added the sent message again");
});

test("two independent identical sends keep separate bubbles and POSTs, in submission order", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, []);
  const responses = [];
  page.replyResponse = () => new Promise((resolve) => responses.push(resolve));
  await page.el("channel-compose-text").setValue("same words");
  const first = page.el("channel-send").click();
  await page.settle();
  await page.el("channel-compose-text").setValue("same words");
  const second = page.el("channel-send").click();
  await page.settle();
  assert.equal(outgoingRows(page).length, 2, "text matching collapsed two intentional sends");
  assert.notEqual(outgoingRows(page)[0].getAttribute("data-outgoing-id"),
    outgoingRows(page)[1].getAttribute("data-outgoing-id"));
  assert.equal(page.repliesPosted.length, 1, "one destination's messages can race out of order");
  responses[0](json(200, { posted: message({ id: "9171", content: "same words" }) }));
  await page.settle();
  assert.equal(page.repliesPosted.length, 2);
  responses[1](json(200, { posted: message({ id: "9172", content: "same words" }) }));
  await Promise.all([first, second]);
  await page.settle();
  assert.deepEqual(page.repliesPosted.map((call) => sentWithoutKey(call.body)), [
    { text: "same words" }, { text: "same words" },
  ]);
  assert.equal(shownIds(page).filter((id) => id === "9171").length, 1);
  assert.equal(shownIds(page).filter((id) => id === "9172").length, 1);
  assert.equal(outgoingRows(page).length, 0);
});

test("confirmed messages survive an older history snapshot and reconcile by provider id after reload", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, []);
  const posted = message({ id: "9181", content: "confirmed before history caught up" });
  page.replyResponse = async () => json(200, { posted });
  await page.el("channel-compose-text").setValue(posted.content);
  await page.el("channel-send").click();
  await page.settle();
  assert.equal(shownIds(page).filter((id) => id === posted.id).length, 1);
  // The read side can lag the successful write. A refresh answering an older snapshot must not
  // erase the user's message or relabel its known success as an unconfirmed send.
  await reReadChannel(page);
  assert.match(page.el("pane-discord").text(), /confirmed before history caught up/);
  const again = reloadWith(new Map(page.storage), []);
  await signIn(again);
  await reopenedChannel(again);
  assert.equal(again.repliesPosted.length, 0);
  assert.match(again.el("pane-discord").text(), /confirmed before history caught up/);
  assert.equal(outgoingRows(again)[0].getAttribute("data-send-state"), "sent");
  assert.equal(outgoingSpinner(outgoingRows(again)[0]), undefined);
  again.messages = [posted];
  await reReadChannel(again);
  assert.equal(shownIds(again).filter((id) => id === posted.id).length, 1);
  assert.equal(outgoingRows(again).length, 0, "provider history and the local confirmation both remain visible");
});

test("a later permission change disables retry without losing the saved message", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, []);
  page.replyResponse = errorResponse(403, "refused", "read-only now");
  await page.el("channel-compose-text").setValue("keep until I can send");
  await page.el("channel-send").click();
  await page.settle();
  const again = reloadWith(new Map(page.storage), [], (p) => {
    p.channels = [{ ...CHANNEL, writable: false }];
  });
  await signIn(again);
  await reopenedChannel(again);
  const retry = outgoingRetry(outgoingRows(again)[0]);
  assert.equal(retry.disabled, true, "a read-only destination still offers to post the old message");
  // The fixture intentionally permits dispatch on a disabled button, proving the handler also
  // checks permission instead of relying on a visual attribute for its safety boundary.
  await retry.click();
  await again.settle();
  assert.equal(again.repliesPosted.length, 0);
  assert.equal(savedOutgoing(again)[0].remaining, "keep until I can send");
});

test("dismissing an unconfirmed local message removes it durably without posting", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, []);
  page.replyResponse = errorResponse(502, "chat_error", "Google Chat returned HTTP 502");
  await page.el("channel-compose-text").setValue("I verified this in the chat client");
  await page.el("channel-send").click();
  await page.settle();
  await outgoingDismiss(outgoingRows(page)[0]).click();
  assert.equal(outgoingRows(page).length, 0);
  assert.equal(savedOutgoing(page).length, 0);
  assert.equal(page.repliesPosted.length, 1);
  const again = reloadWith(new Map(page.storage), []);
  await signIn(again);
  await reopenedChannel(again);
  assert.equal(outgoingRows(again).length, 0);
  assert.equal(again.repliesPosted.length, 0);
});

const DAY_MS = 24 * 60 * 60 * 1000;

test("`#193 compose-draft-loss`: Edit puts a failed message back in the channel's box, under a draft typed since, and takes it out of the outbox", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, []);
  const failed = await failedSend(page, "the words that did not go");
  assert.equal(failed.getAttribute("data-send-state"), "failed");
  assert.deepEqual(outgoingActions(failed), ["Retry", "Edit", "Dismiss"], "a failed send's actions");
  const edit = outgoingEdit(failed);
  // Named by what it says: no label of its own to drift from the word on screen.
  assert.equal(edit.getAttribute("aria-label"), null);
  assert.equal(edit.disabled, false);

  // An empty box takes the text as it was.
  await edit.click();
  await page.settle();
  assert.equal(page.el("channel-compose-text").value, "the words that did not go");
  assert.equal(page.el("channel-compose-state").textContent, "");
  assert.equal(focused(page), page.el("channel-compose-text"), "the box was not handed the focus");
  assert.equal(outgoingRows(page).length, 0, "the edited send stayed in the outbox");
  assert.deepEqual(savedOutgoing(page), []);
  assert.equal(page.repliesPosted.length, 1, "Edit sent something");

  // A box with a draft in it keeps the draft, and the text goes under it.
  await page.el("channel-send").click();
  await page.settle();
  assert.equal(page.el("channel-compose-text").value, "");
  await page.el("channel-compose-text").setValue("a newer thought");
  await outgoingEdit(outgoingRows(page)[0]).click();
  await page.settle();
  const both = "a newer thought\n\nthe words that did not go";
  assert.equal(page.el("channel-compose-text").value, both, "Edit lost or overwrote the draft in the box");
  assert.match(page.el("channel-compose-state").textContent, /under the draft that was already here/);
  assert.equal(outgoingRows(page).length, 0);
  assert.equal(JSON.parse(page.storage.get("vibe-talk.channel-drafts"))[JSON.stringify([CHANNEL.id, null])], both,
    "the box's text is not kept as its draft");

  // ...and it is the box's draft after a reload, as anything typed there is.
  const again = reloadWith(new Map(page.storage), []);
  await signIn(again);
  await reopenedChannel(again);
  assert.equal(again.el("channel-compose-text").value, both);
  assert.equal(outgoingRows(again).length, 0);
  assert.equal(again.repliesPosted.length, 0);
});

test("`#193 compose-draft-loss`: a send that may already be in the channel offers no Edit, so taking it back can never post it twice", async () => {
  // The `#195 send-resilience` incident: a send timed out, was left unconfirmed, and had landed after
  // all. Its outbox id is the key that lets Retry go again without a second post; Edit would drop
  // that key, and the same words sent again from the box would be a new message.
  const keyed = await keyedChannel();
  const lost = await failedSend(keyed, "this may already be there", 502);
  const key = posted(keyed)[0].idempotency_key;
  assert.equal(lost.getAttribute("data-send-state"), "retrying");
  assert.deepEqual(outgoingActions(lost), ["Retry now", "Dismiss"], "Edit is offered on a send that may have landed");
  assert.equal(outgoingEdit(lost), undefined);
  // Its way out is Retry, as the same request.
  keyed.replyResponse = sentOk;
  await outgoingRetry(lost).click();
  await settleSend(keyed);
  assert.deepEqual(posted(keyed).map((body) => body.idempotency_key), [key, key], "the second attempt went under another key");
  assert.equal(outgoingRows(keyed).length, 0);

  // Where no provider promises one post per key, the same: the row says to check the channel, and
  // Edit would hide that warning in a box that knows nothing of it.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, []);
  const unconfirmed = await failedSend(page, "this may be there too", 502);
  assert.equal(unconfirmed.getAttribute("data-send-state"), "unconfirmed");
  assert.match(unconfirmed.text(), /Check history before retrying/);
  assert.deepEqual(outgoingActions(unconfirmed), ["Retry", "Dismiss"]);

  // Refused on its Retry, it is no less possibly in the channel from its first attempt: still no
  // Edit, and none after a reload.
  page.replyResponse = errorResponse(403, "refused", "You cannot post to this channel.");
  await outgoingRetry(unconfirmed).click();
  await settleSend(page);
  const refused = outgoingRows(page)[0];
  assert.equal(refused.getAttribute("data-send-state"), "failed");
  assert.deepEqual(outgoingActions(refused), ["Retry", "Dismiss"], "a refused retry made a possibly delivered send editable");
  assert.equal(savedOutgoing(page)[0].mayHavePosted, true);
  const again = reloadWith(new Map(page.storage), []);
  await signIn(again);
  await reopenedChannel(again);
  assert.deepEqual(outgoingActions(outgoingRows(again)[0]), ["Retry", "Dismiss"], "a reload made it editable");

  // A page closed while a POST was out comes back unconfirmed, without Edit; a refused send saved
  // before this flag existed is taken as the refusal it says it is.
  const saved = savedOutgoing(page)[0];
  const store = new Map(page.storage);
  store.set("vibe-talk.outgoing-messages", JSON.stringify([
    { ...saved, id: "outgoing-closed-1", state: "sending", phase: "posting", mayHavePosted: false, createdAt: 1 },
    { ...saved, id: "outgoing-older-2", state: "failed", mayHavePosted: undefined, createdAt: 2 },
  ]));
  const reopened = reloadWith(store, []);
  await signIn(reopened);
  await reopenedChannel(reopened);
  const [closed, older] = outgoingRows(reopened);
  assert.equal(closed.getAttribute("data-send-state"), "unconfirmed");
  assert.deepEqual(outgoingActions(closed), ["Retry", "Dismiss"]);
  assert.deepEqual(outgoingActions(older), ["Retry", "Edit", "Dismiss"]);
  assert.equal(reopened.repliesPosted.length, 0);
});

test("`#193 compose-draft-loss`: where the browser refuses the draft, Edit leaves the send in the outbox and says so", async () => {
  const refuseDrafts = (page) => {
    const set = page.storage.set.bind(page.storage);
    page.storage.set = (key, value) => {
      if (key === "vibe-talk.channel-drafts" || key === "vibe-talk.voice.drafts") throw new Error("QuotaExceededError");
      return set(key, value);
    };
  };
  const page = newPage();
  await signIn(page);
  await showDiscord(page, []);
  const failed = await failedSend(page, "words with nowhere else to go");
  const id = failed.getAttribute("data-outgoing-id");
  refuseDrafts(page);
  await page.el("channel-compose-text").setValue("a draft typed since");
  await outgoingEdit(failed).click();
  await page.settle();
  assert.equal(page.el("status").textContent, "This browser could not save a draft, so the message stays in the outbox.");
  assert.equal(page.el("channel-compose-text").value, "a draft typed since", "the box was changed although nothing moved");
  assert.equal(outgoingRows(page).length, 1, "the send left the outbox for a draft that was never saved");
  assert.equal(outgoingRows(page)[0].getAttribute("data-outgoing-id"), id);
  assert.deepEqual(savedOutgoing(page).map((entry) => entry.id), [id], "the only saved copy of the text was dropped");
  const again = reloadWith(new Map(page.storage), []);
  await signIn(again);
  await reopenedChannel(again);
  assert.match(outgoingRows(again)[0].text(), /words with nowhere else to go/, "a reload lost the text");

  // A reply, likewise: Reply does not open, and the send stays where it is.
  const replying = newPage();
  await signIn(replying);
  await openReplyOn(replying, [message({ id: "9320", content: "the question" })]);
  replying.replyResponse = errorResponse(403, "refused", "You cannot post to this channel.");
  await replying.el("reply-text").setValue("an answer with nowhere else to go");
  await replying.el("reply-send").click();
  await replying.settle();
  refuseDrafts(replying);
  await outgoingEdit(outgoingRows(replying)[0]).click();
  await replying.settle();
  assert.equal(replying.screen(), "main", "Reply opened on a draft that was never saved");
  assert.equal(replying.el("status").textContent, "This browser could not save a draft, so the message stays in the outbox.");
  assert.equal(outgoingRows(replying).length, 1);
  assert.equal(savedOutgoing(replying)[0].text, "an answer with nowhere else to go");
  assert.equal(draftMarkerOf(rowOf(replying, "9320")).hidden, true, "a draft that was never saved is marked");

  // A full quota is not a refusal: the saved channel rows give their space to the reply's draft.
  const full = newPage();
  await signIn(full);
  await openReplyOn(full, [message({ id: "9321", content: "another question" })]);
  full.replyResponse = errorResponse(403, "refused", "You cannot post to this channel.");
  await full.el("reply-text").setValue("an answer that needs room");
  await full.el("reply-send").click();
  await full.settle();
  assert.ok(full.storage.has(MESSAGE_CACHE_KEY));
  const set = full.storage.set.bind(full.storage);
  full.storage.set = (key, value) => {
    if (key === "vibe-talk.voice.drafts" && full.storage.has(MESSAGE_CACHE_KEY)) throw new Error("QuotaExceededError");
    return set(key, value);
  };
  await outgoingEdit(outgoingRows(full)[0]).click();
  await full.settle();
  assert.ok(replyInStream(full), "a full quota kept the reply from its draft");
  assert.equal(JSON.parse(full.storage.get("vibe-talk.voice.drafts"))["9321"], "an answer that needs room");
  assert.equal(outgoingRows(full).length, 0);
});

test("`#193 compose-draft-loss`: Edit on a thread's failed message puts it in that thread's box, opening the thread when it is pressed in All", async () => {
  const page = await threadPage();
  const thread = "spaces/A/threads/one & two";
  await pickThread(page, `thread:${thread}`);
  const failed = await failedSend(page, "an answer in the thread");
  assert.equal(page.repliesPosted[0].body.thread_id, thread);
  // In the thread, its own box: nothing moves.
  await outgoingEdit(failed).click();
  await page.settle();
  assert.equal(page.el("thread-select").value, `thread:${thread}`);
  assert.equal(page.el("channel-compose-text").value, "an answer in the thread");

  // Sent again and refused again, and found from All, whose box is the main channel's.
  await page.el("channel-send").click();
  await page.settle();
  await pickThread(page, "flat");
  assert.equal(page.el("channel-compose-text").value, "", "All's box holds the thread's text");
  const shown = outgoingRows(page)[0];
  assert.ok(shown, "All does not show the thread's failed send");
  await outgoingEdit(shown).click();
  await page.settle();
  assert.equal(page.el("thread-select").value, `thread:${thread}`, "the thread was not opened for its message");
  assert.equal(page.el("channel-compose-text").value, "an answer in the thread");
  assert.equal(focused(page), page.el("channel-compose-text"));
  assert.equal(outgoingRows(page).length, 0);
  await pickThread(page, "flat");
  assert.equal(page.el("channel-compose-text").value, "", "the thread's text was put in All's box as well");
});

test("`#193 compose-draft-loss`: in two columns, Edit on the right column's failed send puts it back in the right column's box", async () => {
  const { page } = await twoColumnServer();
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  const failed = await failedSend(page, "from the right column", 403, "side-compose-text", "side-send");
  assert.ok(failed, "the right column shows no failed send");
  assert.equal(page.el("channel-compose-text").value, "");
  await outgoingEdit(failed).click();
  await page.settle();
  assert.equal(page.el("side-compose-text").value, "from the right column");
  assert.equal(page.el("channel-compose-text").value, "", "the left column's box took the right column's text");
  assert.equal(focused(page), page.el("side-compose-text"));
  assert.equal(page.el("side-outgoing-log").children.length, 0);
});

test("`#193 compose-draft-loss`: Edit on a failed reply opens Reply on the message it answers, under the draft started since, and sends to that message", async () => {
  const page = newPage();
  await signIn(page);
  const messages = [message({ id: "9310", content: "the question" }), message({ id: "9311", content: "something else" })];
  await openReplyOn(page, messages);
  page.replyResponse = errorResponse(403, "refused", "You cannot post to this channel.");
  await page.el("reply-text").setValue("my first answer");
  await page.el("reply-send").click();
  await page.settle();
  assert.equal(page.screen(), "main");
  assert.equal(outgoingEdit(outgoingRows(page)[0]).getAttribute("title"),
    "Put this text back in Reply on the message it answers, and take it out of the outbox.");
  // A second thought on the same question, left as a draft.
  await openReplyOn(page, messages);
  await page.el("reply-text").setValue("and a second thought");
  await page.el("reply-cancel").click();

  await outgoingEdit(outgoingRows(page)[0]).click();
  await page.settle();
  assert.ok(replyInStream(page), "Edit did not open Reply");
  assert.match(page.el("reply-target-meta").textContent, /id 9310/, "Reply opened on another message");
  const both = "and a second thought\n\nmy first answer";
  assert.equal(page.el("reply-text").value, both, "Edit lost or overwrote the reply draft");
  assert.match(page.el("reply-state").textContent, /under the draft that was already here/);
  assert.equal(focused(page), page.el("reply-text"));
  assert.equal(outgoingRows(page).length, 0);
  assert.equal(JSON.parse(page.storage.get("vibe-talk.voice.drafts"))["9310"], both);

  distinctReplies(page);
  await page.el("reply-send").click();
  await page.settle();
  assert.deepEqual(sentWithoutKey(page.repliesPosted.at(-1).body), { text: both, reply_to: "9310" });
});

test("`#193 compose-draft-loss`: a failed reply that started a new thread comes back to Reply with that choice ticked", async () => {
  const page = newPage();
  page.threadingSupported = true;
  const data = threadData();
  page.threads = data.threads;
  const lone = { id: "spaces/A/threads/lone", root_message_id: "210", is_root: true,
    reply_count: 0, reply_count_exact: true };
  await signIn(page);
  await showDiscord(page, [...data.messages, message({ id: "210", content: "nobody has answered this", thread: lone })]);
  await replyButton(rowOf(page, "210")).click();
  page.el("reply-branch").checked = true;
  await page.el("reply-branch").dispatch("change");
  page.replyResponse = errorResponse(403, "refused", "You cannot post to this channel.");
  await page.el("reply-text").setValue("starting a thread on it");
  await page.el("reply-send").click();
  await page.settle();
  assert.equal(savedOutgoing(page)[0].threadId, lone.id);

  await outgoingEdit(outgoingRows(page)[0]).click();
  await page.settle();
  assert.ok(replyInStream(page));
  assert.equal(page.el("reply-branch").checked, true, "the new thread it was sent to start was forgotten");
  assert.match(page.el("reply-destination").textContent, /new thread/);
  distinctReplies(page);
  await page.el("reply-send").click();
  await page.settle();
  assert.deepEqual(sentWithoutKey(page.repliesPosted.at(-1).body),
    { text: "starting a thread on it", reply_to: "210", thread_id: lone.id });
});

test("`#193 compose-draft-loss`: Dismiss is taken back by a chip that names the message, and the text comes back as it was", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, []);
  const failed = await failedSend(page, "keep this in case I want it after all");
  const id = failed.getAttribute("data-outgoing-id");
  const chip = page.el("undo-send-dismiss");
  assertMarkupContains("scroll-tools", "undo-send-dismiss");
  assert.equal(chip.hidden, true, "the chip is up before anything was dismissed");
  assert.match(outgoingDismiss(failed).getAttribute("title"), /Undo brings it back/);

  await outgoingDismiss(failed).click();
  assert.equal(outgoingRows(page).length, 0);
  assert.deepEqual(savedOutgoing(page), []);
  assert.equal(chip.hidden, false, "no way back from the Dismiss");
  // The chip's words are its name: the message it puts back, in its first few words.
  assert.equal(chip.textContent, "Undo dismiss: “keep this in case I want it…”");
  assert.equal(chip.getAttribute("aria-label"), null);
  assert.equal(page.el("status").textContent, "Dismissed. Undo it, or find it in Settings.");
  assert.equal(savedDismissed(page).length, 1, "the dismissed text is not kept while Undo is offered");

  await chip.click();
  await page.settle();
  assert.equal(chip.hidden, true, "the chip stayed after its Undo");
  const back = outgoingRows(page)[0];
  assert.ok(back, "Undo did not bring the message back");
  assert.equal(back.getAttribute("data-outgoing-id"), id, "Undo made a new send of it");
  assert.equal(back.getAttribute("data-send-state"), "failed");
  assert.match(back.text(), /keep this in case I want it after all/);
  assert.match(back.text(), /You cannot post to this channel/, "its reason was lost");
  assert.equal(savedOutgoing(page)[0].text, "keep this in case I want it after all");
  assert.deepEqual(savedDismissed(page), []);
  assert.equal(page.repliesPosted.length, 1, "Undo sent it");

  // Past the window the chip goes, and the text is still kept, in Settings.
  await outgoingDismiss(back).click();
  assert.ok(page.expireTimers(SEND_UNDO_MS) >= 1, "the chip was never timed to go");
  assert.equal(chip.hidden, true, "the chip outlived its window");
  assert.equal(savedDismissed(page).length, 1, "the window took the text with it");
  const item = dismissedItems(page)[0];
  assert.equal(page.el("dismissed-sends").hidden, false);
  assert.match(item.text(), /keep this in case I want it after all/);
  assert.match(item.text(), /dismissed just now/);
  await dismissedOpenerOf(item).click();
  assert.equal(page.el("dismissed-send-restore").getAttribute("aria-label"), "Put back “keep this in case I want it…”");
  await page.el("dismissed-send-close").click();
  assert.match(page.el("dismissed-sends-state").textContent, /kept on this device for 7 days/);

  // ...and after a reload, where no Undo is offered for a dismissal the page did not see.
  const again = reloadWith(new Map(page.storage), []);
  await signIn(again);
  await reopenedChannel(again);
  assert.equal(outgoingRows(again).length, 0);
  assert.equal(again.el("undo-send-dismiss").hidden, true);
  // Settings names where it was going by the channel's name, once the channels are known.
  await again.el("open-settings").click();
  assert.match(dismissedItems(again)[0].text(), /^lead team · dismissed just now/);
  await again.el("close-settings").click();
  await (await putBackOf(again, dismissedItems(again)[0])).click();
  await again.settle();
  assert.equal(outgoingRows(again)[0].getAttribute("data-outgoing-id"), id);
  assert.equal(outgoingRows(again)[0].getAttribute("data-send-state"), "failed");
  assert.equal(dismissedItems(again).length, 0);
  assert.equal(again.el("dismissed-sends").hidden, true);
  assert.match(again.el("dismissed-sends-state").textContent, /^Nothing dismissed in the last 7 days/);
  assert.equal(again.repliesPosted.length, 0, "putting it back sent it");
});

test("`#193 compose-draft-loss`: Undo puts back a send's wait to go again on its own; Put back later leaves it to Retry", async () => {
  const page = await keyedChannel();
  const lost = await failedSend(page, "going again by itself", 502);
  const key = posted(page)[0].idempotency_key;
  assert.match(lost.text(), retryingFirst());
  await outgoingDismiss(lost).click();
  assert.equal(page.expireTimers(RETRY_FIRST_MS), 0, "a dismissed send went again on its own");
  await page.el("undo-send-dismiss").click();
  await page.settle();
  const back = outgoingRows(page)[0];
  assert.equal(back.getAttribute("data-send-state"), "retrying", "Undo put it back without its wait");
  assert.match(back.text(), retryingFirst());
  page.replyResponse = sentOk;
  assert.equal(page.expireTimers(RETRY_FIRST_MS), 1, "the wait Undo put back never ran out");
  await settleSend(page);
  assert.deepEqual(posted(page).map((body) => body.idempotency_key), [key, key]);
  assert.equal(outgoingRows(page).length, 0);

  // Written offline, dismissed, and taken back: it waits for the connection again.
  const offline = newPage();
  await signIn(offline);
  await showDiscord(offline, []);
  await offline.setOnline(false);
  await sendChannel(offline, "written in a tunnel");
  await outgoingDismiss(outgoingRows(offline)[0]).click();
  await offline.el("undo-send-dismiss").click();
  assert.match(outgoingRows(offline)[0].text(), /Offline\. Will send when online\./);
  await offline.setOnline(true);
  await settleSend(offline);
  assert.deepEqual(posted(offline).map(sentWithoutKey), [{ text: "written in a tunnel" }]);
  assert.equal(outgoingRows(offline).length, 0);
  // ...and one whose connection came back while it was dismissed goes at once, not on an `online`
  // that has already fired.
  await offline.setOnline(false);
  await sendChannel(offline, "written in the next tunnel");
  await outgoingDismiss(outgoingRows(offline)[0]).click();
  await offline.setOnline(true);
  await settleSend(offline);
  assert.equal(offline.repliesPosted.length, 1, "a dismissed send went when the connection came back");
  await offline.el("undo-send-dismiss").click();
  assert.equal(offline.expireTimers(0), 1, "the send taken back was left waiting for a connection it has");
  await settleSend(offline);
  assert.deepEqual(sentWithoutKey(posted(offline)[1]), { text: "written in the next tunnel" });
  assert.equal(outgoingRows(offline).length, 0);

  // Put back once the chip has gone is a decision of the reader's for later: Retry, not a wait.
  const later = await keyedChannel();
  await outgoingDismiss(await failedSend(later, "left to the reader", 502)).click();
  later.expireTimers(SEND_UNDO_MS);
  await (await putBackOf(later, dismissedItems(later)[0])).click();
  await later.settle();
  const put = outgoingRows(later)[0];
  assert.equal(put.getAttribute("data-send-state"), "unconfirmed");
  assert.deepEqual(outgoingActions(put), ["Retry", "Dismiss"]);
  assert.equal(later.expireTimers(RETRY_FIRST_MS), 0, "Put back armed an automatic send");
  assert.equal(later.repliesPosted.length, 1);
});

test("`#193 compose-draft-loss`: Recently dismissed keeps the newest ten, for seven days, and forgets them when asked", async () => {
  assert.equal(sourceConstant("DISMISSED_SENDS_LIMIT"), 10);
  assert.equal(sourceConstant("DISMISSED_SENDS_DAYS"), 7);
  const page = newPage();
  await signIn(page);
  await showDiscord(page, []);
  for (let n = 1; n <= 12; n += 1) {
    const row = await failedSend(page, `dismissed message number ${n}`);
    await outgoingDismiss(row).click();
    page.setClock(page.clock() + 60000);
  }
  const texts = savedDismissed(page).map((held) => held.entry.text);
  assert.equal(texts.length, 10, "more than the newest ten were kept");
  assert.equal(texts[0], "dismissed message number 12", "the newest is not first");
  assert.equal(texts[9], "dismissed message number 3");
  assert.equal(dismissedItems(page).length, 10);
  assert.match(dismissedItems(page)[0].text(), /dismissed message number 12/);

  // Six days on, all ten are there after a reload; eight days on, none, on screen or on the device.
  const sixDays = reloadWith(new Map(page.storage), [], (p) => p.setClock(page.clock() + 6 * DAY_MS));
  await signIn(sixDays);
  assert.equal(dismissedItems(sixDays).length, 10, "kept for less than seven days");
  assert.match(dismissedItems(sixDays)[0].text(), /dismissed 6d ago/);
  const eightDays = reloadWith(new Map(page.storage), [], (p) => p.setClock(page.clock() + 8 * DAY_MS));
  await signIn(eightDays);
  assert.equal(dismissedItems(eightDays).length, 0, "kept for more than seven days");
  assert.equal(eightDays.storage.has("vibe-talk.dismissed-sends"), false, "expired text stayed on the device");
  assert.equal(eightDays.el("forget-dismissed-sends").hidden, true);

  // Forget these now, at once.
  assert.equal(sixDays.el("forget-dismissed-sends").hidden, false);
  await sixDays.el("forget-dismissed-sends").click();
  assert.equal(dismissedItems(sixDays).length, 0);
  assert.equal(sixDays.storage.has("vibe-talk.dismissed-sends"), false);
  assert.equal(sixDays.el("forget-dismissed-sends").hidden, true);
});

test("`#193 compose-draft-loss`: a row with a saved reply draft says Draft beside Reply, which opens the draft, until it is sent or emptied", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "9410", content: "answer me later" }),
    message({ id: "9411", content: "nothing to say to this" })]);
  const marker = (id) => draftMarkerOf(rowOf(page, id));
  assert.equal(marker("9410").hidden, true, "a row with no draft is marked");
  assert.equal(marker("9410").tagName, "button");
  assert.equal(marker("9410").getAttribute("aria-label"), "Draft reply");
  assert.equal(marker("9410").textContent, "Draft");
  // Directly before Reply, among the row's controls.
  const reply = replyButton(rowOf(page, "9410"));
  const siblings = reply.parentNode.children;
  assert.equal(siblings[siblings.indexOf(reply) - 1], marker("9410"), "Draft is not beside Reply");

  await reply.click();
  await page.el("reply-text").setValue("half an answer");
  await page.el("reply-cancel").click();
  assert.equal(marker("9410").hidden, false, "the row whose reply was left unsent is not marked");
  assert.equal(marker("9411").hidden, true, "another row was marked");

  await marker("9410").click();
  assert.ok(replyInStream(page), "Draft did not open Reply");
  assert.match(page.el("reply-target-meta").textContent, /id 9410/);
  assert.equal(page.el("reply-text").value, "half an answer");
  // Emptied, it goes.
  await page.el("reply-text").setValue("");
  await page.el("reply-cancel").click();
  assert.equal(marker("9410").hidden, true, "an emptied draft left its mark");

  // Written again and sent, it goes too.
  await replyButton(rowOf(page, "9410")).click();
  await page.el("reply-text").setValue("the whole answer");
  await page.el("reply-cancel").click();
  assert.equal(marker("9410").hidden, false);
  distinctReplies(page);
  await marker("9410").click();
  await page.el("reply-send").click();
  await page.settle();
  assert.equal(marker("9410").hidden, true, "a sent draft left its mark");
});

test("`#193 compose-draft-loss`: the Draft mark comes back with its draft after a reload, and after a sign-in the page was sent away to", async () => {
  const first = newPage();
  await signIn(first);
  const answered = message({ id: "1311", content: "answer this after signing in again" });
  await showDiscord(first, [answered]);
  await replyButton(rowOf(first, "1311")).click();
  await first.el("reply-text").setValue("what I had written");
  await first.el("reply-cancel").click();
  await first.setVisibility("hidden");

  const reloaded = reloadWith(new Map(first.storage), [answered]);
  await signIn(reloaded);
  await reopenedChannel(reloaded);
  assert.equal(draftMarkerOf(rowOf(reloaded, "1311")).hidden, false, "a reload lost the mark");

  // The token is refused at the reopen, and the reader signs in again.
  const revoked = errorResponse(401, "unauthorized", "token revoked");
  const page = reloadPage(new Map(first.storage), (p) => {
    p.onlineConfig = p.clientConfig;
    p.clientConfig = revoked;
    p.channelPage = revoked;
  });
  await page.settle();
  await page.settle();
  assert.equal(page.screen(), "signin");
  page.clientConfig = page.onlineConfig;
  page.channelPage = async () => json(200, { channel: CHANNEL, messages: [answered] });
  await signIn(page);
  await page.settle();
  assert.equal(page.tab(), "discord");
  const marker = draftMarkerOf(rowOf(page, "1311"));
  assert.equal(marker.hidden, false, "signing in again lost the mark");
  await marker.click();
  assert.ok(replyInStream(page));
  assert.equal(page.el("reply-text").value, "what I had written");
});

test("`#193 compose-draft-loss`: the Draft mark is on its message's row in All, Main, the thread, and both columns", async () => {
  const page = await threadPage();
  assert.equal(page.el("thread-select").value, "flat");
  await replyButton(rowOf(page, "201")).click();
  await page.el("reply-text").setValue("a reply to the first thread's root");
  await page.el("reply-cancel").click();
  assert.equal(draftMarkerOf(rowOf(page, "201")).hidden, false, "not marked in All");
  assert.equal(draftMarkerOf(rowOf(page, "200")).hidden, true);
  await pickThread(page, "main");
  assert.equal(draftMarkerOf(rowOf(page, "201")).hidden, false, "not marked in Main");
  await pickThread(page, "thread:spaces/A/threads/one & two");
  assert.equal(draftMarkerOf(rowOf(page, "201")).hidden, false, "not marked in its thread");
  assert.equal(draftMarkerOf(rowOf(page, "202")).hidden, true);
  await pickThread(page, "flat");
  assert.equal(draftMarkerOf(rowOf(page, "201")).hidden, false, "not marked in All once it was shown again");

  await twoColumns(page);
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  assert.equal(draftMarkerOf(sideRow(page, "201")).hidden, false, "not marked in the right column");
  assert.equal(draftMarkerOf(rowWithId(page, "201")).hidden, false, "not marked in the left column");
  // Sent from the right, it goes from both.
  distinctReplies(page);
  await draftMarkerOf(sideRow(page, "201")).click();
  assert.equal(page.el("reply-text").value, "a reply to the first thread's root");
  await page.el("reply-send").click();
  await page.settle();
  assert.equal(draftMarkerOf(sideRow(page, "201")).hidden, true, "the right column kept the mark of a sent draft");
  assert.equal(draftMarkerOf(rowWithId(page, "201")).hidden, true, "the left column kept the mark of a sent draft");
});

test("a send timeout stops spinning and a late acknowledgement cannot silently retry or rewrite it", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, []);
  const finish = delayedReply(page);
  await page.el("channel-compose-text").setValue("mobile connection stalled");
  const submitted = page.el("channel-send").click();
  await page.settle();
  assert.equal(page.expireTimers(sourceConstant("OUTGOING_TIMEOUT_MS")), 1);
  await submitted;
  await page.settle();
  assert.equal(outgoingRows(page)[0].getAttribute("data-send-state"), "unconfirmed");
  assert.equal(outgoingSpinner(outgoingRows(page)[0]), undefined);
  assert.match(outgoingRows(page)[0].text(), /timed out/i);
  finish(json(200, { posted: message({ id: "9201", content: "mobile connection stalled" }) }));
  await page.settle();
  assert.equal(page.repliesPosted.length, 1);
  assert.equal(outgoingRows(page)[0].getAttribute("data-send-state"), "unconfirmed");
  assert.equal(savedOutgoing(page)[0].remaining, "mobile connection stalled");
});

test("signing out interrupts a pending send and prevents its queued successor from posting", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, []);
  const finish = delayedReply(page);
  await page.el("channel-compose-text").setValue("already on the wire");
  const first = page.el("channel-send").click();
  await page.settle();
  await page.el("channel-compose-text").setValue("still waiting locally");
  const second = page.el("channel-send").click();
  await page.settle();
  assert.equal(page.repliesPosted.length, 1);
  assert.equal(outgoingRows(page).length, 2);
  await page.el("forget-token").click();
  await Promise.all([first, second]);
  await page.settle();
  assert.equal(page.screen(), "signin");
  assert.deepEqual(savedOutgoing(page).map((entry) => [entry.remaining, entry.state]), [
    ["already on the wire", "unconfirmed"], ["still waiting locally", "failed"],
  ]);
  finish(json(200, { posted: message({ id: "9211", content: "already on the wire" }) }));
  await page.settle();
  assert.equal(page.repliesPosted.length, 1, "the queued message used a signed-out credential");
  assert.deepEqual(savedOutgoing(page).map((entry) => entry.state), ["unconfirmed", "failed"]);
  await signIn(page);
  await page.settle();
  assert.equal(page.repliesPosted.length, 1, "signing back in replayed a saved send automatically");
});

test("a sent receipt retires when provider history arrives even if To do hides that own message", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog(2));
  const posted = message({ id: "9221", content: "already handled by me", author_id: "own-account" });
  page.replyResponse = async () => json(200, { posted });
  await page.el("channel-compose-text").setValue(posted.content);
  await page.el("channel-send").click();
  await page.settle();
  assert.equal(savedOutgoing(page).length, 1, "acknowledgement erased the durable fallback before a history read");
  page.messages.push(posted);
  assert.equal(page.el("mark-own-read").checked, true);
  await turnTodoOn(page);
  await page.settle();
  assert.equal(shownIds(page).includes(posted.id), false, "the premise requires the own message to be filtered");
  assert.equal(outgoingRows(page).length, 0, "a hidden provider message left a local receipt stuck on screen");
  assert.equal(savedOutgoing(page).length, 0, "filtered history never acknowledged the durable send record");
});

test("an older in-flight poll cannot erase a sent message, and only a read sent after it retires its receipt", async () => {
  const page = newPage();
  const stream = await withLiveChannel(page, []);
  const finishPost = delayedReply(page);
  await page.el("channel-compose-text").setValue("confirmed while the old read waited");
  const submitted = page.el("channel-send").click();
  await page.settle();
  const standardPage = page.channelPage;
  let finishOldPage;
  let holdFirst = true;
  page.channelPage = (path, options) => {
    if (holdFirst) {
      holdFirst = false;
      return new Promise((resolve) => { finishOldPage = resolve; });
    }
    return standardPage(path, options);
  };
  page.expireTimers(DISCORD_POLL_MS);
  await page.settle();
  assert.ok(finishOldPage, "the old history read must already be in flight");
  const posted = message({ id: "9231", content: "confirmed while the old read waited" });
  finishPost(json(200, { posted }));
  await submitted;
  page.messages = [posted];
  await deliver(page, stream, sseMessage(posted));
  assert.match(page.el("pane-discord").text(), /confirmed while the old read waited/);
  // `#466 reply-promotes-thread`. The stream's copy does not retire the receipt: the read it asks
  // for may be answered from older state. Held, it is not drawn while the message itself is.
  assert.equal(savedOutgoing(page).length, 1, "the stream's copy retired the receipt");
  assert.equal(outgoingRows(page).length, 0, "the receipt was drawn beside the message it stands for");
  // This response describes the instant before the send. Its late arrival must not erase the
  // acknowledged row, nor the receipt kept for it.
  finishOldPage(json(200, { channel: CHANNEL, messages: [], has_more: false, complete: true }));
  await page.settle();
  assert.match(page.el("pane-discord").text(), /confirmed while the old read waited/,
    "an older in-flight page erased the message after its live confirmation");
  assert.equal(shownIds(page).filter((id) => id === posted.id).length, 1);
  // The read the stream's copy asked for, sent after the post and carrying it, has retired it.
  assert.equal(savedOutgoing(page).length, 0, "no read carrying the message retired its receipt");
});

// --- how many replies a message has, and the way down to them ----------------------------------
//
// `#228 reply-visible`. The owner: "I would like the button to change to say Reply (1) Reply (2) if
// there are already replies so that it is clear that it is already replied to." And: "a blue circle
// about the size of our arrow head centered on the lower left corner of the message box ... If we
// click it we should jump to the child, just as clicking the arrow jumps to the parent." The circle's
// geometry — on the corner, over the borders, clear of the text and the buttons, and Reply (2)
// leaving the buttons on one line at 412px — is measured in Chromium by the screenshot states
// `42-reply-count-and-marker` and `43-reply-marker-jumped` (scripts/screenshots.py).

const replyLabel = (li) => replyButton(li).textContent;
const replyMarkerOf = (li) => li.children.find((node) => node.hasClass("reply-marker"));

/** A tap on a row's circle as a browser delivers it: to the circle, then — unless stopped — to the row. */
async function tapMarker(page, li) {
  const marker = replyMarkerOf(li);
  assert.ok(marker, "the row has no circle to tap");
  let stopped = false;
  const event = { target: marker, stopPropagation: () => { stopped = true; } };
  await marker.dispatch("click", event);
  if (!stopped) await li.dispatch("click", event);
  for (let i = 0; i < 4 * REPLY_JUMP_PAGES; i += 1) await page.settle();
}

/**
 * A question, the reply directly under it, a screenful of other messages, and two more replies to the
 * question at the foot: one from a third party, one from the owner.
 */
function answeredThreeTimes() {
  return [
    message({ id: "1000000000000000001", content: longMessage("is the runner wedged?") }),
    message({ id: "1000000000000000002", content: "which runner?", reply_to: "1000000000000000001", ...ALICE }),
    ...Array.from({ length: 6 }, (_unused, i) =>
      message({ id: `10000000000000000${10 + i}`, content: longMessage(`filler ${i}`), ...(i % 2 ? CODER : {}) })),
    message({ id: "1000000000000000098", content: "restarted it", reply_to: "1000000000000000001", ...ALICE }),
    message({ id: "1000000000000000099", content: "and it is green now", reply_to: "1000000000000000001", ...OWNER }),
  ];
}

test("REPLY SAYS HOW MANY REPLIES A MESSAGE HAS: Reply, Reply (1), Reply (2), and in words to a screen reader", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [
    message({ id: "1000000000000000001", content: "is the runner wedged?" }),
    message({ id: "1000000000000000002", content: "which runner?", reply_to: "1000000000000000001", ...ALICE }),
    message({ id: "1000000000000000003", content: "nobody has answered this one", ...OWNER }),
    message({ id: "1000000000000000004", content: "runner four", reply_to: "1000000000000000001", ...CODER }),
    message({ id: "1000000000000000005", content: "thanks", reply_to: "1000000000000000002", ...ALICE }),
  ]);
  const button = (id) => replyButton(rowWithId(page, id));
  assert.equal(button("1000000000000000001").textContent, "Reply (2)", "a message two replies answer does not say so");
  assert.equal(button("1000000000000000001").getAttribute("aria-label"), "Reply, 2 replies already");
  assert.equal(button("1000000000000000001").getAttribute("title"), "Reply to this message. It has 2 replies already.");
  // A reply can be answered too, and is counted as any message is.
  assert.equal(button("1000000000000000002").textContent, "Reply (1)");
  assert.equal(button("1000000000000000002").getAttribute("aria-label"), "Reply, 1 reply already");
  // Nobody has answered it: plain, and named by its words, as it always was.
  for (const id of ["1000000000000000003", "1000000000000000004", "1000000000000000005"]) {
    assert.equal(button(id).textContent, "Reply", `${id} says it has replies it has not`);
    assert.equal(button(id).getAttribute("aria-label"), null, `${id} is named for replies it has not`);
    assert.equal(button(id).getAttribute("title"), "Reply to this message");
  }
  // Short enough to leave the row's buttons on one line: four characters, at most five.
  assert.ok(SCRIPT_CODE.includes("`Reply (${count})`"), "the count is not drawn as Reply (N)");
});

test("...a combined row counts the replies to any message it stands for, once each", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [
    ...splitPost(),
    message({ id: "1000000000000000003", content: "which runner?", reply_to: "1000000000000000002",
      timestamp: "2026-08-19T04:40:00.000Z", ...ALICE }),
    message({ id: "1000000000000000004", content: "and since when?", reply_to: "1000000000000000001",
      timestamp: "2026-08-19T04:41:00.000Z", ...OWNER }),
  ]);
  assert.equal(page.el("discord-log").children.length, 3, "the split post was not drawn as one row");
  assert.equal(replyLabel(row(page, 0)), "Reply (2)", "a reply to the second half of a combined row was not counted");
  // ...but not the row answering itself: its second half naming its first is one message to the reader.
  const self = newPage();
  await signIn(self);
  await showDiscord(self, splitPost({}, { reply_to: "1000000000000000001" }));
  assert.equal(self.el("discord-log").children.length, 1, "the split post was not drawn as one row");
  assert.equal(replyLabel(row(self, 0)), "Reply", "a combined row counted its own half as a reply to it");
  assert.equal(replyMarkerOf(row(self, 0)), undefined);
});

test("...and follows the channel: a reply arriving on the stream adds one, and one deleted upstream takes it away", async () => {
  const page = newPage();
  const question = message({ id: "100", content: "is the runner wedged?" });
  const stream = await withLiveChannel(page, [question]);
  assert.equal(replyLabel(rowWithId(page, "100")), "Reply");

  const answer = message({ id: "200", content: "restarted it", reply_to: "100", ...ALICE });
  page.messages = [question, answer];
  await deliver(page, stream, sseMessage(answer));
  assert.equal(replyLabel(rowWithId(page, "100")), "Reply (1)", "a reply that arrived live was not counted");

  page.messages = [question];
  await deliver(page, stream, sseDelete("delete-200", CHANNEL.id, "200"));
  await page.settle();
  assert.equal(rowWithId(page, "200"), undefined, "the deleted reply is still drawn");
  assert.equal(replyLabel(rowWithId(page, "100")), "Reply", "a reply deleted upstream is still counted");
});

test("THE OWNER'S OWN REPLY COUNTS from the moment Send is pressed, and once when the server answers", async () => {
  const page = newPage();
  await signIn(page);
  let release = null;
  const held = new Promise((resolve) => { release = resolve; });
  const answer = page.replyResponse;
  page.replyResponse = async (path, options) => {
    await held;
    return answer(path, options);
  };
  await openReplyOn(page, conversation(), 2);
  page.el("reply-text").value = "on it";
  const sending = page.el("reply-send").click();
  await page.settle();
  assert.equal(outgoingRows(page).length, 1, "the send is not in the outbox, so this proves nothing");
  const target = () => rowWithId(page, "1000000000000000003");
  assert.equal(replyLabel(target()), "Reply (1)", "the reply on its way was not counted");
  // The outbox's copy is not a message in the list: no circle leads to it.
  assert.equal(replyMarkerOf(target()), undefined, "a circle leads to a reply nothing has posted yet");

  release();
  await sending;
  await page.settle();
  assert.ok(rowWithId(page, "9000000000000000001"), "the acknowledged reply was not drawn");
  assert.equal(replyLabel(target()), "Reply (1)", "the reply was counted twice once the server answered");
});

test("...and once when the stream brings the reply back before the server has answered the send", async () => {
  const page = newPage();
  const question = message({ id: "100", content: "is the runner wedged?" });
  const stream = await withLiveChannel(page, [question]);
  let release = null;
  const held = new Promise((resolve) => { release = resolve; });
  const answer = page.replyResponse;
  page.replyResponse = async (path, options) => {
    await held;
    return answer(path, options);
  };
  await openReplyOn(page, [question], 0);
  page.el("reply-text").value = "restarted it";
  const sending = page.el("reply-send").click();
  await page.settle();
  assert.equal(replyLabel(rowWithId(page, "100")), "Reply (1)");
  // The provider's own copy of the post, on the stream, ahead of the POST's answer.
  const echo = message({ id: "9000000000000000001", content: "restarted it", reply_to: "100", ...OWN });
  page.messages = [question, echo];
  await deliver(page, stream, sseMessage(echo, { self_posted: true }));
  assert.ok(rowWithId(page, "9000000000000000001"), "the stream's copy was not drawn, so this proves nothing");
  assert.equal(replyLabel(rowWithId(page, "100")), "Reply (1)", "the reply was counted from the outbox and the stream both");
  release();
  await sending;
  await page.settle();
  assert.equal(replyLabel(rowWithId(page, "100")), "Reply (1)");
});

test("...not once it has failed, and still while it is waiting to go again on its own", async () => {
  // Refused: nothing was posted.
  const failed = newPage();
  await signIn(failed);
  failed.replyResponse = errorResponse(400, "discord_error", "Discord refused the message");
  await openReplyOn(failed, conversation(), 2);
  failed.el("reply-text").value = "on it";
  await failed.el("reply-send").click();
  await failed.settle();
  assert.match(outgoingRows(failed)[0].text(), /Not sent/);
  assert.equal(replyLabel(rowWithId(failed, "1000000000000000003")), "Reply", "a send that posted nothing was counted");

  // Its answer lost, and nothing sending it again: a reply only once a read brings it.
  const lost = newPage();
  await signIn(lost);
  lost.replyResponse = errorResponse(502, "discord_error", "nope");
  await openReplyOn(lost, conversation(), 2);
  lost.el("reply-text").value = "on it";
  await lost.el("reply-send").click();
  await lost.settle();
  assert.match(outgoingRows(lost)[0].text(), /Delivery unconfirmed/);
  assert.equal(replyLabel(rowWithId(lost, "1000000000000000003")), "Reply", "a send nobody knows arrived was counted");

  const retrying = newPage();
  retrying.idempotentPostsSupported = true;
  await signIn(retrying);
  retrying.replyResponse = errorResponse(502, "chat_error", SLOW_SEND);
  await openReplyOn(retrying, conversation(), 2);
  retrying.el("reply-text").value = "on it";
  await retrying.el("reply-send").click();
  await retrying.settle();
  assert.match(outgoingRows(retrying)[0].text(), /Retrying in/, "no automatic retry was armed, so this proves nothing");
  assert.equal(replyLabel(rowWithId(retrying, "1000000000000000003")), "Reply (1)",
    "a reply the page is still sending stopped counting between attempts");
  // Dismissed, it is gone from the count.
  await outgoingRows(retrying)[0].descendants().find((node) => node.className === "outgoing-dismiss").click();
  await retrying.settle();
  assert.equal(replyLabel(rowWithId(retrying, "1000000000000000003")), "Reply", "a dismissed send is still counted");
});

test("...nor where Reply posts into the message's thread: the thread's chips count those, and Reply stays Reply", async () => {
  // A provider whose only reply is a thread reply. None of its messages names another, and its post
  // comes back naming none either; Thread(N) and N replies already say how many there are.
  const page = await threadPage();
  const root = () => rowWithId(page, "201");
  assert.equal(threadBadge(root()).textContent, "Thread(1)");
  assert.equal(replyLabel(root()), "Reply", "a thread's replies were counted on Reply as well as on its chips");
  let release = null;
  const held = new Promise((resolve) => { release = resolve; });
  page.replyResponse = async (_path, options) => {
    await held;
    return json(200, { posted: message({ id: "9000000000000000009", content: JSON.parse(options.body).text,
      thread: { id: "spaces/A/threads/one & two", root_message_id: "201", is_root: false, reply_count: 2,
        reply_count_exact: true } }) });
  };
  await replyButton(root()).click();
  page.el("reply-text").value = "on it";
  const sending = page.el("reply-send").click();
  await page.settle();
  assert.equal(replyLabel(root()), "Reply", "a send that will carry no pointer was counted while it was on its way");
  release();
  await sending;
  await page.settle();
  assert.equal(replyLabel(root()), "Reply", "a thread reply was counted on Reply");
});

test("THE CIRCLE is on a message a reply further down answers: a button, named for where it goes, and last in the row", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, answeredThreeTimes());
  const question = rowWithId(page, "1000000000000000001");
  const marker = replyMarkerOf(question);
  assert.ok(marker, "the message answered from far below has no circle");
  assert.equal(marker.tagName, "button");
  assert.equal(marker.getAttribute("type"), "button");
  assert.equal(question.getAttribute("data-reply-marker"), "true");
  // After the row's own controls, so the keyboard reaches it last — and drawn by the stylesheet.
  assert.equal(question.children.at(-1), marker, "the circle is not the last thing in the row");
  assert.equal(marker.text(), "", "the circle is a typed glyph");
  // The first tap goes to the earliest reply the arrow does not already join to it: the second of three.
  assert.equal(marker.getAttribute("aria-label"), "Jump to reply 2 of 3 to this message");
  assert.equal(marker.getAttribute("title"), "Jump to reply 2 of 3 to this message. Each tap goes on to the next.");
  assert.equal(replyLabel(question), "Reply (3)");
  // No other row has one: nothing answers them.
  assert.deepStrictEqual(page.el("discord-log").children.filter((li) => replyMarkerOf(li)).map((li) => li.getAttribute("data-id")),
    ["1000000000000000001"]);
});

test("...but not on a message answered only from directly below: the arrow that rises from the reply meets it there", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, conversation());
  assert.equal(arrowStyle(row(page, 1)), "adjacent");
  assert.equal(replyLabel(row(page, 0)), "Reply (1)", "the reply directly under it is not counted");
  assert.equal(replyMarkerOf(row(page, 0)), undefined, "two marks on the one foot for one connection");
  assert.equal(row(page, 0).getAttribute("data-reply-marker"), null);
  // ...and with a row between, the arrow leaves it, and the circle is its other end.
  const later = newPage();
  await signIn(later);
  await showDiscord(later, [
    message({ id: "1000000000000000001", content: "is the runner wedged?" }),
    message({ id: "1000000000000000005", content: "in between" }),
    message({ id: "1000000000000000002", content: "the runner restarted", reply_to: "1000000000000000001", ...ALICE }),
  ]);
  const question = () => rowWithId(later, "1000000000000000001");
  assert.equal(arrowStyle(rowWithId(later, "1000000000000000002")), "disconnected");
  assert.ok(replyMarkerOf(question()), "the other end of a disconnected arrow has no circle");
  // A search that hides the row between joins them again, and the circle goes with it...
  await search(later, "runner");
  assert.ok(rowWithId(later, "1000000000000000005").hasClass("search-hidden"));
  assert.equal(arrowStyle(rowWithId(later, "1000000000000000002")), "adjacent");
  assert.equal(replyMarkerOf(question()), undefined,
    "the circle stayed when the row between was filtered out and the arrow met the message");
  // ...and comes back when the search goes.
  await later.el("search-toggle").click();
  assert.ok(replyMarkerOf(question()), "closing the search left the circle off");
});

test("A CIRCLE AND AN ARROW AT ONE CORNER step aside for each other: the head raised up the side, or the circle lifted", async () => {
  // Answered from directly below and from further away: the reply below's arrow and the circle both
  // end at the question's lower-left corner. Boxes placed by hand, as for the arrows' own measuring;
  // tests/offline_cache_browser.py measures the real ones in Chromium.
  const page = newPage(new Map(), SCRIPT, (p) => p.enableResizeObserver());
  await signIn(page);
  await showDiscord(page, answeredThreeTimes());
  const question = () => rowWithId(page, "1000000000000000001");
  const below = () => rowWithId(page, "1000000000000000002");
  assert.ok(replyMarkerOf(question()), "the question has no circle, so this proves nothing");
  assert.equal(arrowStyle(below()), "adjacent");
  assert.equal(replyArrowOf(below()).getAttribute("data-reply-hub"), "true", "the arrow does not know it shares a corner");
  // No other arrow steps aside: nothing else meets a row with a circle.
  for (const id of ["1000000000000000098", "1000000000000000099"]) {
    assert.equal(replyArrowOf(rowWithId(page, id)).getAttribute("data-reply-hub"), null, `${id}'s arrow stepped aside`);
  }
  const list = page.el("discord-log");
  list.clientWidth = 380;
  const place = (li, left) => {
    li.getBoundingClientRect = () => ({ ...FakeElement.prototype.getBoundingClientRect.call(li), left, right: left + 300 });
  };
  const measured = (parentLeft) => {
    place(question(), parentLeft);
    place(below(), 44);
    page.resized(list);
    page.expireTimers(0);
    return [replyArrowOf(below()).getAttribute("data-reply-reach"), question().getAttribute("data-reply-lift")];
  };
  // A row spanning the gutter: the head comes up 2.3rem along its foot, clear of the circle.
  assert.deepStrictEqual(measured(-12), ["below", null], "the circle was lifted from a head nowhere near it");
  // The owner's inset row: the head comes up as near its corner as the shaft may go, and the circle
  // makes way, up the row's side.
  assert.deepStrictEqual(measured(22), ["below", "true"], "the circle was left on a head at its corner");
  // A row starting past the reply's box: the head meets its side, raised by the stylesheet, and the
  // circle stays on the corner.
  assert.deepStrictEqual(measured(96), ["side", null], "the circle was lifted for a head that meets the row's side");
  measured(22);
  assert.equal(question().getAttribute("data-reply-lift"), "true");
  // A search that hides the reply below leaves nothing at the corner but the circle: it goes back on it.
  await search(page, "wedged");
  assert.ok(below().hasClass("search-hidden"), "the search did not hide the reply below");
  assert.equal(replyArrowOf(below()).getAttribute("data-reply-hub"), null, "a hidden reply's arrow still steps aside");
  assert.equal(question().getAttribute("data-reply-lift"), null, "the circle stayed lifted for a head nobody can see");
  await page.el("search-toggle").click();
  assert.equal(replyArrowOf(below()).getAttribute("data-reply-hub"), "true");
  // The far replies gone, the circle goes, and so does all its stepping aside.
  page.messages = answeredThreeTimes().slice(0, 8);
  await reReadChannel(page);
  assert.equal(replyMarkerOf(question()), undefined, "the circle outlived the replies it led to");
  assert.equal(replyArrowOf(below()).getAttribute("data-reply-hub"), null, "the arrow still steps aside for no circle");
  assert.equal(question().getAttribute("data-reply-lift"), null, "the row is still lifted for no circle");
  // The stylesheet's half.
  assert.match(cssBlock('#discord-log .reply-jump[data-reply-reach="side"][data-reply-hub] .reply-jump-mark::after'),
    /top:\s*-1\.4rem/, "a head that meets the row's side is not raised clear of the circle");
  assert.match(cssBlock('#discord-log .reply-jump[data-reply-reach="side"] .reply-jump-mark::after'), /top:\s*-0\.9rem/);
  assert.match(cssBlock("#discord-log li.discord-message[data-reply-lift] > .reply-marker"),
    /bottom:\s*calc\(-1px - var\(--reply-marker-target\) \/ 2 \+ 0\.7rem\)/, "a lifted circle is not lifted");
  assert.ok(/^const REPLY_HUB_CLEAR_REM = 1\.1;$/m.test(SCRIPT_CODE), "the head's clearance from the circle has changed");
});

test("TAPPING THE CIRCLE lands on the reply, lit, the way the arrow lands on what it answers", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [
    message({ id: "1000000000000000001", content: longMessage("is the runner wedged?") }),
    ...Array.from({ length: 8 }, (_unused, i) =>
      message({ id: `10000000000000000${10 + i}`, content: longMessage(`filler ${i}`), ...(i % 2 ? CODER : {}) })),
    message({ id: "1000000000000000099", content: longMessage("restarted it"), reply_to: "1000000000000000001", ...ALICE }),
    // Enough after it that the reply can come to the head of the list.
    ...Array.from({ length: 4 }, (_unused, i) =>
      message({ id: `10000000000000001${10 + i}`, content: longMessage(`later ${i}`), ...(i % 2 ? {} : CODER) })),
  ]);
  const area = page.el("scroll-area");
  const question = rowWithId(page, "1000000000000000001");
  const reply = rowWithId(page, "1000000000000000099");
  area.scrollTop = 0;
  await area.dispatch("scroll");
  assert.ok(reply.getBoundingClientRect().top > area.clientHeight, "the reply was already on screen");
  assert.equal(replyMarkerOf(question).getAttribute("aria-label"), "Jump to the reply to this message");
  assert.equal(replyMarkerOf(question).getAttribute("title"), "Jump to the reply to this message");

  await tapMarker(page, question);
  const top = reply.getBoundingClientRect().top;
  assert.ok(top >= 0 && top <= 3 * LINE_PX, `the reply landed at ${top}px, not at the head of the list`);
  assert.equal(reply.getAttribute("data-landed"), "true", "nothing says which row the jump was for");
  assert.equal(statusText(page), "Here is the reply to it.");
  assert.equal(page.expireTimers(REPLY_LANDED_MS), 1, "the light was never timed to go out");
  assert.equal(reply.getAttribute("data-landed"), null);
  // ...and the reply's own arrow is the way back up.
  await tapArrow(page, reply);
  assert.equal(question.getAttribute("data-landed"), "true");
});

test("...with several, each tap goes on to the next in time order, round to the first", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, answeredThreeTimes());
  const question = () => rowWithId(page, "1000000000000000001");
  const landed = () => page.el("discord-log").children.filter((li) => li.getAttribute("data-landed") === "true")
    .map((li) => li.getAttribute("data-id"));
  // The reply already joined to it from below is not where the first tap goes: it is in view.
  await tapMarker(page, question());
  assert.deepStrictEqual(landed(), ["1000000000000000098"]);
  assert.equal(statusText(page), "Here is reply 2 of 3 to it.");
  assert.equal(replyMarkerOf(question()).getAttribute("aria-label"), "Jump to reply 3 of 3 to this message");
  await tapMarker(page, question());
  assert.deepStrictEqual(landed(), ["1000000000000000099"]);
  assert.equal(statusText(page), "Here is reply 3 of 3 to it.");
  await tapMarker(page, question());
  assert.deepStrictEqual(landed(), ["1000000000000000002"], "the taps did not go round to the first");
  assert.equal(statusText(page), "Here is reply 1 of 3 to it.");
  // A redraw does not start the round again.
  await reReadChannel(page);
  assert.equal(replyMarkerOf(question()).getAttribute("aria-label"), "Jump to reply 2 of 3 to this message");
  await tapMarker(page, question());
  assert.deepStrictEqual(landed(), ["1000000000000000098"]);
});

test("...and the tap is the circle's: it neither folds the row nor reads it, nor starts a swipe or a hold", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, answeredThreeTimes());
  const question = rowWithId(page, "1000000000000000001");
  assert.equal(question.getAttribute("data-collapsed"), "true", "the question is not foldable, so this proves nothing");
  await tapMarker(page, question);
  assert.equal(question.getAttribute("data-collapsed"), "true", "tapping the circle also opened the row");
  await readButton(page).click();
  await page.settle();
  await tapMarker(page, question);
  assert.deepStrictEqual(page.speakCalls, [], "tapping the circle started reading the row aloud");

  const marker = replyMarkerOf(question);
  const commit = sourceConstant("SWIPE_COMMIT_PX") + 10;
  const touch = (x) => ({ pointerId: 1, pointerType: "touch", clientX: x, clientY: 10, target: marker });
  await question.dispatch("pointerdown", touch(10));
  assert.equal(page.expireTimers(HOLD_MS), 0, "a finger resting on the circle armed the row's press-and-hold");
  await question.dispatch("pointermove", touch(10 + commit));
  assert.ok(!question.style.transform, "the row followed a finger that came down on its circle");
  await question.dispatch("pointerup", touch(10 + commit));
  await page.settle();
  assert.deepEqual(page.dismissCalls, [], "a drag that began on the circle archived the row");
});

test("...it opens a run of read messages the reply is collapsed into, and closes a search that hides it", async () => {
  const page = newPage();
  page.threadingSupported = true;
  const messages = [
    message({ id: "301", content: "is the runner wedged?" }),
    message({ id: "302", content: "already dealt with" }),
    message({ id: "303", content: "restarted it", reply_to: "301", ...ALICE }),
    message({ id: "304", content: "dealt with too" }),
    message({ id: "305", content: "still to read" }),
  ];
  for (const id of ["302", "303", "304"]) page.dealtWith.add(id);
  await signIn(page);
  await showDiscord(page, messages);
  await chooseReadMode(page, "Fold read");
  const reply = () => rowWithId(page, "303");
  assert.equal(reply().getAttribute("data-read-run"), "member", "the reply is not collapsed, so this proves nothing");
  const question = rowWithId(page, "301");
  assert.equal(replyMarkerOf(question).getAttribute("aria-label"), "Jump to the reply to this message",
    "a reply collapsed into a run has no way down to it");
  await tapMarker(page, question);
  assert.equal(reply().hidden, false, "the run the reply is in was not opened");
  assert.equal(reply().getAttribute("data-landed"), "true");

  const searched = newPage();
  await signIn(searched);
  await showDiscord(searched, answeredThreeTimes());
  await search(searched, "wedged");
  // The reply directly under the question is hidden as well, so nothing joins it to the question:
  // the first tap is for it.
  assert.ok(rowWithId(searched, "1000000000000000002").hasClass("search-hidden"), "the search did not hide the reply");
  assert.equal(replyMarkerOf(rowWithId(searched, "1000000000000000001")).getAttribute("aria-label"),
    "Jump to reply 1 of 3 to this message", "a reply the search hides was taken as joined to the question");
  await tapMarker(searched, rowWithId(searched, "1000000000000000001"));
  assert.equal(searched.el("search-field").hidden, true, "the search hiding the reply was left open");
  assert.equal(rowWithId(searched, "1000000000000000002").getAttribute("data-landed"), "true");
});

test("IN MAIN, the circle on a message answered from inside a thread opens All and lands there", async () => {
  const page = newPage();
  const data = threadData();
  // The second thread's answer answers the main channel's announcement, which Main shows.
  data.messages[4].reply_to = "200";
  page.threadingSupported = true;
  page.threads = data.threads;
  await signIn(page);
  await showDiscord(page, data.messages);
  await pickThread(page, "main");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "Main is not what this test assumes");
  const announcement = rowWithId(page, "200");
  assert.equal(replyLabel(announcement), "Reply (1)", "a reply Main does not show, but the page holds, was not counted");
  assert.ok(replyMarkerOf(announcement), "a reply in another view has no way down to it");

  await tapMarker(page, announcement);
  assert.equal(page.el("thread-select").value, "flat", "the jump did not open All");
  assert.equal(rowWithId(page, "204").getAttribute("data-landed"), "true", "All opened and the reply was not shown");
  assert.equal(statusText(page), "Here is the reply to it.");
  // In All the reply is two rows below, its arrow disconnected: the circle is still there.
  assert.ok(replyMarkerOf(rowWithId(page, "200")));
});

test("...IN A THREAD as well; but not to a reply Hide read leaves out of All too, which Reply still counts", async () => {
  const page = newPage();
  const data = threadData();
  // The first thread's answer answers the second thread's root.
  data.messages[2].reply_to = "203";
  page.threadingSupported = true;
  page.threads = data.threads;
  await signIn(page);
  await showDiscord(page, data.messages);
  await pickThread(page, `thread:${data.threads[1].id}`);
  assert.deepStrictEqual(shownIds(page), ["203", "204"]);
  assert.ok(replyMarkerOf(rowWithId(page, "203")), "from a thread, a reply elsewhere has no way down to it");
  await tapMarker(page, rowWithId(page, "203"));
  assert.equal(page.el("thread-select").value, "flat");
  assert.equal(rowWithId(page, "202").getAttribute("data-landed"), "true");

  const hidden = newPage();
  const again = threadData();
  again.messages[4].reply_to = "200";
  hidden.threadingSupported = true;
  hidden.threads = again.threads;
  hidden.dealtWith.add("204");
  await signIn(hidden);
  await showDiscord(hidden, again.messages);
  await turnTodoOn(hidden);
  await pickThread(hidden, "main");
  const announcement = rowWithId(hidden, "200");
  assert.ok(announcement, "Main under Hide read is not what this test assumes");
  assert.equal(replyLabel(announcement), "Reply (1)", "a reply that was written, and read, stopped counting");
  assert.equal(replyMarkerOf(announcement), undefined,
    "a circle leads to a reply Hide read hides wherever it could be shown");
});

test("GATHERED, the stack under its message is the connection: no circle on the parent or in it", async () => {
  const page = await scatteredPage();
  // 601's replies by pointer: 602 from directly below, 607 from far below. 604 and 606 are the
  // thread's, which its chips count.
  assert.equal(replyLabel(rowWithId(page, "601")), "Reply (2)");
  assert.equal(gatherChip(page, "601").textContent, "2 replies");
  assert.equal(replyMarkerOf(rowWithId(page, "601")).getAttribute("aria-label"), "Jump to reply 2 of 2 to this message");
  assert.equal(replyLabel(rowWithId(page, "602")), "Reply (1)");
  assert.equal(replyMarkerOf(rowWithId(page, "602")), undefined, "603 answers it from directly below");
  await gatherChip(page, "601").click();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), GATHERED);
  assert.ok(page.el("discord-log").children.every((li) => !replyMarkerOf(li)), "a circle in a gathered conversation");
  assert.equal(replyLabel(rowWithId(page, "601")), "Reply (2)", "gathering changed the count");
  await gutterChild(rowWithId(page, "602"), "reply-ungather").dispatch("click", { stopPropagation() {} });
  assert.ok(replyMarkerOf(rowWithId(page, "601")), "putting the replies back left no way down to them");
});

test("NOT IN THE PINNED LIST: there Reply is Reply, and there is no circle", async () => {
  const page = newPage();
  const messages = answeredThreeTimes();
  page.pinned.set(messages[0].id, storedPin(messages[0]));
  page.servePinsRevision = true;
  page.pinsRevision = 1;
  await signIn(page);
  await showDiscord(page, messages);
  await showPinned(page);
  const pinned = page.el("pinned-log").children[0];
  assert.equal(pinned.getAttribute("data-id"), messages[0].id);
  assert.equal(replyLabel(pinned), "Reply", "a pinned row says how many replies it has in a list without them");
  assert.equal(replyMarkerOf(pinned), undefined, "a pinned row has a circle to replies the list does not hold");
});

test("THE CIRCLE IS DRAWN AS THE ARROWHEAD IS: its size, its blue, on the corner, over the borders, with a ring", () => {
  // The disconnected arrow's head is a right-angled triangle 0.7rem along each side; the disc is the same box.
  assert.match(cssBlock('#discord-log .reply-jump[data-reply-style="disconnected"] .reply-jump-mark::after'),
    /border-top:\s*0\.7rem solid currentColor/, "the arrowhead the disc is sized by has changed under it");
  const marker = cssBlock("#discord-log .reply-marker");
  assert.match(marker, /--reply-marker-dot:\s*0\.7rem/, "the disc is not an arrowhead wide");
  assert.match(marker, /color:\s*var\(--accent\)/, "the disc is not in the arrow's blue");
  assert.match(marker, /position:\s*absolute/);
  assert.match(marker, /z-index:\s*1/, "the disc is not drawn over the row's borders and veil");
  // Centred on the corner of the border box: out by half its target and by the row's one-pixel border.
  assert.match(marker, /left:\s*calc\(-1px - var\(--reply-marker-target\) \/ 2\)/);
  assert.match(marker, /bottom:\s*calc\(-1px - var\(--reply-marker-target\) \/ 2\)/);
  assert.match(marker, /border-radius:\s*50%/, "the target is not round, so its corner reaches into the row");
  const dot = cssBlock("#discord-log .reply-marker::before");
  assert.match(dot, /width:\s*var\(--reply-marker-dot\)/);
  assert.match(dot, /background:\s*currentColor/);
  assert.match(cssBlock("#discord-log .reply-marker:focus-visible"), /outline:\s*2px solid var\(--accent\)/,
    "a keyboard on the circle has no ring");
  // At full strength on a receding row, as the arrow is: the row recedes in its box instead.
  assert.match(cssBlock("#discord-log li.discord-message[data-reply-marker][data-who]"), /opacity:\s*1/,
    "a row with a circle still fades as a whole, circle and all");
  assert.match(cssBlock("#discord-log li.discord-message[data-reply-marker]::after"),
    /color-mix\(in srgb, var\(--bg\) calc\(\(1 - var\(--row-opacity, 1\)\) \* 100%\)/,
    "a row with a circle no longer recedes in its box");
  assert.match(CSS, /li\.discord-message\[data-reply-marker\] > :not\(\.thread-strip\):not\(\.reply-marker\)/,
    "the circle is greyed with the row it is on");
  // The agent's tile meets the screen's edge on a phone, where half a disc on its corner would be cut
  // off: there the disc stands just inside the corner. A desk holds the tile in the column, and the
  // disc goes back on the corner.
  const tile = '#discord-log li.discord-message[data-who="coder"]:not([data-is-reply="true"]):not([data-stack-under]) > .reply-marker';
  assert.match(cssBlock(tile), /left:\s*calc\(env\(safe-area-inset-left\) - 1px - \(var\(--reply-marker-target\) - var\(--reply-marker-dot\)\) \/ 2\)/);
  assert.match(cssBlockIn("(min-width: 900px) and (pointer: fine)", tile), /left:\s*calc\(-1px - var\(--reply-marker-target\) \/ 2\)/,
    "a desk leaves the agent's disc inside its tile's corner");
  // A collapsed run's line hides every part of its row: the circle too.
  assert.match(cssBlock('#discord-log li.discord-message[data-read-run="head"] > :not(.read-run)'), /display:\s*none/);
});

test("THE COUNTS AND THE CIRCLES READ NO LAYOUT: a redraw with circles and no adjacent arrow lays nothing out", async () => {
  const { page, server } = await forwardPage({}, (p) => {
    p.messages = [...p.messages,
      message({ id: "205", content: "is the runner wedged?" }),
      message({ id: "206", content: "in between" }),
      message({ id: "207", content: "restarted it", reply_to: "205", ...ALICE })];
  });
  const list = page.el("discord-log");
  list.clientWidth = 380;
  assert.ok(replyMarkerOf(rowWithId(page, "205")), "there is no circle, so this proves nothing");
  const read = FakeElement.prototype.getBoundingClientRect;
  const asked = [];
  FakeElement.prototype.getBoundingClientRect = function rect() {
    if (this === list || this.hasClass("reply-marker")) asked.push(this === list ? "list" : "circle");
    return read.call(this);
  };
  try {
    server.post(message({ id: "208", content: "one more thing", reply_to: "205", ...OWNER }));
    await pollOnce(page);
    assert.equal(replyLabel(rowWithId(page, "205")), "Reply (2)");
    assert.ok(replyMarkerOf(rowWithId(page, "205")));
    assert.deepStrictEqual(asked, [], "drawing the counts and the circles laid the list out");
  } finally {
    FakeElement.prototype.getBoundingClientRect = read;
  }
});

test("`#224 keyboard-shortcuts` x `#221 read-modes`: on the line of a collapsed run, Enter opens the run and e acts on nothing", async () => {
  const page = newPage();
  const messages = runThread(page, { tail: RUN_TAIL });
  page.messages = messages;
  await enterRunThread(page, messages, () => chooseReadMode(page, "Fold read"));
  const head = rowWithId(page, runId(1));
  assert.equal(head.getAttribute("data-read-run"), "head");
  // Up from the landed answer to the line, as the keys go.
  page.el("scroll-area").focus();
  await press(page, page.el("scroll-area"), "ArrowUp");
  for (let i = 0; i < 6 && focused(page) !== head; i += 1) await press(page, focused(page), "k");
  assert.equal(focused(page), head, `↑ and k did not reach the run's line: the focus is on ${focusName(page)}`);
  for (const key of ["e", "p", "r", "i"]) {
    const event = await press(page, head, key);
    assert.equal(event.defaulted, false, `${key} on the run's line was left to the browser`);
    assert.equal(page.screen(), "main", `${key} on the run's line left the list`);
  }
  assert.deepStrictEqual(page.dismissCalls, [], "e on the run's line marked a message");
  assert.deepStrictEqual(page.restoreCalls, [], "e on the run's line put a read message back");
  assert.deepStrictEqual(page.pinCalls, [], "p on the run's line pinned a message");
  assert.match(statusText(page), /Enter opens them/, "nothing said why the key did nothing");
  await press(page, head, "Enter");
  await page.settle();
  assert.equal(head.hasAttribute("data-read-run"), false, "Enter on the run's line did not open the run");
  assert.equal(focused(page), head, "opening the run took the focus off it");
});

// --- `#185 reply-new-thread`, beside the grey Thread ------------------------------------------------
//
// Reply's "Start a new thread from this message" and the grey Thread tag are the same decision about
// the same message (`unansweredThread`), and a first reply sent from either goes into the message's
// thread the same way: with `thread_id`, the thread the provider named.

test("`#185 reply-new-thread`: a new thread started from Reply on Main goes into the message's thread, and the message shows its thread at once", async () => {
  const { page, data } = await freshMessagePage();
  await pickThread(page, "main");
  await replyButton(rowOf(page, "210")).click();
  assert.equal(page.el("reply-branch-row").hidden, false, "no new-thread choice on a message nobody has answered");
  await page.el("reply-text").setValue("the first answer, in a thread of its own");
  await page.el("reply-branch").setChecked(true);
  assert.equal(page.el("reply-destination").textContent, "Posts in a new thread on this message, as its first reply.");
  await page.el("reply-send").click();
  await page.settle();
  assert.deepStrictEqual(page.repliesPosted.map((call) => sentWithoutKey(call.body)),
    [{ text: "the first answer, in a thread of its own", reply_to: "210", thread_id: data.lone.id }]);

  // Back where Reply was pressed, as after every reply: Main, which shows no thread's replies, so the
  // reply is its receipt there, with the tag of the thread it went into.
  assert.equal(page.screen(), "main");
  assert.equal(page.el("thread-select").value, "main", "sending took the reader out of Main");
  const receipt = outgoingRows(page).find((row) => row.text().includes("the first answer, in a thread of its own"));
  assert.ok(receipt && threadBadge(receipt), "the reply's receipt does not lead to the thread it went into");
  // With no read: the message heads a thread now, coloured, with its reply counted...
  const tag = tagOf(rowOf(page, "210"));
  assert.deepStrictEqual({ ...tag, hue: undefined },
    { state: "open", text: "Thread(1)", title: "Open this thread", label: "Open this thread, 1 reply", hue: undefined, hidden: false },
    "the message did not show the thread its reply started");
  // ...and Reply, by the same rule as the tag, has no new thread to offer on it. The tick went with the
  // reply sent: the next reply on the message is an ordinary one, to Main, not bound for the thread.
  await replyButton(rowOf(page, "210")).click();
  assert.equal(page.el("reply-branch-row").hidden, true, "Reply offered a new thread on a message that heads one");
  assert.equal(page.el("reply-destination").hidden, true, "the sent reply's tick carried over to the next reply");
});

test("`#185 reply-new-thread`: a live reply the page holds makes the message a thread for Reply as it does for its tag", async () => {
  // The count the provider last gave the root is still nothing; the reply is in the store. The tag
  // goes by what the store holds, and so does Reply: ticking the box would post into a thread that
  // has a reply while saying it starts one.
  const { page, data } = await freshMessagePage();
  page.timeline = errorResponse(502, "chat_error", "history read timed out");
  const live = message({ id: "211", content: "somebody answered it just now",
    thread: { ...data.lone, is_root: false, reply_count: null, reply_count_exact: false } });
  page.messages = [...page.messages, live];
  await deliver(page, page.stream(), sseMessage(live));
  await page.settle();
  assert.equal(tagOf(rowOf(page, "210")).state, "open", "the tag did not count the reply the page holds");
  await replyButton(rowOf(page, "210")).click();
  assert.equal(page.el("reply-branch-row").hidden, true, "Reply offered a new thread the tag says exists");
});

test("`#185 reply-new-thread`: a draft put down with the new-thread box ticked comes back ticked, and only for its own message", async () => {
  const { page } = await freshMessagePage((_page, data) => {
    data.messages.push(message({ id: "212", content: "another message nobody has answered",
      thread: { id: "spaces/A/threads/other", root_message_id: "212", is_root: true, reply_count: 0, reply_count_exact: true } }));
  });
  const box = () => page.el("reply-branch");
  await replyButton(rowOf(page, "210")).click();
  await page.el("reply-text").setValue("half an answer");
  await box().setChecked(true);
  await page.el("reply-cancel").click();

  // Another message: its own empty box, and the choice did not follow the reader there.
  await replyButton(rowOf(page, "212")).click();
  assert.equal(page.el("reply-branch-row").hidden, false);
  assert.equal(page.el("reply-text").value, "");
  assert.equal(box().checked, false, "the tick followed the reader to another message");
  await page.el("reply-cancel").click();

  // Back to the draft: the words, and where they were going, the note with them.
  await replyButton(rowOf(page, "210")).click();
  assert.equal(page.el("reply-text").value, "half an answer");
  assert.equal(box().checked, true, "the draft came back bound for the main channel");
  assert.equal(page.el("reply-destination").textContent, "Posts in a new thread on this message, as its first reply.");

  // A draft emptied takes its tick with it.
  await page.el("reply-text").setValue("");
  await page.el("reply-cancel").click();
  await replyButton(rowOf(page, "210")).click();
  assert.equal(box().checked, false, "an emptied draft kept its tick");

  // And a reply sent: the next one on a message starts from nothing. (Sent unticked, so the message
  // is still one nobody has answered in a thread, and the box is still offered.)
  await page.el("reply-text").setValue("first a ticked draft");
  await box().setChecked(true);
  await page.el("reply-cancel").click();
  await replyButton(rowOf(page, "212")).click();
  await page.el("reply-text").setValue("an answer on the main channel");
  await box().setChecked(true);
  await box().setChecked(false);
  await page.el("reply-send").click();
  await page.settle();
  await replyButton(rowOf(page, "212")).click();
  assert.equal(page.el("reply-text").value, "");
  assert.equal(box().checked, false, "a sent reply's tick carried over");
  await page.el("reply-cancel").click();
  await replyButton(rowOf(page, "210")).click();
  assert.equal(box().checked, true, "sending another message's reply cleared this draft's tick");
});

// A reply arriving in the thread while Reply is open on its message. The grey Thread becomes Thread(1)
// on the row behind the screen, and the screen, which offered to START that thread, has to stop
// saying so: the box goes, and a reply the reader had ticked it for goes into the thread as a reply in
// it — said in the title and the note — rather than to the main channel without a word.

const INTO_ANSWERED_NOTE = "Posts in this message's thread, as you chose. It has a reply already, so yours will not be the first.";

/** A reply in the thread of `freshMessageData`'s 210, delivered live, with no count from the provider. */
async function answerLoneLive(page, data, id = "211") {
  const live = message({ id, content: "somebody answered it just now",
    thread: { ...data.lone, is_root: false, reply_count: null, reply_count_exact: false } });
  page.messages = [...page.messages, live];
  await deliver(page, page.stream(), sseMessage(live));
  await page.settle();
}

test("`#185 reply-new-thread`: a reply arriving while Reply is open takes the new-thread box away, and a ticked reply goes into the thread, saying so", async () => {
  const { page, data } = await freshMessagePage();
  page.timeline = errorResponse(502, "chat_error", "history read timed out");
  await replyButton(rowOf(page, "210")).click();
  await page.el("reply-text").setValue("my answer");
  await page.el("reply-branch").setChecked(true);
  assert.equal(replyNote(page), "Posts in a new thread on this message, as its first reply.");

  await answerLoneLive(page, data);
  assert.ok(replyInStream(page));
  assert.equal(tagOf(rowOf(page, "210")).text, "Thread(1)", "the tag did not count the live reply");
  assert.equal(page.el("reply-branch-row").hidden, true, "the box still offered to start a thread that has a reply");
  assert.equal(page.el("reply-destination").textContent, INTO_ANSWERED_NOTE);
  assert.equal(page.el("reply-destination").hidden, false);
  assert.equal(page.el("reply-text").value, "my answer", "the words went with the box");

  // Put down and picked up again: still bound for the thread, by the draft's kept choice.
  await page.el("reply-cancel").click();
  await replyButton(rowOf(page, "210")).click();
  assert.equal(page.el("reply-text").value, "my answer");
  assert.equal(page.el("reply-branch-row").hidden, true);
  assert.equal(page.el("reply-destination").textContent, INTO_ANSWERED_NOTE, "the kept draft forgot where it was going");

  await page.el("reply-send").click();
  await page.settle();
  assert.deepStrictEqual(page.repliesPosted.map((call) => sentWithoutKey(call.body)),
    [{ text: "my answer", reply_to: "210", thread_id: data.lone.id }]);
  // Sent, the binding went with the words: the next reply on it is an ordinary one, to Main.
  await replyButton(rowOf(page, "210")).click();
  assert.equal(page.el("reply-text").value, "");
  assert.equal(page.el("reply-branch-row").hidden, true);
  assert.equal(page.el("reply-destination").hidden, true, "a sent reply's thread carried over to the next one");
});

test("`#185 reply-new-thread`: a reply arriving while Reply is open, unticked, leaves it a reply on the main channel", async () => {
  const { page, data } = await freshMessagePage();
  page.timeline = errorResponse(502, "chat_error", "history read timed out");
  await replyButton(rowOf(page, "210")).click();
  await page.el("reply-text").setValue("on the main channel");
  assert.equal(page.el("reply-destination").textContent, "Posts in the main channel as a reply to this message.");

  await answerLoneLive(page, data);
  assert.equal(page.el("reply-branch-row").hidden, true, "the box still offered to start a thread that has a reply");
  assert.equal(page.el("reply-branch").checked, false);
  // As Reply opened on it now would say: Main, which needs no note.
  assert.equal(page.el("reply-destination").hidden, true);
  await page.el("reply-send").click();
  await page.settle();
  assert.deepStrictEqual(page.repliesPosted.map((call) => sentWithoutKey(call.body)), [{ text: "on the main channel", reply_to: "210" }]);
});

test("`#185 reply-new-thread`: Edit of a failed new-thread reply whose thread has been answered since puts it back into that thread", async () => {
  const { page, data } = await freshMessagePage();
  page.timeline = errorResponse(502, "chat_error", "history read timed out");
  await replyButton(rowOf(page, "210")).click();
  await page.el("reply-branch").setChecked(true);
  const posts = page.replyResponse;
  page.replyResponse = errorResponse(403, "refused", "You cannot post to this channel.");
  await page.el("reply-text").setValue("starting a thread on it");
  await page.el("reply-send").click();
  await page.settle();
  assert.equal(savedOutgoing(page)[0].threadId, data.lone.id);

  await answerLoneLive(page, data);
  await outgoingEdit(outgoingRows(page)[0]).click();
  await page.settle();
  assert.ok(replyInStream(page));
  assert.equal(page.el("reply-branch-row").hidden, true, "a new thread was offered on a message that heads one");
  assert.equal(page.el("reply-destination").textContent, INTO_ANSWERED_NOTE, "Edit sent the reply to the main channel");
  page.replyResponse = posts;
  await page.el("reply-send").click();
  await page.settle();
  assert.deepStrictEqual(sentWithoutKey(page.repliesPosted.at(-1).body),
    { text: "starting a thread on it", reply_to: "210", thread_id: data.lone.id });
});

test("`#267 quote-reply`: words selected in one message make its Reply Quote reply, and it is Reply again when they go", async () => {
  const page = await quotePage();
  const first = rowWithId(page, "1000000000000000001");
  const second = rowWithId(page, "1000000000000000002");
  assert.equal(replyLabel(second), "Reply (1)", "the premise: the second message counts its reply");
  await page.selectText(wordsIn(first), null, "about the runner");
  assert.equal(replyLabel(first), "Quote reply", "a selection in the message did not make its Reply Quote reply");
  assert.equal(replyButton(first).getAttribute("data-quote"), "true", "Quote reply is not drawn as one");
  assert.equal(replyButton(first).getAttribute("title"), sourceString("QUOTE_REPLY_TITLE"));
  assert.equal(replyButton(first).getAttribute("aria-label"), null, "Quote reply is named other than by its words");
  for (const id of ["1000000000000000002", "1000000000000000003"]) {
    assert.match(replyLabel(rowWithId(page, id)), /^Reply/, `${id}: a selection in another message changed its Reply`);
    assert.equal(replyButton(rowWithId(page, id)).hasAttribute("data-quote"), false);
  }
  // To the next message: the first is Reply again, the second Quote reply, its count kept.
  await page.selectText(wordsIn(second), null, "the deploy");
  assert.equal(replyLabel(first), "Reply", "the first message kept Quote reply after the selection moved on");
  assert.equal(replyButton(first).getAttribute("title"), "Reply to this message");
  assert.equal(replyLabel(second), "Quote reply (1)", "a message with replies lost its count, or was not Quote reply");
  assert.equal(replyButton(second).getAttribute("aria-label"), "Quote reply, 1 reply already");
  // Let go, with the mouse: Reply at once, the count as it was.
  await page.clearSelection();
  assert.equal(replyLabel(second), "Reply (1)", "Quote reply outlived the selection");
  assert.equal(replyButton(second).getAttribute("aria-label"), "Reply, 1 reply already");
  assert.equal(replyButton(second).hasAttribute("data-quote"), false);
  // Selected words that are only spaces are no quote.
  await page.selectText(wordsIn(first), null, "   ");
  assert.equal(replyLabel(first), "Reply", "a selection of nothing but spaces was offered as a quote");
});

test("`#267 quote-reply`: a selection across two messages, in a row's header or buttons, or in a message box, leaves every Reply as it is", async () => {
  const page = await quotePage();
  const first = rowWithId(page, "1000000000000000001");
  const second = rowWithId(page, "1000000000000000002");
  const labels = () => page.el("discord-log").children.map(replyLabel);
  const plain = labels();
  const author = first.descendants().find((node) => node.hasClass("msg-author"));
  for (const [from, to, what] of [
    [wordsIn(first), wordsIn(second), "across two messages"],
    [author, wordsIn(first), "from the author's name into the words"],
    [author, null, "on the author's name"],
    [replyButton(first), null, "on Reply itself"],
    [page.el("channel-compose-text"), null, "in the channel's message box"],
    [page.el("status-line"), null, "in the page's own controls"],
  ]) {
    await page.selectText(from, to, "some words");
    assert.deepStrictEqual(labels(), plain, `a selection ${what} changed a Reply`);
  }
  // A selection that was one message's and then grows across two goes at once, after a touch too.
  await page.documentEvent("pointerdown", { pointerType: "touch", target: wordsIn(first) });
  await page.selectText(wordsIn(first), null, "the first");
  assert.equal(replyLabel(first), "Quote reply");
  await page.selectText(wordsIn(first), wordsIn(second), "the first message ... the second");
  assert.deepStrictEqual(labels(), plain, "a selection that grew across two messages left Quote reply behind");
  await replyButton(first).click();
  assert.ok(replyInStream(page));
  assert.equal(page.el("reply-text").value, "", "Reply quoted a selection that was not its message's");
});

test("`#267 quote-reply`: Quote reply opens the reply with the words quoted line by line, an empty line, and the box focused with the caret under them; Send posts it as Reply would", async () => {
  const page = await quotePage();
  const first = rowWithId(page, "1000000000000000001");
  // As a browser reads a selection over two paragraphs and a line break, untidy at the ends.
  await page.selectText(wordsIn(first), null, "  the first   message, about\n\n\n the runner \t\nand  more  \n");
  await replyButton(first).click();
  assert.ok(replyInStream(page), "Quote reply did not open the reply in the list");
  const quote = "> the first message, about\n>\n> the runner\n> and more\n\n";
  assert.equal(page.el("reply-text").value, quote, "the quote is not the selection, line by line, then an empty line");
  assert.equal(focused(page), page.el("reply-text"), `the box was not ready to write in: ${focusName(page)}`);
  assert.equal(page.el("reply-text").selectionStart, quote.length, "the caret is not under the quote");
  assert.equal(page.el("reply-text").selectionEnd, quote.length);
  assert.ok(page.selection.cleared >= 1, "the words quoted stayed selected on the hidden list");
  assert.equal(replyLabel(first), "Reply", "Reply stayed Quote reply after its quote was taken");
  // Written under it, and sent: the quote and the answer, to the message, as Reply sends.
  await page.el("reply-text").setValue(`${quote}it is wedged on the cache`);
  await page.el("reply-send").click();
  await page.settle();
  assert.deepStrictEqual(sentWithoutKey(page.repliesPosted.at(-1).body),
    { text: `${quote}it is wedged on the cache`, reply_to: "1000000000000000001" },
    "the quote reply did not post as a reply to its message");

  // Longer than the cap: cut at the end of a word, with "…", the whole quote within it.
  const max = sourceConstant("QUOTE_MAX_CHARS");
  const fresh = await quotePage();
  const row = rowWithId(fresh, "1000000000000000002");
  const long = "the overnight run is still going on the shard ".repeat(80);
  await fresh.selectText(wordsIn(row), null, long);
  await replyButton(row).click();
  const value = fresh.el("reply-text").value;
  assert.match(value, /^> the overnight run/);
  assert.ok(value.endsWith("…\n\n"), "a long quote does not end with …");
  const said = value.slice(2, -2);
  assert.ok(said.length <= max, `the quote is ${said.length} characters, over ${max}`);
  assert.ok(said.length > max - 30, `the quote was cut far short of ${max}: ${said.length}`);
  assert.match(said, /[a-z]…$/, "the quote was cut in the middle of a word, or after a space");
  assert.ok(long.startsWith(said.slice(0, -1)), "the cut quote is not the start of the selection");
  // The control: one under the cap is quoted whole.
  const short = await quotePage();
  const whole = "x".repeat(max);
  const shortRow = rowWithId(short, "1000000000000000003");
  await short.selectText(wordsIn(shortRow), null, whole);
  await replyButton(shortRow).click();
  assert.equal(short.el("reply-text").value, `> ${whole}\n\n`, "a quote exactly at the cap was cut");
});

test("`#267 quote-reply`: a draft already written stays under the quote; a quote left untouched is not kept as a draft", async () => {
  const store = new Map();
  const page = newPage(store);
  await signIn(page);
  await showDiscord(page, [message({ id: "1000000000000000001", content: "the first message, about the runner" })]);
  const row = () => rowWithId(page, "1000000000000000001");
  const saved = () => JSON.parse(store.get("vibe-talk.voice.drafts") || "{}")["1000000000000000001"];
  await replyButton(row()).click();
  await page.el("reply-text").setValue("my half-written answer");
  await page.el("reply-cancel").click();
  assert.equal(saved(), "my half-written answer", "the premise: the draft was kept");
  // Quote reply: the quote over the draft, the caret between them.
  await page.selectText(wordsIn(row()), null, "about the runner");
  await replyButton(row()).click();
  assert.equal(page.el("reply-text").value, "> about the runner\n\nmy half-written answer", "the draft was lost or the quote not over it");
  assert.equal(page.el("reply-text").selectionStart, "> about the runner\n\n".length);
  // Cancel without writing: the draft as it was, and no quote in it.
  await page.el("reply-cancel").click();
  assert.equal(saved(), "my half-written answer", "an untouched quote was kept in the draft");
  assert.equal(row().draftMarker.hidden, false, "the draft's marker went");
  // With no draft at all, an untouched quote leaves none.
  await replyButton(row()).click();
  await page.el("reply-text").setValue("");
  await page.el("reply-cancel").click();
  assert.equal(saved(), undefined, "the premise: no draft");
  await page.selectText(wordsIn(row()), null, "about the runner");
  await replyButton(row()).click();
  await page.el("reply-cancel").click();
  assert.equal(saved(), undefined, "a quote nobody wrote under became a draft");
  assert.equal(row().draftMarker.hidden, true, "a quote nobody wrote under put Draft on the row");
  // Written under, it is a draft like any other, quote and all; quoting the same words again adds none.
  await page.selectText(wordsIn(row()), null, "about the runner");
  await replyButton(row()).click();
  await page.el("reply-text").setValue("> about the runner\n\nstill looking");
  await page.el("reply-cancel").click();
  assert.equal(saved(), "> about the runner\n\nstill looking");
  await page.selectText(wordsIn(row()), null, "about the runner");
  await replyButton(row()).click();
  assert.equal(page.el("reply-text").value, "> about the runner\n\nstill looking", "the same quote was put in twice");
});

test("`#267 quote-reply`: on Slack Quote reply is offered too and posts where Reply posts; on Google Chat it keeps Reply's new-thread choice", async () => {
  for (const slack of [true, false]) {
    const name = slack ? "Slack" : "Google Chat";
    const { page } = await freshMessagePage((p) => { p.repliesInThread = slack || undefined; });
    await pickThread(page, "main");
    const row = rowOf(page, "210");
    await page.selectText(wordsIn(row), null, "nobody has answered");
    assert.equal(replyLabel(row), "Quote reply", `${name}: no Quote reply`);
    await replyButton(row).click();
    assert.equal(page.el("reply-text").value, "> nobody has answered\n\n", `${name}: the quote is not in the box`);
    assert.equal(page.el("reply-branch-row").hidden, slack, `${name}: the new-thread box is not as Reply offers it`);
    await page.el("reply-text").setValue("> nobody has answered\n\nI will");
    await page.el("reply-send").click();
    await page.settle();
    // What Reply posts on each (`#252 thread-button-state`): Slack's server threads it from there.
    assert.deepStrictEqual(sentWithoutKey(page.repliesPosted.at(-1).body), { text: "> nobody has answered\n\nI will", reply_to: "210" },
      `${name}: Quote reply did not post as Reply does`);
  }
});

test("`#267 quote-reply`: r on the selected message quotes the words selected in it, and only in it", async () => {
  const page = await keyboardPage();
  const row = channelRow(page, "29");
  row.focus();
  FakeElement.focusVisible = true;
  await page.selectText(wordsIn(row), null, "a sentence about the overnight run");
  await press(page, row, "r");
  assert.ok(replyInStream(page), "r did not open the reply in the list");
  assert.equal(page.el("reply-text").value, "> a sentence about the overnight run\n\n", "r did not quote the selection");
  assert.equal(focused(page), page.el("reply-text"), `after r the focus is on ${focusName(page)}`);
  await page.el("reply-cancel").click();
  await page.settle();
  // Words selected in another message: r answers the message it acts on, plainly.
  const other = channelRow(page, "28");
  await page.selectText(wordsIn(other), null, "message 28");
  assert.equal(replyLabel(other), "Quote reply");
  const selected = channelRow(page, "29");
  selected.focus();
  await press(page, selected, "r");
  assert.ok(replyInStream(page));
  assert.match(page.el("reply-target-meta").textContent, /id 29$/, "r answered another message");
  assert.equal(page.el("reply-text").value, "", "r quoted words from a message it did not answer");
  // The list of keys says so.
  assert.ok(page.el("keyboard-keys").text().includes("Quote reply"), "the list of keys does not say r quotes");
});

test("`#267 quote-reply`: after a touch, a selection the tap took away still quotes for a moment, and the label says so until then", async () => {
  const hold = sourceConstant("QUOTE_HOLD_MS");
  const page = await quotePage();
  const first = rowWithId(page, "1000000000000000001");
  await page.documentEvent("pointerdown", { pointerType: "touch", target: wordsIn(first) });
  await page.selectText(wordsIn(first), null, "about the runner");
  // The phone clears the selection as the finger lands on Reply, before the click.
  await page.clearSelection();
  assert.equal(replyLabel(first), "Quote reply", "the label went before the tap could arrive");
  await replyButton(first).click();
  assert.equal(page.el("reply-text").value, "> about the runner\n\n", "the tap that cleared the selection lost the quote");
  await page.el("reply-cancel").click();
  await page.settle();
  // Let go and left alone: Reply again once the moment is over, and a press is a plain Reply.
  await page.selectText(wordsIn(first), null, "about the runner");
  await page.clearSelection();
  assert.equal(page.expireTimers(hold), 1, "nothing lets a touch's held selection go");
  assert.equal(replyLabel(first), "Reply", "Quote reply outlived its moment");
  await replyButton(first).click();
  assert.equal(page.el("reply-text").value, "", "a selection let go long ago was quoted");
  // A new selection inside the moment holds again from the start, and is the one quoted.
  await page.el("reply-cancel").click();
  await page.settle();
  await page.selectText(wordsIn(first), null, "the first");
  await page.clearSelection();
  await page.selectText(wordsIn(first), null, "the runner");
  assert.equal(page.expireTimers(hold), 0, "a selection made again was still let go on the old timer");
  await replyButton(first).click();
  assert.equal(page.el("reply-text").value, "> the runner\n\n");
});

test("`#267 quote-reply`: dragging over a message's words neither folds it nor reads it aloud; the click that ends the drag selects the row", async () => {
  const page = newPage();
  await signIn(page);
  const lines = await showDiscord(page, [message({ id: "1000000000000000001", content: "x ".repeat(2000) }),
    message({ id: "1000000000000000002", content: "short" })]);
  const li = lines[0];
  assert.equal(li.getAttribute("data-collapsed"), "true", "the premise: a long message starts folded");
  await page.documentEvent("pointerdown", { pointerType: "mouse", target: wordsIn(li) });
  await page.selectText(wordsIn(li), null, "x x x");
  const click = { target: wordsIn(li) };
  await li.dispatch("click", click);
  await page.el("scroll-area").dispatch("click", click);
  assert.equal(li.getAttribute("data-collapsed"), "true", "a drag over the words folded the message open");
  assert.equal(selectedId(page), "1000000000000000001", "the drag's click did not select the row it was in");
  assert.equal(replyLabel(li), "Quote reply");
  // In Read, the same drag reads nothing aloud; the control, a tap with nothing selected, does.
  const reading = newPage();
  await signIn(reading);
  const [row] = await inReadingMode(reading, [message({ id: "1000000000000000003", content: "read me" })]);
  const heard = () => reading.speakCalls.length + reading.players.length +
    (reading.deviceSpeech ? reading.deviceSpeech.utterances.length : 0);
  await reading.selectText(wordsIn(row), null, "read");
  await row.dispatch("click", { target: wordsIn(row) });
  await reading.settle();
  assert.equal(heard(), 0, "a drag over the words in Read read it aloud");
  await reading.clearSelection();
  await row.dispatch("click", { target: wordsIn(row) });
  await reading.settle();
  assert.ok(heard() > 0, "a tap in Read read nothing aloud, so this test cannot see a drag doing it");
});

test("`#267 quote-reply`: Help says what Quote reply does, where it posts, and how long a quote may be", () => {
  const entry = helpEntry("channel");
  assert.match(entry, /<strong>Quote reply<\/strong>/);
  assert.match(entry, /on Slack that is the message's\s+thread/);
  assert.match(entry, /a quote you leave untouched\s+is not kept as a draft/);
  const max = sourceConstant("QUOTE_MAX_CHARS");
  assert.match(entry, new RegExp(`Up to ${max.toLocaleString("en-US")} characters are quoted`), "Help does not state the cap the code keeps");
  // Drawn in the accent, over the selected row's and a focused row's ink.
  for (const list of ["#side-log", "#discord-log", "#pinned-log"]) {
    const block = cssBlock(`:root ${list} li.discord-message .meta .reply-button[data-quote]`);
    assert.match(block, /color:\s*var\(--accent\)/);
    // "Quote" in the width Reply had: its words kept inside it, at Reply's weight.
    const short = cssBlock(`:root ${list} li.discord-message .meta .reply-button[data-quote="short"]`);
    for (const rule of [/box-sizing:\s*border-box/, /padding-left:\s*0/, /padding-right:\s*0/, /overflow:\s*hidden/,
      /white-space:\s*nowrap/, /font-weight:\s*inherit/]) {
      assert.match(short, rule, `${list}: the short Quote rule lacks ${rule}`);
    }
  }
});

test("`#267 quote-reply`: where Quote reply would wrap the header line, the button keeps Reply's width and reads Quote, so the message's words do not move", async () => {
  const page = await quotePage();
  const first = rowWithId(page, "1000000000000000001");
  const second = rowWithId(page, "1000000000000000002");
  const third = rowWithId(page, "1000000000000000003");
  // A header line with room for Reply and not for Quote reply: as wide as Quote reply the button
  // wraps it, a line taller, as a long author's name and a reply count do on a phone.
  let measured = 0;
  for (const li of [first, second]) {
    const meta = metaOf(li);
    const drawn = meta.getBoundingClientRect.bind(meta);
    meta.getBoundingClientRect = () => {
      measured += 1;
      const box = drawn();
      const wraps = replyLabel(li).startsWith("Quote reply") ? 22 : 0;
      return { ...box, height: box.height + wraps, bottom: box.bottom + wraps };
    };
    replyButton(li).clientWidth = 52;
  }
  await page.selectText(wordsIn(first), null, "about the runner");
  assert.equal(replyLabel(first), "Quote", "Quote reply was drawn where it wraps the header line");
  assert.equal(replyButton(first).getAttribute("data-quote"), "short");
  assert.equal(replyButton(first).style.width, "52px", "the button did not keep the width it had as Reply");
  assert.equal(replyButton(first).getAttribute("aria-label"), "Quote reply", "cut to Quote, its name lost the rest");
  assert.equal(replyButton(first).getAttribute("title"), sourceString("QUOTE_REPLY_TITLE"));
  // Decided once, as the selection begins, not again at every change while it grows.
  const once = measured;
  for (const text of ["about the", "about the run", "about the runner"]) await page.selectText(wordsIn(first), null, text);
  assert.equal(measured, once, "the header line was measured again at every change of the selection");
  // A press quotes as the full label does.
  await replyButton(first).click();
  assert.equal(page.el("reply-text").value, "> about the runner\n\n", "Quote did not quote");
  await page.el("reply-cancel").click();
  await page.settle();
  assert.equal(replyLabel(first), "Reply");
  assert.equal(replyButton(first).style.width, "", "Reply kept the width Quote was held at");
  assert.equal(replyButton(first).hasAttribute("data-quote"), false);
  assert.equal(replyButton(first).getAttribute("aria-label"), null);
  // With its count: "Quote (1)", named in full; Reply (1) again when the selection goes.
  await page.selectText(wordsIn(second), null, "the deploy");
  assert.equal(replyLabel(second), "Quote (1)");
  assert.equal(replyButton(second).getAttribute("aria-label"), "Quote reply, 1 reply already");
  await page.clearSelection();
  assert.equal(replyLabel(second), "Reply (1)");
  assert.equal(replyButton(second).getAttribute("aria-label"), "Reply, 1 reply already");
  assert.equal(replyButton(second).style.width, "");
  // The control: where the line keeps its height, Quote reply in full.
  await page.selectText(wordsIn(third), null, "an answer");
  assert.equal(replyLabel(third), "Quote reply", "a header line with room was given the short label");
  assert.equal(replyButton(third).getAttribute("data-quote"), "true");
  assert.equal(replyButton(third).style.width || "", "");
});

test("`#267 quote-reply`: a range that runs past the message's words to the very start of the next row, as a triple-click leaves one, is the message's; one that takes in a word outside them is not", async () => {
  const page = await quotePage();
  const first = rowWithId(page, "1000000000000000001");
  const second = rowWithId(page, "1000000000000000002");
  const part = (li, name) => li.descendants().find((node) => node.hasClass(name));
  const words = "the first message, about the runner";
  assert.equal(part(second, "msg-author").hidden, false, "the premise: the next row draws its author's name first");
  // As Chromium's triple-click leaves it: the paragraph and the break after it, the range ending at
  // the start of the next row's first word.
  await page.selectText(wordsIn(first), part(second, "msg-author"), `${words}\n\n`, { endOffset: 0 });
  assert.equal(replyLabel(first), "Quote reply", "a triple-click's paragraph was not offered as a quote");
  assert.match(replyLabel(second), /^Reply/, "the next row, none of it selected, became Quote reply");
  await replyButton(first).click();
  assert.equal(page.el("reply-text").value, `> ${words}\n\n`, "the triple-click's paragraph was not quoted");
  await page.el("reply-cancel").click();
  await page.settle();
  // Words drawn nowhere between the two do not count; drawn, they do.
  const aside = page.document.createElement("span");
  aside.textContent = "words the row does not draw";
  aside.hidden = true;
  first.append(aside);
  await page.selectText(wordsIn(first), part(second, "msg-author"), `${words}\n\n`, { endOffset: 0 });
  assert.equal(replyLabel(first), "Quote reply", "words hidden after the message's own counted as selected");
  aside.hidden = false;
  await page.selectText(wordsIn(first), part(second, "msg-author"), `${words}\n\naside`, { endOffset: 0 });
  assert.equal(replyLabel(first), "Reply", "words drawn after the message's own were quoted as if they were not there");
  first.removeChild(aside);
  // Run on into the next row, or start in this row's own header line: words outside the message are
  // selected, and that is no quote.
  for (const [from, to, ends, what] of [
    [wordsIn(first), part(second, "msg-author"), { endOffset: 2 }, "two letters of the next row's author"],
    [wordsIn(first), part(second, "msg-time"), { endOffset: 0 }, "the next row's author's name, up to its time"],
    [wordsIn(first), wordsIn(second), { endOffset: 3 }, "the next row's words"],
    [part(first, "msg-time"), wordsIn(first), { startOffset: 0 }, "this row's time"],
  ]) {
    await page.selectText(from, to, "some words", ends);
    assert.equal(replyLabel(first), "Reply", `a selection taking in ${what} was offered as a quote`);
    assert.match(replyLabel(second), /^Reply/, `a selection taking in ${what} made the next row Quote reply`);
  }
});

test("`#267 quote-reply`: after the mouse a selection's words as drawn are read once, when Quote reply is pressed; after a touch, at each change", async () => {
  const page = await quotePage();
  const first = rowWithId(page, "1000000000000000001");
  await page.documentEvent("pointerdown", { pointerType: "mouse", target: wordsIn(first) });
  for (const text of ["the", "the first", "the first message"]) await page.selectText(wordsIn(first), null, text);
  assert.equal(replyLabel(first), "Quote reply");
  assert.equal(page.selection.reads, 0, "a drag read the words as drawn at every move of the mouse");
  await replyButton(first).click();
  assert.equal(page.el("reply-text").value, "> the first message\n\n");
  assert.ok(page.selection.reads >= 1, "the press did not read the words it quotes");
  await page.el("reply-cancel").click();
  await page.settle();
  // After a touch the phone may take the selection before its tap on Reply arrives, so they are read
  // as they change, and the last of them is what a tap after it quotes.
  const touch = await quotePage();
  const row = rowWithId(touch, "1000000000000000001");
  await touch.documentEvent("pointerdown", { pointerType: "touch", target: wordsIn(row) });
  await touch.selectText(wordsIn(row), null, "the first");
  await touch.selectText(wordsIn(row), null, "the first message");
  assert.equal(touch.selection.reads, 2, "after a touch the words were not read as they changed");
  await touch.clearSelection();
  await replyButton(row).click();
  assert.equal(touch.el("reply-text").value, "> the first message\n\n", "the tap did not quote the last words selected");
});

/** The rows edged as the message a reply open in a column answers, as "left:id" and "right:id". */
const replyingRows = (page) => [
  ...page.el("discord-log").children.filter((li) => li.hasAttribute("data-replying")).map((li) => `left:${li.getAttribute("data-id")}`),
  ...page.el("side-log").children.filter((li) => li.hasAttribute("data-replying")).map((li) => `right:${li.getAttribute("data-id")}`),
];

test("`#277 reply-in-column`: in two columns Reply opens its box in the column it was pressed in, both columns staying, the other untouched, and Cancel puts it away with nothing moving", async () => {
  const { page } = await replyColumnsPage();
  const left = page.el("scroll-area");
  const right = page.el("side-scroll");
  const before = { left: left.scrollTop, right: right.scrollTop, leftIds: shownIds(page), rightIds: sideIds(page) };
  for (const [column, row, id, list] of [
    ["left", () => rowWithId(page, "200"), "200", left],
    ["right", () => sideRow(page, "202"), "202", right],
  ]) {
    await replyButton(row()).click();
    assert.equal(page.screen(), "main", `${column}: Reply replaced both columns with the reply screen`);
    assert.ok(inTwoColumns(page), `${column}: Reply took the page out of two columns`);
    assert.equal(replyBoxAt(page), column, `${column}: the box did not open in the column Reply was pressed in`);
    const [own, other] = column === "left" ? ["column-reply", "side-reply"] : ["side-reply", "column-reply"];
    assert.equal(page.el(own).hidden, false, `${column}: the column's place for a reply is not shown`);
    assert.equal(page.el(other).hidden, true, `${column}: the other column has a box too`);
    assert.deepStrictEqual(replyingRows(page), [`${column}:${id}`], `${column}: the message answered is not the one edged`);
    assert.match(page.el("reply-target-meta").textContent, new RegExp(`id ${id}$`), `${column}: the box does not say what it answers`);
    assert.equal(focused(page), page.el("reply-text"), `${column}: the box did not take the focus; ${focusName(page)} has it`);
    assert.deepStrictEqual([shownIds(page), sideIds(page)], [before.leftIds, before.rightIds], `${column}: a column's rows changed`);
    // `#410 inline-reply-editor`. Neither column moves: the other is not touched, and the pressed one is
    // not moved to a box this fixture draws at its foot (no anchored layout here; the browser walk
    // measures the box under its row).
    assert.deepStrictEqual([left.scrollTop, right.scrollTop], [before.left, before.right], `${column}: a column moved`);
    await page.el("reply-cancel").click();
    assert.equal(page.screen(), "main");
    assert.equal(replyBoxAt(page), "closed", `${column}: Cancel left the box in the column`);
    assert.deepStrictEqual([page.el("column-reply").hidden, page.el("side-reply").hidden], [true, true],
      `${column}: Cancel left a column's place for a reply on screen`);
    assert.deepStrictEqual(replyingRows(page), [], `${column}: Cancel left the message edged`);
    assert.deepStrictEqual([left.scrollTop, right.scrollTop], [before.left, before.right], `${column}: Cancel moved a column`);
    assert.ok(nodeWithinFixture(list, focused(page)), `${column}: after Cancel the keys are on ${focusName(page)}, not the column's list`);
  }

  // THE CONTROL: every reply opened in the left column's place. The walk above sees it.
  const { page: control } = await replyColumnsPage(brokenScript(
    '  return nodeWithin(leftEl("side-column"), list) ? leftEl("side-reply") : leftEl("column-reply");',
    '  return leftEl("column-reply");'));
  await replyButton(sideRow(control, "202")).click();
  assert.equal(replyBoxAt(control), "left", "the control opened the right column's reply on the right, so this test cannot see it");
});

test("`#277 reply-in-column`: Send from a column posts what a reply in one column posts — Main's message to Main, the thread's into its thread — once, and leaves the column where it was", async () => {
  // One column first: what a reply posts for each, from Main and from the thread.
  const one = await threadPage("main");
  distinctReplies(one);
  await replyButton(rowOf(one, "200")).click();
  assert.ok(replyInStream(one), "the premise is the reply in one column");
  await one.el("reply-text").setValue("noted");
  await one.el("reply-send").click();
  await one.settle();
  await pickThread(one, "thread:spaces/A/threads/one & two");
  await replyButton(rowOf(one, "202")).click();
  await one.el("reply-text").setValue("agreed");
  await one.el("reply-send").click();
  await one.settle();
  const expected = one.repliesPosted.map((call) => keyless(call.body));
  assert.equal(expected.length, 2);
  assert.equal(expected[1].thread_id, "spaces/A/threads/one & two", "the premise is a reply in the thread posted into it");

  const { page } = await replyColumnsPage();
  distinctReplies(page);
  const left = page.el("scroll-area");
  const right = page.el("side-scroll");
  // Where each column was: at its newest message, which a reader there goes on following as the reply
  // is posted at the foot, or a place up its list, which is kept.
  const place = (area) => (atBottomOf(area) ? "newest" : area.scrollTop);
  await replyButton(rowWithId(page, "200")).click();
  const before = [place(left), place(right)];
  await page.el("reply-text").setValue("noted");
  await page.el("reply-send").click();
  // A second press arrives at a box that has gone: nothing more is posted.
  await page.el("reply-send").click();
  await page.settle();
  assert.equal(replyBoxAt(page), "closed", "Send left the box in the column");
  assert.equal(page.screen(), "main", "Send from a column went to another screen");
  assert.equal(place(left), before[0], "Send moved the left column");
  assert.ok(nodeWithinFixture(left, focused(page)), `after Send the keys are on ${focusName(page)}, not the left column's list`);
  await replyButton(sideRow(page, "202")).click();
  assert.equal(replyBoxAt(page), "right");
  before[1] = place(right);
  await page.el("reply-text").setValue("agreed");
  await press(page, page.el("reply-text"), "Enter");
  await page.settle();
  assert.equal(replyBoxAt(page), "closed", "Enter sent and left the box in the column");
  assert.deepStrictEqual(page.repliesPosted.map((call) => keyless(call.body)), expected,
    "a reply sent from a column did not post what a reply in one column posts, once each");
  assert.equal(place(right), before[1], "Enter moved the right column");
  assert.ok(nodeWithinFixture(right, focused(page)), `after Enter the keys are on ${focusName(page)}, not the right column's list`);
  assert.deepStrictEqual(replyingRows(page), []);
  // The Done a sent reply makes is drawn where the message stands, in the column it was answered in.
  assert.equal(sideRow(page, "202").getAttribute("data-archived"), "true", "the reply's Done was not drawn on the right");
  assert.equal(rowWithId(page, "200").getAttribute("data-archived"), "true", "the reply's Done was not drawn on the left");
});

test("`#277 reply-in-column`: Quote reply in either column quotes the words selected there in that column's box, the caret under the quote, and Cancel keeps only the draft there was", async () => {
  const { page } = await replyColumnsPage();
  distinctReplies(page);
  for (const [column, row, id, words] of [
    ["left", () => rowWithId(page, "200"), "200", "announcement"],
    ["right", () => sideRow(page, "202"), "202", "first thread answer"],
  ]) {
    await page.selectText(wordsIn(row()), null, words);
    assert.equal(replyLabel(row()), "Quote reply", `${column}: no Quote reply`);
    await replyButton(row()).click();
    assert.equal(replyBoxAt(page), column, `${column}: Quote reply did not open in its column`);
    assert.equal(page.screen(), "main");
    assert.equal(page.el("reply-text").value, `> ${words}\n\n`, `${column}: the quote is not in the box`);
    assert.equal(page.el("reply-text").selectionStart, `> ${words}\n\n`.length, `${column}: the caret is not under the quote`);
    assert.equal(focused(page), page.el("reply-text"));
    assert.deepStrictEqual(replyingRows(page), [`${column}:${id}`]);
    await page.el("reply-cancel").click();
    assert.equal(draftMarkerOf(row()).hidden, true, `${column}: a quote left untouched was kept as a draft`);
  }
  // Written under and sent: the quote and the words, into the thread.
  await page.selectText(wordsIn(sideRow(page, "202")), null, "first thread answer");
  await replyButton(sideRow(page, "202")).click();
  await page.el("reply-text").setValue("> first thread answer\n\nyes, this");
  await page.el("reply-send").click();
  await page.settle();
  assert.deepStrictEqual(keyless(page.repliesPosted.at(-1).body),
    { text: "> first thread answer\n\nyes, this", reply_to: "202", thread_id: "spaces/A/threads/one & two", keyed: true });
});

test("`#277 reply-in-column`: Escape closes an empty box at once; with words, the first goes to the column's list and keeps the draft, and the next puts the reply down; m goes back to the box", async () => {
  const { page } = await replyColumnsPage();
  const left = page.el("scroll-area");
  await replyButton(rowWithId(page, "200")).click();
  await press(page, page.el("reply-text"), "Escape");
  assert.equal(replyBoxAt(page), "closed", "Escape in an empty box did not put the reply down");
  assert.ok(nodeWithinFixture(left, focused(page)), `after Escape the keys are on ${focusName(page)}`);

  await replyButton(rowWithId(page, "200")).click();
  await page.el("reply-text").setValue("half a thought");
  await press(page, page.el("reply-text"), "Escape");
  assert.equal(replyBoxAt(page), "left", "Escape in a box with words in it put the reply down");
  assert.ok(nodeWithinFixture(left, focused(page)), `Escape out of the box left the keys on ${focusName(page)}, not its column's list`);
  await press(page, focused(page), "m");
  assert.equal(focused(page), page.el("reply-text"), `m went to ${focusName(page)}, not the reply open in the column`);
  await press(page, page.el("reply-text"), "Escape");
  await press(page, focused(page), "Escape");
  assert.equal(replyBoxAt(page), "closed", "the second Escape did not put the reply down");
  assert.equal(page.screen(), "main");
  assert.ok(inTwoColumns(page), "Escape left two columns");
  assert.equal(sideIds(page).length, 2, "Escape closed the thread on the right as well as the reply");
  assert.equal(draftMarkerOf(rowWithId(page, "200")).hidden, false, "the words were not kept as a draft");
  await replyButton(rowWithId(page, "200")).click();
  assert.equal(page.el("reply-text").value, "half a thought", "Reply did not open with the draft");
});

test("`#410 inline-reply-editor`: two columns going while a reply is open keep it in the stream where its message is drawn, words and focus and all; where it is not, it opens again, words intact, as soon as its message is drawn", async () => {
  // The coordinator, 2026-10-21: "when two columns collapse with a reply open, the reply stays inline in
  // the left column if its message is drawn there; otherwise it becomes a draft that reopens inline, with
  // its text intact, as soon as that message is drawn again."
  const { page } = await replyColumnsPage();
  await replyButton(sideRow(page, "202")).click();
  await page.el("reply-text").setValue("keep me");
  await page.setDesk(false);
  assert.ok(!inTwoColumns(page), "the premise is one column");
  // Two columns chosen from All go back to it, where the thread's answer is drawn: the reply is open
  // under it there at once, the reader still typing in it.
  assert.ok(shownIds(page).includes("202"), `the premise is All in one column: ${shownIds(page)}`);
  assert.equal(replyBoxAt(page), "left", "the reply was not kept open in the one column, where its message is drawn");
  assert.deepStrictEqual(inlineAnchors(page), ["discord-log:202"]);
  assert.equal(focused(page), page.el("reply-text"), `the reader typing lost the box: ${focusName(page)} has the focus`);
  assert.equal(page.el("reply-text").value, "keep me", "the words did not go with the reply");
  assert.match(page.el("reply-target-meta").textContent, /id 202$/);
  distinctReplies(page);
  await page.el("reply-send").click();
  await page.settle();
  assert.deepStrictEqual(keyless(page.repliesPosted.at(-1).body),
    { text: "keep me", reply_to: "202", thread_id: "spaces/A/threads/one & two", keyed: true },
    "the reply opened again did not post where it was going");
  await page.setDesk(true);
  assert.ok(inTwoColumns(page));
  assert.deepStrictEqual(sideIds(page), ["201", "202"]);

  // The other path, made certain: two columns chosen from Main, which they go back to, and the thread's
  // reply open on the right, the keys on the left column's list.
  const away = await threadPage("main");
  await twoColumns(away);
  await threadBadge(rowWithId(away, "201")).click();
  await away.settle();
  await replyButton(sideRow(away, "202")).click();
  await away.el("reply-text").setValue("wait for me");
  away.el("scroll-area").focus();
  await away.setDesk(false);
  assert.ok(!shownIds(away).includes("202"), `the premise is Main in one column, without the thread's answer: ${shownIds(away)}`);
  assert.equal(replyBoxAt(away), "closed", "a reply was left open with its message nowhere on screen");
  await pickThread(away, "thread:spaces/A/threads/one & two");
  assert.equal(replyBoxAt(away), "left", "the reply did not open again once its message was drawn");
  assert.deepStrictEqual(inlineAnchors(away), ["discord-log:202"]);
  assert.equal(away.el("reply-text").value, "wait for me", "the reply opened again without its words");
  assert.notEqual(focused(away), away.el("reply-text"), "a reply opened again later took the keys from the list");

  // One column: Reply is in the list, as it is everywhere.
  await replyButton(rowWithId(page, "200")).click();
  assert.equal(replyBoxAt(page), "left", "Reply in one column did not open in the list");
  await page.el("reply-cancel").click();

  // Back in two: the X on the thread a reply is open under puts the reply away, keeping its words.
  await replyButton(sideRow(page, "201")).click();
  await page.el("reply-text").setValue("about the root");
  await page.el("side-close").click();
  await page.settle();
  assert.equal(replyBoxAt(page), "closed", "a reply open under a thread outlived the thread");
  assert.equal(page.screen(), "main");
  assert.equal(draftMarkerOf(rowWithId(page, "201")).hidden, false, "the words of the reply put away were not kept");
  // ...and the call's view, which has no columns, does too.
  await replyButton(rowWithId(page, "200")).click();
  assert.equal(replyBoxAt(page), "left");
  await page.el("view-switch").click();
  await page.settle();
  assert.equal(replyBoxAt(page), "closed", "the call's view kept a reply open in a column");
});

// `#314 reply-focus-flake`. The browser check of the reply open in a column (tests/reply_in_column_browser.py,
// since folded into tests/inline_reply_browser.py) failed now and then on a loaded host with "the box lost
// the focus on the way to the reply screen". The page decides one column or two at the frame after a resize,
// and Chromium, once it has laid the narrower window out without the column, takes the focus off the box it
// no longer draws in a task of its own — on a slow frame, before the page's. The hand-off then saw no focus
// in the box (`leaveColumns`). That check held the page's frames back to make the order certain; here it is
// the order of the steps, the reply now opened again in the one column (`#410 inline-reply-editor`).

/** A reply open in `column` of two columns, on `row`, with words in its box and the focus. */
async function replyTypedInColumn(page, column, row) {
  await replyButton(row(page)).click();
  assert.equal(replyBoxAt(page), column, `${column}: the premise is a reply open in the column`);
  await page.el("reply-text").setValue("halfway through a sentence");
  assert.equal(focused(page), page.el("reply-text"), `${column}: the premise is the focus in the box`);
}

/** The window narrowed with the reader typing, the browser taking the focus off the box before the page's frame. */
async function narrowedUnderSlowFrame(page, column) {
  page.wideDesk = false;
  assert.equal(page.el("reply-compose").getClientRects().length, 0, `${column}: the premise is a box no longer drawn`);
  await page.loseFocus();
  assert.equal(focused(page), null, `${column}: the premise is the focus taken off the box`);
  await page.setDesk(false);
}

const REPLY_COLUMNS_ROWS = [["left", (page) => rowWithId(page, "200")], ["right", (page) => sideRow(page, "202")]];

test("`#314 reply-focus-flake`: a window narrowed under a reply open in a column keeps the box, opened again in the one column, with the focus in it, even where the browser took the focus off it before the page's frame; a box the reader left for nothing is not given it back", async () => {
  for (const [column, row] of REPLY_COLUMNS_ROWS) {
    // A quick frame: the box still has the focus when the page hands the reply on, to the one column (All,
    // which draws both messages: `#410 inline-reply-editor`).
    const { page: quick } = await replyColumnsPage();
    await replyTypedInColumn(quick, column, row);
    await quick.setDesk(false);
    assert.ok(replyInStream(quick), `${column}: the premise is the reply open in the one column`);
    assert.equal(focused(quick), quick.el("reply-text"), `${column}, quick frame: ${focusName(quick)} has the focus, not the box`);

    // A slow one: the browser has taken it already.
    const { page } = await replyColumnsPage();
    await replyTypedInColumn(page, column, row);
    await narrowedUnderSlowFrame(page, column);
    assert.ok(replyInStream(page), `${column}: one column did not keep the reply`);
    assert.equal(replyBoxAt(page), "left");
    assert.equal(page.el("reply-text").value, "halfway through a sentence", `${column}: the words did not go with the reply`);
    assert.equal(focused(page), page.el("reply-text"), `${column}, slow frame: ${focusName(page)} has the focus, not the box`);

    // The reader's own leaving is theirs: a click away from the box onto what takes no focus, the box still
    // drawn, and then the window narrowed. The box does not take them back, as for any reader not typing.
    await page.el("reply-cancel").click();
    await page.setDesk(true);
    assert.ok(inTwoColumns(page), `${column}: the premise is two columns again`);
    await replyTypedInColumn(page, column, row);
    assert.ok(page.el("reply-compose").getClientRects().length > 0, `${column}: the premise is a box drawn in its column`);
    await page.loseFocus();
    await page.setDesk(false);
    assert.ok(replyInStream(page));
    assert.notEqual(focused(page), page.el("reply-text"),
      `${column}: a box the reader had left took the focus back in the one column`);
  }

  // Taken by the browser, but the window wide again before the page's frame, and the reader then on the
  // column's list and off it onto nothing: what took the focus since is where they went, and a later
  // narrowing does not take them back to the box.
  const movedOn = async (script) => {
    const { page } = await replyColumnsPage(script);
    await replyTypedInColumn(page, "right", REPLY_COLUMNS_ROWS[1][1]);
    page.wideDesk = false;
    await page.loseFocus();
    await page.setDesk(true);
    assert.equal(replyBoxAt(page), "right", "the premise is the reply still open in its column");
    page.el("side-scroll").focus();
    await page.documentEvent("focusin", { type: "focusin", target: page.el("side-scroll"), relatedTarget: null });
    await page.loseFocus();
    await page.setDesk(false);
    assert.ok(replyInStream(page));
    return focused(page) === page.el("reply-text");
  };
  assert.equal(await movedOn(SCRIPT), false, "the box took back a focus the reader had given the column's list since");

  // THE CONTROLS: the hand-off deciding from where the focus is alone, as it did; and the browser's taking
  // never forgotten. The walk above sees each.
  const { page: control } = await replyColumnsPage(brokenScript(
    "(nodeWithin(el(\"reply-compose\"), document.activeElement) || (replyFocusTaken && focusLost()))",
    "nodeWithin(el(\"reply-compose\"), document.activeElement)"));
  await replyTypedInColumn(control, "right", REPLY_COLUMNS_ROWS[1][1]);
  await narrowedUnderSlowFrame(control, "right");
  assert.ok(replyInStream(control));
  assert.notEqual(focused(control), control.el("reply-text"), "the control kept the focus in the box, so this test cannot see it lost");
  assert.equal(await movedOn(brokenScript('document.addEventListener("focusin", () => {\n  replyFocusTaken = false;\n});', "")), true,
    "the control that never forgets gave the list the focus too, so this test cannot see a focus taken back");
});

async function tallReplyColumnsPage(script = SCRIPT) {
  const { page, data } = await twoColumnServer(tallColumns, script);
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  return { page, data };
}

test("`#277 reply-in-column` with `#467 inline-send-in-place`: a reply sent from a column that does not go stays where it was written, with Retry, and the status line says so aloud", async () => {
  // `#277 reply-in-column` told the reader where the row with Retry was, at the foot of the column,
  // with the column's Newest chip as the way down. `#467 inline-send-in-place`: the row is drawn where
  // the reply's box was, under the message it answers, where the reader is; the status line still says
  // it aloud, and no chip leads anywhere.
  const { page } = await tallReplyColumnsPage();
  const left = page.el("scroll-area");
  const right = page.el("side-scroll");
  assert.ok(left.scrollHeight > left.clientHeight * 2 && right.scrollHeight > right.clientHeight * 2,
    "the columns are too short to stand part way up, so this proves nothing");
  assert.match(HTML_CODE, /<span id="status" role="status" aria-live="polite">/, "the status line is not a live region");
  for (const [column, row, area, jump, label, answer, said, pane] of [
    ["left", () => rowWithId(page, "200"), left, "jump-newest", "jump-newest-label", errorResponse(422, "chat_error", "Google Chat returned HTTP 400"),
      { status: /^Your reply was not sent\. Retry is under the message it answers\.$/ }, "pane-discord"],
    ["right", () => sideRow(page, "202"), right, "side-jump-newest", "side-jump-newest-label", errorResponse(502, "chat_error", "Google Chat returned HTTP 502"),
      { status: /^Your reply may not have been posted\. Check it under the message it answers\.$/ }, "side-pane"],
  ]) {
    area.scrollTop = 0;
    await area.dispatch("scroll");
    page.replyResponse = answer;
    await replyButton(row()).click();
    assert.equal(replyBoxAt(page), column);
    await page.el("reply-text").setValue("this will not go");
    await page.el("reply-send").click();
    await page.settle();
    assert.equal(replyBoxAt(page), "closed", `${column}: Send left the box in the column`);
    assert.equal(area.scrollTop, 0, `${column}: Send moved the column`);
    assert.equal(page.el("status-line").hidden, false, `${column}: the status line said nothing of the failure`);
    assert.match(page.el("status").textContent, said.status, `${column}: the status line said the wrong thing`);
    const held = inPlaceRows(page, pane).find((one) => one.descendants().some((node) => node.textContent === "this will not go"));
    assert.ok(held && outgoingRetry(held), `${column}: the reply that did not go is not where it was written, with Retry`);
    assert.ok(!page.el(jump).hasAttribute("data-unsent"), `${column}: the chip leads down to a row the reader is at`);
    assert.notEqual(page.el(label).textContent, "Reply not sent", `${column}: the chip names a reply drawn where the reader is`);
    await outgoingDismiss(held).click();
    await page.settle();
    page.replyResponse = undefined;
  }

  // THE CONTROL: nothing told. The walk above sees the status line and the chip only through it.
  const { page: control } = await tallReplyColumnsPage(brokenScript(
    "if (columnReplies.has(entry.id) && !job.stopped && !unchanged) reportColumnReply(entry);", ""));
  const area = control.el("scroll-area");
  area.scrollTop = 0;
  await area.dispatch("scroll");
  control.replyResponse = errorResponse(422, "chat_error", "Google Chat returned HTTP 400");
  await replyButton(rowWithId(control, "200")).click();
  await control.el("reply-text").setValue("this will not go");
  await control.el("reply-send").click();
  await control.settle();
  assert.doesNotMatch(control.el("status").textContent, /reply/i, "the control told the reader, so this test cannot tell");
  assert.equal(control.el("jump-newest-label").textContent, "Newest");
});

test("`#277 reply-in-column`: a reply sent from a column that the page sends by itself later says it is waiting and leaves the chip alone; sent, nothing more is said of it", async () => {
  const { page } = await tallReplyColumnsPage();
  const left = page.el("scroll-area");
  left.scrollTop = 0;
  await left.dispatch("scroll");
  await page.setOnline(false);
  await replyButton(rowWithId(page, "200")).click();
  await page.el("reply-text").setValue("when the wifi is back");
  await page.el("reply-send").click();
  await page.settle();
  assert.match(page.el("status").textContent, /^Your reply is not sent yet: this device is offline\./,
    "a reply held while offline was not said to be waiting");
  assert.notEqual(page.el("jump-newest-label").textContent, "Reply not sent", "a reply that goes by itself was called not sent");
  assert.equal(page.el("jump-newest").hasAttribute("data-unsent"), false);
  page.el("status").textContent = "";
  await page.setOnline(true);
  await page.settle();
  assert.equal(page.repliesPosted.at(-1).body.text, "when the wifi is back", "the held reply did not go when the device was online");
  assert.doesNotMatch(page.el("status").textContent, /your reply/i, "a reply that went was reported as one that did not");
  assert.equal(page.el("jump-newest").hasAttribute("data-unsent"), false);
});

test("`#277 reply-in-column`: Escape on the other column's list does what it does there, and leaves a reply open in the first as it is", async () => {
  const { page } = await replyColumnsPage();
  await replyButton(rowWithId(page, "200")).click();
  await page.el("reply-text").setValue("still writing on the left");
  await press(page, page.el("reply-text"), "Escape");
  await press(page, focused(page), "]");
  assert.ok(nodeWithinFixture(page.el("side-scroll"), focused(page)), `] went to ${focusName(page)}, not the right column's list`);
  await press(page, focused(page), "Escape");
  assert.equal(replyBoxAt(page), "left", "Escape on the right column's list closed the reply open on the left");
  assert.equal(page.el("reply-text").value, "still writing on the left");
  await press(page, focused(page), "[");
  await press(page, focused(page), "Escape");
  assert.equal(replyBoxAt(page), "closed", "Escape on the reply's own column did not put it down");
  assert.equal(draftMarkerOf(rowWithId(page, "200")).hidden, false, "the words were not kept as a draft");

  // THE CONTROL: Back that puts down whatever reply is open, wherever the keys are.
  const { page: control } = await replyColumnsPage(brokenScript(
    "else if (replyOnScreen() && replyKeysHere()) closeReply();",
    "else if (replyOnScreen()) closeReply();"));
  await replyButton(rowWithId(control, "200")).click();
  await control.el("reply-text").setValue("still writing on the left");
  await press(control, control.el("reply-text"), "Escape");
  await press(control, focused(control), "]");
  await press(control, focused(control), "Escape");
  assert.equal(replyBoxAt(control), "closed", "the control kept the reply, so this test cannot see it closed");
});

/** A page that opens the reply's box in the stream and leaves the column's own box drawn beside it. */
const TWO_BOXES = ['  for (const id of ["channel-composer", "side-composer"]) leftEl(id).hidden = true;\n', ""];

test("`#410 inline-reply-editor`: in one column Reply opens the box in the list under the message, the channel's box hidden; Cancel brings that box back, each draft kept where it was", async () => {
  const page = await scatteredPage();
  await page.el("channel-compose-text").setValue("for the whole channel");
  await page.el("channel-compose-text").dispatch("input", {});
  const area = page.el("scroll-area");
  const before = area.scrollTop;
  await replyButton(rowWithId(page, "600")).click();
  assert.equal(page.screen(), "main", "Reply went to another screen");
  assert.equal(replyBoxAt(page), "left", `the box is ${replyBoxAt(page)}, not in the list`);
  assert.equal(page.el("column-reply").parentNode, page.el("pane-discord"), "the box's place is not in the channel's pane");
  assert.equal(page.el("column-reply").hidden, false);
  assert.equal(page.el("channel-composer").hidden, true, "the channel's box is still drawn beside the reply's: two boxes");
  assert.deepStrictEqual(inlineAnchors(page), ["discord-log:600"], "the box is not drawn under the message answered");
  assert.equal(page.el("column-reply").hasAttribute("data-anchored"), true);
  assert.deepStrictEqual(shownIds(page), SCATTERED, "a message with no replies moved rows");
  // This fixture lays out no anchored box: the box is drawn at the list's foot, as where a browser has
  // no anchor positioning, and that is no reason to move the reader there. (Where it is drawn under its
  // row, tests/inline_reply_browser.py measures how little the list moves to show it.)
  assert.equal(area.scrollTop, before, "the list moved to a box drawn at its foot");
  assert.equal(focused(page), page.el("reply-text"), `the box did not take the focus; ${focusName(page)} has it`);
  await page.el("reply-text").setValue("half a thought");
  await page.el("reply-text").dispatch("input", {});
  await page.el("reply-cancel").click();
  assert.equal(replyBoxAt(page), "closed", "Cancel left the box in the list");
  assert.equal(page.el("column-reply").hidden, true);
  assert.equal(page.el("channel-composer").hidden, false, "Cancel did not bring the channel's box back");
  assert.deepStrictEqual(inlineAnchors(page), [], "Cancel left a row keeping room for a box");
  assert.equal(draftMarkerOf(rowWithId(page, "600")).hidden, false, "the reply's words were not kept as its draft");
  assert.equal(page.el("channel-compose-text").value, "for the whole channel", "the channel's draft was touched");
  assert.equal(area.scrollTop, before, "putting the reply down moved the list");
  await replyButton(rowWithId(page, "600")).click();
  assert.equal(page.el("reply-text").value, "half a thought", "the draft did not come back with the box");
  assert.equal(page.el("channel-compose-text").value, "for the whole channel");

  // THE CONTROL: the reply's box beside the column's own, two boxes. The walk above sees it.
  const control = newPage(new Map(), brokenScript(...TWO_BOXES));
  const data = scatteredThread();
  control.threadingSupported = true;
  control.threads = data.threads;
  await signIn(control);
  await showDiscord(control, data.messages);
  await replyButton(rowWithId(control, "600")).click();
  assert.equal(control.el("channel-composer").hidden, false, "the control hid the channel's box, so this test cannot see two");
});

test("`#410 inline-reply-editor`: in All Reply gathers the message's replies under it and opens the box under the last; Escape puts them back, and a sent reply leaves them gathered", async () => {
  const page = await scatteredPage();
  await replyButton(rowWithId(page, "601")).click();
  assert.deepStrictEqual(shownIds(page), GATHERED, "the replies were not gathered under the message answered");
  assert.deepStrictEqual(stackPlaces(page).map(([id]) => id), ["601", "602", "603", "604", "606", "607"],
    "the gathered replies are not on the bridge");
  assert.deepStrictEqual(inlineAnchors(page), ["discord-log:607"], "the box is not under the last gathered reply");
  await press(page, page.el("reply-text"), "Escape");
  assert.equal(replyBoxAt(page), "closed", "Escape in an empty box did not put the reply down");
  assert.deepStrictEqual(shownIds(page), SCATTERED, "putting the reply down did not put its replies back in time order");
  assert.deepStrictEqual(inlineAnchors(page), []);

  await replyButton(rowWithId(page, "601")).click();
  await page.el("reply-text").setValue("on it");
  await page.el("reply-send").click();
  await page.settle();
  assert.equal(replyBoxAt(page), "closed", "Send left the box in the list");
  assert.equal(page.el("channel-composer").hidden, false, "Send did not bring the channel's box back");
  assert.deepStrictEqual(shownIds(page).slice(0, 10), GATHERED.slice(0, 10), "Send put the gathered replies back");
  assert.equal(page.repliesPosted.at(-1).body.reply_to, "601", "the reply did not answer the message");
  // The next Reply, on a message nothing answers, puts that gathering back: one gathering at a time.
  await replyButton(rowWithId(page, "605")).click();
  assert.equal(shownIds(page).indexOf("602"), SCATTERED.indexOf("602"), "the gathering a sent reply left outlived the next Reply");
  await page.el("reply-cancel").click();

  // Negative control: a message nothing answers gathers nothing, and its box is under it.
  const quiet = await scatteredPage();
  await replyButton(rowWithId(quiet, "605")).click();
  assert.deepStrictEqual(shownIds(quiet), SCATTERED, "a message with no replies moved rows");
  assert.deepStrictEqual(inlineAnchors(quiet), ["discord-log:605"]);
});

test("`#410 inline-reply-editor`: on a thread's root in Main, Reply draws its thread's replies under it as rows of the list, bridged, and Cancel takes them away again", async () => {
  const page = await scatteredPage();
  await pickThread(page, "main");
  const main = shownIds(page);
  assert.ok(!main.includes("604") && !main.includes("606"), `the premise is Main without the thread's replies: ${main}`);
  await replyButton(rowWithId(page, "601")).click();
  const rows = shownIds(page);
  const at = rows.indexOf("601");
  assert.deepStrictEqual(rows.slice(at, at + 6), ["601", "602", "603", "604", "606", "607"],
    `the thread's replies are not gathered under its root: ${rows}`);
  const brought = page.el("discord-log").children.filter((li) => li.hasAttribute("data-gathered-thread")).map((li) => li.getAttribute("data-id"));
  assert.deepStrictEqual(brought, ["604", "606"], "the rows the thread brought are not told from Main's own");
  assert.ok(stackPlaces(page).some(([id]) => id === "604"), "the thread's replies are not on the bridge");
  assert.deepStrictEqual(inlineAnchors(page), ["discord-log:607"], "the box is not under the last of them");
  await page.el("reply-cancel").click();
  assert.deepStrictEqual(shownIds(page), main, "Cancel left the thread's replies in Main");
  assert.equal(page.el("discord-log").children.some((li) => li.hasAttribute("data-gathered-thread")), false);
});

test("`#410 inline-reply-editor`: one box — in two columns Reply in the thread on the right opens under the thread, its own box hidden, and Reply on the left moves the one box there", async () => {
  const { page } = await replyColumnsPage();
  await replyButton(sideRow(page, "201")).click();
  assert.equal(replyBoxAt(page), "right");
  assert.equal(page.el("side-reply").parentNode, page.el("side-pane"), "the right column's place is not in its thread");
  assert.deepStrictEqual(inlineAnchors(page), ["side-log:202"], "a reply to the thread's root is not under its whole thread");
  assert.deepStrictEqual([page.el("side-composer").hidden, page.el("channel-composer").hidden], [true, true],
    "a column's own box is still drawn beside the reply's");
  await page.el("reply-text").setValue("for the root");
  await page.el("reply-text").dispatch("input", {});
  await replyButton(rowWithId(page, "200")).click();
  assert.equal(replyBoxAt(page), "left", "Reply on the left did not move the box");
  assert.deepStrictEqual([page.el("column-reply").hidden, page.el("side-reply").hidden], [false, true], "two places hold a box");
  assert.deepStrictEqual([page.el("side-composer").hidden, page.el("channel-composer").hidden], [true, true],
    "a column's own box came back while the reply is open on the left");
  assert.deepStrictEqual(inlineAnchors(page), ["discord-log:200"]);
  assert.equal(draftMarkerOf(sideRow(page, "201")).hidden, false, "the first reply's words were not kept as its draft");
  await page.el("reply-cancel").click();

  // Reply on Main's root of the thread the right column shows: its replies are not drawn a second time
  // beside themselves, as N replies declines to in two columns.
  await replyButton(rowWithId(page, "201")).click();
  assert.equal(replyBoxAt(page), "left");
  assert.deepStrictEqual(page.el("discord-log").children.filter((li) => li.hasAttribute("data-gathered-thread")).map((li) => li.getAttribute("data-id")), [],
    "the thread shown on the right was drawn again under its root on the left");
  await page.el("reply-cancel").click();

  // A reply open on the right is the one box from the left column too: Ctrl+S there goes to it.
  await replyButton(sideRow(page, "202")).click();
  await press(page, page.el("scroll-area"), "ArrowDown");
  await press(page, focused(page), "s", { ctrlKey: true });
  assert.equal(focused(page), page.el("reply-text"), `Ctrl+S from the left column went to ${focusName(page)}, not the reply open on the right`);
});

test("`#410 inline-reply-editor`: the reply's box is a composer — Grow is the one choice, and Ctrl+S and m go to the box open in the list", async () => {
  assert.match(cssBlockIn(DESKTOP_QUERY, "#screen-main[data-composer-grow] #reply-text"), /field-sizing:\s*content/,
    "the reply's box does not grow with Grow ticked");
  assertMarkupContains("reply-compose", "reply-grow");
  const page = await scatteredPage();
  await replyButton(rowWithId(page, "600")).click();
  await page.el("reply-grow").setChecked(true);
  assert.equal(page.storage.get("vibe-talk.voice.composer-grow"), "1", "ticking the reply's Grow kept nothing");
  assert.equal(page.el("channel-compose-grow").checked, true, "the reply's Grow is a choice of its own");
  await press(page, page.el("scroll-area"), "s", { ctrlKey: true });
  assert.equal(focused(page), page.el("reply-text"), `Ctrl+S went to ${focusName(page)}, not the reply open in the list`);
  await press(page, page.el("reply-text"), "Escape");
  await press(page, page.el("scroll-area"), "m");
  assert.equal(focused(page), page.el("channel-compose-text"), `with the reply put down, m went to ${focusName(page)}`);
});

test("`#410 inline-reply-editor`: a reply open under a row unpacked among the cards goes, as a draft, when a card opens its thread", async () => {
  // From review: the reply is the left column's (`#407 unpack-row-parity`), drawn among the cards; a card
  // opening its thread hides the cards, and a reply left open there would be out of sight with both
  // columns' own boxes hidden.
  const { page } = await parityPage({ long: false });
  const area = page.el("side-scroll");
  area.focus();
  await mouseOnto(page, unpackedRow(page, "703"));
  await pressHere(page, "r");
  assert.equal(replyBoxAt(page), "right", "the premise is the reply open among the cards");
  await page.el("reply-text").setValue("about the unpacked row");
  await page.el("reply-text").dispatch("input", {});
  const card = page.el("side-cards").children.find((li) => li.hasAttribute("data-card"));
  await card.descendants().find((node) => node.hasClass("side-card-open")).click();
  await page.settle();
  assert.equal(replyBoxAt(page), "closed", "the reply was left open among the hidden cards");
  assert.deepStrictEqual([page.el("channel-composer").hidden, page.el("side-composer").hidden], [false, false],
    "a column's own box stayed hidden after the reply went");
  assert.equal(JSON.parse(page.storage.get("vibe-talk.voice.drafts") || "{}")["703"], "about the unpacked row",
    "the reply's words were not kept as its draft");
});

// --- `#315 removed-reply-target`: a reply to a message removed upstream -----------------------------
//
// Since `#308 live-reply-in-main` a removal from the stream took a message that arrived without its
// thread out of the unplaced set at once, while the store kept its bare copy until the read behind the
// removal landed. The reply route then took that copy for a message on the main channel, and a Reply to
// it was posted there, naming the removed message, with no read first; with the reads failing, that
// window had no end. A reply to a message removed upstream is now refused, and says so.

/** A negative control: the reply route taking a message removed upstream for whatever its copy says. */
const ROUTE_IGNORES_REMOVAL = [
  "  if (removedUpstream(message.id)) return { ...route, unknown: true, removed: true, note: REPLY_TO_REMOVED };\n", ""];

/**
 * The channel read in Main, on a server with a change record: the bridge's reply pushed with no thread
 * record (`bridgeReply`), and then — after `between`, if given — removed upstream, the stream saying so.
 * With `reads` "held", every read from the push on waits for `release`, answered as the server had it
 * when asked: the read behind the push, and so the one behind the removal, is still out. With "failing"
 * every read fails.
 */
async function removedBareReply(script, reads, between = null) {
  const { page, server } = await forwardPage({}, (p) => { p.servePlacing = true; }, script);
  const serve = page.timeline;
  /** @type {Array<() => void>} */
  const held = [];
  const failing = errorResponse(502, "chat_error", "history read timed out");
  let holding = true;
  page.timeline = (path, options) => {
    if (!holding) return serve(path, options);
    if (reads === "failing") return failing();
    const answer = Promise.resolve(serve(path, options)).then(async (response) => {
      const body = await response.text();
      return { ...response, text: async () => body };
    });
    return new Promise((resolve) => held.push(() => resolve(answer)));
  };
  const { first, pushed } = bridgeReply(page, server);
  await deliver(page, page.stream(), sseMessage(pushed));
  for (let i = 0; i < 3; i += 1) await page.settle();
  assert.ok(shownIds(page).includes("300"), "fixture: the pushed reply is not drawn on Main");
  if (between) await between(page);
  server.remove("300");
  server.edit("201", { thread: { ...first, reply_count: 1 } });
  await deliver(page, page.stream(), sseDelete("del-300", CHANNEL.id, "300"));
  for (let i = 0; i < 3; i += 1) await page.settle();
  assert.ok(shownIds(page).includes("300"), "fixture: the read behind the removal took the reply off Main already");
  const release = async () => {
    holding = false;
    while (held.length) held.shift()();
    // The read behind the removal waited for the read on the wire, polling for it every 50 ms.
    for (let i = 0; i < 6; i += 1) {
      await page.settle();
      page.expireTimers(50);
    }
  };
  return { page, server, first, release };
}

/** The reads made since the `from`th for a reply's own placing (`placeReplyTarget`). */
const replyPlacingReads = (page, from) => readsSince(page, from).filter(([, , reason]) => reason === "reply-place");

for (const reads of ["held", "failing"]) {
  test(`\`#315 removed-reply-target\`: Reply on a message removed upstream, the read behind the removal ${reads === "held" ? "still out" : "failing"}, says so, and Send posts nothing and reads nothing`, async () => {
    const run = async (script) => {
      const { page, release } = await removedBareReply(script, reads);
      distinctReplies(page);
      const from = page.timelineCalls.length;
      await replyButton(rowOf(page, "300")).click();
      await page.settle();
      const opened = { note: page.el("reply-destination").textContent };
      await page.el("reply-text").setValue("an answer to a message that is gone");
      await page.el("reply-send").click();
      for (let i = 0; i < 3; i += 1) await page.settle();
      const sent = { posted: page.repliesPosted.map((call) => sentWithoutKey(call.body)), placing: replyPlacingReads(page, from),
        state: page.el("reply-state").textContent, open: replyInStream(page), text: page.el("reply-text").value,
        outbox: outgoingRows(page).length };
      return { page, opened, sent, release };
    };
    const { page, opened, sent, release } = await run(SCRIPT);
    assert.match(opened.note, /was removed from the channel/, `Reply did not say the message was removed: ${opened.note}`);
    assert.deepStrictEqual(sent.posted, [], "a reply to a message removed upstream was posted");
    assert.deepStrictEqual(sent.placing, [], "Send read the channel for a message removed upstream");
    assert.match(sent.state, /^Not sent: the message you are answering was removed/);
    assert.equal(sent.open, true, "the refused reply put its box away");
    assert.equal(sent.text, "an answer to a message that is gone", "the refused reply lost its words");
    assert.equal(sent.outbox, 0, "the refused reply was queued, to hold up what is written after it");
    if (reads === "held") {
      // The read behind the removal lands at last: the message is gone from Main.
      await release();
      assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "the removed reply is still on Main");
    }

    // THE CONTROL, as #308 left it: the bare copy reads as a message on the main channel, and the reply
    // goes there, naming the removed message, with no read first.
    const control = await run(brokenScript(...ROUTE_IGNORES_REMOVAL));
    assert.deepStrictEqual(control.sent.posted, [{ text: "an answer to a message that is gone", reply_to: "300" }],
      "without the check the reply still did not go to the main channel, so this test cannot see it");
    assert.deepStrictEqual(control.sent.placing, []);
  });
}

test("`#315 removed-reply-target`: a reply open on a message as it is removed upstream says so at once, before the read behind the removal lands", async () => {
  const run = async (script) => {
    let before = null;
    const { page } = await removedBareReply(script, "held", async (p) => {
      await replyButton(rowOf(p, "300")).click();
      await p.settle();
      before = p.el("reply-destination").textContent;
    });
    return { page, before, after: page.el("reply-destination").textContent };
  };
  const { page, before, after } = await run(SCRIPT);
  assert.match(before, /not known yet/, "fixture: the reply did not wait to know where the message is");
  assert.match(after, /was removed from the channel/, `the open reply did not follow the removal: ${after}`);
  distinctReplies(page);
  await page.el("reply-text").setValue("too late");
  await page.el("reply-send").click();
  await page.settle();
  assert.deepStrictEqual(page.repliesPosted, [], "the reply was posted");

  // THE CONTROL: a removal that does not tell the open reply leaves it saying where it would go.
  const control = await run(brokenScript("  followReplyStore();\n  saveChannelScope();\n}", "  saveChannelScope();\n}"));
  assert.match(control.after, /not known yet/, "the open reply followed the removal anyway, so this test cannot see it");
});

test("`#315 removed-reply-target`: a reply sent before its message was removed, its placing read failing, is refused when it is placed again, with nothing posted and no read", async () => {
  const run = async (script) => {
    let from = 0;
    const { page } = await removedBareReply(script, "failing", async (p) => {
      distinctReplies(p);
      await replyQuietly(p, "300", "an answer sent as the message was taken back");
      await sendBehind(p, "a note to the main channel after it");
      assert.equal(outgoingRows(p)[0].getAttribute("data-send-state"), "retrying", "fixture: the placing read did not fail for a moment");
      from = p.timelineCalls.length;
    });
    // Its next try, after the removal.
    assert.ok(page.expireTimers(retryWait(0)) >= 1, "fixture: the reply was not to be placed again");
    for (let i = 0; i < 3; i += 1) await page.settle();
    const rows = outgoingRows(page);
    return { page, from, posted: page.repliesPosted.map((call) => sentWithoutKey(call.body)), rows: rows.map((row) => row.text()),
      state: rows.length ? rows[0].getAttribute("data-send-state") : null };
  };
  const { page, from, posted, rows, state } = await run(SCRIPT);
  assert.deepStrictEqual(posted, [], "a reply to a message removed upstream was posted, or the note went past it");
  assert.deepStrictEqual(replyPlacingReads(page, from), [], "the reply read the channel for a message removed upstream");
  assert.equal(state, "failed");
  assert.match(rows[0], /The message you answered was removed from the channel, so this was not posted anywhere\./);
  assert.match(rows[1], /goes first, and was not sent: the message it answers was removed\. Edit or Dismiss it to let this one go\./,
    `the note does not say what holds it: ${rows[1]}`);
  // Nor on its own later: a refusal is an answer.
  for (const wait of [1, 2, 3, 4].map(retryWait)) page.expireTimers(wait);
  await page.settle();
  assert.deepStrictEqual(page.repliesPosted, []);

  // THE CONTROL, as #308 left it: placed from the store's bare copy, the reply goes to the main channel
  // with no read first, and the note after it follows.
  const control = await run(brokenScript("  refuseRemoved();\n  const held = current() ? heldMessage(id) : null;",
    "  const held = current() ? heldMessage(id) : null;"));
  assert.deepStrictEqual(control.posted[0], { text: "an answer sent as the message was taken back", reply_to: "300" },
    "without the check the reply still did not go to the main channel, so this test cannot see it");
  assert.deepStrictEqual(replyPlacingReads(control.page, control.from), []);
});

test("`#315 removed-reply-target`: after a reload during the outage, a reply to the removed message is still refused", async () => {
  const run = async (script) => {
    const { page } = await removedBareReply(script, "failing");
    const again = newPage(page.storage, script, (p) => {
      p.threadingSupported = true;
      p.threads = page.threads;
      p.messages = page.messages;
      p.timeline = offline;
    });
    await reopenedChannel(again);
    assert.ok(rowOf(again, "300"), "fixture: the saved bare copy was not drawn after the reload");
    distinctReplies(again);
    await replyButton(rowOf(again, "300")).click();
    await again.settle();
    const note = again.el("reply-destination").textContent;
    await again.el("reply-text").setValue("still answering it");
    await again.el("reply-send").click();
    for (let i = 0; i < 3; i += 1) await again.settle();
    return { note, posted: again.repliesPosted.map((call) => sentWithoutKey(call.body)) };
  };
  const { note, posted } = await run(SCRIPT);
  assert.match(note, /was removed from the channel/, `after a reload Reply did not say the message was removed: ${note}`);
  assert.deepStrictEqual(posted, [], "after a reload the reply went to the main channel");

  // THE CONTROL: the removal not saved with the store, the reload takes the bare copy for a message on
  // the main channel.
  const control = await run(await brokenModule([{ file: "src/core/channels/saved.ts",
    find: "    removed: [...canon.removed.keys()].filter((id) => held.has(id)),\n", replace: "" }]));
  assert.deepStrictEqual(control.posted, [{ text: "still answering it", reply_to: "300" }],
    "without saving the removal the reply was still refused, so this test cannot see it");
});

// --- `#466 reply-promotes-thread`: the owner's reply that vanished --------------------------------
//
// The owner, on Slack: "the message I was replying to was a main-channel root message that was not yet
// a thread ... my sent message disappeared, and I didn't see it anywhere on the screen." Reproduced
// here before it was fixed: the stream's copy of the reply asked for a newest page of All, sent after
// the post and answered from state without the reply, which took it out as deleted upstream.

test("`#466 reply-promotes-thread`: a newest page answered from older state does not take away the reply just sent", async () => {
  const { page, data } = await freshMessagePage((p) => { p.repliesInThread = true; });
  assert.equal(page.el("thread-select").value, "flat", "the premise is All");
  const mine = message({ id: "9466000000000000001", content: "my answer to the fresh message",
    thread: { id: data.lone.id, root_message_id: "210", is_root: false, reply_count: null, reply_count_exact: false } });
  // Posted, but the service's reads do not show it yet: a bridge or a memory answering from older state.
  page.replyResponse = async () => json(200, { posted: mine, parts: [mine] });
  await replyButton(rowOf(page, "210")).click();
  await page.el("reply-text").setValue("my answer to the fresh message");
  await page.el("reply-send").click();
  await page.settle();
  assert.ok(shownIds(page).includes(mine.id), "the reply was not drawn as it was sent");
  const reads = page.timelineCalls.length;
  await deliver(page, page.stream(), sseMessage(mine, { self_posted: true }));
  await page.settle();
  assert.ok(page.timelineCalls.length > reads, "the premise is a read the stream's copy asks for");
  assert.ok(shownIds(page).includes(mine.id), "a page without the reply took it away");
  await reReadChannel(page);
  assert.ok(shownIds(page).includes(mine.id), "the next stale page took it away");
  await pickThread(page, "main");
  assert.equal(tagOf(rowOf(page, "210")).text, "Thread(1)", "the message answered went back to starting a thread");
  // A removal the page is told of still takes it away.
  await pickThread(page, "flat");
  await deliver(page, page.stream(), sseDelete("del-9466", CHANNEL.id, mine.id));
  await page.settle();
  assert.ok(!shownIds(page).includes(mine.id), "a reply removed upstream stayed drawn");
});

// --- `#466 reply-promotes-thread`: the thread a reply starts, drawn the moment it is sent ----------
//
// The owner: "I expected the main-channel message to be promoted to the root of a new thread, and that
// didn't visually happen, not locally and not quickly ... It would be great if promoting a root message
// to a thread and visually linking the sent message to it happened locally, without waiting for some
// asynchronous server refresh." On Slack every message on the channel heads a thread of its own nobody
// has answered (`#252 thread-button-state`), and a reply to it is posted in that thread. The page now
// counts a send still in the outbox into the thread it goes to, and shows it there, deriving both from
// the outbox at every pass and writing nothing into its copy of the message answered.

/** A Slack-like page on `freshMessageData`, Main showing, a reply to 210 held on its way. */
async function promotingPage() {
  const { page, data } = await freshMessagePage((p) => { p.repliesInThread = true; });
  await pickThread(page, "main");
  const mine = message({ id: "9466000000000000002", content: "the first answer to 210",
    thread: { id: data.lone.id, root_message_id: "210", is_root: false, reply_count: null, reply_count_exact: false } });
  const finish = delayedReply(page);
  await replyButton(rowOf(page, "210")).click();
  await page.el("reply-text").setValue("the first answer to 210");
  const sent = page.el("reply-send").click();
  await page.settle();
  return { page, data, mine, finish, sent };
}

const cardIdsOf = (page) => page.el("side-cards").children.map((card) => card.getAttribute("data-id"));

test("`#466 reply-promotes-thread`: a reply on its way makes the message it answers a thread, in Main, the Threads view and the thread", async () => {
  const { page, data, mine, finish, sent } = await promotingPage();
  assert.equal(page.repliesPosted.length, 1, "the premise is a reply on its way");
  // Main: the message answered is the root of a thread with one reply, in its thread's colour.
  const tag = tagOf(rowOf(page, "210"));
  assert.equal(tag.state, "open", `the message answered still offers to start a thread: ${JSON.stringify(tag)}`);
  assert.equal(tag.text, "Thread(1)");
  assert.notEqual(tag.hue, "", "the new thread has no colour");
  // The Threads view lists the thread it starts.
  await pickThread(page, "threads");
  assert.ok(cardIdsOf(page).includes(data.lone.id), `the Threads view has no card for the new thread: ${cardIdsOf(page)}`);
  // The thread itself shows the reply, still sending.
  await pickThread(page, "main");
  await threadBadge(rowOf(page, "210")).click();
  await page.settle();
  assert.equal(page.el("thread-select").value, `thread:${data.lone.id}`, "the tag did not open the new thread");
  const rows = [...outgoingRows(page)];
  assert.equal(rows.length, 1, "the thread does not show the reply on its way");
  assert.equal(rows[0].getAttribute("data-send-state"), "sending");
  assert.match(rows[0].text(), /the first answer to 210/);
  // Sent: the reply itself in the thread, once, and the count unchanged on Main.
  finish(json(200, { posted: mine, parts: [mine] }));
  await sent;
  await page.settle();
  assert.equal(shownIds(page).filter((id) => id === mine.id).length, 1, "the reply is not drawn once in its thread");
  assert.equal(outgoingRows(page).length, 0, "the reply is drawn twice, as itself and as its row in the outbox");
  await pickThread(page, "main");
  assert.equal(tagOf(rowOf(page, "210")).text, "Thread(1)", "the count changed as the reply landed");
  // Its card stays, held up by the reply now in the store, before any read of the Threads list names it.
  await pickThread(page, "threads");
  assert.ok(cardIdsOf(page).includes(data.lone.id), "the new thread's card went as the reply landed");
});

test("`#466 reply-promotes-thread`: a reply that was not sent keeps its thread, with Retry, until it is dismissed", async () => {
  const { page, data, finish, sent } = await promotingPage();
  finish(json(403, { error: "refused", detail: "You cannot post to this channel." }));
  await sent;
  await page.settle();
  assert.equal(tagOf(rowOf(page, "210")).text, "Thread(1)", "a reply not sent took its thread away at once");
  await threadBadge(rowOf(page, "210")).click();
  await page.settle();
  const row = outgoingRows(page)[0];
  assert.ok(row, "the reply not sent is not in its thread");
  assert.equal(row.getAttribute("data-send-state"), "failed");
  assert.ok(outgoingRetry(row), "the reply not sent offers no Retry in its thread");
  await outgoingDismiss(row).click();
  await page.settle();
  await pickThread(page, "main");
  assert.equal(tagOf(rowOf(page, "210")).state, "start", "dismissed, the reply still made the message a thread");
  assert.ok(data);
});

test("`#466 reply-promotes-thread`: in two columns the thread a reply on its way starts is a card on the right, and shows the reply there", async () => {
  const { page, data, mine, finish, sent } = await promotingPage();
  await twoColumns(page);
  assert.ok(cardIdsOf(page).includes(data.lone.id), `the right column has no card for the new thread: ${cardIdsOf(page)}`);
  assert.equal(tagOf(rowOf(page, "210")).text, "Thread(1)");
  await threadBadge(rowOf(page, "210")).click();
  await page.settle();
  const right = () => [...inPlaceRows(page, "side-pane"), ...page.el("side-outgoing-log").children];
  assert.equal(right().length, 1, "the thread on the right does not show the reply on its way");
  assert.match(right()[0].text(), /the first answer to 210/);
  finish(json(200, { posted: mine, parts: [mine] }));
  await sent;
  await page.settle();
  assert.equal(right().length, 0, "the reply stayed in the outbox on the right once it was there");
  assert.ok(sideIds(page).includes(mine.id), `the reply is not drawn in its thread on the right: ${sideIds(page)}`);
});

// --- `#467 inline-send-in-place` -------------------------------------------------------------------
//
// The owner: "The compose box jumps to where it should be, in situ in the thread... But then the
// little 'Sending..' box, the one that would show Retry if it fails, jumps back down to the BOTTOM,
// not where the compose box was."

/** The row of `pane`'s list named as the anchor an in-place row is drawn under, and that row's group. */
function inPlaceAnchor(page, pane = "pane-discord") {
  const group = page.el(pane).children.find((node) => node.hasClass && node.hasClass("outgoing-inplace"));
  if (!group) return null;
  const name = group.style.getPropertyValue("position-anchor");
  const list = page.el(pane).children.find((node) => node.tagName === "OL" && node.children.some((row) =>
    row.style.getPropertyValue("anchor-name") === name));
  const row = list ? list.children.find((one) => one.style.getPropertyValue("anchor-name") === name) : null;
  return { group, name, row };
}

for (const view of ["thread", "flat"]) {
  test(`\`#467 inline-send-in-place\`: in ${view === "thread" ? "a thread" : "All"}, a reply sent from the box in the stream stays where the box was until it is drawn there itself`, async () => {
    const page = await threadPage(view === "thread" ? "main" : "flat");
    if (view === "thread") {
      await threadBadge(page.el("discord-log").children[1]).click();
      await page.settle();
    }
    const target = view === "thread" ? "202" : "203";
    const finish = delayedReply(page);
    await replyButton(rowOf(page, target)).click();
    assert.ok(replyInStream(page), "the premise is the box in the stream");
    await page.el("reply-text").setValue("answered where I read it");
    const sent = page.el("reply-send").click();
    await page.settle();
    assert.equal(page.el("outgoing-log").children.length, 0, "the reply went to the foot of the list");
    const placed = inPlaceRows(page);
    assert.equal(placed.length, 1, "the reply is not drawn where its box was");
    assert.match(placed[0].text(), /answered where I read it/);
    assert.equal(placed[0].getAttribute("data-send-state"), "sending");
    const anchor = inPlaceAnchor(page);
    assert.ok(anchor && anchor.row, "the reply's row is named to no row of the list");
    assert.ok(anchor.row.hasAttribute("data-outgoing-anchor"), "the row it is under keeps no room for it");
    const rows = page.el("discord-log").children;
    assert.ok(rows.indexOf(anchor.row) >= rows.indexOf(rowOf(page, target)),
      "the reply is not under the message it answers, or what is gathered under it");
    const parent = page.messages.find((held) => held.id === target);
    const mine = message({ id: "9467000000000000001", content: "answered where I read it", reply_to: target,
      ...(parent.thread ? { thread: { ...parent.thread, is_root: false } } : {}) });
    page.messages = [...page.messages, mine];
    finish(json(200, { posted: mine }));
    await sent;
    await page.settle();
    assert.equal(inPlaceRows(page).length, 0, "the reply's row stayed beside the reply itself");
    assert.ok(shownIds(page).includes(mine.id), "the reply is not drawn");
    assert.equal(page.el("discord-log").children.filter((row) => row.hasAttribute("data-outgoing-anchor")).length, 0,
      "a row still keeps room for a reply that is drawn");
  });
}

test("`#467 inline-send-in-place`: a message sent from the box at the foot keeps its row at the foot", async () => {
  const page = await threadPage("main");
  delayedReply(page);
  await page.el("channel-compose-text").setValue("to the main channel");
  page.el("channel-send").click();
  await page.settle();
  assert.equal(inPlaceRows(page).length, 0, "a message from the foot was drawn under some row");
  assert.equal(page.el("outgoing-log").children.length, 1, "a message from the foot is not at the foot");
});

test("`#467 inline-send-in-place`: a reply that does not go stays where it was written, with Retry, and Retry sends it from there", async () => {
  const page = await threadPage("flat");
  page.replyResponse = errorResponse(422, "chat_error", "Google Chat returned HTTP 400");
  await replyButton(rowOf(page, "203")).click();
  await page.el("reply-text").setValue("not this time");
  await page.el("reply-send").click();
  await page.settle();
  const row = inPlaceRows(page)[0];
  assert.ok(row && outgoingRetry(row), "the reply that did not go is not where it was written, with Retry");
  page.replyResponse = errorResponse(422, "chat_error", "Google Chat returned HTTP 400");
  await outgoingRetry(row).click();
  await page.settle();
  assert.equal(page.repliesPosted.length, 2);
  assert.equal(inPlaceRows(page).length, 1, "Retry moved the reply's row");
});

test("`#466 reply-promotes-thread`: the stream's copy of a reply still on its way is counted once, not with its send", async () => {
  const { page, data, mine, finish, sent } = await promotingPage();
  assert.equal(tagOf(rowOf(page, "210")).text, "Thread(1)");
  const key = posted(page)[0].idempotency_key;
  await deliver(page, page.stream(), sseMessage(mine, { self_posted: true, client_key: key }));
  await page.settle();
  assert.equal(tagOf(rowOf(page, "210")).text, "Thread(1)", "the reply on its way was counted twice, as itself and its send");
  finish(json(200, { posted: mine, parts: [mine] }));
  await sent;
  await page.settle();
  assert.equal(tagOf(rowOf(page, "210")).text, "Thread(1)");
  assert.ok(data);
});

test("`#467 inline-send-in-place`: the row is drawn straight after the list, and not under the Pinned filter", async () => {
  const page = await threadPage("flat");
  delayedReply(page);
  await replyButton(rowOf(page, "203")).click();
  await page.el("reply-text").setValue("where it was written");
  page.el("reply-send").click();
  await page.settle();
  const pane = page.el("pane-discord").children;
  const at = pane.indexOf(page.el("discord-log"));
  assert.ok(pane[at + 1] && pane[at + 1].hasClass("outgoing-inplace"),
    "the reply's row is not straight after the list, where it is drawn without anchor positioning");
  await page.el("pinned-filter").click();
  await page.settle();
  assert.equal(inPlaceRows(page).length, 0, "the reply's row is drawn under the Pinned filter");
  await page.el("pinned-filter").click();
  await page.settle();
  assert.equal(inPlaceRows(page).length, 1, "the reply's row did not come back with the list");
});
