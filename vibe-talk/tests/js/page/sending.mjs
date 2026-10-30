// The app page's tests: sending: the send path, the order of posts, one post per message, and receipts.
//
// One feature's file of the suite tests/js/voice_page.test.mjs runs, against the fixture in
// ./fixture.mjs. Run with the rest of the suite, or alone: node --test tests/js/page/sending.mjs.

import assert from "node:assert/strict";

import { CHANNEL, CSS, RETRY_FIRST_MS, SEND_UNDO_MS, SLOW_SEND, THREADS_MAIN_STORAGE,
  UNPACK_STORAGE, cssBlock, cssRules, delayedReply, deliver, dismissedItems, errorResponse,
  forwardTimeline, freshMessagePage, heldRow, inPlaceRows, json, keyedChannel, message, mouseOnto,
  newPage, openReplyOn, outgoingActions, outgoingDismiss, outgoingEdit, outgoingRetry, outgoingRows,
  outgoingSpinner, parityData, parityPage, passTime, pickThread, posted, postedWhere, pressHere,
  putBackOf, reReadChannel, reloadWith, reopenedChannel, replyButton, replyInStream, retryWait,
  retryingFirst, rowOf, savedOutgoing, sendBehind, sendChannel, sentOk, settleSend, showDiscord,
  shownIds, signIn, sourceConstant, sseMessage, test, threadBadge, threadData, threadedService,
  unpackedRow } from "./fixture.mjs";

const RETRY_FLOOR_MS = sourceConstant("OUTGOING_RETRY_FLOOR_MS");

// --- one destination, in the order written -------------------------------------------------------
//
// `#277 send-order`. The owner: "I write A then B but B lands first." A's POST failed, A went to
// wait for its automatic retry, and B — queued behind A only while A's POST was on the wire — went
// at once; A's retry landed after it. These hold B behind A for as long as A is not sent, through
// its retries, a reload, and a refusal, and let it go when A lands, or is dismissed or edited back
// into its box. Every text here is invented.

/** A bridge's own word that a send failed, answered at once: the owner's failure. */
const BRIDGE_FAILED = 'Example Chat returned HTTP 502: {"message":"Example Chat message send failed"}';

/** Answer every POST to the reply route as its own new message in the channel it was posted to. */
function postedAsSent(page) {
  let sent = 0;
  page.replyResponse = async (path, options) => {
    const body = JSON.parse(options.body);
    sent += 1;
    const channel = decodeURIComponent(/\/channels\/([^/]+)\/reply(\?|$)/.exec(String(path))[1]);
    return json(200, {
      posted: message({ id: `92770000000000000${String(sent).padStart(2, "0")}`, channel_id: channel,
        content: body.text, ...(body.thread_id ? { thread: { id: body.thread_id, root_message_id: null,
          is_root: false, reply_count: null, reply_count_exact: false } } : {}) }),
    });
  };
}

const sentTexts = (page) => posted(page).map((body) => body.text);

test("`#277 send-order`: a message written while the one before it waits to retry waits behind it, and both land in the order written", async () => {
  const page = await keyedChannel();
  page.replyResponse = errorResponse(502, "chat_error", BRIDGE_FAILED);
  await sendChannel(page, "A, written first");
  assert.equal(outgoingRows(page)[0].getAttribute("data-send-state"), "retrying");
  assert.match(outgoingRows(page)[0].text(), retryingFirst("Sending… Example Chat could not answer: Example Chat message send failed\\. "));

  postedAsSent(page);
  await sendBehind(page, "B, written second");
  assert.deepEqual(sentTexts(page), ["A, written first"],
    "B went while A waited to go again, so B would land first");
  const behind = outgoingRows(page)[1];
  assert.equal(behind.getAttribute("data-send-state"), "queued");
  assert.match(behind.text(), new RegExp(
    `Waiting to send… “A, written first” goes first, and goes again in ${RETRY_FIRST_MS / 1000} s\\.`));
  assert.equal(outgoingSpinner(behind), undefined, "a message waiting its turn claims to be sending");
  assert.equal(savedOutgoing(page)[1].phase, "queued", "the outbox saved B as something other than waiting");

  assert.equal(page.expireTimers(RETRY_FIRST_MS), 1, "A's retry was not armed");
  await settleSend(page);
  await settleSend(page);
  assert.deepEqual(sentTexts(page), ["A, written first", "A, written first", "B, written second"]);
  assert.equal(posted(page)[1].idempotency_key, posted(page)[0].idempotency_key, "A's retry went under another key");
  assert.notEqual(posted(page)[2].idempotency_key, posted(page)[0].idempotency_key, "B went under A's key");
  assert.equal(outgoingRows(page).length, 0, "a receipt was left behind");
  const shown = shownIds(page);
  assert.ok(shown.indexOf("9277000000000000001") < shown.indexOf("9277000000000000002"),
    `the channel shows B before A: ${shown.join(", ")}`);
});

test("`#277 send-order`: three behind one that keeps failing wait in the order written, and drain in it", async () => {
  const page = await keyedChannel();
  page.replyResponse = errorResponse(502, "chat_error", BRIDGE_FAILED);
  await sendChannel(page, "one");
  for (const text of ["two", "three", "four"]) await sendBehind(page, text);
  assert.equal(page.repliesPosted.length, 1);
  assert.deepEqual(outgoingRows(page).map((row) => row.getAttribute("data-send-state")),
    ["retrying", "queued", "queued", "queued"]);
  // Each waits behind the first message not yet sent, which is the one that is failing.
  for (const row of outgoingRows(page).slice(1)) assert.match(row.text(), /“one” goes first/);

  // The first retry fails too: the queue keeps waiting, and is told the new wait.
  assert.equal(page.expireTimers(retryWait(0)), 1);
  await settleSend(page);
  assert.equal(page.repliesPosted.length, 2, "a message behind the failing one went");
  assert.match(outgoingRows(page)[3].text(), new RegExp(`goes again in ${retryWait(1) / 1000} s\\.`));

  postedAsSent(page);
  // `>=`: the status line's own timer can share the delay.
  assert.ok(page.expireTimers(retryWait(1)) >= 1, "the second retry was not armed");
  for (let step = 0; step < 4; step += 1) await settleSend(page);
  assert.deepEqual(sentTexts(page), ["one", "one", "one", "two", "three", "four"]);
  assert.equal(outgoingRows(page).length, 0);
});

test("`#277 send-order`: a reload keeps a message waiting behind the one before it, and it goes once that one is retried", async () => {
  const page = await keyedChannel();
  page.replyResponse = errorResponse(502, "chat_error", BRIDGE_FAILED);
  await sendChannel(page, "A, before the reload");
  await sendBehind(page, "B, behind it");
  assert.equal(page.repliesPosted.length, 1);
  const key = posted(page)[0].idempotency_key;

  const again = reloadWith(new Map(page.storage), [], (p) => {
    p.idempotentPostsSupported = true;
  });
  await signIn(again);
  await reopenedChannel(again);
  for (const wait of [0, 1, 2, 3].map(retryWait)) again.expireTimers(wait);
  await settleSend(again);
  assert.equal(again.repliesPosted.length, 0, "reopening the app sent a message on its own");
  const [first, second] = outgoingRows(again);
  assert.equal(first.getAttribute("data-send-state"), "unconfirmed");
  assert.equal(second.getAttribute("data-send-state"), "queued", "the reload forgot that B waits behind A");
  assert.match(second.text(),
    /Waiting to send… “A, before the reload” goes first, and its delivery is unconfirmed\. Retry or Dismiss it to let this one go\./);
  assert.deepEqual(outgoingActions(second), ["Edit", "Dismiss"],
    "a message waiting its turn offers to send out of turn, or cannot be taken back");

  postedAsSent(again);
  await outgoingRetry(first).click();
  await settleSend(again);
  await settleSend(again);
  assert.deepEqual(sentTexts(again), ["A, before the reload", "B, behind it"]);
  assert.equal(posted(again)[0].idempotency_key, key, "A's retry after the reload went under another key");
  assert.notEqual(posted(again)[1].idempotency_key, key);
  assert.equal(outgoingRows(again).length, 0);
});

test("`#277 send-order`: dismissing the message in front lets the next one go, and so does editing it back into the box", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, []);
  page.replyResponse = errorResponse(403, "refused", "You cannot post to this channel.");
  await sendChannel(page, "A, refused");
  postedAsSent(page);
  await sendBehind(page, "B, behind it");
  assert.deepEqual(sentTexts(page), ["A, refused"], "B went past a message that was not sent");
  const [refused, waiting] = outgoingRows(page);
  assert.equal(waiting.getAttribute("data-send-state"), "queued");
  assert.match(waiting.text(),
    /Waiting to send… “A, refused” goes first, and was not sent\. Retry, Edit or Dismiss it to let this one go\./);
  await outgoingDismiss(refused).click();
  await settleSend(page);
  assert.deepEqual(sentTexts(page), ["A, refused", "B, behind it"], "a Dismiss did not let B go");
  assert.equal(outgoingRows(page).length, 0);

  page.replyResponse = errorResponse(403, "refused", "You cannot post to this channel.");
  await sendChannel(page, "C, refused");
  postedAsSent(page);
  await sendBehind(page, "D, behind it");
  assert.equal(page.repliesPosted.length, 3, "D went past a message that was not sent");
  await outgoingEdit(outgoingRows(page)[0]).click();
  await settleSend(page);
  assert.deepEqual(sentTexts(page).slice(2), ["C, refused", "D, behind it"], "an Edit did not let D go");
  assert.equal(page.el("channel-compose-text").value, "C, refused", "the edited message is not in the box");
  assert.equal(outgoingRows(page).length, 0);
});

test("`#277 send-order`: a message waiting its turn can be taken back with Edit or Dismiss, but never sent out of turn", async () => {
  const page = await keyedChannel();
  page.replyResponse = errorResponse(502, "chat_error", BRIDGE_FAILED);
  await sendChannel(page, "A, failing");
  for (const text of ["B, to edit", "C, to dismiss", "D, kept"]) await sendBehind(page, text);
  const rowWith = (text) => outgoingRows(page).find((row) => row.text().includes(text));
  assert.deepEqual(outgoingActions(rowWith("B, to edit")), ["Edit", "Dismiss"], "no way to take back a waiting message");
  await outgoingEdit(rowWith("B, to edit")).click();
  await page.settle();
  assert.equal(page.el("channel-compose-text").value, "B, to edit", "Edit did not put the waiting message in the box");
  await outgoingDismiss(rowWith("C, to dismiss")).click();
  await page.settle();
  assert.equal(page.repliesPosted.length, 1, "taking a waiting message back sent something");
  assert.deepEqual(outgoingRows(page).map((row) => row.getAttribute("data-send-state")), ["retrying", "queued"]);
  assert.match(rowWith("D, kept").text(), /“A, failing” goes first/, "D stopped waiting behind A");

  postedAsSent(page);
  assert.equal(page.expireTimers(RETRY_FIRST_MS), 1);
  await settleSend(page);
  await settleSend(page);
  assert.deepEqual(sentTexts(page), ["A, failing", "A, failing", "D, kept"]);

  // Dismissed while it waited, it is kept as a message that was not sent, and goes only on Retry.
  page.expireTimers(SEND_UNDO_MS);
  await (await putBackOf(page, dismissedItems(page)[0])).click();
  await page.settle();
  const back = rowWith("C, to dismiss");
  assert.equal(back.getAttribute("data-send-state"), "failed");
  assert.match(back.text(), /Not sent\. Dismissed while it waited its turn, so it was not sent\./);
  assert.deepEqual(outgoingActions(back), ["Retry", "Edit", "Dismiss"]);
  await settleSend(page);
  assert.equal(page.repliesPosted.length, 3, "putting it back sent it");
});

test("`#277 send-order`: another thread, or another channel, does not wait for a message that is failing here", async () => {
  const page = newPage();
  const data = threadData();
  const other = { id: "2220000000000000277", label: "another channel", writable: true };
  page.channels.push(other);
  page.threadingSupported = true;
  page.threads = data.threads;
  page.idempotentPostsSupported = true;
  page.chatProviderName = "Example Chat";
  await signIn(page);
  await showDiscord(page, data.messages);
  await pickThread(page, "main");
  page.replyResponse = errorResponse(502, "chat_error", BRIDGE_FAILED);
  await sendChannel(page, "to Main, failing");
  assert.equal(outgoingRows(page)[0].getAttribute("data-send-state"), "retrying");

  postedAsSent(page);
  await threadBadge(page.el("discord-log").children[1]).click();
  await page.settle();
  await sendChannel(page, "into the thread");
  assert.deepEqual(postedWhere(page), [["to Main, failing", null], ["into the thread", data.threads[0].id]],
    "a thread waited for a message failing on Main");
  await page.el("thread-back").click();
  await page.settle();
  await sendBehind(page, "to Main, behind it");
  assert.equal(page.repliesPosted.length, 2, "a second message to Main went past the failing one");

  page.el("discord-channel").value = other.id;
  await page.el("discord-channel").dispatch("change");
  await page.settle();
  await sendChannel(page, "to another channel");
  assert.equal(page.repliesPosted.length, 3, "another channel waited for a message failing in this one");
  assert.ok(page.repliesPosted[2].path.includes(other.id));

  assert.equal(page.expireTimers(RETRY_FIRST_MS), 1);
  await settleSend(page);
  await settleSend(page);
  assert.deepEqual(sentTexts(page).slice(3), ["to Main, failing", "to Main, behind it"]);
});

test("`#277 send-order`: a send put back from Dismiss waits for the one already on its way, never beside it", async () => {
  const page = await keyedChannel();
  page.replyResponse = errorResponse(502, "chat_error", BRIDGE_FAILED);
  await sendChannel(page, "A, dismissed");
  await outgoingDismiss(outgoingRows(page)[0]).click();
  const finish = delayedReply(page);
  await sendBehind(page, "B, on its way");
  assert.equal(page.repliesPosted.length, 2);
  await page.el("undo-send-dismiss").click();
  await page.settle();
  // Back where it was written, before B, with its wait to go again: which runs out while B's POST
  // is still on the wire.
  assert.equal(page.expireTimers(RETRY_FIRST_MS), 1, "Undo did not put back A's wait");
  await page.settle();
  assert.equal(page.repliesPosted.length, 2, "two POSTs to one destination were on the wire at once");
  const held = outgoingRows(page).find((row) => /A, dismissed/.test(row.text()));
  assert.equal(held.getAttribute("data-send-state"), "queued");
  assert.match(held.text(), /Waiting to send… “B, on its way” goes first, and is still sending\./);

  finish(json(200, { posted: message({ id: "9277000000000000099", content: "B, on its way" }) }));
  postedAsSent(page);
  await settleSend(page);
  await settleSend(page);
  assert.deepEqual(sentTexts(page), ["A, dismissed", "B, on its way", "A, dismissed"]);
  assert.equal(posted(page)[2].idempotency_key, posted(page)[0].idempotency_key);
  assert.equal(outgoingRows(page).length, 0);
});

test("`#277 send-order`: an attempt that took longer to fail than its wait goes again after the floor, not a wait more", async () => {
  // A slow bridge: the attempt itself was the wait. Counted from when it started, the next try is
  // due already, and goes after the floor.
  const page = await keyedChannel();
  page.replyResponse = async () => {
    page.setClock(page.clock() + 25000);
    return json(502, { error: "chat_error", detail: SLOW_SEND });
  };
  await sendChannel(page, "slow to fail");
  assert.match(outgoingRows(page)[0].text(), new RegExp(`Retrying in ${RETRY_FLOOR_MS / 1000} s\\.`));
  // The next fails at once, and waits its own wait from when it started.
  page.replyResponse = errorResponse(502, "chat_error", BRIDGE_FAILED);
  assert.equal(page.expireTimers(RETRY_FLOOR_MS), 1, "the retry waited more than the floor");
  await settleSend(page);
  assert.equal(page.repliesPosted.length, 2);
  assert.match(outgoingRows(page)[0].text(), new RegExp(`Retrying in ${retryWait(1) / 1000} s\\.`));
});

test("`#277 send-order`: the rest of a post continuing in a thread holds a message written into that thread after it", async () => {
  // `#263 post-split-per-provider`: after a 207 the rest goes to the thread the first part started,
  // so a message written into that thread later waits for it, as it would behind one sent there.
  const page = newPage();
  const data = threadData();
  page.threadingSupported = true;
  page.threads = data.threads;
  page.idempotentPostsSupported = true;
  page.chatProviderName = "Example Chat";
  await signIn(page);
  await showDiscord(page, data.messages);
  await pickThread(page, "main");
  const thread = data.threads[0].id;
  page.replyResponse = async () => json(207, {
    error: "partially_posted", posted: 1, unsent: "the rest of A", detail: SLOW_SEND, retryable: true,
    resumable: true, thread_id: thread,
  });
  await sendChannel(page, "the start of A. the rest of A");
  assert.equal(savedOutgoing(page)[0].continueIn, thread);

  postedAsSent(page);
  await threadBadge(page.el("discord-log").children[1]).click();
  await page.settle();
  await sendBehind(page, "B, into the thread");
  assert.equal(page.repliesPosted.length, 1, "B went into the thread ahead of the rest of A");
  const behind = outgoingRows(page).find((row) => row.text().includes("B, into the thread"));
  assert.match(behind.text(), /Waiting to send… “the rest of A” goes first, and goes again in/);

  assert.ok(page.expireTimers(RETRY_FIRST_MS) >= 1, "the rest of A was not armed to go again");
  await settleSend(page);
  await settleSend(page);
  assert.deepEqual(postedWhere(page).slice(1), [["the rest of A", thread], ["B, into the thread", thread]]);
});

test("`#277 send-order`: a send put back goes where it was written and holds what came after it, and one dismissed while it waited to go again stays unconfirmed", async () => {
  const page = await keyedChannel();
  page.replyResponse = errorResponse(403, "refused", "You cannot post to this channel.");
  await sendChannel(page, "W, refused first");
  await outgoingDismiss(outgoingRows(page)[0]).click();
  await page.settle();
  page.setClock(page.clock() + 2000);
  page.replyResponse = errorResponse(502, "chat_error", BRIDGE_FAILED);
  await sendChannel(page, "V, lost second");
  assert.equal(outgoingRows(page)[0].getAttribute("data-send-state"), "retrying");

  // Undo puts W back where it was written, before V, so V going again waits behind it.
  await page.el("undo-send-dismiss").click();
  await page.settle();
  const rowWith = (text) => outgoingRows(page).find((row) => row.text().includes(text));
  assert.deepEqual(outgoingRows(page).map((row) => row.text().includes("W, refused first")), [true, false],
    "W was not put back where it was written");
  // Not awaited: the attempt it starts is over only once V has gone, and V waits behind W.
  outgoingRetry(rowWith("V, lost second")).click();
  await page.settle();
  assert.equal(page.repliesPosted.length, 2, "V went again past a message written before it");
  const waiting = rowWith("V, lost second");
  assert.equal(waiting.getAttribute("data-send-state"), "queued");
  assert.match(waiting.text(), /“W, refused first” goes first, and was not sent\. Retry, Edit or Dismiss it to let this one go\./);
  assert.deepEqual(outgoingActions(waiting), ["Dismiss"], "a message that may have landed offers Edit");

  // Dismissed while it waited, after a try that may have landed: kept as unconfirmed, never as unsent.
  await outgoingDismiss(waiting).click();
  page.expireTimers(SEND_UNDO_MS);
  await page.settle();
  await (await putBackOf(page, dismissedItems(page).find((item) => item.text().includes("V, lost second")))).click();
  await page.settle();
  const back = rowWith("V, lost second");
  assert.equal(back.getAttribute("data-send-state"), "unconfirmed");
  assert.match(back.text(), /Delivery unconfirmed\. Retrying cannot post it twice\. Dismissed while it waited its turn to go again, after a try whose delivery was not confirmed\./);
  assert.doesNotMatch(back.text(), /so it was not sent/);
  assert.deepEqual(outgoingActions(back), ["Retry", "Dismiss"]);
  assert.equal(page.repliesPosted.length, 2, "putting it back sent it");
});

// --- a confirmed send's receipt goes (#500 sent-receipts-pile-up) ---------------------------------------
//
// The owner, 2026-10-24, on Slack in the Threads view on a desk: "Some weird bug where messages I sent stack
// up in the 'Sent.' cards at the bottom of the screen, and they survive even the page restart." Five
// "You · Reply … Sent." receipts of thread replies, each with its Thread chip, at the foot of the cards over
// the main channel's box, long after they posted. A receipt went only once a READ carried its reply
// (`#466 reply-promotes-thread`), and none of this view's reads carries a thread's replies; and it was
// saved, so every reload drew it again.

const SENT_RECEIPT_MS = sourceConstant("SENT_RECEIPT_MS");

/** The Threads view as the owner had it: Main channel among the cards, threads with unread replies unpacked. */
async function ownersCards(storage = new Map()) {
  const data = parityData({ long: false });
  const page = newPage(new Map([[UNPACK_STORAGE, "unread"], [THREADS_MAIN_STORAGE, "on"], ...storage]));
  page.threadingSupported = true;
  page.threads = data.threads;
  page.messages = data.messages;
  const server = forwardTimeline(page);
  for (const id of ["700", "701", "702", "704", "705", "706"]) page.dealtWith.add(id);
  await signIn(page);
  await showDiscord(page, data.messages);
  await pickThread(page, "threads");
  await page.settle();
  return { page, data, server };
}

/** Reply `text` on message `id` from the box under its unpacked row; the server posts it in that thread. */
async function replyFromCards(page, server, data, id, text, postedId) {
  const answered = data.messages.find((m) => m.id === id);
  const mine = message({ id: postedId, content: text, reply_to: id,
    thread: { ...answered.thread, is_root: false, reply_count: null, reply_count_exact: false } });
  page.replyResponse = async () => json(200, { posted: server.post(mine) });
  await replyButton(unpackedRow(page, id)).click();
  await page.settle();
  assert.ok(replyInStream(page), `Reply on ${id} did not open among the cards`);
  await page.el("reply-text").setValue(text);
  await page.el("reply-send").click();
  await page.settle();
  await deliver(page, page.stream(), sseMessage(mine, { self_posted: true }));
  await page.settle();
  return mine;
}

/** The receipts drawn anywhere — at the foot, or in place under a row — that say `text`, by their state. */
const receiptsSaying = (page, text) => [...outgoingRows(page), ...inPlaceRows(page, "side-list")]
  .filter((li) => li.text().includes(text)).map((li) => li.getAttribute("data-send-state"));

test("a sent-receipt timer firing before its deadline rechecks the same deadline", async () => {
  const { page, data, server } = await ownersCards();
  const posted = await replyFromCards(page, server, data, "703", "receipt with an early timer", "7989");
  const confirmed = page.clock();
  page.setClock(confirmed + SENT_RECEIPT_MS - 1);
  page.expireTimers(SENT_RECEIPT_MS);
  await page.settle();
  assert.deepEqual(receiptsSaying(page, posted.content), ["sent"], "the receipt retired before its deadline");
  page.setClock(confirmed + SENT_RECEIPT_MS);
  assert.ok(page.expireTimers(1) > 0, "the early timer postponed its next check to the ten-minute grace");
  await page.settle();
  assert.deepEqual(receiptsSaying(page, posted.content), [], "the receipt survived its showing deadline");
});

test("`#500 sent-receipts-pile-up`: thread replies sent from the cards say Sent for a while, then go; none is saved, and a reload draws none while the replies stay", async () => {
  const { page, data, server } = await ownersCards();
  // The first reply: its thread is read now, and packs, so the reply is drawn nowhere in the view.
  const first = await replyFromCards(page, server, data, "703", "first answer from the cards", "7990");
  // The second, from a thread unpacked again for it, which packs again after.
  await page.el("side-unpack").click(); // Unpack unread -> Unpack all
  await page.settle();
  const second = await replyFromCards(page, server, data, "707", "second answer from the cards", "7991");
  await page.el("side-unpack").click(); // Unpack all -> Pack all
  await page.el("side-unpack").click(); // Pack all -> Unpack unread
  await page.settle();
  // Said, for the reader, with the thread it went into: the owner's cards, as they first stood.
  assert.deepEqual(receiptsSaying(page, first.content), ["sent"], "the first reply said nothing of being sent");
  assert.deepEqual(receiptsSaying(page, second.content), ["sent"], "the second reply said nothing of being sent");
  const shown = outgoingRows(page).find((li) => li.text().includes(first.content));
  assert.equal(threadBadge(shown)?.textContent, "Thread", "the receipt does not lead to the thread its reply is in");
  assert.deepEqual(savedOutgoing(page), [], "a confirmed send was saved, for a reload to draw again");
  // No read of this view carries either reply. Before the fix the receipts stayed for good.
  await passTime(page, SENT_RECEIPT_MS);
  assert.deepEqual(receiptsSaying(page, first.content), [], "the first reply's receipt outlived its showing");
  assert.deepEqual(receiptsSaying(page, second.content), [], "the second reply's receipt outlived its showing");
  assert.deepEqual(savedOutgoing(page), []);
  // The replies themselves are held, and drawn in their threads.
  await pickThread(page, `thread:${data.threads[0].id}`);
  await page.settle();
  assert.ok(shownIds(page).includes(first.id), "the first reply went with its receipt");
  await pickThread(page, "threads");
  await page.settle();
  // A reload: nothing to draw again, and the replies still in their threads.
  const again = reloadWith(new Map(page.storage), page.messages, (p) => {
    p.threadingSupported = true;
    p.threads = page.threads;
    forwardTimeline(p);
  });
  await signIn(again);
  await reopenedChannel(again);
  assert.equal(again.el("thread-select").value, "threads", "the reload did not come back to the Threads view");
  assert.deepEqual([...receiptsSaying(again, first.content), ...receiptsSaying(again, second.content)], [],
    "a reload drew a receipt again");
  await pickThread(again, `thread:${data.threads[1].id}`);
  await again.settle();
  assert.ok(shownIds(again).includes(second.id), "the second reply is not in its thread after the reload");
});

test("`#500 sent-receipts-pile-up`: the receipts an earlier build saved for sends long confirmed are dropped on load, and an unsent one is kept", async () => {
  // As the owner's device holds them: thread replies, each "sent", with its server parts, and nothing
  // saying when it was confirmed.
  const data = parityData({ long: false });
  const pile = ["703", "707", "703"].map((answers, at) => {
    const answered = data.messages.find((m) => m.id === answers);
    const part = message({ id: `7980${at}`, content: `an old answer ${at}`, reply_to: answers,
      thread: { ...answered.thread, is_root: false } });
    return { id: `outgoing-old-${at}`, channel: CHANNEL.id, text: part.content, remaining: part.content,
      state: "sent", threadId: answered.thread.id, replyTo: answers, originView: "main", createdAt: 1,
      postedCount: 0, parts: [part], observedParts: [], detail: "" };
  });
  const unsent = { id: "outgoing-not-sent", channel: CHANNEL.id, text: "still to go", remaining: "still to go",
    state: "failed", threadId: null, replyTo: null, createdAt: 1, postedCount: 0, parts: [], observedParts: [],
    detail: "refused" };
  const { page } = await ownersCards(new Map([["vibe-talk.outgoing-messages", JSON.stringify([...pile, unsent])]]));
  for (const entry of pile) {
    assert.deepEqual(receiptsSaying(page, entry.text), [], `the old receipt "${entry.text}" was drawn again`);
  }
  assert.deepEqual(savedOutgoing(page).map((entry) => entry.id), ["outgoing-not-sent"],
    "the saved outbox still holds the confirmed sends, or lost the one not sent");
  assert.deepEqual(receiptsSaying(page, "still to go"), ["failed"], "the send not sent is not offered again");
});

test("`#500 sent-receipts-pile-up`: a reply sent from the box under an unpacked row is drawn there while it goes, not at the foot of the cards", async () => {
  const { page, data, server } = await ownersCards(new Map([[UNPACK_STORAGE, "on"]]));
  const finish = delayedReply(page);
  const answered = data.messages.find((m) => m.id === "707");
  await replyButton(unpackedRow(page, "707")).click();
  await page.settle();
  await page.el("reply-text").setValue("on its way from the cards");
  const sent = page.el("reply-send").click();
  await page.settle();
  const placed = inPlaceRows(page, "side-list").filter((li) => li.text().includes("on its way from the cards"));
  assert.deepEqual(placed.map((li) => li.getAttribute("data-send-state")), ["sending"],
    "the reply going is not drawn under the row it answers");
  assert.deepEqual(page.el("outgoing-log").children.filter((li) => li.text().includes("on its way from the cards")), [],
    "the reply going is drawn at the foot of the cards as well");
  assert.ok(unpackedRow(page, "707").hasAttribute("data-outgoing-anchor"), "the row answered keeps no room under it");
  const mine = message({ id: "7992", content: "on its way from the cards", reply_to: "707",
    thread: { ...answered.thread, is_root: false, reply_count: null, reply_count_exact: false } });
  finish(json(200, { posted: server.post(mine) }));
  await sent;
  await page.settle();
  // Drawn among the cards now, in its thread: nothing more to stand in for (`#491`).
  assert.deepEqual(receiptsSaying(page, "on its way from the cards"), [], "the reply is drawn twice once sent");
});

test("`#500 sent-receipts-pile-up`: a reload in the grace keeps a reply the server does not list yet, its receipt no longer saved to draw it", async () => {
  // From review: the saved receipt was what drew the page's own post after a reload whose first read
  // came from older state. The post is kept now as the page that sent it kept it (`ownPostStands`).
  const run = async (forget) => {
    const { page, data } = await freshMessagePage((p) => { p.repliesInThread = true; });
    const mine = message({ id: "9500000000000000001", content: "said just before a reload",
      thread: { id: data.lone.id, root_message_id: "210", is_root: false, reply_count: null, reply_count_exact: false } });
    // Posted, but the service's reads do not show it yet, before the reload or after it.
    page.replyResponse = async () => json(200, { posted: mine, parts: [mine] });
    await replyButton(rowOf(page, "210")).click();
    await page.el("reply-text").setValue("said just before a reload");
    await page.el("reply-send").click();
    await page.settle();
    assert.ok(shownIds(page).includes(mine.id), "the premise: the reply is drawn as it is sent");
    assert.deepEqual(savedOutgoing(page), [], "a confirmed send was saved");
    const store = new Map(page.storage);
    if (forget) store.delete("vibe-talk.own-posts");
    const again = reloadWith(store, page.messages, (p) => {
      p.threadingSupported = true;
      p.threads = data.threads;
      p.repliesInThread = true;
      threadedService(p);
    });
    await signIn(again);
    await reopenedChannel(again);
    await reReadChannel(again);
    return shownIds(again).includes(mine.id);
  };
  assert.equal(await run(false), true, "a read from older state took the reply out after a reload");
  // Negative control: without what the page saved of its own posts, the same read takes it out.
  assert.equal(await run(true), false, "the premise: the reload's reads would take the reply out");
});

test("`#492 threads-row-widths`: an unpacked thread's rows reach the card's right edge, the owner's half an inset narrower, the reply as wide", () => {
  // The owner, 2026-10-22: the thread's rows came in on the right, his own were the wider ones, and the
  // reply box was wider than the rows it posts among. Chromium measures it; this pins the rules.
  const rows = cssBlock("#side-cards > li.discord-message.unpack-row[data-who]");
  assert.match(rows, /margin-right:\s*0;/, "an unpacked row keeps a right inset");
  assert.match(rows, /--row-inset-right:\s*0px/, "a line on an unpacked row stops short of the list's right edge");
  const own = cssBlock('#side-cards > li.discord-message.unpack-row[data-who="me"]:not([data-read-run="head"])');
  // `#524 thread-bridge-even-connectors`: the owner's rows begin at the thread's one left edge,
  // their narrowness the room they stop short on the right, so every connector is one length.
  assert.match(own, /margin-left:\s*var\(--unpack-row-left\);/,
    "the owner's unpacked rows do not begin at the thread's left edge");
  assert.match(own, /margin-right:\s*calc\(min\(15%, 6rem\) \/ 2\);/,
    "the owner's unpacked rows are not half the old inset narrower");
  assert.match(cssBlock("#side-list"), /--unpack-row-left:\s*calc\(var\(--speaker-inset\) \/ 2 \+ var\(--reply-gutter\)\)/,
    "the thread's indent is not the stack's own");
  assert.match(cssBlock("#side-list:has(> #side-cards > li.unpack-row[data-inline-reply]) > #side-reply[data-anchored]"),
    /left:\s*calc\(anchor\(--inline-reply-list left\) \+ var\(--unpack-row-left\)\)/,
    "a reply under an unpacked row is not as wide as the thread's rows");
  // The spine stays one line, and no bridge piece reaches further for the owner's rows.
  assert.equal(cssRules(CSS, '#side-cards > li.discord-message.unpack-row[data-who="me"]:not([data-read-run="head"]) > .reply-bridge').length,
    0, "the owner's unpacked rows still carry a wider bridge piece");
});

test("`#407 unpack-row-parity`: n goes to the next unread message over unpacked cards, a card standing for its unread replies", async () => {
  const { page } = await parityPage({ long: false });
  const area = page.el("side-scroll");
  area.focus();
  await mouseOnto(page, unpackedRow(page, "700"));
  await pressHere(page, "n");
  assert.equal(heldRow(page), "703", "n did not skip the read rows to the unread reply");
});

// --- one post per message, whatever the provider ------------------------------------------------
//
// `#448 send-dedupe`. The owner sent a long message to a Slack channel; it was cut in three parts and
// took twelve seconds. "That long message appeared to fail to send the first time, and then on retry
// appeared to succeed, but apparently that caused message duplication." The server now remembers each
// keyed post itself, and the page asks it what became of a send whose answer was lost before deciding
// anything. The provider name and every text here are invented.

const OUTGOING_PROGRESS_MS = sourceConstant("OUTGOING_PROGRESS_MS");
const OUTGOING_OUTCOME_WAIT_S = sourceConstant("OUTGOING_OUTCOME_WAIT_S");

/** A signed-in channel on a provider that promises nothing per key, as Slack does. */
async function unkeyedChannel() {
  const page = newPage();
  page.chatProviderName = "Other Chat";
  await signIn(page);
  await showDiscord(page, []);
  return page;
}

const gatewayLost = async () => ({ ok: false, status: 504, text: async () => "<html>Gateway Timeout</html>" });
const threeParts = (texts) => texts.map((content, at) => message({ id: `944800000000000000${at + 1}`, content }));

test("`#448 send-dedupe`: a send whose answer a gateway lost is confirmed from the server and never posted again", async () => {
  const page = await unkeyedChannel();
  const parts = threeParts(["part one. ", "part two. ", "part three"]);
  page.replyResponse = gatewayLost;
  page.outcomeResponse = async () => json(200, { posted: parts[0], parts });
  await sendChannel(page, "part one. part two. part three");
  assert.equal(page.repliesPosted.length, 1, "the page posted the message again");
  assert.equal(page.outcomeAsks.length, 1, "the page did not ask what became of the send");
  const asked = page.outcomeAsks[0];
  assert.match(asked.path, new RegExp(`/reply/outcome\\?wait=${OUTGOING_OUTCOME_WAIT_S}$`));
  assert.deepEqual(asked.body, posted(page)[0], "the ask did not name the post that was sent");
  assert.ok(asked.body.idempotency_key, "a send to a provider that promises nothing went without its key");
  assert.equal(outgoingRows(page).length, 0, `the send was left as a row: ${outgoingRows(page)[0]?.text()}`);
  assert.deepEqual(shownIds(page), parts.map((part) => part.id), "the parts the server posted are not drawn");
  page.expireTimers(60000);
  await settleSend(page);
  assert.equal(page.repliesPosted.length, 1, "something sent it again later");
});

test("`#448 send-dedupe`: a lost answer for a post still landing says which part is going, then Sent", async () => {
  const page = await unkeyedChannel();
  const parts = threeParts(["part one. ", "part two. ", "part three"]);
  page.replyResponse = gatewayLost;
  let finish;
  const answers = [
    async () => json(202, { state: "posting", posted: 1, parts: 3 }),
    () => new Promise((resolve) => { finish = resolve; }),
  ];
  page.outcomeResponse = () => answers.shift()();
  page.el("channel-compose-text").setValue("part one. part two. part three");
  const sent = page.el("channel-send").click();
  await page.settle();
  await page.settle();
  const row = outgoingRows(page)[0];
  assert.equal(row.getAttribute("data-send-state"), "sending");
  assert.match(row.text(), /Posting part 2 of 3…/);
  assert.equal(outgoingRetry(row), undefined, "a send still going offered Retry");
  finish(json(200, { posted: parts[0], parts }));
  await sent;
  await page.settle();
  assert.equal(page.repliesPosted.length, 1);
  assert.equal(page.outcomeAsks.length, 2);
  assert.equal(outgoingRows(page).length, 0);
});

test("`#448 send-dedupe`: a slow send says which part is being posted while its POST is on the wire", async () => {
  const page = await unkeyedChannel();
  const parts = threeParts(["part one. ", "part two. ", "part three"]);
  const finish = delayedReply(page);
  let progress = { state: "posting", posted: 0, parts: 3 };
  page.outcomeResponse = async () => json(202, progress);
  page.el("channel-compose-text").setValue("part one. part two. part three");
  const sent = page.el("channel-send").click();
  await page.settle();
  assert.match(outgoingRows(page)[0].text(), /^You.*Sending…✓✓$/, "the row claimed progress before any was known");
  assert.equal(page.expireTimers(OUTGOING_PROGRESS_MS), 1, "no ask of how far it has got was armed");
  await page.settle();
  assert.match(outgoingRows(page)[0].text(), /Posting part 1 of 3…/);
  progress = { state: "posting", posted: 2, parts: 3 };
  page.expireTimers(OUTGOING_PROGRESS_MS);
  await page.settle();
  assert.match(outgoingRows(page)[0].text(), /Posting part 3 of 3…/);
  assert.match(page.outcomeAsks[0].path, /\/reply\/outcome\?wait=0$/);
  assert.equal(page.outcomeAsks[0].body.idempotency_key, posted(page)[0].idempotency_key);
  finish(json(200, { posted: parts[0], parts }));
  await sent;
  await page.settle();
  const asks = page.outcomeAsks.length;
  page.expireTimers(OUTGOING_PROGRESS_MS);
  await page.settle();
  assert.equal(page.outcomeAsks.length, asks, "the page kept asking after the send was over");
  assert.equal(page.repliesPosted.length, 1);
  assert.equal(outgoingRows(page).length, 0);
});

test("`#448 send-dedupe`: Retry of a send the server has no record of carries the same key; a new message a new one", async () => {
  const page = await unkeyedChannel();
  page.replyResponse = async () => ({ ok: false, status: 502, text: async () => "<html>Bad Gateway</html>" });
  await sendChannel(page, "the same words");
  const row = outgoingRows(page)[0];
  assert.equal(row.getAttribute("data-send-state"), "unconfirmed");
  assert.match(row.text(), /Check history before retrying/);
  assert.equal(page.outcomeAsks.length, 1, "the page did not ask before leaving it to the reader");
  page.expireTimers(60000);
  await settleSend(page);
  assert.equal(page.repliesPosted.length, 1, "a send nobody can vouch for went again on its own");
  page.replyResponse = sentOk;
  await outgoingRetry(row).click();
  await settleSend(page);
  assert.equal(page.repliesPosted.length, 2);
  assert.deepEqual(posted(page)[1], posted(page)[0], "Retry was not the same request under the same key");
  // The same words written again are another message: a new key, and the server posts it.
  await sendChannel(page, "the same words");
  assert.equal(page.repliesPosted.length, 3);
  assert.equal(posted(page)[2].text, "the same words");
  assert.notEqual(posted(page)[2].idempotency_key, posted(page)[0].idempotency_key);
});

test("`#448 send-dedupe`: where a send goes is frozen by its first attempt, so a retry after its reply was moved into a thread asks for the same post", async () => {
  // A page that draws a reply into the thread it starts before the server's copy arrives (the
  // reply-promotes-thread work) may rewrite the entry from "reply to X" to "thread X". The key names
  // the post, and the retry must ask for the post the first attempt made, where it made it.
  const page = newPage();
  page.chatProviderName = "Other Chat";
  page.messages = [message({ id: "210", content: "nobody has answered this" })];
  await signIn(page);
  await openReplyOn(page, page.messages);
  page.replyResponse = async () => ({ ok: false, status: 502, text: async () => "<html>Bad Gateway</html>" });
  await page.el("reply-text").setValue("an answer");
  await page.el("reply-send").click();
  await page.settle();
  const first = posted(page)[0];
  assert.equal(first.reply_to, "210");
  assert.equal(savedOutgoing(page)[0].state, "unconfirmed");
  // The promotion, as a later page would make it, written where a reload reads it.
  const saved = savedOutgoing(page);
  Object.assign(saved[0], { replyTo: null, threadId: "210" });
  const store = new Map(page.storage);
  store.set("vibe-talk.outgoing-messages", JSON.stringify(saved));
  const again = reloadWith(store, page.messages);
  await signIn(again);
  await reopenedChannel(again);
  again.replyResponse = sentOk;
  await outgoingRetry(outgoingRows(again)[0]).click();
  await settleSend(again);
  assert.deepEqual(posted(again)[0], first, "the retry asked for the post somewhere else");
});

test("`#448 send-dedupe`: a gateway's JSON 5xx is a lost answer too, and the server is asked", async () => {
  const page = await unkeyedChannel();
  const parts = threeParts(["one. ", "two"]).slice(0, 2);
  page.replyResponse = async () => json(504, { message: "upstream timed out" });
  page.outcomeResponse = async () => json(200, { posted: parts[0], parts });
  await sendChannel(page, "one. two");
  assert.equal(page.outcomeAsks.length, 1, "a JSON gateway page was taken for the server's answer");
  assert.equal(page.repliesPosted.length, 1);
  assert.equal(outgoingRows(page).length, 0);
});

test("`#448 send-dedupe`: an ask that never answers ends, and the send is left unconfirmed rather than stuck", async () => {
  const page = await unkeyedChannel();
  page.replyResponse = gatewayLost;
  page.outcomeResponse = () => new Promise(() => {});
  page.el("channel-compose-text").setValue("down a dead connection");
  const sent = page.el("channel-send").click();
  await page.settle();
  assert.match(outgoingRows(page)[0].text(), /Slack is slow to confirm…/);
  page.expireTimers((OUTGOING_OUTCOME_WAIT_S + 10) * 1000);
  await sent;
  await page.settle();
  const row = outgoingRows(page)[0];
  assert.equal(row.getAttribute("data-send-state"), "unconfirmed", "the send stayed on an ask that never answered");
  assert.equal(page.repliesPosted.length, 1);
});

test("`#468 send-dedupe`: the stream's copy of a post made under a send's key settles it, unconfirmed, without posting", async () => {
  const page = await unkeyedChannel();
  const parts = threeParts(["landed after all"]).slice(0, 1);
  page.replyResponse = gatewayLost;
  await sendChannel(page, "landed after all");
  const row = outgoingRows(page)[0];
  assert.equal(row.getAttribute("data-send-state"), "unconfirmed", "the premise is a send nobody could vouch for");
  const key = posted(page)[0].idempotency_key;
  // Somebody else's key, or no key, settles nothing and asks nothing.
  await deliver(page, page.stream(), sseMessage(message({ id: "9448000000000000099", content: "other" }),
    { self_posted: true, client_key: "outgoing-not-this-page" }));
  await page.settle();
  assert.equal(page.outcomeAsks.length, 1, "a key this page does not hold was asked about");
  page.outcomeResponse = async () => json(200, { posted: parts[0], parts });
  await deliver(page, page.stream(), sseMessage(parts[0], { self_posted: true, client_key: key }));
  await settleSend(page);
  assert.equal(page.outcomeAsks.length, 2, "the echo did not ask what became of the send");
  assert.equal(page.outcomeAsks[1].body.idempotency_key, key);
  assert.equal(page.repliesPosted.length, 1, "settling it posted it again");
  assert.equal(outgoingRows(page).length, 0, `the send stayed: ${outgoingRows(page)[0]?.text()}`);
});

test("`#468 send-dedupe`: a send left unconfirmed by a closed page is asked about once when the page loads again", async () => {
  const page = await unkeyedChannel();
  page.replyResponse = gatewayLost;
  await sendChannel(page, "sent before the reload");
  const first = posted(page)[0];
  assert.equal(savedOutgoing(page)[0].state, "unconfirmed");
  const parts = threeParts(["sent before the reload"]).slice(0, 1);
  // The server has it: settled on load, and nothing posted.
  const found = reloadWith(new Map(page.storage), []);
  found.chatProviderName = "Other Chat";
  found.outcomeResponse = async () => json(200, { posted: parts[0], parts });
  await signIn(found);
  await settleSend(found);
  assert.equal(found.outcomeAsks.length, 1, "the reloaded page did not ask about its unconfirmed send");
  assert.deepEqual(found.outcomeAsks[0].body, first, "the ask did not name the post that was sent");
  assert.equal(found.repliesPosted.length, 0, "the reloaded page posted it");
  assert.equal(savedOutgoing(found)[0]?.state ?? "sent", "sent");
  // The server does not have it: left as it was, said as it was, for the reader.
  const unknown = reloadWith(new Map(page.storage), []);
  unknown.chatProviderName = "Other Chat";
  await signIn(unknown);
  await settleSend(unknown);
  assert.equal(unknown.outcomeAsks.length, 1);
  assert.equal(unknown.repliesPosted.length, 0);
  const saved = savedOutgoing(unknown)[0];
  assert.equal(saved.state, "unconfirmed");
  assert.ok(saved.detail, "the unconfirmed send lost what was said of it");
  assert.equal(saved.detail, savedOutgoing(page)[0].detail, "asking changed what is said of the send");
});
