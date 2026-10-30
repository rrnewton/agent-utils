// The app page's tests: Done and the to-do view, and Done's races with sends, undo and other views.
//
// One feature's file of the suite tests/js/voice_page.test.mjs runs, against the fixture in
// ./fixture.mjs. Run with the rest of the suite, or alone: node --test tests/js/page/done.mjs.

import assert from "node:assert/strict";

import { CHANNEL, CLEAR_ARMED_MS, CSS, CSS_CODE, DISCORD_POLL_MS, DONE_FLASH_FADE_MS,
  DONE_FLASH_HOLD_MS, GLYPHS, HOLD_MS, INBOX_NOTICE, OTHER_CHANNEL, SCRIPT, SCRIPT_CODE, backlog,
  brokenScript, brokenScriptEach, channelRow, chooseReadMode, conversation, cssBlock, cssRules,
  delayedReply, deliver, doneButton, elidedOf, errorResponse, focusName, focused, inboxChannel,
  inboxId, json, jumpSaysArrived, keyboardPageAtTop, listCardSays, longMessage, message, newPage,
  nodeIn, openReplyOn, openThree, outgoingRetry, outgoingRows, pagedTimeline, pickThread, press,
  pressCopy, pressHere, providerDown, reReadChannel, readChannel, readThroughButton, reloadWith,
  reopenedChannel, replyAndSend, replyButton, replyInStream, rowMore, rowMoreButton, rowMoreMenu,
  rowWithId, savedScopes, scopeKey, selectedId, serveByChannel, serverFloor, showDiscord, shownIds,
  sideIds, sideRow, signIn, sourceConstant, sseMessage, statusText, tallChannel, test, textOfRow,
  threadBadge, threadData, threadPage, timelineReads, turnTodoOn, twoColumnServer, twoColumns,
  unreadChip, unreadSaid, untilDeepLink, upstreamReadButton, viewOf, withLiveChannel } from "./fixture.mjs";

test("the to-do filter reads the filtered route, and leaving it reads the whole channel again", async () => {
  // A page that filtered the rows it already had would be filtering by a rule of its own, beside
  // the server's, and the two would drift the first time either changed.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog());
  assert.equal(page.todoReads, 0, "the channel view read the to-do route unasked");

  await turnTodoOn(page);
  assert.equal(page.todoReads, 1, "turning the filter on did not read the to-do list");
  assert.equal(page.el("todo-filter").getAttribute("aria-pressed"), "true");
  assert.equal(shownIds(page).length, 4);

  await page.el("todo-filter").click();
  await page.settle();
  assert.equal(page.el("todo-filter").getAttribute("aria-pressed"), "false");
  assert.equal(page.todoReads, 1, "leaving the filter read the to-do route again");
  assert.equal(shownIds(page).length, 4);
});

test("the page says, in the interface, that this read state is not Discord's", async () => {
  // `#61 unread-status`, said once and plainly rather than left to be found from a divergence: an
  // unread badge in the Discord app that will not clear, or a message this page still calls
  // undealt-with that he answered on his laptop. QUOTED from the server, so the page cannot come
  // to describe the posture differently from the server that enforces it.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog());
  assert.equal(page.el("inbox-note").hidden, true, "the notice is up before the mode is");

  await turnTodoOn(page);
  assert.equal(page.el("inbox-note").hidden, false, "nothing on screen says whose read state this is");
  assert.equal(
    page.el("inbox-note").textContent,
    INBOX_NOTICE,
    "the page did not carry the server's own statement through unchanged"
  );
  assert.ok(
    !SCRIPT_CODE.includes("written back to it"),
    "web/voice.js has its own copy of the read-state notice, which can drift from the server's"
  );
});

test("Done removes exactly the message it was pressed on, and tells the server which one", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog());
  await turnTodoOn(page);

  const rows = page.el("discord-log").children;
  await doneButton(rows[1]).click();
  await page.settle();

  assert.deepStrictEqual(
    page.dismissCalls,
    [{ messages: ["8000000000000000001"] }],
    "the page dismissed the wrong message, or dismissed by position rather than by id"
  );
  assert.deepStrictEqual(
    shownIds(page),
    ["8000000000000000000", "8000000000000000002", "8000000000000000003"],
    "the wrong row left the list"
  );
});

test("undo puts back exactly the set the server said it cleared", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog());
  await turnTodoOn(page);
  assert.equal(page.el("undo-dismiss").hidden, true, "an undo is offered before anything was done");

  await doneButton(page.el("discord-log").children[2]).click();
  await page.settle();
  assert.equal(page.el("undo-dismiss").hidden, false, "no way back from a dismissal");

  await page.el("undo-dismiss").click();
  await page.settle();
  assert.deepStrictEqual(
    page.restoreCalls,
    [{ messages: ["8000000000000000002"] }],
    "the undo restored something other than what was cleared"
  );
  assert.equal(shownIds(page).length, 4, "the undo did not bring the message back");
  assert.equal(page.el("undo-dismiss").hidden, true, "the undo is still offered after being taken");
});

test("...and an undo cannot be taken twice, however fast the second tap is", async () => {
  // The offer is withdrawn BEFORE the request goes out, not after it comes back. A reader on a
  // slow connection taps a chip that is still on screen, and a second restore of the same set
  // would be a request the server answers happily and that means nothing.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog());
  await turnTodoOn(page);
  await doneButton(page.el("discord-log").children[0]).click();
  await page.settle();

  const first = page.el("undo-dismiss").click();
  const second = page.el("undo-dismiss").click();
  await first;
  await second;
  await page.settle();
  assert.equal(page.restoreCalls.length, 1, "a doubled tap restored twice");
});

test("declaring bankruptcy says how many it will clear, and takes two taps to do it", async () => {
  // Bulk AND destructive. A single tap that emptied the backlog would be the one interaction on
  // this page nobody could recover from without noticing what had gone.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog(5));
  await turnTodoOn(page);

  const clear = page.el("clear-backlog");
  assert.equal(clear.hidden, false, "there is no way to declare bankruptcy at all");
  assert.match(clear.textContent, /5/, "the control does not say how much it is about to clear");

  await clear.click();
  await page.settle();
  assert.deepStrictEqual(page.dismissCalls, [], "the first tap cleared the backlog");
  assert.match(clear.textContent, /5\?/, "the first tap did not ask");
  assert.match(page.el("status").textContent, /Tap again/, "the first tap said nothing");

  await clear.click();
  await page.settle();
  assert.deepStrictEqual(
    page.dismissCalls,
    [{ through: "8000000000000000004", limit: readWindow(page) }],
    "bankruptcy did not go through the newest message as a boundary, in the window it displayed"
  );
  assert.deepStrictEqual(shownIds(page), [], "the backlog survived being declared bankrupt");
});

/** The `limit` the page last read the to-do list with, as a number. */
function readWindow(page) {
  const last = page.todoReadPaths[page.todoReadPaths.length - 1];
  assert.ok(last, "the page never read the to-do list");
  const asked = new URL(last, "https://example.invalid").searchParams.get("limit");
  assert.ok(asked, `the to-do read named no window at all: ${last}`);
  return Number(asked);
}

test("a bulk clear names the window it READ with, so it cannot clear what it never showed", async () => {
  // The worst failure this view has, and it is a two-sided fact: the server resolves `{through}`
  // against a window of its own, so a boundary sent without the window the client displayed is
  // resolved against the server's DEFAULT — and every message above the reader's page is marked
  // dealt with having never been on screen. Recoverable by the undo, but only for a reader who
  // noticed, and the whole point of this control is that it is used when they have stopped reading.
  //
  // The server half is in tests/todo.rs and tests/api.rs. What is asserted HERE is that the two
  // numbers this page sends are the SAME number: a page that read fifty and declared three, or
  // read three and declared nothing, is the defect whichever way round it is.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog(3));
  await turnTodoOn(page);
  await page.el("clear-backlog").click();
  await page.el("clear-backlog").click();
  await page.settle();

  assert.equal(page.dismissCalls.length, 1);
  assert.equal(
    page.dismissCalls[0].limit,
    readWindow(page),
    `the clear declared a window of ${page.dismissCalls[0].limit} over a list read with ` +
      `${readWindow(page)}: the server would resolve the boundary against messages this page ` +
      "never displayed"
  );
});

test("bankruptcy is undoable, and the undo restores every message it cleared", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog(5));
  await turnTodoOn(page);
  await page.el("clear-backlog").click();
  await page.el("clear-backlog").click();
  await page.settle();
  assert.equal(shownIds(page).length, 0);

  await page.el("undo-dismiss").click();
  await page.settle();
  assert.equal(page.restoreCalls.length, 1);
  assert.equal(
    page.restoreCalls[0].messages.length,
    5,
    "the undo restored a count rather than the set the server named"
  );
  assert.equal(shownIds(page).length, 5, "the whole backlog did not come back");
});

test("the arming lapses, so a stray later tap does not clear the backlog", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog(3));
  await turnTodoOn(page);
  await page.el("clear-backlog").click();
  await page.settle();
  assert.match(page.el("clear-backlog").textContent, /3\?/);

  assert.ok(page.expireTimers(CLEAR_ARMED_MS) > 0, "the arming never lapses at all");
  assert.match(
    page.el("clear-backlog").textContent,
    /Clear the backlog/,
    "the control stayed armed after its window"
  );
  await page.el("clear-backlog").click();
  await page.settle();
  assert.deepStrictEqual(page.dismissCalls, [], "a tap after the window cleared the backlog");
});

test("a background re-read while the filter is on stays filtered", async () => {
  // Every path that re-reads the channel goes through one function, so the mode is honoured in one
  // place. Without that, the 45-second poll would silently put the dealt-with messages back on
  // screen and the reader would work through them twice.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog(4));
  await turnTodoOn(page);
  await doneButton(page.el("discord-log").children[0]).click();
  await page.settle();
  assert.equal(shownIds(page).length, 3);

  page.expireTimers(DISCORD_POLL_MS);
  await page.settle();
  assert.equal(
    shownIds(page).length,
    3,
    "the background poll re-read the UNFILTERED channel and undid the filter"
  );
  assert.equal(page.el("inbox-note").hidden, false, "the poll took the read-state notice away");
});

test("the walk back is not offered over a filtered list", async () => {
  // The cursor belongs to the unfiltered channel. Stepping back with it would prepend unfiltered
  // rows into a filtered list — messages the reader has already dealt with, arriving above the
  // ones they have not, with nothing saying why.
  const page = newPage();
  await signIn(page);
  page.channelPage = async () =>
    json(200, {
      channel: CHANNEL,
      messages: backlog(4),
      has_more: true,
      next_before: "8000000000000000000",
    });
  await showDiscord(page, backlog(4));
  assert.equal(page.el("load-older").hidden, false, "the unfiltered walk is not offered either");

  await turnTodoOn(page);
  assert.equal(page.el("load-older").hidden, true, "a filtered list offered an unfiltered step back");
});

test("the to-do controls exist only where they mean something", async () => {
  const page = newPage();
  await signIn(page);
  assert.equal(page.el("inbox-note").hidden, true);
  assert.equal(page.el("clear-backlog").hidden, true);
  assert.equal(page.el("undo-dismiss").hidden, true);

  const rows = await showDiscord(page, backlog(2));
  assert.ok(doneButton(rows[0]), "a channel row has no way to say it has been dealt with");
  // It used to be HIDDEN here, on the reasoning that pressing it outside the filter would appear
  // to do nothing. That reasoning expired when the channel view started greying archived rows:
  // pressing it now has a visible effect exactly where the reader is looking, and hiding it would
  // leave the swipe as the only way in.
  assert.equal(
    doneButton(rows[0]).hidden,
    false,
    "the channel view hides the only way to archive that a keyboard can reach"
  );

  await turnTodoOn(page);
  assert.equal(doneButton(page.el("discord-log").children[0]).hidden, false);

  // An empty backlog is the one case where the bulk control has nothing to bulk.
  await doneButton(page.el("discord-log").children[0]).click();
  await doneButton(page.el("discord-log").children[0]).click();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), []);
  assert.equal(page.el("clear-backlog").hidden, true, "bankruptcy is offered over an empty list");
});

test("source-provider read marking is absent unless the server advertises it", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, backlog(2));

  assert.ok(upstreamReadButton(rows[0]), "the capability-controlled action has no stable control");
  // The menu itself stays: Copy text is always in it (`#198 copy-message-text`).
  assert.equal(rowMore(rows[0]).hidden, false, "the ⋯ menu holding Copy text is not offered");
  assert.equal(
    upstreamReadButton(rows[0]).hidden,
    true,
    "a provider that cannot mark read was offered a write that will fail"
  );
  assert.deepStrictEqual(page.upstreamReadCalls, []);
});

test("one ⋯ menu at a time, and a tap anywhere else or Escape closes it", async () => {
  // `#202 read-through-here`. The owner photographed two menus open at once.
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, backlog(3));
  await rowMoreButton(rows[0]).click();
  assert.equal(rowMoreMenu(rows[0]).hidden, false);
  await rowMoreButton(rows[1]).click();
  assert.equal(rowMoreMenu(rows[1]).hidden, false, "the second menu did not open");
  assert.equal(rowMoreMenu(rows[0]).hidden, true, "opening a second menu left the first open");
  assert.equal(rowMoreButton(rows[0]).getAttribute("aria-expanded"), "false");

  // A tap inside the open menu is not "elsewhere"...
  await page.documentEvent("pointerdown", { target: readThroughButton(rows[1]) });
  assert.equal(rowMoreMenu(rows[1]).hidden, false, "a tap inside the menu closed it");
  // ...a tap on another row is.
  await page.documentEvent("pointerdown", { target: rows[2] });
  assert.equal(rowMoreMenu(rows[1]).hidden, true, "a tap elsewhere left the menu open");

  await rowMoreButton(rows[2]).click();
  await page.documentEvent("keydown", { key: "Escape" });
  assert.equal(rowMoreMenu(rows[2]).hidden, true, "Escape left the menu open");
  // Every item in the menu is drawn the same, larger size — the two in the read group too, which
  // `#206 pin-message` put one level down, and the one item that is a link (`#409
  // open-upstream-link`) with them.
  assert.match(CSS_CODE, /\.row-more-menu > button,\s*\.row-more-menu > a,\s*\.row-more-group > button,\s*\.row-more-group > \.upstream-read-button\s*\{[^}]*min-height: 2\.75rem/);
});

test("Mark read through here greys this row and everything above it, and empties Hide read at the newest", async () => {
  // `#202 read-through-here`. The owner pressed the provider's read marker expecting the rows to go
  // grey here; this is that act.
  const page = newPage();
  await signIn(page);
  let rows = await showDiscord(page, backlog(4));
  await rowMoreButton(rows[1]).click();
  await readThroughButton(rows[1]).click();
  await page.settle();
  assert.deepStrictEqual(
    page.dismissCalls.map((call) => call.messages),
    [["8000000000000000000", "8000000000000000001"]],
    "it did not mark exactly this row and the ones above it"
  );
  assert.deepStrictEqual(page.upstreamReadCalls, [], "it wrote to a provider that does not offer it");
  assert.equal(page.el("status").textContent, "Marked 2 messages read.");

  // On the newest row with Hide read on: nothing is left.
  await turnTodoOn(page);
  rows = [...page.el("discord-log").children];
  const newest = rows[rows.length - 1];
  await rowMoreButton(newest).click();
  await readThroughButton(newest).click();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), [], "Hide read still shows rows after marking through the newest");
});

test("Mark read through here also moves the provider's marker where it can, in one short line", async () => {
  const page = newPage();
  page.upstreamReadMarkSupported = true;
  await signIn(page);
  const rows = await showDiscord(page, backlog(3));
  // Short on screen, under the group's "Mark read" caption; whole to a screen reader. `#206`.
  assert.equal(readThroughButton(rows[2]).textContent, "Through here · also in Discord");
  assert.equal(readThroughButton(rows[2]).getAttribute("aria-label"), "Mark read through here · also in Discord");
  await rowMoreButton(rows[2]).click();
  await readThroughButton(rows[2]).click();
  await page.settle();
  assert.equal(page.dismissCalls.length, 1);
  assert.deepStrictEqual(page.upstreamReadCalls, [{ message_id: "8000000000000000002" }]);
  assert.equal(page.el("status").textContent, "Marked 3 messages read here and in Discord.");
});

test("Copy text in the ⋯ menu copies the whole row's text, and says so", async () => {
  // `#198 copy-message-text`. Every constituent of a combined row, in order, as written.
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [
    message({ id: "8000000000000000200", content: "first half of a **long** answer",
      timestamp: "2026-08-19T04:31:00.000Z" }),
    message({ id: "8000000000000000201", content: "second half, with a path like a_b_c.txt",
      timestamp: "2026-08-19T04:31:01.000Z" }),
  ]);
  assert.equal(rows.length, 1, "the fixture did not produce one combined row");
  const copy = rows[0].descendants().find((node) => node.className === "row-copy-button");
  assert.ok(copy, "the ⋯ menu has no Copy text");
  assert.equal(copy.textContent, "Copy text");
  await rowMoreButton(rows[0]).click();
  await copy.click();
  await page.settle();
  assert.match(page.clipboard, /^first half of a \*\*long\*\* answer/, "the source text was not copied");
  assert.match(page.clipboard, /second half, with a path like a_b_c\.txt$/, "the combined row lost its tail");
  assert.equal(rowMoreMenu(rows[0]).hidden, true, "the ⋯ menu stayed open after copying");
  assert.equal(page.el("status").textContent, "Copied the message text.");

  // The Clipboard API refusing, the old way through a hidden textarea copies the same text, and gives
  // the focus back to what had it. `#296 copy-highlighted-message`.
  page.clipboardRefuses = true;
  page.clipboard = undefined;
  await rowMoreButton(rows[0]).click();
  await copy.click();
  await page.settle();
  assert.match(String(page.clipboard), /^first half of a \*\*long\*\* answer[\s\S]*a_b_c\.txt$/,
    "with the Clipboard API refusing, the textarea did not copy the text");
  assert.equal(page.el("status").textContent, "Copied the message text.");
  const held = page.document.activeElement;
  assert.notEqual(String(held && held.tagName), "textarea", "the hidden textarea kept the focus");
  assert.equal(page.document.body.children.length, 0, "the hidden textarea was left in the page");

  // Both refusing, it is said, never mistaken for a copy.
  page.execCopyRefuses = true;
  page.clipboard = undefined;
  await rowMoreButton(rows[0]).click();
  await copy.click();
  await page.settle();
  assert.equal(page.clipboard, undefined);
  assert.equal(page.el("status").textContent, "This browser would not copy to the clipboard.");
});

test("Open in the ⋯ menu links to the message's own address in its service, and is absent without one", async () => {
  // `#408 copy-upstream-link` as `#409 open-upstream-link` leaves it. The address is the
  // server's, carried on the message; the item is a real link to it, named for the message's
  // provider and sitting with the copy. A pointer copies the address from the link itself;
  // l still copies it for a keyboard, and Cmd+O (Ctrl+O off a Mac) opens it — Shift+O folds
  // a message away instead since `#480 fold-one-message-key`.
  const page = newPage();
  await signIn(page);
  const address = "https://discord.com/channels/900000000000000001/900000000000000002/8000000000000000300";
  const rows = await showDiscord(page, [
    message({ id: "8000000000000000300", content: "with a link", upstream_url: address,
      timestamp: "2026-08-19T04:31:00.000Z" }),
    message({ id: "8000000000000000301", content: "without one", author: "ada", author_is_bot: false,
      timestamp: "2026-08-19T04:36:00.000Z" }),
    // Addresses of no shape the server forms: the href is the one sink that is not plain text,
    // so neither is offered, to open or to copy.
    message({ id: "8000000000000000302", content: "with a script", author: "grace", author_is_bot: false,
      upstream_url: "javascript:alert(1)", timestamp: "2026-08-19T04:41:00.000Z" }),
    message({ id: "8000000000000000303", content: "with plain http", author: "hopper", author_is_bot: false,
      upstream_url: "http://discord.com/channels/1/2/3", timestamp: "2026-08-19T04:46:00.000Z" }),
  ]);
  assert.equal(rows.length, 4, "the fixture did not produce four rows");
  const items = rowMoreMenu(rows[0]).children;
  assert.deepStrictEqual(items.map((node) => node.className).slice(0, 4),
    ["row-copy-button", "row-open-link-button", "row-copy-link-button", "row-copy-deep-link-button"],
    "the link and its copies are not with Copy text at the menu's head");
  const link = rows[0].descendants().find((node) => node.className === "row-open-link-button");
  assert.equal(String(link.tagName).toUpperCase(), "A", "the item is not a real link");
  assert.equal(link.textContent, "Open in Discord");
  assert.equal(link.getAttribute("href"), address, "the link does not hold the message's own address");
  assert.equal(link.getAttribute("target"), "_blank", "the link does not open in a new tab");
  assert.equal(link.getAttribute("rel"), "noopener noreferrer", "the opened page would hold this one");
  assert.equal(link.getAttribute("aria-keyshortcuts"), "Control+O Meta+O", "a screen reader is not told the key");
  // `#461 upstream-link-native`: beside the link, its Copy as an item of its own, for the
  // contexts whose long-press or installed-app menu cannot be counted on to offer the
  // browser's copy-link-address. It says `l` to a screen reader, as the key makes the same copy.
  const copyLink = rows[0].descendants().find((node) => node.className === "row-copy-link-button");
  assert.ok(copyLink, "no Copy link item beside the link");
  assert.equal(copyLink.textContent, "Copy Discord link");
  assert.equal(copyLink.getAttribute("aria-keyshortcuts"), "L", "a screen reader is not told the Copy item's key");
  assert.equal(rows[1].descendants().find((node) => node.className === "row-copy-link-button"), undefined,
    "a message with no address was offered a copy of one");
  assert.equal(rows[1].descendants().find((node) => node.className === "row-open-link-button"), undefined,
    "a message with no address was offered a way to one");
  for (const row of rows.slice(2)) {
    assert.equal(row.descendants().find((node) => node.className === "row-open-link-button"), undefined,
      "an address of no formable shape was offered as a link");
  }

  // Choosing it shuts the menu and copies nothing: the opening is the browser's, on the link.
  await rowMoreButton(rows[0]).click();
  await link.click();
  await page.settle();
  assert.equal(rowMoreMenu(rows[0]).hidden, true, "the ⋯ menu stayed open after choosing the link");
  assert.equal(page.clipboard, undefined, "choosing the link copied the address");

  // The Copy item puts the same address on the clipboard and shuts the menu, as l does.
  await rowMoreButton(rows[0]).click();
  await copyLink.click();
  await page.settle();
  assert.equal(page.clipboard, address, "the Copy item did not copy the address");
  assert.equal(page.el("status").textContent, "Copied the link in Discord.");
  assert.equal(rowMoreMenu(rows[0]).hidden, true, "the ⋯ menu stayed open after the Copy item");
  page.clipboard = undefined;

  // Cmd+O presses the same link on the highlighted row: with the menu open, the press is
  // what shuts it. (The opening itself is the Chromium walk's to see.) The keys that follow act
  // on a row in sight, as they must: the reader is at the top of the list for them.
  page.el("scroll-area").scrollTop = 0;
  await rowMoreButton(rows[0]).click();
  const opened = await press(page, rows[0], "o", { ctrlKey: true });
  assert.equal(opened.defaulted, false, "Cmd+O was left to the browser on a row with a link");
  assert.equal(rowMoreMenu(rows[0]).hidden, true, "Cmd+O did not press the row's link");

  // l copies the same address on the highlighted row, and the ? list says both keys.
  page.clipboard = undefined;
  await press(page, rows[0], "l");
  assert.equal(page.clipboard, address, "l did not copy the link");
  assert.equal(page.el("status").textContent, "Copied the link in Discord.");
  const listed = page.el("keyboard-keys").descendants().filter((node) => node.tagName === "kbd").map((node) => node.textContent);
  assert.ok(listed.includes("l"), `the list of keys does not name l: ${listed.join(" | ")}`);
  assert.ok(listed.includes("Ctrl+O"), `the list of keys does not name Ctrl+O: ${listed.join(" | ")}`);
  const copyEntry = page.el("keyboard-keys").descendants().find((node) => node.tagName === "kbd" && node.textContent === "l");
  assert.match(copyEntry.parentNode.text(), /Copy its link in Discord/);
  const openEntry = page.el("keyboard-keys").descendants().find((node) => node.tagName === "kbd" && node.textContent === "Ctrl+O");
  assert.match(openEntry.parentNode.text(), /Open it in Discord/);
  // On the row with no address, l is nothing.
  page.clipboard = undefined;
  const event = await press(page, rows[1], "l");
  assert.equal(event.defaulted, true, "l was taken on a row with no link");
  assert.equal(page.clipboard, undefined);
  const dead = await press(page, rows[1], "o", { ctrlKey: true });
  assert.equal(dead.defaulted, true, "Cmd+O was taken on a row with no link");
  const scripted = await press(page, rows[2], "l");
  assert.equal(scripted.defaulted, true, "l was taken on a row whose address is a script");
  assert.equal(page.clipboard, undefined, "l copied an address of no formable shape");

  // In the menu itself the link is an item like the buttons: ↓ through the open menu lands on
  // it, and Space presses it rather than scrolling the list under the open menu.
  await rowMoreButton(rows[0]).click();
  assert.equal(rowMoreMenu(rows[0]).hidden, false, "the ⋯ did not open the menu");
  await press(page, rowMoreButton(rows[0]), "ArrowDown");
  await press(page, focused(page), "ArrowDown");
  assert.equal(focused(page), link, "↓ in the menu did not land on the Open link");
  let defaulted = false;
  await link.dispatch("keydown", { key: " ", preventDefault: () => { defaulted = true; } });
  assert.equal(defaulted, true, "Space on the Open link was left to scroll");
  await link.dispatch("keyup", { key: " ", preventDefault: () => {} });
  await page.settle();
  assert.equal(rowMoreMenu(rows[0]).hidden, true, "Space on the Open link did not press it");

  // A long press on it is the phone's own (its copy-link menu), not the row's press-and-hold
  // details: no hold timer is armed by a finger resting on the link.
  await rows[0].dispatch("pointerdown",
    { pointerId: 1, pointerType: "touch", clientX: 10, clientY: 10, target: link });
  assert.equal(page.expireTimers(HOLD_MS), 0, "a finger on the link armed the row's hold");
  assert.equal(rows[0].descendants().find((node) => node.className === "msg-details"), undefined,
    "a long press on the link opened the row's details");
});

test("`#481 slack-app-deep-link`: with the app chosen, Open takes the slack:// address and no tab; copies still copy the web address", async () => {
  // The app address is the server's, carried beside the web one; the choice is this device's,
  // kept in its own storage. Only the one shape is offered from it: anything else a message
  // carries there, the ordinary web address is opened instead — as it is by default, with
  // nothing chosen, and with a stored word that is neither choice.
  const address = "https://acme.slack.com/archives/C0123ABCD/p1700000060000200";
  const inApp = "slack://channel?team=T0123ABCD&id=C0123ABCD&message=1700000060.000200";
  const withLinks = (appUrl, n, minute) => message({
    id: `80000000000000004${n}0`, content: `with links ${n}`, upstream_url: address,
    ...(appUrl === undefined ? {} : { upstream_app_url: appUrl }),
    timestamp: `2026-08-19T04:${minute}:00.000Z`,
  });
  const openLinkOf = (row) => row.descendants().find((node) => node.className === "row-open-link-button");

  const page = newPage(new Map([["vibe-talk.voice.slack-open", "app"]]));
  await signIn(page);
  assert.equal(page.el("slack-open-in").value, "app", "the setting does not show the stored choice");
  const rows = await showDiscord(page, [
    withLinks(inApp, 0, "31"),
    withLinks("javascript:alert(1)", 1, "36"),
    withLinks("slack://channel?team=t0123abcd&id=C0123ABCD&message=1700000060.000200", 2, "41"),
    withLinks("https://acme.slack.com/archives/C0123ABCD/p1700000060000200", 3, "46"),
    withLinks(undefined, 4, "51"),
  ]);
  assert.equal(rows.length, 5, "the fixture did not produce five rows");
  const link = openLinkOf(rows[0]);
  assert.equal(link.getAttribute("href"), inApp, "the app was chosen and the link still opens the web address");
  assert.equal(link.getAttribute("target"), null, "the app address would open the blank tab it exists to avoid");
  assert.equal(link.getAttribute("rel"), null, "the app address carries a new-tab rel it does not open with");
  assert.equal(link.getAttribute("title"), `Open this message in Discord: ${inApp}`);
  for (const row of rows.slice(1)) {
    const web = openLinkOf(row);
    assert.equal(web.getAttribute("href"), address,
      "an app address of no formable shape (or none) was opened instead of the web address");
    assert.equal(web.getAttribute("target"), "_blank");
    assert.equal(web.getAttribute("rel"), "noopener noreferrer");
  }
  // Copying is for sending to anybody, app or no app: the web address, as l copies it.
  const copyLink = rows[0].descendants().find((node) => node.className === "row-copy-link-button");
  await rowMoreButton(rows[0]).click();
  await copyLink.click();
  await page.settle();
  assert.equal(page.clipboard, address, "the Copy item copied the app address nobody else can open");

  for (const stored of [undefined, "browser", "banana"]) {
    const store = stored === undefined ? new Map() : new Map([["vibe-talk.voice.slack-open", stored]]);
    const webPage = newPage(store);
    await signIn(webPage);
    assert.equal(webPage.el("slack-open-in").value, "browser",
      `a stored choice of ${JSON.stringify(stored)} is not shown as the browser`);
    const webRows = await showDiscord(webPage, [withLinks(inApp, 5, "31")]);
    const webLink = openLinkOf(webRows[0]);
    assert.equal(webLink.getAttribute("href"), address,
      `the app address was opened with ${JSON.stringify(stored)} stored, not the app chosen`);
    assert.equal(webLink.getAttribute("target"), "_blank");
    assert.equal(webLink.getAttribute("rel"), "noopener noreferrer");
  }
});

test("`#481 slack-app-deep-link`: CHOOSING THE APP SAVES THE CHOICE, AND IT SURVIVES A RELOAD", async () => {
  const page = newPage();
  await signIn(page);
  assert.equal(page.el("slack-open-in").value, "browser", "the setting does not start at the browser");
  page.el("slack-open-in").value = "app";
  await page.el("slack-open-in").dispatch("change");
  assert.match(page.el("slack-open-state").textContent, /Saved/);
  assert.match(page.el("slack-open-state").textContent, /goes to the app/);
  // Rows drawn from now on open in the app, and so do a reload's: same storage, new page.
  const address = "https://acme.slack.com/archives/C0123ABCD/p1700000060000200";
  const inApp = "slack://channel?team=T0123ABCD&id=C0123ABCD&message=1700000060.000200";
  const carried = () => message({ id: "8000000000000000500", content: "in the app",
    upstream_url: address, upstream_app_url: inApp, timestamp: "2026-08-19T04:31:00.000Z" });
  const rows = await showDiscord(page, [carried()]);
  assert.equal(rows[0].descendants().find((node) => node.className === "row-open-link-button").getAttribute("href"), inApp,
    "a row drawn after the choice still opens the web address");
  const again = reloadWith(page.storage, [carried()]);
  await signIn(again);
  assert.equal(again.el("slack-open-in").value, "app", "the choice was lost");
  const reloaded = await reopenedChannel(again);
  assert.equal(reloaded[0].descendants().find((node) => node.className === "row-open-link-button").getAttribute("href"), inApp,
    "a reload opens the web address again");
});

test("`#481 slack-app-deep-link`: a browser that refuses to store where links open says so", async () => {
  const page = newPage();
  await signIn(page);
  page.storage.set = () => page.storage;
  page.el("slack-open-in").value = "app";
  await page.el("slack-open-in").dispatch("change");
  assert.doesNotMatch(page.el("slack-open-state").textContent, /Saved/, "it claimed success");
  assert.match(page.el("slack-open-state").textContent, /refused to store/);
});

test("`#462 webapp-deep-links`: Copy vibe-talk link copies durable upstream identity, beside the service's own", async () => {
  const page = newPage(new Map(), SCRIPT, (p) => p.enableHistory());
  page.threadingSupported = true;
  const data = threadData();
  page.threads = data.threads;
  await signIn(page);
  const rows = await showDiscord(page, data.messages);
  // A reply's link opens its thread on it; a root's opens Main on it. Ids only — never text.
  const replyRow = rowWithId(page, "202");
  const copy = replyRow.descendants().find((node) => node.className === "row-copy-deep-link-button");
  assert.ok(copy, "no Copy vibe-talk link in the menu");
  assert.equal(copy.textContent, "Copy vibe-talk link");
  const items = rowMoreMenu(replyRow).children.map((node) => node.className);
  assert.ok(items.indexOf("row-copy-deep-link-button") > items.indexOf("row-copy-button"),
    `the vibe-talk copy is not with the other copies: ${items}`);
  await rowMoreButton(replyRow).click();
  await copy.click();
  await page.settle();
  const link = page.clipboard;
  assert.equal(typeof link, "string", "the Copy item copied nothing");
  assert.ok(link.startsWith("https://vibe.example/?"), `the link is not this app's address: ${link}`);
  const query = Object.fromEntries(link.split("?")[1].split("&").map((pair) => {
    const at = pair.indexOf("=");
    return [pair.slice(0, at), decodeURIComponent(pair.slice(at + 1))];
  }));
  assert.equal(query.c, CHANNEL.id, "the link names another channel");
  assert.equal(query.v, "thread");
  assert.equal(query.t, "spaces/A/threads/one & two", "the link does not name the reply's thread");
  const ref = query.m.split(":").map((part) => decodeURIComponent(part));
  assert.deepStrictEqual(ref, ["discord", CHANNEL.id, "202", "spaces/A/threads/one & two"],
    "the link does not carry provider, channel, message and thread");
  assert.ok(!link.includes("first thread answer"), "the link carries message text");
  assert.equal(page.el("status").textContent, "Copied the vibe-talk link.");

  // The pure round-trip: a ref out and back is itself, separators inside ids included.
  const pure = GLYPHS.deepLinkRefFrom(GLYPHS.deepLinkRefText(
    { provider: "google chat", channel: "spaces/A", message: "spaces/A/messages/x.y", thread: "spaces/A/threads/one & two" }));
  assert.deepStrictEqual(pure,
    { provider: "google chat", channel: "spaces/A", message: "spaces/A/messages/x.y", thread: "spaces/A/threads/one & two" });
  assert.equal(GLYPHS.deepLinkRefFrom("garbage"), null, "a malformed ref was taken for one");

  // And the address follows the reader: opening that thread writes its place, state untouched.
  await pickThread(page, "thread:spaces/A/threads/one & two");
  await page.settle();
  const last = page.history.addresses[page.history.addresses.length - 1] || "";
  assert.match(last, /^\/?\?c=1110000000000000001&v=thread&t=spaces%2FA%2Fthreads%2Fone%20%26%20two$/,
    `the address does not name the open thread: ${last}`);
});

test("`#462 webapp-deep-links`: a link opens its view and thread over the restored state, and lands on its message", async () => {
  const data = threadData();
  const page = newPage(new Map(), SCRIPT, (p) => {
    p.enableHistory();
    p.setAddress({ search: "?c=1110000000000000001&v=thread&t=spaces%2FA%2Fthreads%2Fanother" +
      "&m=discord%3A1110000000000000001%3A204%3Aspaces%252FA%252Fthreads%252Fanother" });
  });
  page.threadingSupported = true;
  page.threads = data.threads;
  await signIn(page);
  await showDiscord(page, data.messages);
  await untilDeepLink(page, () => !page.el("thread-heading").hidden && rowWithId(page, "204")?.getAttribute("data-landed") === "true");
  assert.equal(page.el("thread-heading").hidden, false, "the link did not open its thread");
  assert.equal(page.el("thread-select").value, "thread:spaces/A/threads/another");
  const landed = rowWithId(page, "204");
  assert.ok(landed, "the linked message is not drawn");
  assert.equal(landed.getAttribute("data-landed"), "true", "the linked message was not landed on");
  assert.match(statusText(page), /Here is the linked message/);
  // The address still names it, so a reload or a copy of the address bar lands there again.
  // Read as the page computes it: this fixture's location does not move when the page
  // replaces the address, as a browser's does, so the written list lags the standing one.
  assert.match(page.pure("deepLinkQuery(channelPlace(), deepLinkFocus)"), /&m=discord%3A/,
    "the address let go of its message");
  assert.ok(page.pure("deepLinkFocus !== null"), "the focus a link brought was dropped");
});

test("`#462 webapp-deep-links`: a reader who chooses a view while the link is still landing is not chased by it", async () => {
  const data = threadData();
  const page = newPage(new Map(), SCRIPT, (p) => {
    p.enableHistory();
    p.setAddress({ search: "?c=1110000000000000001&v=thread&t=spaces%2FA%2Fthreads%2Fanother" });
  });
  page.threadingSupported = true;
  page.threads = data.threads;
  await signIn(page);
  await showDiscord(page, data.messages);
  // The reader picks All before the link's waits have run out: their choice stands.
  page.el("thread-select").value = "flat";
  await page.el("thread-select").dispatch("change");
  await untilDeepLink(page, () => page.pure("deepLinkApplied") === true);
  assert.equal(page.el("thread-select").value, "flat", "the link overrode the reader's own choice");
  assert.equal(page.el("thread-heading").hidden, true, "the link opened its thread over the reader");
});

test("`#462 webapp-deep-links`: a reader who chooses another channel while the link is landing keeps it", async () => {
  const data = threadData();
  const second = { id: "1110000000000000002", label: "second", writable: true };
  const page = newPage(new Map(), SCRIPT, (p) => {
    p.enableHistory();
    p.setAddress({ search: "?c=1110000000000000001&v=thread&t=spaces%2FA%2Fthreads%2Fanother" });
  });
  page.threadingSupported = true;
  page.threads = data.threads;
  page.channels = [{ ...CHANNEL }, second];
  await signIn(page);
  await showDiscord(page, data.messages);
  page.el("discord-channel").value = second.id;
  await page.el("discord-channel").dispatch("change");
  await untilDeepLink(page, () => page.pure("deepLinkApplied") === true);
  assert.equal(page.el("discord-channel").value, second.id,
    "the link moved the reader back to its own channel");
  assert.equal(page.el("thread-heading").hidden, true, "the link opened its thread over the reader");
});

test("`#462 webapp-deep-links`: each view round-trips, voice is refused politely, garbage is ignored", async () => {
  const cases = [
    ["main", "main"], ["all", "flat"], ["threads", "threads"],
  ];
  for (const [said, picker] of cases) {
    const data = threadData();
    const page = newPage(new Map(), SCRIPT, (p) => {
      p.enableHistory();
      p.setAddress({ search: `?c=${CHANNEL.id}&v=${said}` });
    });
    page.threadingSupported = true;
    page.threads = data.threads;
    await signIn(page);
    await showDiscord(page, data.messages);
    await untilDeepLink(page, () => page.el("thread-select").value === picker);
    assert.equal(page.el("thread-select").value, picker, `v=${said} opened ${page.el("thread-select").value}`);
    assert.match(page.pure("deepLinkQuery(channelPlace(), deepLinkFocus)"),
      new RegExp(`(^|&)v=${said}(&|$)`), `the address does not say ${said} back`);
  }
  // A list opens with its cards. (Round-trip back out of it is ↓'s walk in the back tests.)
  // Voice: never a call from a link.
  const voice = newPage(new Map(), SCRIPT, (p) => {
    p.enableHistory();
    p.setAddress({ search: `?c=${CHANNEL.id}&v=voice` });
  });
  voice.threadingSupported = true;
  voice.threads = threadData().threads;
  await signIn(voice);
  await showDiscord(voice, threadData().messages);
  await untilDeepLink(voice, () => /never start a call/.test(statusText(voice)));
  assert.equal(voice.tab(), "discord", "a voice link left the channel view");
  assert.match(statusText(voice), /never start a call/, "a voice link was refused in silence");
  // Garbage: opened as a plain open, on the restored/default view (All, with threads).
  const junk = newPage(new Map(), SCRIPT, (p) => {
    p.enableHistory();
    p.setAddress({ search: "?c=%&v=nope&m=garbage&t=" });
  });
  junk.threadingSupported = true;
  junk.threads = threadData().threads;
  await signIn(junk);
  await showDiscord(junk, threadData().messages);
  await junk.settle();
  assert.equal(junk.tab(), "discord", "a garbage link broke the open");
  assert.equal(junk.el("thread-select").value, "flat", "a garbage link chose a view");
});

test("`#462 webapp-deep-links`: a linked message further back than the loaded page is walked back to", async () => {
  const page = newPage(new Map(), SCRIPT, (p) => {
    p.enableHistory();
    p.setAddress({ search: `?c=${CHANNEL.id}&m=discord%3A${CHANNEL.id}%3A5001` });
  });
  const all = pagedTimeline(page, { steps: 4, size: 4 });
  await signIn(page);
  await showDiscord(page, []);
  await untilDeepLink(page, () => Boolean(rowWithId(page, all[1].id)));
  const landed = rowWithId(page, all[1].id);
  assert.ok(landed, "the linked message was never walked back to");
  assert.equal(landed.getAttribute("data-landed"), "true", "the walk did not land on the linked message");
  assert.match(statusText(page), /Here is the linked message/);
  const older = timelineReads(page).filter((path) => /[?&]before=/.test(path));
  assert.ok(older.length >= 3, `the walk read ${older.length} older pages for a message three pages back`);
});

// --- the to-do list while things are still arriving ----------------------------------------------
//
// The view is a filtered list of what has not been dealt with, and it is READ once per act. But the
// channel does not stop while the reader works through it: a message can arrive on the live stream,
// and a background poll can bring one in off screen. Both change what the list IS, and everything
// that DESCRIBES the list has to move with it — the count on the bulk control, the head of the
// list, and the chip that says something turned up. A view that draws a new row while its own head
// still says "nothing left to deal with" is worse than one that missed the row: it is a false
// statement with the counter-example visible directly beneath it.

/** A to-do backlog of rows tall enough to scroll, so "off screen" means something. */
const tallBacklog = (count) =>
  Array.from({ length: count }, (_unused, i) =>
    message({ id: `90000000000000${String(i).padStart(4, "0")}`, content: longMessage(`todo ${i}`) })
  );

test("a message arriving live while the filter is on is COUNTED, not just drawn", async () => {
  // The contradiction in its purest form: an empty to-do list, one message arrives, and the head of
  // the list goes on saying there is nothing to deal with — over a message.
  const page = newPage();
  const stream = await withLiveChannel(page, backlog(1));
  await turnTodoOn(page);
  await doneButton(page.el("discord-log").children[0]).click();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), [], "the premise is an emptied to-do list");
  assert.match(page.el("channel-summary").text(), /nothing left to deal with/);
  assert.equal(page.el("clear-backlog").hidden, true, "an empty list offered a bulk clear");

  const arriving = message({ id: "9100000000000000001", content: "the runner came back" });
  page.messages = [...page.messages, arriving]; // the server holds it too, as it really would
  await deliver(page, stream, sseMessage(arriving));

  assert.deepStrictEqual(
    shownIds(page),
    ["9100000000000000001"],
    "the arriving message never reached the filtered list"
  );
  const head = page.el("channel-summary").text();
  assert.doesNotMatch(
    head,
    /nothing left to deal with/,
    `the head of the list denies the message drawn beneath it: ${head}`
  );
  assert.match(head, /^1 of/, `the head does not count the arrival: ${head}`);
  assert.equal(
    page.el("clear-backlog").hidden,
    false,
    "a list with something in it offered no way to bulk-clear it"
  );
});

test("the bulk control counts a message that arrived live, and really clears it", async () => {
  // The dangerous half. The label is drawn from the page's own count and the act is drawn from the
  // rows, so a count that stops at the last read means a control that says "clear 2" over three
  // rows and takes all three — a bulk destructive action understating itself.
  const page = newPage();
  const stream = await withLiveChannel(page, backlog(2));
  await turnTodoOn(page);
  assert.match(page.el("clear-backlog").textContent, /\(2\)/);

  const arriving = message({ id: "9100000000000000002", content: "and another one" });
  page.messages = [...page.messages, arriving];
  await deliver(page, stream, sseMessage(arriving));

  assert.equal(shownIds(page).length, 3);
  assert.match(
    page.el("clear-backlog").textContent,
    /\(3\)/,
    `the control says ${page.el("clear-backlog").textContent} over three rows it would clear`
  );

  await page.el("clear-backlog").click();
  await page.settle();
  assert.match(page.el("status").textContent, /clear 3/, "the confirmation undercounts what it asks about");
  await page.el("clear-backlog").click();
  await page.settle();
  assert.equal(page.dismissCalls[0].through, "9100000000000000002", "the clear stopped at the last read");
  assert.deepStrictEqual(shownIds(page), []);
});

test("a reply posted from the to-do list is counted by the list: what it answered leaves, and the reply, his own words, never joins", async () => {
  // The SECOND door into the same defect. A posted reply is appended to the log without any read,
  // exactly as a live arrival is, and it is in the window from now on — so the next `/todo` answer
  // will count it. A page that counted it only from that next read would spend the time in between
  // saying one number over a list of another, and the bulk control would clear the row it never
  // offered to.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog(2));
  await turnTodoOn(page);
  assert.match(page.el("clear-backlog").textContent, /\(2\)/, "the premise is a count of two");

  await replyButton(page.el("discord-log").children[0]).click();
  page.el("reply-text").value = "on it";
  await page.el("reply-text").dispatch("input");
  await page.el("reply-send").click();
  await page.settle();

  // `#256 reply-marks-read`: the message answered is Done once the reply is sent, so it leaves the
  // list. `#549 own-read-stored`: the reply is the reader's own words, which start read, so it does
  // not join it. The count follows both.
  assert.deepStrictEqual(shownIds(page), ["8000000000000000001"],
    "the posted reply joined the list, or what it answered stayed in it");
  assert.match(
    page.el("clear-backlog").textContent,
    /\(1\)/,
    `the control says ${page.el("clear-backlog").textContent} over one row it would clear`
  );
  assert.match(page.el("channel-summary").text(), /^1 of/, "the head of the list miscounts");
});

test("a message arriving during a to-do POLL raises the chip, as it does in the channel", async () => {
  // The one view whose entire purpose is surfacing new work was the one that said nothing when new
  // work turned up off screen. The unfiltered channel beside it has raised this chip since `#44`.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, tallBacklog(12));
  await turnTodoOn(page);
  const area = page.el("scroll-area");
  area.scrollTop = 0; // working through the older ones, not pinned to the bottom

  page.messages = tallBacklog(13);
  page.expireTimers(DISCORD_POLL_MS);
  await page.settle();

  assert.equal(page.todoReads, 2, "the poll did not read the to-do list");
  assert.equal(shownIds(page).length, 13, "the poll did not bring the message in");
  assert.equal(area.scrollTop, 0, "a background poll moved the reader");
  assert.equal(
    page.el("jump-newest").hidden,
    false,
    "the to-do list gained a message off screen and said nothing — the reader cannot tell"
  );
});

test("a to-do poll that returns the same list, and the reader's own act, raise nothing", async () => {
  // THE CONTROL for the test above, in both directions. A chip raised by an unchanged poll would
  // nag every forty-five seconds about nothing; a chip raised by the reader's own Done would answer
  // their tap with an offer to jump to a bottom nothing arrived at.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, tallBacklog(12));
  await turnTodoOn(page);
  const area = page.el("scroll-area");
  area.scrollTop = 0;

  page.expireTimers(DISCORD_POLL_MS); // same page.messages: nothing new
  await page.settle();
  assert.equal(page.todoReads, 2, "the poll did not fire");
  // The reader is up the list, so the way down is offered (`#207 scrollback-jump`) — but as the way
  // down, not as news.
  assert.equal(
    jumpSaysArrived(page),
    false,
    "an unchanged to-do list raised an arrival; every 45 seconds it would nag about nothing"
  );

  const rows = page.el("discord-log").children;
  await doneButton(rows[rows.length - 1]).click(); // deal with the NEWEST, so the newest id moves
  await page.settle();
  assert.equal(shownIds(page).length, 11, "the dismissal did not take");
  assert.equal(
    jumpSaysArrived(page),
    false,
    "dealing with the newest row was reported back to the reader as an arrival"
  );
});

const flashOf = (page, id) => channelRow(page, id).getAttribute("data-done-flash");

test("e DRAWS THE MOMENT ON THE ROW IT MARKED, though the selection moves on: marking, fading, gone (#245)", async () => {
  const page = await keyboardPageAtTop();
  await press(page, page.el("scroll-area"), "ArrowDown");
  assert.equal(selectedId(page), "1");
  await pressHere(page, "e");
  assert.equal(selectedId(page), "2", "the selection did not move on to the next unread message");
  assert.equal(channelRow(page, "1").getAttribute("data-archived"), "true");
  // The row it left, not the row it went to, carries the change.
  assert.equal(flashOf(page, "1"), "marking", "the row just marked Done shows nothing of it");
  assert.equal(flashOf(page, "2"), null, "the next message was drawn as if it had been marked");
  assert.equal(page.expireTimers(DONE_FLASH_HOLD_MS), 1, "nothing ends the grey ring's hold");
  assert.equal(flashOf(page, "1"), "fading", "the ring was not let go after its hold");
  assert.equal(page.expireTimers(DONE_FLASH_FADE_MS), 1, "nothing ends the ring's letting go");
  assert.equal(flashOf(page, "1"), null, "the row never became an ordinary read row again");
  // Putting a message back is not marking it: no flash.
  await pressHere(page, "k");
  await pressHere(page, "e");
  assert.equal(channelRow(page, "1").getAttribute("data-archived"), "false");
  assert.equal(flashOf(page, "1"), null, "putting a message back drew it as marked Done");
});

test("ON A THREADED CHANNEL, where a Done redraws every row, the new row carries the moment (#245)", async () => {
  // The store redraws the list as new elements (`renderCachedTimeline`), so a flag put on the row
  // that was pressed would be thrown away with it. The moment is kept by message, and drawn again.
  const page = newPage();
  page.threadingSupported = true;
  await signIn(page);
  await showDiscord(page, tallChannel(12));
  const pressed = channelRow(page, "3");
  await doneButton(pressed).click();
  const drawn = channelRow(page, "3");
  assert.notEqual(drawn, pressed, "the fixture did not redraw the row, so this proves nothing new");
  assert.equal(drawn.getAttribute("data-archived"), "true");
  assert.equal(drawn.getAttribute("data-done-flash"), "marking", "the redrawn row lost the moment of marking");
  assert.equal(page.expireTimers(DONE_FLASH_HOLD_MS), 1);
  assert.equal(channelRow(page, "3").getAttribute("data-done-flash"), "fading");
  assert.equal(page.expireTimers(DONE_FLASH_FADE_MS), 1);
  assert.equal(channelRow(page, "3").hasAttribute("data-done-flash"), false, "the moment outlived its timers");
});

test("A DONE THE SERVER REFUSED takes the moment back, so an unread row never wears the grey ring (#245)", async () => {
  const page = await keyboardPageAtTop();
  page.inboxWriteHook = async (kind) => (kind === "dismiss" ? providerDown() : null);
  await doneButton(channelRow(page, "2")).click();
  await page.settle();
  assert.equal(channelRow(page, "2").getAttribute("data-archived"), "false", "the refused Done stayed");
  assert.equal(channelRow(page, "2").hasAttribute("data-done-flash"), false, "a refused Done still drew the row as marked");
  assert.equal(page.expireTimers(DONE_FLASH_HOLD_MS), 0, "a timer was left to draw a Done that never happened");
});

test("the Done button, a swipe, Done through here and Shift+E draw it too, on the row they acted on (#245, #295)", async () => {
  const page = await keyboardPageAtTop();
  await doneButton(channelRow(page, "4")).click();
  assert.equal(flashOf(page, "4"), "marking", "the Done button marked a row and drew nothing");
  const commit = sourceConstant("SWIPE_COMMIT_PX") + 10;
  const touch = (x) => ({ pointerId: 1, pointerType: "touch", clientX: x, clientY: 40 });
  const swiped = channelRow(page, "6");
  await swiped.dispatch("pointerdown", touch(300));
  await swiped.dispatch("pointermove", touch(300 - commit));
  await swiped.dispatch("pointerup", touch(300 - commit));
  await page.settle();
  assert.equal(channelRow(page, "6").getAttribute("data-archived"), "true", "the swipe marked nothing");
  assert.equal(flashOf(page, "6"), "marking", "a swipe marked a row and drew nothing");
  // Done through here from row 9's ⋯ menu: every row above it is marked, and the one it was asked
  // from is the one drawn.
  const boundary = "9";
  await readThroughButton(channelRow(page, boundary)).click();
  await page.settle();
  assert.equal(channelRow(page, "7").getAttribute("data-archived"), "true", "Done through here marked nothing");
  assert.equal(flashOf(page, boundary), "marking", "Done through here drew nothing on the row it was asked from");
  assert.equal(flashOf(page, "7"), null, "every row Done through here marked was flashed, not the one asked from");
  // Shift+E's read and collapse (`#295 shift-e-read-collapse`) on row 12, with rows 10 and 11 still to
  // read above it: it marks row 12 alone, through `markRowDone`, and draws the moment on that row. Done
  // through here, which Shift+E used to be, marks 10 and 11 too and draws only 12: that is the
  // difference these lines see, where a row Done and drawn on 12 alone would be true of both.
  page.el("scroll-area").scrollTop = 0;
  await press(page, page.el("scroll-area"), "ArrowDown");
  for (let i = 0; i < 11; i += 1) await pressHere(page, "j");
  assert.equal(selectedId(page), "12");
  for (const id of ["10", "11"]) assert.equal(channelRow(page, id).getAttribute("data-archived"), "false", `the premise is ${id} unread`);
  await pressHere(page, "E", { shiftKey: true });
  assert.deepStrictEqual(page.dismissCalls.at(-1), { messages: ["12"] }, "Shift+E did not write row 12 alone Done");
  assert.equal(flashOf(page, "12"), "marking", "Shift+E marked a row and drew nothing");
  for (const id of ["10", "11"]) {
    assert.equal(channelRow(page, id).getAttribute("data-archived"), "false", `Shift+E marked row ${id} above it Done`);
    assert.equal(flashOf(page, id), null, `Shift+E drew the moment on row ${id}, which it was not pressed on`);
  }
});

test("THE UNDO CHIP NAMES THE MESSAGE it would put back, and counts a Done through here (#245)", async () => {
  const page = await keyboardPageAtTop();
  assert.equal(page.el("undo-dismiss").hidden, true);
  await press(page, page.el("scroll-area"), "ArrowDown");
  await pressHere(page, "e");
  assert.equal(page.el("undo-dismiss").hidden, false, "there is no way back after a Done");
  // tallChannel's first message reads "message 1: a sentence about the overnight run. …": its words
  // to eighty characters, cut at a word.
  const name = "message 1: a sentence about the overnight run. a sentence about the overnight…";
  assert.equal(page.el("undo-dismiss-label").text(), `Undo “${name}”`,
    `the Undo chip says "${page.el("undo-dismiss-label").text()}", not which message it would put back`);
  // The name and its quotation marks in a span the stylesheet leaves out on a phone's narrow row, and
  // the name in a span of its own inside it, which the stylesheet cuts short to the room the chips
  // leave; the whole of it in the chip's accessible name and its tooltip, whatever the chip shows.
  const named = page.el("undo-dismiss-label").children.find((node) => node.className === "undo-named");
  assert.ok(named, "the name is not in a span a narrow row can leave out");
  const span = named.children.find((node) => node.className === "undo-name");
  assert.ok(span, "the name is not in a span the stylesheet can cut short");
  assert.equal(span.textContent, name);
  assert.equal(page.el("undo-dismiss").getAttribute("title"), `Undo “${name}”`, "the tooltip does not carry the whole name");
  assert.equal(page.el("undo-dismiss").getAttribute("aria-label"), `Undo “${name}”`,
    "a screen reader is not told the whole name where the chip shows only Undo");
  assert.ok(!page.createdTags.includes("img") && !/</.test(span.textContent), "the name was drawn as markup");
  await page.el("undo-dismiss").click();
  await page.settle();
  assert.equal(channelRow(page, "1").getAttribute("data-archived"), "false", "the chip put nothing back");
  assert.equal(page.el("undo-dismiss").hidden, true, "the chip outlived what it undid");
  assert.equal(page.el("undo-dismiss-label").text(), "Undo");
  assert.equal(page.el("undo-dismiss-label").children.length, 0, "the name outlived the dismissal it named");
  assert.equal(page.el("undo-dismiss").hasAttribute("aria-label"), false, "the name outlived the dismissal it named");
  // Several rows at once are counted, not named after one of them: Done through here, from the ⋯ menu.
  await readThroughButton(channelRow(page, "4")).click();
  await page.settle();
  assert.match(page.el("undo-dismiss-label").textContent, /^Undo Done \(\d+\)$/);
  assert.equal(page.el("undo-dismiss-label").textContent, `Undo Done (${page.dismissCalls.at(-1).messages.length})`);  assert.equal(page.el("undo-dismiss").getAttribute("title"), page.el("undo-dismiss-label").textContent);
});

test("ON A PHONE'S NARROW ROW THE UNDO CHIP SAYS ONLY UNDO; wider, its name gives way, never the row (#245)", () => {
  // From review: at 412px the name gave way to one letter, "Undo “B… ”". The row of chips is a size
  // container, and narrower than 30rem — every phone — the name and its quotation marks are left out.
  // tests/done_feedback_browser.py measures the row at 360px, 412px and 1280px; this pins the rules.
  assert.match(cssBlock("#scroll-tools"), /container:\s*scroll-tools \/ inline-size;/, "the row of chips is not a size container");
  const narrow = /@container scroll-tools \(width < 30rem\) \{\s*#undo-dismiss \.undo-named \{\s*display:\s*none;\s*\}\s*\}/;
  assert.match(CSS_CODE, narrow, "a phone's narrow row still draws the name, which gives way to a letter");
  const wide = /@container scroll-tools \(width >= 30rem\) \{\s*#undo-dismiss:has\(\.undo-name\) \{([^}]*)\}\s*\}/.exec(CSS_CODE);
  assert.ok(wide, "the named chip's room is not decided by the row's width");
  // A dozen characters of the name at least, where it is drawn: its least width is stated.
  assert.match(wide[1], /min-width:\s*12em;/, "a named chip can give way to a letter of its name");
  // The space "Undo " ends with is dropped at the end of a flex line: drawn as a margin.
  assert.match(cssBlock("#undo-dismiss .undo-named"), /margin-inline-start:\s*0\.3em;/);
  // From review, the first round: the named chip is about three times as wide as "Undo", and at 412px
  // it wrapped the row up over the list and out over the Send button. Wider, the name gives way.
  const chip = cssBlock("#undo-dismiss:has(.undo-name)");
  assert.match(chip, /flex:\s*1 1 0;/, "the chip goes onto its line at the whole name's width, so the name wraps the row");
  assert.match(chip, /min-width:\s*\d+(\.\d+)?em;/, "the chip's least width is the whole name's, so the name wraps the row");
  assert.match(chip, /max-width:\s*max-content;/, "the chip grows past its name");
  const label = cssBlock("#undo-dismiss-label");
  assert.match(label, /display:\s*flex;/);
  assert.match(label, /min-width:\s*0;/);
  assert.match(label, /white-space:\s*nowrap;/, "the chip's words can wrap inside it");
  const name = cssBlock("#undo-dismiss .undo-name");
  for (const rule of [/overflow:\s*hidden;/, /text-overflow:\s*ellipsis;/, /max-width:\s*\d+(\.\d+)?em;/, /min-width:\s*\d+(\.\d+)?em;/]) {
    assert.match(name, rule, `the name is not cut short with an ellipsis: ${rule}`);
  }
});

test("SELECTED AND READ IS RINGED GREY, selected and unread in the accent, and marking animates between them (#245)", () => {
  // The look is the browser's to prove (tests/done_feedback_browser.py reads the computed styles and
  // the running animations); what this pins is that the rules saying it are where they have to be.
  const readSelected = /#discord-log li\.discord-message\[data-selected="true"\]:is\(\[data-archived="true"\], \[data-noise="true"\]\)[^{]*\{([^}]*)\}/.exec(CSS_CODE);
  assert.ok(readSelected, "nothing draws a selected read message differently from a selected unread one");
  assert.match(readSelected[1], /outline-color:\s*var\(--read-ring\)/, "a selected read message's ring is not grey");
  // From review: in the light scheme the grey and the blue are both dark inks a little apart in hue.
  // A dashed ring against a solid one is told apart in either scheme, and without the hue.
  assert.match(readSelected[1], /outline-style:\s*dashed/, "a selected read message's ring is told from the blue by hue alone");
  assert.match(readSelected[1], /border-color:\s*transparent/, "the row's edge fills the gaps between the dashes");
  assert.match(cssBlock('#discord-log li.discord-message[data-selected="true"]'), /outline:\s*2px solid var\(--accent\)/,
    "a selected unread message lost its accent ring");
  // The moment of marking is an animation FROM the selected unread look, so it runs on a row the Done
  // redrew as a new element as well as on one it kept: a transition has no earlier style there.
  const from = /@keyframes done-marking \{\s*from \{([^}]*)\}\s*\}/.exec(CSS_CODE);
  assert.ok(from, "nothing animates the row just marked from the look it had");
  for (const [property, value] of [["outline-color", "var\\(--accent\\)"], ["background-color", "var\\(--row-hover\\)"],
    ["opacity", "1"], ["filter", "none"]]) {
    assert.match(from[1], new RegExp(`${property}:\\s*${value};`), `marking does not start from the selected ${property}`);
  }
  assert.match(cssBlock('#discord-log li.discord-message[data-done-flash="marking"]'), /animation:\s*done-marking 0\.25s/,
    "the row just marked does not change over a quarter of a second");
  assert.match(cssBlock('#discord-log li.discord-message[data-done-flash="marking"]:not([data-selected="true"])'),
    /outline-color:\s*var\(--read-ring\)/, "the row just marked is not ringed grey");
  assert.match(cssBlock('#discord-log li.discord-message[data-done-flash]:not([data-selected="true"])'),
    /outline:\s*2px dashed transparent/, "the ring just marked is not the dashed ring of a read message");
  assert.match(from[1], /outline-style:\s*solid;/, "marking does not start from the solid ring of an unread message");
  assert.match(cssBlock('#discord-log li.discord-message[data-done-flash="fading"]:not([data-selected="true"])'),
    /animation:\s*done-fading 0\.3s/, "the grey ring is switched off rather than let go");
  assert.equal(DONE_FLASH_FADE_MS, 300, "the fade's timer and its animation are different lengths");
  assert.match(CSS_CODE, /@media \(prefers-reduced-motion: reduce\) \{\s*#side-log li\.discord-message\[data-done-flash\],[^{]*#discord-log li\.discord-message\[data-done-flash\],[^{]*\{\s*animation:\s*none;/,
    "under reduced motion the marking still animates");
  const plain = cssRules(CSS, '#discord-log li.discord-message[data-selected="true"]').join("");
  assert.equal(/transition|animation/.test(plain), false, "every selected row animates, so the ring lags behind j and k");
});

test("THE READ RING HAS A VALUE IN EACH SCHEME, and in the light one it is lighter than the dark grey ink (#245)", () => {
  // `--muted` and `--accent` are both dark inks in the light scheme; the read ring there is a lighter
  // grey that still stands at 3:1 against the lit surface. tests/done_feedback_browser.py reads the
  // contrast off the page in both schemes.
  const dark = /:root \{[^}]*--read-ring:\s*(#[0-9a-f]{6});/.exec(CSS_CODE);
  const light = /@media \(prefers-color-scheme: light\) \{\s*:root \{[^}]*--read-ring:\s*(#[0-9a-f]{6});/.exec(CSS_CODE);
  assert.ok(dark && light, "the read ring is declared for one scheme only, and draws as nothing in the other");
  const luminance = (hex) => {
    const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255)
      .map((c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
  };
  const contrast = (a, b) => (Math.max(luminance(a), luminance(b)) + 0.05) / (Math.min(luminance(a), luminance(b)) + 0.05);
  const lightHover = /@media \(prefers-color-scheme: light\) \{\s*:root \{[^}]*--row-hover:\s*(#[0-9a-f]{6});/.exec(CSS_CODE)[1];
  assert.ok(contrast(light[1], lightHover) >= 3, `the light read ring is ${contrast(light[1], lightHover).toFixed(2)}:1 on the lit surface`);
  assert.ok(luminance(light[1]) > luminance("#5d6674"), "the light read ring is as dark as the grey ink it was told apart from");
});

test("EVERY WAY BACK TAKES THE MOMENT AWAY AT ONCE: the Undo chip, z, the row's own button, another device (#245)", async () => {
  // From review: an Undo pressed within the hold left the grey ring on a row that was unread again.
  const ways = {
    "the Undo chip": async (page) => {
      await page.el("undo-dismiss").click();
      await page.settle();
    },
    z: (page) => pressHere(page, "z"),
    "the row's own button": async (page) => {
      await doneButton(channelRow(page, "1")).click();
      await page.settle();
    },
    // The read after the put back speaks for the message, and says it is not Done.
    "another device": async (page) => {
      page.dealtWith.delete("1");
      await reReadChannel(page);
    },
  };
  for (const [way, putBack] of Object.entries(ways)) {
    const page = await keyboardPageAtTop();
    await press(page, page.el("scroll-area"), "ArrowDown");
    await pressHere(page, "e");
    await page.settle();
    assert.equal(flashOf(page, "1"), "marking", `${way}: the fixture drew no moment to take back`);
    assert.ok(page.dealtWith.has("1"), `${way}: the Done did not reach the fixture's server`);
    await putBack(page);
    assert.equal(channelRow(page, "1").getAttribute("data-archived"), "false", `${way} did not put the message back`);
    assert.equal(flashOf(page, "1"), null, `${way}: a row that is unread again still wears the grey ring`);
    assert.equal(page.expireTimers(DONE_FLASH_HOLD_MS), 0, `${way}: a timer was left to draw a Done that was taken back`);
    assert.equal(flashOf(page, "1"), null);
  }
});

test("A RICH HEADER'S ROW is drawn marked Done like any other (#245, #242)", async () => {
  const lead = "[build-bot coordinator] Backup check, 10:52 PM ET. ";
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [
    message({ id: "1", content: `${lead}${longMessage("rich")}`,
      heading: { tag: "build-bot coordinator", time: "10:52 PM ET", title: "Backup check", cut: lead.length } }),
    message({ id: "2", content: "the next one" }),
  ]);
  assert.ok(textOfRow(channelRow(page, "1")).children[0].hasAttribute("data-rh"), "the fixture's heading was not shown");
  await doneButton(channelRow(page, "1")).click();
  assert.equal(flashOf(page, "1"), "marking", "a rich header's row was marked Done and drew nothing");
  assert.equal(page.expireTimers(DONE_FLASH_HOLD_MS), 1);
  assert.equal(flashOf(page, "1"), "fading");
  assert.equal(page.expireTimers(DONE_FLASH_FADE_MS), 1);
  assert.equal(flashOf(page, "1"), null);
  assert.ok(elidedOf(channelRow(page, "1")), "a rich header's long row has no count under it");
});

test("UNDER HIDE READ, the row e takes away hands the selection to the next unread message", async () => {
  const page = await keyboardPageAtTop();
  await turnTodoOn(page);
  page.el("scroll-area").scrollTop = 0;
  await press(page, page.el("scroll-area"), "ArrowDown");
  const first = selectedId(page);
  await pressHere(page, "e");
  assert.notEqual(selectedId(page), first, "the selection stayed on a message Hide read took away");
  assert.ok(channelRow(page, selectedId(page)), `after e the focus is on ${selectedId(page)}, not a row`);
  assert.notEqual(channelRow(page, selectedId(page)).getAttribute("data-archived"), "true");
});

test("Enter AND o FOLD the selected message as a tap does; Enter on a button inside the row is the button's", async () => {
  const page = await keyboardPageAtTop();
  await press(page, page.el("scroll-area"), "ArrowDown");
  const row = channelRow(page, "1");
  assert.equal(row.getAttribute("data-collapsed"), "true", "a long message did not arrive folded");
  let event = await pressHere(page, "Enter");
  assert.equal(event.defaulted, false);
  assert.equal(row.getAttribute("data-collapsed"), "false", "Enter did not open the selected message");
  event = await pressHere(page, "o");
  assert.equal(row.getAttribute("data-collapsed"), "true", "o did not close it again");
  // Enter on the row's own Reply button presses that button, not the row.
  event = await press(page, replyButton(row), "Enter");
  assert.equal(event.defaulted, true, "Enter on a button in the row was taken from the button");
  assert.equal(row.getAttribute("data-collapsed"), "true", "Enter on a button in the row folded the row");
});

test("→ ENTERS THE SELECTED MESSAGE'S THREAD; ← and Esc come back, the focus on the message it was opened from", async () => {
  const page = await threadPage();
  assert.deepStrictEqual(shownIds(page), ["200", "201", "202", "203", "204"]);
  page.el("scroll-area").scrollTop = 0;
  await press(page, page.el("scroll-area"), "ArrowDown");
  assert.equal(selectedId(page), "200");
  // A message in no thread has nowhere to go: → is left alone.
  let event = await pressHere(page, "ArrowRight");
  assert.equal(event.defaulted, true, "→ on a message in no thread was taken");
  await pressHere(page, "j");
  await pressHere(page, "j");
  assert.equal(selectedId(page), "202");
  for (const back of ["ArrowLeft", "Escape"]) {
    event = await pressHere(page, "ArrowRight");
    await page.settle();
    assert.equal(event.defaulted, false, "→ on a message in a thread was left to the browser");
    assert.deepStrictEqual(shownIds(page), ["201", "202"], "→ did not open the thread of the selected message");
    assert.equal(page.el("thread-heading").hidden, false, "→ did not open the thread");
    assert.ok(focused(page) === page.el("scroll-area") || nodeIn(page.el("discord-log"), focused(page)),
      `in the thread the focus is on ${focusName(page)}`);
    // ↓ in the thread moves through its messages.
    await pressHere(page, "ArrowDown");
    assert.ok(["201", "202"].includes(selectedId(page)), `↓ in the thread selected ${selectedId(page)}`);
    event = await pressHere(page, back);
    await page.settle();
    assert.equal(event.defaulted, false, `${back} in a thread was left to the browser`);
    assert.deepStrictEqual(shownIds(page), ["200", "201", "202", "203", "204"], `${back} did not leave the thread`);
    assert.equal(selectedId(page), "202", `back from the thread with ${back}, the focus is on ${selectedId(page)}`);
  }
  // ← outside a thread is nobody's.
  event = await pressHere(page, "ArrowLeft");
  assert.equal(event.defaulted, true, "← outside a thread was taken");
});

test("ESC IN A MESSAGE BOX leaves the box and keeps the draft; the next Esc goes Back; an empty box goes Back at once", async () => {
  const page = await threadPage();
  await pickThread(page, "main");
  await threadBadge(channelRow(page, "201")).click();
  await page.settle();
  assert.equal(page.el("thread-heading").hidden, false, "the thread did not open");
  const box = page.el("channel-compose-text");
  box.focus();
  await box.setValue("half a reply");
  let event = await press(page, box, "Escape");
  assert.equal(event.defaulted, false);
  assert.notEqual(focused(page), box, "Esc in a box with text in it did not leave the box");
  assert.equal(page.el("thread-heading").hidden, false, "the first Esc in a box with text left the thread too");
  assert.equal(box.value, "half a reply", "Esc lost the draft");
  event = await pressHere(page, "Escape");
  await page.settle();
  assert.equal(page.el("thread-heading").hidden, true, "the second Esc did not leave the thread");
  // An empty box has nothing to keep: one Esc goes Back.
  await threadBadge(channelRow(page, "201")).click();
  await page.settle();
  await box.setValue("");
  event = await press(page, box, "Escape");
  await page.settle();
  assert.equal(page.el("thread-heading").hidden, true, "Esc in an empty box did not go Back");
  // The reply screen: Esc leaves its box first, then closes it, and the draft is still there after.
  const reply = newPage();
  await signIn(reply);
  await openReplyOn(reply, tallChannel(5), 1);
  reply.el("reply-text").focus();
  await reply.el("reply-text").setValue("not sent yet");
  await press(reply, reply.el("reply-text"), "Escape");
  assert.ok(replyInStream(reply), "the first Esc in the reply box with text put the reply away");
  assert.equal(focused(reply), reply.el("scroll-area"), `after Esc the focus is on ${focusName(reply)}`);
  await press(reply, reply.el("scroll-area"), "Escape");
  assert.equal(reply.screen(), "main", "the second Esc did not leave the reply screen");
  await replyButton(reply.el("discord-log").children[1]).click();
  assert.equal(reply.el("reply-text").value, "not sent yet", "the draft did not survive Esc");
});

test("THE CALL'S BOX IS A TEXT FIELD TO THE KEYMAP: no single key fires in it, and Esc leaves it", async () => {
  // `#241 voice-composer` put a message box in the call's view, inside the scrolling list. The
  // keymap's rules for a field are the ones it already has (`#224 keyboard-shortcuts`): a key that
  // types a character is the box's, and Esc leaves the box for the list, keeping the draft.
  const page = newPage();
  await signIn(page);
  assert.equal(page.tab(), "voice");
  const box = page.el("compose-text");
  box.focus();
  await box.setValue("a commit hash, 9c07d3e");
  for (const key of [",", "/", "?", "j", "k", "e"]) {
    const event = await press(page, box, key);
    assert.equal(event.defaulted, true, `${key} in the call's box was taken from the box`);
    assert.equal(page.screen(), "main", `${key} in the call's box left the main screen`);
    assert.equal(page.el("search-field").hidden, true, `${key} in the call's box opened the search`);
  }
  const event = await press(page, box, "Escape");
  assert.equal(event.defaulted, false, "Esc in the call's box was left to the browser");
  assert.equal(focused(page), page.el("scroll-area"), `after Esc the focus is on ${focusName(page)}`);
  assert.equal(box.value, "a commit hash, 9c07d3e", "Esc lost the draft");
  assert.equal(page.screen(), "main");
  assert.equal(page.sockets.length, 0, "a key in the call's box opened a conversation");
});

test("ESC NEVER MARKS ANYTHING READ, from the list, a selected message, or a thread", async () => {
  const page = await threadPage();
  page.el("scroll-area").scrollTop = 0;
  await press(page, page.el("scroll-area"), "ArrowDown");
  for (let i = 0; i < 3; i += 1) await pressHere(page, "Escape");
  await press(page, page.el("scroll-area"), "Escape");
  assert.deepStrictEqual(page.dismissCalls, [], "Escape marked something read");
  assert.equal(page.screen(), "main");
});

test("r REPLIES with the box ready; p pins; . opens the ⋯ menu on its first item; i opens the details; Ctrl+C copies", async () => {
  const page = await keyboardPageAtTop();
  await press(page, page.el("scroll-area"), "ArrowDown");
  await pressHere(page, "j");
  const row = channelRow(page, "2");
  // p
  let event = await pressHere(page, "p");
  assert.equal(event.defaulted, false);
  assert.deepStrictEqual(page.pinCalls.map((call) => call.id), ["2"], "p did not pin the selected message");
  // .
  event = await pressHere(page, ".");
  assert.equal(rowMoreMenu(channelRow(page, "2")).hidden, false, ". did not open the ⋯ menu");
  const firstItem = focused(page);
  assert.equal(String(firstItem.tagName).toUpperCase(), "BUTTON", `. left the focus on ${focusName(page)}`);
  assert.ok(nodeIn(rowMoreMenu(channelRow(page, "2")), firstItem), "the focus is not on an item in the menu");
  await pressHere(page, "ArrowDown");
  assert.notEqual(focused(page), firstItem, "↓ in the menu did not move to its next item");
  assert.ok(nodeIn(rowMoreMenu(channelRow(page, "2")), focused(page)), "↓ in the menu left the menu");
  await pressHere(page, "Escape");
  assert.equal(rowMoreMenu(channelRow(page, "2")).hidden, true, "Esc did not close the menu");
  assert.equal(focused(page), rowMoreButton(channelRow(page, "2")), "the focus did not go back to the ⋯");
  // i, and Esc closes the sheet with the focus back on the row.
  await press(page, channelRow(page, "2"), "i");
  const sheet = () => channelRow(page, "2").descendants().find((node) => node.className === "msg-details");
  assert.ok(sheet(), "i did not open the details of the selected message");
  assert.match(sheet().text(), /id 2/, "the details do not name the message");
  await press(page, channelRow(page, "2"), "Escape");
  assert.equal(sheet(), undefined, "Esc did not close the details");
  // Ctrl+C with no text selected: the text of the message, by the copy the browser fires for the key.
  // `#296 copy-highlighted-message`, whose own tests follow.
  event = await pressCopy(page, channelRow(page, "2"));
  assert.equal(event.defaulted, true, "Ctrl+C on the selected message was taken from the browser, whose copy carries it");
  assert.match(String(page.clipboard), /^message 2: /, "Ctrl+C did not copy the text of the selected message");
  // In a field, Ctrl+C is the field's.
  page.clipboard = undefined;
  event = await pressCopy(page, page.el("channel-compose-text"));
  assert.equal(event.defaulted, true, "Ctrl+C in a message box was taken from it");
  assert.equal(page.clipboard, undefined, "Ctrl+C in an empty message box copied the message");
  // r: the reply, its box ready to type in, in the list (`#410 inline-reply-editor`).
  await press(page, channelRow(page, "2"), "r");
  assert.ok(replyInStream(page), "r did not open the reply in the list");
  assert.equal(focused(page), page.el("reply-text"), `on Reply after r the focus is on ${focusName(page)}`);
  assert.ok(row, "the row went");
});

const ANSWERED_HOOK = "    if (entry.marksRead) guardQuietly(markSentRead)(entry);\n";

test("`#256 reply-marks-read`: a SENT reply marks the message it answers Done — the button, the look, the server and the Undo chip agree, and Undo puts it back", async () => {
  const run = async (script) => {
    const page = newPage(new Map(), script);
    await signIn(page);
    await showDiscord(page, conversation());
    return page;
  };
  const target = "1000000000000000003";
  const page = await run(SCRIPT);
  assert.equal(doneButton(rowWithId(page, target)).text(), "Done", "the premise is an unread message");
  // Opening Reply marks nothing, and nor does leaving it.
  await replyButton(rowWithId(page, target)).click();
  await page.el("reply-cancel").click();
  assert.deepStrictEqual(page.dismissCalls, [], "opening Reply marked the message");
  assert.equal(rowWithId(page, target).getAttribute("data-archived"), "false");

  await replyAndSend(page, target, "answering the open one");
  assert.equal(page.repliesPosted.at(-1).body.reply_to, target, "the reply did not answer the message");
  assert.deepStrictEqual(page.dismissCalls, [{ messages: [target] }], "the reply's Done was not written to the server");
  assert.ok(page.dealtWith.has(target), "the server does not hold the message as Done");
  const row = rowWithId(page, target);
  assert.equal(row.getAttribute("data-archived"), "true", "the message answered is not drawn as read");
  assert.equal(doneButton(row).text(), "Unarchive", "the button still offers Done on a message the reply marked read");
  // Only the message answered: the one somebody else answered stays as it was, dimmed and unread.
  const other = rowWithId(page, "1000000000000000001");
  assert.equal(other.getAttribute("data-replied"), "true");
  assert.equal(other.getAttribute("data-archived"), "false", "a message this reply did not answer was marked");
  assert.equal(page.el("undo-dismiss").hidden, false, "the reply's Done offers no Undo");

  await page.el("undo-dismiss").click();
  await page.settle();
  assert.deepStrictEqual(page.restoreCalls, [{ messages: [target] }], "Undo did not put the message back on the server");
  assert.equal(rowWithId(page, target).getAttribute("data-archived"), "false", "Undo left the message read");
  assert.equal(doneButton(rowWithId(page, target)).text(), "Done");
  assert.ok(!page.dealtWith.has(target));
  assert.equal(page.el("undo-dismiss").hidden, true);

  // THE CONTROL, which is the owner's report: without the reply's Done the message is dimmed as
  // answered while its button still offers Done, and nothing reaches the server.
  const control = await run(brokenScript(ANSWERED_HOOK, ""));
  await replyAndSend(control, target, "answering the open one");
  const dimmed = rowWithId(control, target);
  assert.equal(dimmed.getAttribute("data-replied"), "true", "the control did not even dim the message");
  assert.equal(dimmed.getAttribute("data-archived"), "false",
    "with the reply's Done taken out the message still went read, so this test cannot see it");
  assert.equal(doneButton(dimmed).text(), "Done");
  assert.deepStrictEqual(control.dismissCalls, []);
});

test("`#256 reply-marks-read`: the reply's Done is the Done button's own act, called only where a send has succeeded", () => {
  assert.match(SCRIPT_CODE, /async function markAnsweredDone\([\s\S]*?toggleArchived\(/,
    "the reply's Done does not go through the act the Done button and the swipe use");
  // `#300 reply-marks-read`: through the one act that marks a confirmed send read.
  assert.match(SCRIPT_CODE, /async function markSentRead\([\s\S]*?await markAnsweredDone\(entry\.channel, id\)/,
    "a confirmed send's read does not begin with the reply's Done");
  assert.equal(SCRIPT_CODE.split("markAnsweredDone(").length - 1, 2, "the reply's Done is called from more than one place");
  // Once, in the send's success, after the posted parts are drawn and before anything can fail it.
  assert.equal(SCRIPT_CODE.split("markSentRead)(").length - 1, 1, "a send's read is marked from more than one place");
  const send = SCRIPT_CODE.slice(SCRIPT_CODE.indexOf("async function dispatchOutgoingMessage("));
  const success = send.indexOf('entry.state = "sent";');
  const call = send.indexOf("markSentRead)(entry)");
  assert.ok(success > 0 && call > success && call < send.indexOf("} catch (error) {", success),
    "the reply's Done is not in the send's success");
});

test("`#256 reply-marks-read`: Hide read leaves the message out and Fold read folds it, once the reply is sent", async () => {
  const page = newPage();
  await signIn(page);
  const ids = backlog(4).map((m) => m.id);
  await showDiscord(page, backlog(4));
  await doneButton(rowWithId(page, ids[0])).click();
  await page.settle();
  await chooseReadMode(page, "Fold read");
  assert.equal(rowWithId(page, ids[1]).hasAttribute("data-read-run"), false, "the premise is an unread message out of any run");
  await replyAndSend(page, ids[1]);
  assert.ok(rowWithId(page, ids[1]).hasAttribute("data-read-run"),
    "Fold read did not fold the message the reply marked read in with the read one above it");
  assert.equal(rowWithId(page, ids[2]).hasAttribute("data-read-run"), false, "Fold read folded an unread message");
  await chooseReadMode(page, "Hide read");
  assert.ok(!shownIds(page).includes(ids[1]), "Hide read kept the message the reply marked read");
  assert.ok(shownIds(page).includes(ids[2]), "Hide read lost an unread message");
});

test("`#256 reply-marks-read`: a send that fails marks nothing, and the Retry that sends it marks the message", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog(2));
  const target = backlog(2)[0].id;
  const succeeds = page.replyResponse;
  page.replyResponse = errorResponse(400, "bad_request", "the chat service refused the message");
  await replyAndSend(page, target, "this one is refused");
  assert.equal(outgoingRows(page)[0].getAttribute("data-send-state"), "failed", "the premise is a refused send");
  assert.deepStrictEqual(page.dismissCalls, [], "a reply that was not sent marked the message");
  assert.equal(rowWithId(page, target).getAttribute("data-archived"), "false");
  assert.equal(doneButton(rowWithId(page, target)).text(), "Done");
  assert.equal(page.el("undo-dismiss").hidden, true, "a failed send offered an Undo");
  // A partial post is not a sent reply either: the rest of it is still to go.
  page.replyResponse = async () => json(207, { error: "partially_posted", posted: 1, unsent: "the rest", detail: "Google Chat returned HTTP 502" });
  await outgoingRetry(outgoingRows(page)[0]).click();
  await page.settle();
  assert.deepStrictEqual(page.dismissCalls, [], "a partly posted reply marked the message");
  page.replyResponse = succeeds;
  await outgoingRetry(outgoingRows(page)[0]).click();
  await page.settle();
  assert.deepStrictEqual(page.dismissCalls, [{ messages: [target] }], "the Retry that sent the reply did not mark the message");
  assert.equal(doneButton(rowWithId(page, target)).text(), "Unarchive");
});

test("`#256 reply-marks-read`: a reply to a message already Done changes nothing, and the Undo chip keeps the last Done", async () => {
  const page = newPage();
  await signIn(page);
  const [first, second] = backlog(3).map((m) => m.id);
  await showDiscord(page, backlog(3));
  await doneButton(rowWithId(page, first)).click();
  await page.settle();
  await doneButton(rowWithId(page, second)).click();
  await page.settle();
  assert.equal(page.dismissCalls.length, 2);
  await replyAndSend(page, first, "an afterthought");
  assert.equal(page.repliesPosted.length, 1, "the reply was not sent");
  assert.equal(page.dismissCalls.length, 2, "a reply to a message already Done wrote it Done again");
  assert.deepStrictEqual(page.restoreCalls, [], "a reply to a message already Done put it back");
  assert.equal(doneButton(rowWithId(page, first)).text(), "Unarchive");
  // The Undo chip is still the last Done's, the second message, and not taken over by the reply.
  await page.el("undo-dismiss").click();
  await page.settle();
  assert.deepStrictEqual(page.restoreCalls, [{ messages: [second] }], "the Undo chip no longer puts back the last Done");
  assert.equal(rowWithId(page, first).getAttribute("data-archived"), "true");
});

test("`#256 reply-marks-read`: a reply in a thread marks the message it answers and only it, and the root's count of unread replies on Main follows", async () => {
  const page = newPage();
  const messages = inboxChannel(page);
  await signIn(page);
  await showDiscord(page, messages);
  await pickThread(page, "main");
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: null, 7: "2 unread" }, "the premise");
  await pickThread(page, "flat");
  await replyAndSend(page, inboxId(8), "thanks, reviewing it");
  assert.deepStrictEqual(page.dismissCalls, [{ messages: [inboxId(8)] }], "the reply marked more than the message it answers");
  assert.equal(rowWithId(page, inboxId(8)).getAttribute("data-archived"), "true");
  // The thread's other unread reply is left; its root, his own question, started read (`#549
  // own-read-stored`) and was not written by the reply either.
  assert.equal(rowWithId(page, inboxId(9)).getAttribute("data-archived"), "false", "the reply marked 9 of the same thread");
  await pickThread(page, "main");
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: null, 7: "1 unread" },
    "the root still counts the reply the owner answered as unread");
  // Undo, and the count is back.
  await page.el("undo-dismiss").click();
  await page.settle();
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: null, 7: "2 unread" }, "Undo did not bring the count back");
});

test("`#256 reply-marks-read`: in two columns the reply's Done is drawn in both, from whichever column Reply was opened", async () => {
  const { page } = await twoColumnServer();
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  assert.deepStrictEqual(sideIds(page), ["201", "202"]);
  assert.ok(unreadChip(page, "201"), "the premise is a root on Main counting an unread reply");
  // From the right: the thread's reply.
  await replyAndSend(page, "202", "agreed", "side-log");
  assert.deepStrictEqual(page.dismissCalls, [{ messages: ["202"] }]);
  assert.equal(sideRow(page, "202").getAttribute("data-archived"), "true", "the right column did not draw the reply's Done");
  assert.equal(doneButton(sideRow(page, "202")).text(), "Unarchive");
  assert.equal(unreadChip(page, "201"), null, "Main still counts the reply answered on the right as unread");
  assert.equal(sideRow(page, "201").getAttribute("data-archived"), "false", "the reply marked the thread's root as well");
  // From the left: the root, which the right column draws too.
  await replyAndSend(page, "201", "and the whole thread");
  assert.deepStrictEqual(page.dismissCalls.at(-1), { messages: ["201"] });
  assert.equal(rowWithId(page, "201").getAttribute("data-archived"), "true");
  assert.equal(sideRow(page, "201").getAttribute("data-archived"), "true", "the right column did not draw Done made on the left");
});

test("`#256 reply-marks-read`: a reply sent after you moved to another channel is marked Done on the server, and nothing here is", async () => {
  const page = newPage();
  page.channels = [{ ...CHANNEL }, { ...OTHER_CHANNEL }];
  const here = message({ id: "3100000000000000001", channel_id: CHANNEL.id, content: "answer me" });
  const there = message({ id: "3100000000000000002", channel_id: OTHER_CHANNEL.id, content: "elsewhere" });
  serveByChannel(page, { [CHANNEL.id]: [here], [OTHER_CHANNEL.id]: [there] });
  await signIn(page);
  await showDiscord(page, []);
  const finish = delayedReply(page);
  await replyButton(rowWithId(page, here.id)).click();
  await page.el("reply-text").setValue("on it");
  const sending = page.el("reply-send").click();
  await readChannel(page, OTHER_CHANNEL.id);
  assert.deepStrictEqual(shownIds(page), [there.id]);
  finish(json(200, { posted: message({ id: "3100000000000000003", channel_id: CHANNEL.id, content: "on it", reply_to: here.id }) }));
  await sending;
  await page.settle();
  assert.deepStrictEqual(page.dismissCalls, [{ messages: [here.id] }], "the message answered was not marked");
  assert.ok(page.requests.includes(`POST /api/v1/channels/${encodeURIComponent(CHANNEL.id)}/dismiss`),
    "the Done went to another channel than the reply's");
  assert.equal(rowWithId(page, there.id).getAttribute("data-archived"), "false", "a message of this channel was marked");
  assert.equal(page.el("undo-dismiss").hidden, true, "an Undo here would put back a message of this channel");
});

test("`#245 done-feedback`: a SENT reply's Done draws its moment on the row it answers, in the column Reply was opened from", async () => {
  // `markAnsweredDone` marks through `toggleArchived`, as the Done button does, and hands it the row
  // standing for the message answered: without that row the Done is made and drawn read, but the
  // moment of marking it, which every other way of marking draws, is not.
  const run = async (script) => {
    const page = newPage(new Map(), script);
    await signIn(page);
    await showDiscord(page, conversation());
    return page;
  };
  const target = "1000000000000000003";
  const page = await run(SCRIPT);
  await replyAndSend(page, target, "answering the open one");
  assert.equal(rowWithId(page, target).getAttribute("data-archived"), "true", "the premise is the reply's Done");
  assert.equal(rowWithId(page, target).getAttribute("data-done-flash"), "marking",
    "the reply's Done drew nothing of the moment on the row it answered");
  assert.equal(rowWithId(page, "1000000000000000001").hasAttribute("data-done-flash"), false,
    "a row the reply did not answer was drawn as marked");
  assert.ok(page.expireTimers(DONE_FLASH_HOLD_MS) >= 1, "nothing ends the grey ring's hold");
  assert.equal(rowWithId(page, target).getAttribute("data-done-flash"), "fading", "the ring was not let go after its hold");
  assert.ok(page.expireTimers(DONE_FLASH_FADE_MS) >= 1, "nothing ends the ring's letting go");
  assert.equal(rowWithId(page, target).hasAttribute("data-done-flash"), false, "the moment outlived its timers");

  // In two columns, from the right: the row there is the one answered.
  const { page: two } = await twoColumnServer();
  await threadBadge(rowWithId(two, "201")).click();
  await two.settle();
  await replyAndSend(two, "202", "agreed", "side-log");
  assert.equal(sideRow(two, "202").getAttribute("data-archived"), "true", "the premise is the reply's Done on the right");
  assert.equal(sideRow(two, "202").getAttribute("data-done-flash"), "marking",
    "the reply's Done drew nothing on the right column's row it answered");
  assert.equal(sideRow(two, "201").hasAttribute("data-done-flash"), false, "the thread's root was drawn as marked");

  // THE CONTROL: the reply's Done handed no row, as it was, marks the message and draws no moment.
  const control = await run(brokenScript("toggleArchived(ids, found ? found.row : null)", "toggleArchived(ids)"));
  await replyAndSend(control, target, "answering the open one");
  assert.equal(rowWithId(control, target).getAttribute("data-archived"), "true", "the control made no Done");
  assert.equal(rowWithId(control, target).hasAttribute("data-done-flash"), false,
    "with no row handed over the moment was drawn anyway, so this test cannot see it");
});

// --- `#306 done-stale-across-views` -------------------------------------------------------------
//
// Found in review of #295 shift-e-read-collapse: a message marked Done in Main still showed as not
// Done in All, loaded earlier in the session, until some later read redrew it. Every view the page
// holds keeps a copy of which messages are Done for when it is shown again, and only the list the Done
// was made in, and the right column while it showed a thread, were told. A Done or an Undo now reaches
// every held view's copy at once — Main, All, a thread, the right column put away, and the cards'
// unread counts — with no read.

/** The line in `dismissMessages` that brings the views held into line, taken out for a negative control. */
const DONE_FOLLOWS = ["    followDoneEverywhere(requested, column);\n    // Before the redraw", "    // Before the redraw"];
/** ...the same in `restoreMessages`. */
const UNDO_FOLLOWS = ["    followDoneEverywhere(restored, column);\n    if (lastDismissal", "    if (lastDismissal"];
/** The check that draws a held view's rows again as it is shown, where they say otherwise than its copy. */
const HELD_REDRAWS = ["if (!caughtUp && (held.readMode !== readMode || rowsMissDone())) renderCachedTimeline();",
  "if (!caughtUp && held.readMode !== readMode) renderCachedTimeline();"];

/**
 * The channel of `threadData` opened in All, then Main read for itself as its poll reads it, so that
 * Main and All each keep their own copy of what is Done, as they do after any read of Main. Answers the
 * page and the timeline reads made so far.
 */
async function mainAndAllHeld(script = SCRIPT, arrange = null) {
  const page = newPage(new Map(), script);
  const data = threadData();
  page.threadingSupported = true;
  page.threads = data.threads;
  if (arrange) arrange(page);
  await signIn(page);
  await showDiscord(page, data.messages);
  await pickThread(page, "main");
  await reReadChannel(page);
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"]);
  return { page, reads: page.timelineCalls.length };
}

const doneDrawn = (page, id) => channelRow(page, id).getAttribute("data-archived");

test("`#306 done-stale-across-views`: a Done in Main is drawn in All, held from earlier, as All is shown, with no read; an Undo in All is drawn in Main", async () => {
  const run = async (script = SCRIPT) => {
    const { page, reads } = await mainAndAllHeld(script);
    await doneButton(channelRow(page, "201")).click();
    await page.settle();
    assert.equal(doneDrawn(page, "201"), "true", "the premise is 201 marked Done in Main");
    await pickThread(page, "flat");
    return { page, reads };
  };
  /** A Done on 203 in All, whose write saves the snapshot from All's copy; answers whether that kept 201 Done. */
  const laterSave = async (page) => {
    await doneButton(channelRow(page, "203")).click();
    await page.settle();
    return savedScopes(page)[scopeKey(CHANNEL.id)].dismissed.includes("201");
  };
  const { page, reads } = await run();
  assert.deepStrictEqual(shownIds(page), ["200", "201", "202", "203", "204"]);
  assert.equal(doneDrawn(page, "201"), "true", "the message marked Done in Main shows as not Done in All");
  assert.equal(doneButton(channelRow(page, "201")).text(), "Unarchive", "All's Done button still offers Done");
  assert.deepStrictEqual(page.timelineCalls.slice(reads), [], "All waited on a read to draw the Done");

  // Undo, from All: put back in All, and in Main as Main is shown again.
  await page.el("undo-dismiss").click();
  await page.settle();
  assert.deepStrictEqual(page.restoreCalls, [{ messages: ["201"] }]);
  assert.equal(doneDrawn(page, "201"), "false", "the premise is 201 put back in All");
  await pickThread(page, "main");
  assert.equal(doneDrawn(page, "201"), "false", "the message put back in All still shows as Done in Main");
  assert.equal(doneButton(channelRow(page, "201")).text(), "Done");
  assert.deepStrictEqual(page.timelineCalls.slice(reads), [], "Main waited on a read to draw the Undo");

  // A view shown again with nothing changed in it keeps its very rows: the check redraws nothing then.
  const drawn = [...page.el("discord-log").children];
  await pickThread(page, "flat");
  await pickThread(page, "main");
  assert.ok(page.el("discord-log").children.every((li, at) => li === drawn[at]), "a held view nothing changed was drawn again");

  // Done in Main once more, and a later write from All, which saves the snapshot from All's copy: the
  // Done is kept, there and in Main.
  await doneButton(channelRow(page, "201")).click();
  await page.settle();
  await pickThread(page, "flat");
  assert.ok(await laterSave(page), "a write from All saved the snapshot without the Done made in Main");
  await pickThread(page, "main");
  assert.equal(doneDrawn(page, "201"), "true", "a write from All took the Done made in Main out of Main");

  // THE CONTROLS. Without the views held brought into line, All shows the Done made in Main as not Done,
  // and the next write from All saves that stale copy as the store's, so the Done goes from every view.
  // With them brought into line but not drawn again as they are shown, All's rows show what they showed
  // when they were left. Without an Undo reaching them, Main shows the message put back as Done.
  const unfollowed = await run(brokenScript(...DONE_FOLLOWS));
  assert.equal(doneDrawn(unfollowed.page, "201"), "false", "without the views held brought into line All showed the Done, so this test cannot see it");
  assert.equal(await laterSave(unfollowed.page), false,
    "the stale copy did not reach the snapshot, so this test's claim about it is untested");
  const undrawn = await run(brokenScript(...HELD_REDRAWS));
  assert.equal(doneDrawn(undrawn.page, "201"), "false", "without the redraw as All is shown All showed the Done, so this test cannot see it");
  const unundone = await run(brokenScript(...UNDO_FOLLOWS));
  await unundone.page.el("undo-dismiss").click();
  await unundone.page.settle();
  await pickThread(unundone.page, "main");
  assert.equal(doneDrawn(unundone.page, "201"), "true", "without the Undo reaching the views held Main showed it, so this test cannot see it");
});

test("`#306 done-stale-across-views`: a Done or an Undo the server refuses is taken back in every view held, as All is shown", async () => {
  const refuse = (kind) => (page) => {
    page.inboxWriteHook = async (asked) => (asked === kind ? json(502, { error: "store_error", detail: "the store is read-only" }) : null);
  };
  const run = async (kind, script = SCRIPT) => {
    const { page } = await mainAndAllHeld(script);
    if (kind === "restore") {
      await doneButton(channelRow(page, "201")).click();
      await page.settle();
    }
    refuse(kind)(page);
    await doneButton(channelRow(page, "201")).click();
    await page.settle();
    assert.equal(page.el("error").hidden, false, `the premise is a ${kind} refused, and said`);
    await pickThread(page, "flat");
    return page;
  };
  // A Done refused in Main: 201 is not Done in All either. An Undo refused: it is Done in All still.
  assert.equal(doneDrawn(await run("dismiss"), "201"), "false", "a Done the server refused shows in All as made");
  assert.equal(doneDrawn(await run("restore"), "201"), "true", "an Undo the server refused shows in All as made");
  // THE CONTROLS: without the failure taken back in the views held, each shows in All as made.
  const done = await run("dismiss", brokenScript("    followDoneEverywhere(requested, column);\n    renderTodoControls();",
    "    renderTodoControls();"));
  assert.equal(doneDrawn(done, "201"), "true", "without the take-back reaching the views held the refused Done did not show, so this test cannot see it");
  const undo = await run("restore", brokenScript("    followDoneEverywhere(restored, column);\n    renderTodoControls();",
    "    renderTodoControls();"));
  assert.equal(doneDrawn(undo, "201"), "false", "without the take-back reaching the views held the refused Undo did not show, so this test cannot see it");
});

/** The channel of `threadData` in All, read once: `threadPage`, on `script`. */
async function threadPageOn(script) {
  const page = newPage(new Map(), script);
  const data = threadData();
  page.threadingSupported = true;
  page.threads = data.threads;
  await signIn(page);
  await showDiscord(page, data.messages);
  return page;
}

test("`#306 done-stale-across-views`: the right column's thread, put away with two columns, shows a Done made meanwhile in one column as two columns come back", async () => {
  const run = async (script = SCRIPT) => {
    const page = await threadPageOn(script);
    await pickThread(page, "main");
    await pickThread(page, "thread:spaces/A/threads/one & two");
    await twoColumns(page);
    assert.deepStrictEqual(sideIds(page), ["201", "202"], "the premise is the thread on the right");
    assert.equal(sideRow(page, "202").getAttribute("data-archived"), "false");
    await page.el("column-toggle").click();
    await page.settle();
    assert.equal(page.el("thread-select").value, "thread:spaces/A/threads/one & two", "the premise is the thread in one column");
    const reads = page.timelineCalls.length;
    await doneButton(channelRow(page, "202")).click();
    await page.settle();
    assert.equal(doneDrawn(page, "202"), "true", "the premise is the reply marked Done in one column");
    await page.el("column-toggle").click();
    await page.settle();
    assert.deepStrictEqual(sideIds(page), ["201", "202"], "two columns did not come back to the thread");
    return { page, reads };
  };
  const { page, reads } = await run();
  assert.equal(sideRow(page, "202").getAttribute("data-archived"), "true", "the right column shows the reply marked Done meanwhile as not Done");
  assert.deepStrictEqual(page.timelineCalls.slice(reads).filter((call) => viewOf(call) === "thread"), [],
    "the right column waited on a read of the thread to draw the Done");
  // And the right column's copy holds it, so a Done read there says so: its button offers Unarchive.
  assert.equal(doneButton(sideRow(page, "202")).text(), "Unarchive");

  // THE CONTROLS. Without the right column's copy brought into line while it is put away, or without its
  // rows drawn again as it comes back, the reply shows there as not Done.
  const unfollowed = await run(brokenScript(
    "  if (!sideShowing() && activeColumn !== columns.side && columns.side.left) {", "  if (false) {"));
  assert.equal(unfollowed.page.el("side-log").children.find((li) => li.getAttribute("data-id") === "202").getAttribute("data-archived"),
    "false", "without the right column's copy brought into line it showed the Done, so this test cannot see it");
  const undrawn = await run(brokenScript("      if (rowsMissDone()) renderCachedTimeline();\n      return undefined;",
    "      return undefined;"));
  assert.equal(undrawn.page.el("side-log").children.find((li) => li.getAttribute("data-id") === "202").getAttribute("data-archived"),
    "false", "without the redraw as two columns come back it showed the Done, so this test cannot see it");
});

test("`#306 done-stale-across-views`: a thread's card on the right counts an Undo made with its thread closed at once, with no read", async () => {
  const one = "spaces/A/threads/one & two";
  const run = async (script = SCRIPT) => {
    const { page } = await twoColumnServer(null, script);
    assert.equal(listCardSays(page, one).count, "1 reply · 1 unread", "the premise is a card counting its unread reply");
    await threadBadge(channelRow(page, "201")).click();
    await page.settle();
    assert.deepStrictEqual(sideIds(page), ["201", "202"]);
    await doneButton(sideRow(page, "202")).click();
    await page.settle();
    await page.el("side-close").click();
    await page.settle();
    assert.equal(listCardSays(page, one).count, "1 reply", "the premise is the card counting the reply Done");
    const reads = page.timelineCalls.length;
    await page.el("undo-dismiss").click();
    await page.settle();
    assert.deepStrictEqual(page.restoreCalls, [{ messages: ["202"] }], "the premise is the Undo put back on the server");
    return { page, reads };
  };
  const { page, reads } = await run();
  assert.equal(listCardSays(page, one).count, "1 reply · 1 unread", "the card does not count the reply the Undo put back");
  assert.deepStrictEqual(page.timelineCalls.slice(reads), [], "the card waited on a read to count the Undo");
  // THE CONTROL: without the cards drawn again after the Undo, the card says nothing unread.
  const control = await run(brokenScript("    });\n  }\n  if (sideColumnUp()) renderSideColumn();\n}", "    });\n  }\n}"));
  assert.equal(listCardSays(control.page, one).count, "1 reply", "without the cards drawn again the card counted the Undo, so this test cannot see it");
});

// A Done or an Undo made in the right column reaches the left one through `mirrorArchived`, which works
// only while the right column shows a thread. Found in the third review round of #306: with the right
// column's thread closed, or two columns put away, while the write was on its way, a refusal was taken
// back in the right column and in the views held, but the left column on screen kept showing it as
// made, and the next write from there saved that over the store, so the device kept a Done the server
// had refused, or dropped one it had kept.

/** The left column's own catch-up after a Done or an Undo made in the right column, taken out for a negative control. */
const LEFT_FOLLOWS = ["  if (madeIn === columns.side && !sideShowing()) {", "  if (false) {"];
/** ...its rows drawn again where they disagree with its copy, taken out: only Main's counts drawn. */
const LEFT_REDRAWS = ["        if (rowsMissDone()) renderCachedTimeline();\n        else syncUnreadReplies();",
  "        syncUnreadReplies();"];
/** ...Main's roots' counts of unread replies drawn again, taken out: only rows that disagree drawn. */
const LEFT_COUNTS = ["        if (rowsMissDone()) renderCachedTimeline();\n        else syncUnreadReplies();",
  "        if (rowsMissDone()) renderCachedTimeline();"];

/**
 * Two columns over the channel of `threadData`, thread one & two on the right; a Done on message `id`
 * in the right column, or for `kind` "restore" an Undo of a Done on it saved first, its write held on
 * its way to the server; `meanwhile` done while it is held; then the write refused. `before` is done
 * once the thread is on the right, before anything is marked. Answers the page and the timeline reads
 * made by the time it was refused.
 */
async function refusedFromTheRight(kind, meanwhile, id, script = SCRIPT, before = null) {
  const { page } = await twoColumnServer(null, script);
  await threadBadge(channelRow(page, "201")).click();
  await page.settle();
  assert.deepStrictEqual(sideIds(page), ["201", "202"], "the premise is the thread on the right");
  if (before) await before(page);
  if (kind === "restore") {
    await doneButton(sideRow(page, id)).click();
    await page.settle();
    assert.deepStrictEqual(page.dismissCalls, [{ messages: [id] }], "the premise is a Done saved, to undo");
  }
  let release = null;
  page.inboxWriteHook = async (asked) => (asked === kind && release === null
    ? new Promise((resolve) => {
      release = () => resolve(json(502, { error: "store_error", detail: "the store is read-only" }));
    })
    : null);
  // Not waited for: the button's act ends with its write, which is held.
  const pressed = doneButton(sideRow(page, id)).click();
  await page.settle();
  assert.ok(release, `the premise is the ${kind} on its way`);
  // Drawn so at once; under Hide read a Done takes its row off at once.
  const drawn = sideRow(page, id);
  if (drawn) assert.equal(drawn.getAttribute("data-archived"), String(kind === "dismiss"), "the premise is the act drawn at once");
  else assert.ok(kind === "dismiss" && before === hideRead, "the premise is the act drawn at once");
  await meanwhile(page);
  await page.settle();
  const reads = page.timelineCalls.length;
  release();
  await pressed.catch(() => {});
  for (let i = 0; i < 4; i += 1) await page.settle();
  assert.equal(page.el("error").hidden, false, `the premise is the ${kind} refused, and said`);
  return { page, reads };
}

/** The read state of message `id` in the snapshot of the channel the device keeps. */
const savedDone = (page, id) => savedScopes(page)[scopeKey(CHANNEL.id)].dismissed.includes(id);

/** What the root 201 says about its thread's unread replies on Main, or null when it says nothing. */
const rootUnread = (page) => (unreadChip(page, "201") ? unreadChip(page, "201").textContent : null);

/** The right column's thread closed while the write is on its way: the cards come back, and Main stays on the left. */
const closeTheThread = async (page) => { await page.el("side-close").click(); };
/** Two columns put away while the write is on its way: All comes back in one column. */
const putColumnsAway = async (page) => { await page.el("column-toggle").click(); };
/** Hide read, from the read mode control, which steps through the modes. */
const hideRead = async (page) => {
  for (let i = 0; i < 3 && page.el("todo-filter").getAttribute("data-read-mode") !== "hide"; i += 1) {
    await page.el("todo-filter").click();
    await page.settle();
  }
  assert.equal(page.el("todo-filter").getAttribute("data-read-mode"), "hide", "the premise is Hide read");
};

test("`#306 done-stale-across-views`: a Done or an Undo made in the right column and refused after its thread is closed is taken back on Main at once, and a later write from Main does not save it", async () => {
  // On root 201, drawn on Main: drawn as it was, and kept so in the snapshot through a Done on 203 from Main.
  for (const kind of ["dismiss", "restore"]) {
    const was = String(kind === "restore");
    const { page, reads } = await refusedFromTheRight(kind, closeTheThread, "201");
    assert.equal(page.el("thread-select").value, "columns", "the premise is two columns, showing their cards");
    assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "the premise is Main on the left");
    assert.equal(doneDrawn(page, "201"), was, `a ${kind} the server refused shows on Main as made`);
    assert.deepStrictEqual(page.timelineCalls.slice(reads), [], `Main waited on a read to take back the refused ${kind}`);
    await doneButton(channelRow(page, "203")).click();
    await page.settle();
    assert.equal(savedDone(page, "201"), kind === "restore",
      `a later write from Main saved the ${kind} the server refused into the device's snapshot`);
    assert.equal(doneDrawn(page, "201"), was, `a later write from Main drew the refused ${kind} again`);
  }
  // On reply 202, which Main does not draw: its root's count of unread replies says so again at once.
  const done = await refusedFromTheRight("dismiss", closeTheThread, "202");
  assert.equal(rootUnread(done.page), "1 unread", "after a refused Done on its reply the root counts nothing unread");
  const undo = await refusedFromTheRight("restore", closeTheThread, "202");
  assert.equal(rootUnread(undo.page), null, "after a refused Undo of its reply the root counts it unread");

  // THE CONTROLS. Without the left column's own copy brought into line, Main shows each refusal as made,
  // and the next write from it saves the refused state; its root counts the reply wrongly. With the copy
  // brought into line but Main's counts not drawn again, the root counts the reply wrongly still.
  const unfollowed = brokenScript(...LEFT_FOLLOWS);
  for (const kind of ["dismiss", "restore"]) {
    const { page } = await refusedFromTheRight(kind, closeTheThread, "201", unfollowed);
    assert.equal(doneDrawn(page, "201"), String(kind === "dismiss"),
      `without the left column brought into line Main showed the refused ${kind} taken back, so this test cannot see it`);
    await doneButton(channelRow(page, "203")).click();
    await page.settle();
    assert.equal(savedDone(page, "201"), kind === "dismiss",
      `without the left column brought into line the snapshot kept the refused ${kind} out, so this test's claim about it is untested`);
  }
  assert.equal(rootUnread((await refusedFromTheRight("dismiss", closeTheThread, "202", unfollowed)).page), null,
    "without the left column brought into line the root counted the reply put back, so this test cannot see it");
  assert.equal(rootUnread((await refusedFromTheRight("restore", closeTheThread, "202", unfollowed)).page), "1 unread",
    "without the left column brought into line the root counted the reply Done again, so this test cannot see it");
  assert.equal(rootUnread((await refusedFromTheRight("dismiss", closeTheThread, "202", brokenScript(...LEFT_COUNTS))).page), null,
    "without Main's counts drawn again the root counted the reply put back, so this test cannot see it");
});

test("`#306 done-stale-across-views`: a Done or an Undo made in the right column and refused after two columns are put away is taken back in All, then on screen, and in every view shown after it", async () => {
  for (const kind of ["dismiss", "restore"]) {
    const was = String(kind === "restore");
    for (const id of ["201", "202"]) {
      const { page, reads } = await refusedFromTheRight(kind, putColumnsAway, id);
      assert.equal(page.el("thread-select").value, "flat", "the premise is All in one column");
      assert.equal(doneDrawn(page, id), was, `a ${kind} of ${id} the server refused shows in All as made`);
      assert.deepStrictEqual(page.timelineCalls.slice(reads), [], `All waited on a read to take back the refused ${kind}`);
      // A later write from All, which saves the snapshot from All's copy.
      await doneButton(channelRow(page, "203")).click();
      await page.settle();
      assert.equal(savedDone(page, id), kind === "restore",
        `a later write from All saved the ${kind} of ${id} the server refused into the device's snapshot`);
      // The thread opened next in the one column, and Main.
      await pickThread(page, "thread:spaces/A/threads/one & two");
      assert.deepStrictEqual(shownIds(page), ["201", "202"]);
      assert.equal(doneDrawn(page, id), was, `the thread opened after the refused ${kind} of ${id} shows it as made`);
      if (id === "201") {
        await pickThread(page, "main");
        assert.equal(doneDrawn(page, id), was, `Main shows the refused ${kind} of ${id} as made`);
      }
    }
  }
  // Under Hide read, the root a refused Done took off All is back on it at once.
  const hidden = await refusedFromTheRight("dismiss", putColumnsAway, "201", SCRIPT, hideRead);
  assert.deepStrictEqual(shownIds(hidden.page), ["200", "201", "202", "203", "204"],
    "under Hide read All leaves out the root whose Done the server refused");

  // THE CONTROLS. Without the left column's own copy brought into line, All shows the refusal as made,
  // and its next write saves it. With the copy brought into line but All's rows not drawn again, All
  // shows it as made still, though the snapshot is right.
  for (const kind of ["dismiss", "restore"]) {
    const made = String(kind === "dismiss");
    const unfollowed = await refusedFromTheRight(kind, putColumnsAway, "201", brokenScript(...LEFT_FOLLOWS));
    assert.equal(doneDrawn(unfollowed.page, "201"), made,
      `without the left column brought into line All showed the refused ${kind} taken back, so this test cannot see it`);
    await doneButton(channelRow(unfollowed.page, "203")).click();
    await unfollowed.page.settle();
    assert.equal(savedDone(unfollowed.page, "201"), kind === "dismiss",
      `without the left column brought into line the snapshot kept the refused ${kind} out, so this test's claim about it is untested`);
    const undrawn = await refusedFromTheRight(kind, putColumnsAway, "201", brokenScript(...LEFT_REDRAWS));
    assert.equal(doneDrawn(undrawn.page, "201"), made,
      `without All's rows drawn again All showed the refused ${kind} taken back, so this test cannot see it`);
  }
  for (const [patch, without] of [[LEFT_FOLLOWS, "the left column brought into line"], [LEFT_REDRAWS, "All's rows drawn again"]]) {
    const control = await refusedFromTheRight("dismiss", putColumnsAway, "201", brokenScript(...patch), hideRead);
    assert.deepStrictEqual(shownIds(control.page), ["200", "202", "203", "204"],
      `without ${without} Hide read brought the root back, so this test cannot see it`);
  }
});

// The mirror image, found in the fourth review round of #306. A Done or an Undo made in the left column
// while the right column's thread is put away reaches the right column's copy at once (above), which
// leaves nothing for the `mirrorArchived` that takes a refusal back to change. With the thread shown on
// the right again (opened again, or two columns brought back) while the write was on its way, a refusal
// was taken back on the left only: the right column kept showing it as made; a Reply sent from there
// wrote no Done, its message looking Done already, and asked the chat service to mark read a message
// whose Done the server had refused; and the next write from the right saved the refused state over
// the store.

/** The right column's own catch-up after a Done or an Undo made in the left column, taken out for a negative control. */
const RIGHT_FOLLOWS = ["  if (madeIn !== columns.side && sideShowing()) {", "  if (false) {"];
/** ...its rows drawn again where they disagree with its copy, taken out. */
const RIGHT_REDRAWS = ["      if (rowsMissDone()) renderCachedTimeline();\n    });\n  }\n  if (sideColumnUp())",
  "    });\n  }\n  if (sideColumnUp())"];

/** The right column's thread closed, Main on the left beside the cards; then opened again from its root's tag. */
const closedThenReopened = {
  away: closeTheThread,
  view: "columns",
  back: async (page) => { await threadBadge(channelRow(page, "201")).click(); },
  says: "its thread opened again",
};
/** Two columns put away, All in one column; then brought back, the thread on the right again. */
const columnsOffThenOn = { away: putColumnsAway, view: "flat", back: putColumnsAway, says: "two columns brought back" };

/**
 * Two columns over the channel of `threadData`, thread one & two on the right; that thread put away
 * (`how.away`); a Done on root 201 in the left column, or for `kind` "restore" an Undo of a Done on it
 * saved first, its write held on its way to the server; the thread shown on the right again
 * (`how.back`); then the write refused, or with `saved` saved. `arrange` is given the page before it
 * signs in; `before` is done once the thread is on the right, before it is put away. Answers the page
 * and the timeline reads made by the time the write was answered.
 */
async function answeredFromTheLeft(kind, how, { script = SCRIPT, arrange = null, before = null, saved = false } = {}) {
  const { page } = await twoColumnServer(arrange, script);
  await threadBadge(channelRow(page, "201")).click();
  await page.settle();
  assert.deepStrictEqual(sideIds(page), ["201", "202"], "the premise is the thread on the right");
  if (before) await before(page);
  await how.away(page);
  await page.settle();
  assert.equal(page.el("thread-select").value, how.view, "the premise is the right column's thread put away");
  if (kind === "restore") {
    await doneButton(channelRow(page, "201")).click();
    await page.settle();
    assert.deepStrictEqual(page.dismissCalls, [{ messages: ["201"] }], "the premise is a Done saved, to undo");
  }
  let release = null;
  page.inboxWriteHook = async (asked) => (asked === kind && release === null
    ? new Promise((resolve) => {
      release = () => resolve(saved ? null : json(502, { error: "store_error", detail: "the store is read-only" }));
    })
    : null);
  // Not waited for: the button's act ends with its write, which is held.
  const pressed = doneButton(channelRow(page, "201")).click();
  await page.settle();
  assert.ok(release, `the premise is the ${kind} on its way`);
  const made = String(kind === "dismiss");
  // Drawn so at once; under Hide read a Done takes its row off at once.
  const drawn = channelRow(page, "201");
  if (drawn) assert.equal(drawn.getAttribute("data-archived"), made, "the premise is the act drawn at once");
  else assert.ok(kind === "dismiss" && before === hideRead, "the premise is the act drawn at once");
  await how.back(page);
  await page.settle();
  assert.deepStrictEqual(sideIds(page), drawn ? ["201", "202"] : ["202"], "the premise is the thread on the right again, showing the act");
  if (drawn) {
    assert.equal(sideRow(page, "201").getAttribute("data-archived"), made, "the premise is the right column showing the act as made");
  }
  const reads = page.timelineCalls.length;
  release();
  await pressed.catch(() => {});
  for (let i = 0; i < 4; i += 1) await page.settle();
  assert.equal(page.el("error").hidden, saved, `the premise is the ${kind} ${saved ? "saved" : "refused, and said"}`);
  return { page, reads };
}

/** Whether message `id` is drawn Done in the right column's thread. */
const sideDoneDrawn = (page, id) => sideRow(page, id).getAttribute("data-archived");

test("`#306 done-stale-across-views`: a Done or an Undo made in the left column while the right column's thread is put away, and refused once it is shown again, is taken back on the right at once, and a later write from the right does not save it", async () => {
  for (const how of [closedThenReopened, columnsOffThenOn]) {
    for (const kind of ["dismiss", "restore"]) {
      const was = String(kind === "restore");
      const { page, reads } = await answeredFromTheLeft(kind, how);
      assert.equal(sideDoneDrawn(page, "201"), was, `a ${kind} the server refused shows on the right as made, ${how.says}`);
      assert.equal(doneButton(sideRow(page, "201")).text(), kind === "restore" ? "Unarchive" : "Done",
        `the right column's button offers what the refused ${kind} made, ${how.says}`);
      assert.equal(doneDrawn(page, "201"), was, `a ${kind} the server refused shows on the left as made, ${how.says}`);
      assert.deepStrictEqual(page.timelineCalls.slice(reads), [], `the right column waited on a read to take back the refused ${kind}`);
      // A later write from the right, which saves the snapshot from the right column's copy.
      await doneButton(sideRow(page, "202")).click();
      await page.settle();
      assert.equal(savedDone(page, "201"), kind === "restore",
        `a later write from the right saved the ${kind} the server refused into the device's snapshot, ${how.says}`);
      assert.equal(sideDoneDrawn(page, "201"), was, `a later write from the right drew the refused ${kind} again, ${how.says}`);
      assert.equal(doneDrawn(page, "201"), was, `a later write from the right drew the refused ${kind} again on the left, ${how.says}`);

      // Saved, the act stands on both sides and in the snapshot.
      const made = String(kind === "dismiss");
      const kept = await answeredFromTheLeft(kind, how, { saved: true });
      assert.equal(sideDoneDrawn(kept.page, "201"), made, `a ${kind} the server saved shows on the right as not made, ${how.says}`);
      assert.equal(doneDrawn(kept.page, "201"), made, `a ${kind} the server saved shows on the left as not made, ${how.says}`);
      await doneButton(sideRow(kept.page, "202")).click();
      await kept.page.settle();
      assert.equal(savedDone(kept.page, "201"), kind === "dismiss",
        `a later write from the right left the ${kind} the server saved out of the snapshot, ${how.says}`);
    }
  }
  // Under Hide read, the root a refused Done took off the right column's thread is back on it at once.
  const hidden = await answeredFromTheLeft("dismiss", columnsOffThenOn, { before: hideRead });
  assert.deepStrictEqual(sideIds(hidden.page), ["201", "202"],
    "under Hide read the right column leaves out the root whose Done the server refused");
  assert.equal(sideDoneDrawn(hidden.page, "201"), "false");

  // THE CONTROLS. Without the right column's copy brought into line, it shows the refusal as made, and
  // its next write saves it. With the copy brought into line but its rows not drawn again, it shows it
  // as made still, though the snapshot is right.
  for (const how of [closedThenReopened, columnsOffThenOn]) {
    for (const kind of ["dismiss", "restore"]) {
      const made = String(kind === "dismiss");
      const unfollowed = await answeredFromTheLeft(kind, how, { script: brokenScript(...RIGHT_FOLLOWS) });
      assert.equal(sideDoneDrawn(unfollowed.page, "201"), made,
        `without the right column brought into line it showed the refused ${kind} taken back, ${how.says}, so this test cannot see it`);
      await doneButton(sideRow(unfollowed.page, "202")).click();
      await unfollowed.page.settle();
      assert.equal(savedDone(unfollowed.page, "201"), kind === "dismiss",
        `without the right column brought into line the snapshot kept the refused ${kind} out, ${how.says}, so this test's claim about it is untested`);
      const undrawn = await answeredFromTheLeft(kind, how, { script: brokenScript(...RIGHT_REDRAWS) });
      assert.equal(sideDoneDrawn(undrawn.page, "201"), made,
        `without the right column's rows drawn again it showed the refused ${kind} taken back, ${how.says}, so this test cannot see it`);
    }
  }
  for (const [patch, without] of [[RIGHT_FOLLOWS, "the right column brought into line"], [RIGHT_REDRAWS, "its rows drawn again"]]) {
    const control = await answeredFromTheLeft("dismiss", columnsOffThenOn, { script: brokenScript(...patch), before: hideRead });
    assert.deepStrictEqual(sideIds(control.page), ["202"],
      `without ${without} Hide read brought the root back on the right, so this test cannot see it`);
  }
});

test("`#306 done-stale-across-views`: a Reply sent from the right column on a message whose Done, made in the left column, the server refused once the thread was shown again writes its Done before it asks the chat service", async () => {
  // `#300 reply-marks-read`: a confirmed send marks its message Done, and only once that is saved asks
  // the chat service to move its own read marker, which cannot be moved back.
  const run = async (how, script = SCRIPT) => {
    let asked = null;
    const { page } = await answeredFromTheLeft("dismiss", how, {
      script,
      arrange: (fresh) => { fresh.upstreamReadMarkSupported = true; asked = serverFloor(fresh); },
    });
    assert.equal(page.dealtWith.has("201"), false, "the premise is the server holding no Done for 201");
    page.inboxWriteHook = null;
    await replyAndSend(page, "201", "on it", "side-log");
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.equal(page.repliesPosted.at(-1).body.reply_to, "201", "the premise is the reply sent and confirmed");
    return { page, asked };
  };
  for (const how of [closedThenReopened, columnsOffThenOn]) {
    const { page, asked } = await run(how);
    assert.deepStrictEqual(page.dismissCalls, [{ messages: ["201"] }, { messages: ["201"] }],
      `the reply wrote no Done for a message whose Done the server refused, ${how.says}`);
    assert.equal(page.dealtWith.has("201"), true, `the reply's Done did not reach the server, ${how.says}`);
    const dismissed = page.requests.map((one, at) => (one.endsWith("/dismiss") ? at : -1)).filter((at) => at >= 0);
    const marked = page.requests.findIndex((one) => one.endsWith("/upstream-read"));
    assert.ok(dismissed.length === 2 && marked > dismissed[1], `the chat service was asked before the reply's Done was written, ${how.says}`);
    assert.deepStrictEqual(page.upstreamReadCalls, [{ message_id: "201", after_reply: true }]);
    assert.deepStrictEqual(asked, ["201"]);
    assert.match(page.el("status").textContent, /^Marked read here and in Discord\./);
    assert.equal(sideDoneDrawn(page, "201"), "true", `the reply's Done does not show on the right, ${how.says}`);
    assert.equal(doneDrawn(page, "201"), "true", `the reply's Done does not show on the left, ${how.says}`);
  }
  // THE CONTROL: without the right column brought into line, the refused Done still shows there, so the
  // reply writes none and asks the chat service to mark read a message the server holds no Done for.
  for (const how of [closedThenReopened, columnsOffThenOn]) {
    const { page, asked } = await run(how, brokenScript(...RIGHT_FOLLOWS));
    assert.deepStrictEqual(page.dismissCalls, [{ messages: ["201"] }],
      `without the right column brought into line the reply wrote a Done, ${how.says}, so this test cannot see it`);
    assert.deepStrictEqual(asked, ["201"],
      `without the right column brought into line the chat service was not asked, ${how.says}, so this test cannot see it`);
    assert.equal(page.dealtWith.has("201"), false);
  }
});

// --- `#311 sent-read-waits-for-done` ------------------------------------------------------------
//
// Found in review of #295 shift-e-read-collapse. A confirmed send marks read what it answers: Done,
// then the chat service's own read marker moved through it (`#300 reply-marks-read`). A message is
// drawn Done the moment it is marked, before its write is even sent, so a send confirming while that
// Done was still being saved took it for saved and asked the service at once; a Done that then failed
// left the message unread here and read in the service, whose marker cannot be moved back. Shift+E
// already waited (`doneSaved`, #295); a confirmed send now waits the same way.

/** The line in `markAnsweredDone` that waits for a Done still on its way and looks again, for negative controls. */
const SENT_WAITS = "    return doneSaved(ids, () => String(selectedChannelId()) === channel && doneHeldHere(ids));\n";

/**
 * Message 3 of `conversation` marked Done with its Done button, the write held on its way to the server;
 * a Reply on it sent and confirmed meanwhile; then the Done answered `answer`, or saved where that is
 * null. Answers the page, what was asked of the chat service while the Done was on its way, and the
 * server's floor's asks of the service.
 */
async function replyWhileDoneSaving(answer, script = SCRIPT) {
  const target = "1000000000000000003";
  const page = newPage(new Map(), script);
  page.upstreamReadMarkSupported = true;
  const asked = serverFloor(page);
  await signIn(page);
  await showDiscord(page, conversation());
  let release = () => {};
  page.inboxWriteHook = async (kind) => (kind === "dismiss"
    ? new Promise((resolve) => { release = () => resolve(answer); }) : null);
  // Not waited for: the button's act ends with the Done's write, which is held.
  const pressed = doneButton(rowWithId(page, target)).click();
  await page.settle();
  assert.deepStrictEqual(page.dismissCalls, [{ messages: [target] }], "the premise is the Done on its way");
  assert.equal(rowWithId(page, target).getAttribute("data-archived"), "true", "the premise is the message drawn Done already");
  await replyAndSend(page, target, "answering while the Done is saved");
  assert.equal(page.repliesPosted.at(-1).body.reply_to, target, "the premise is the reply sent and confirmed");
  await page.settle();
  const whileSaving = [...page.upstreamReadCalls];
  release();
  await pressed.catch(() => {});
  for (let i = 0; i < 3; i += 1) await page.settle();
  return { page, target, whileSaving, asked };
}

test("`#311 sent-read-waits-for-done`: a send confirmed while its message's Done is being saved asks the chat service only once that Done is saved, and nothing after it failed", async () => {
  const failing = json(502, { error: "store_error", detail: "the store is read-only" });
  // Failed: nothing asked, before or after; the message unread here, and the failure said.
  const failed = await replyWhileDoneSaving(failing);
  assert.deepStrictEqual(failed.whileSaving, [], "the chat service was asked before the Done was saved");
  assert.deepStrictEqual(failed.page.upstreamReadCalls, [], "the chat service was asked after the Done failed");
  assert.deepStrictEqual(failed.asked, [], "the chat service's marker was moved for a Done that failed");
  assert.equal(rowWithId(failed.page, failed.target).getAttribute("data-archived"), "false", "a Done that failed left the row Done");
  assert.equal(failed.page.el("error").hidden, false, "a Done that failed said nothing");
  assert.doesNotMatch(failed.page.el("status").textContent, /^Marked read here and in/, "a Done that failed was said as a read in the service");
  // Saved: asked once it is, after it, as a reply's mark, and said; no second Done was written.
  const saved = await replyWhileDoneSaving(null);
  assert.deepStrictEqual(saved.whileSaving, [], "the chat service was asked before the Done was saved");
  assert.deepStrictEqual(saved.page.dismissCalls, [{ messages: [saved.target] }], "the send wrote a second Done");
  assert.deepStrictEqual(saved.page.upstreamReadCalls, [{ message_id: saved.target, after_reply: true }],
    "the saved Done was not marked in the chat service as a reply's mark");
  assert.deepStrictEqual(saved.asked, [saved.target]);
  const dismissed = saved.page.requests.findIndex((one) => one.endsWith("/dismiss"));
  const marked = saved.page.requests.findIndex((one) => one.endsWith("/upstream-read"));
  assert.ok(dismissed >= 0 && marked > dismissed, "the chat service was asked before the Done was written");
  assert.match(saved.page.el("status").textContent, /^Marked read here and in Discord\./, "the mark in the chat service was not said");
  assert.equal(rowWithId(saved.page, saved.target).getAttribute("data-archived"), "true");
  assert.equal(saved.page.el("error").hidden, true, `the mark put up the error banner: ${saved.page.el("error").textContent}`);

  // A message whose Done was saved before the send confirmed is asked of the service with no wait.
  const settled = newPage();
  settled.upstreamReadMarkSupported = true;
  await signIn(settled);
  await showDiscord(settled, conversation());
  await doneButton(rowWithId(settled, "1000000000000000003")).click();
  await settled.settle();
  await replyAndSend(settled, "1000000000000000003", "after the Done was saved");
  assert.deepStrictEqual(settled.upstreamReadCalls, [{ message_id: "1000000000000000003", after_reply: true }],
    "a message Done and saved before the send was not marked in the chat service");
  assert.equal(settled.dismissCalls.length, 1, "the send wrote a second Done over a saved one");

  // THE CONTROLS. Without the wait the send takes the Done on its way for a saved one: it asks the
  // service at once, so the marker moves for a Done that then fails. Waiting without looking again asks
  // it once that Done has failed.
  const unwaited = await replyWhileDoneSaving(failing, brokenScript(SENT_WAITS, "    return true;\n"));
  assert.deepStrictEqual(unwaited.whileSaving, [{ message_id: unwaited.target, after_reply: true }],
    "without the wait the service was not asked before the Done was saved, so this test cannot see it");
  assert.deepStrictEqual(unwaited.asked, [unwaited.target], "without the wait the service's marker did not move, so this test cannot see it");
  const unlooked = await replyWhileDoneSaving(failing, brokenScript(SENT_WAITS, "    return doneSaved(ids, () => true);\n"));
  assert.deepStrictEqual(unlooked.whileSaving, [], "the control waited no longer");
  assert.deepStrictEqual(unlooked.asked, [unlooked.target],
    "waiting without looking again asked nothing after the failed Done, so this test cannot see it");
});

test("`#311 sent-read-waits-for-done`: a Done put back while the send waits for it, or a channel left meanwhile, asks the chat service nothing", async () => {
  const here = message({ id: "3100000000000000001", channel_id: CHANNEL.id, content: "answer me" });
  const there = message({ id: "3100000000000000002", channel_id: OTHER_CHANNEL.id, content: "elsewhere" });
  /** `here` marked Done, held on its way; a Reply on it sent; `meanwhile`; then the Done saved. */
  const run = async (meanwhile, script = SCRIPT) => {
    const page = newPage(new Map(), script);
    page.upstreamReadMarkSupported = true;
    page.channels = [{ ...CHANNEL }, { ...OTHER_CHANNEL }];
    serveByChannel(page, { [CHANNEL.id]: [here], [OTHER_CHANNEL.id]: [there] });
    await signIn(page);
    await showDiscord(page, []);
    const held = [];
    page.inboxWriteHook = async (kind) => (kind === "dismiss" && held.length === 0
      ? new Promise((resolve) => { held.push(() => resolve(null)); }) : null);
    const pressed = doneButton(rowWithId(page, here.id)).click();
    await page.settle();
    assert.equal(held.length, 1, "the premise is the Done on its way");
    await replyAndSend(page, here.id, "answering while the Done is saved");
    assert.equal(page.repliesPosted.at(-1).body.reply_to, here.id, "the premise is the reply sent");
    const acted = meanwhile(page);
    await page.settle();
    held[0]();
    await pressed.catch(() => {});
    await acted;
    for (let i = 0; i < 3; i += 1) await page.settle();
    return page;
  };
  // Undo pressed while the Done is on its way: written once the Done is, and nothing read in the service.
  const undo = (page) => page.el("undo-dismiss").click();
  const undone = await run(undo);
  assert.deepStrictEqual(undone.restoreCalls, [{ messages: [here.id] }], "the premise is the Undo written");
  assert.equal(rowWithId(undone, here.id).getAttribute("data-archived"), "false");
  assert.deepStrictEqual(undone.upstreamReadCalls, [], "a message put back before its Done was saved was read in the chat service");
  // Another channel read meanwhile: the Done is saved, and nothing is asked for a channel the page has left.
  const away = (page) => readChannel(page, OTHER_CHANNEL.id);
  const left = await run(away);
  assert.deepStrictEqual(shownIds(left), [there.id], "the premise is the other channel on screen");
  assert.ok(left.dealtWith.has(here.id), "the premise is the Done saved");
  assert.deepStrictEqual(left.upstreamReadCalls, [], "a channel left before the Done was saved was read in the chat service");
  // THE CONTROLS. The Done waited for and not looked at again, both are asked of the service. Looked at
  // again without asking whether the channel is still the one on screen, the channel left is: the set
  // kept over the change still names the message, its Done having been on its way when the next
  // channel's read landed.
  const looked = brokenScript(SENT_WAITS, "    return doneSaved(ids, () => true);\n");
  assert.equal((await run(undo, looked)).upstreamReadCalls.length, 1,
    "without looking again the put back message was not asked, so this test cannot see it");
  assert.equal((await run(away, looked)).upstreamReadCalls.length, 1,
    "without looking again the channel left was not asked, so this test cannot see it");
  const anywhere = brokenScript(SENT_WAITS, "    return doneSaved(ids, () => doneHeldHere(ids));\n");
  assert.equal((await run(away, anywhere)).upstreamReadCalls.length, 1,
    "without the channel looked at the channel left was not asked, so this test cannot see it");
  assert.deepStrictEqual((await run(undo, anywhere)).upstreamReadCalls, [], "the channel's check is what keeps a put back unasked");
});

/** The line in `markAnsweredDone` that asks at once for a message no Done of its own is on its way for, for negative controls. */
const SENT_SAVED_ALREADY = "    if (!ids.some((one) => doneSaving.has(String(one)))) return true;\n";

/**
 * Found in review of #311 itself. Under Hide read on a channel without threads a message not drawn
 * counts as Done (`doneWhereNotDrawn`): it has left the list. A Done that took it off was not always
 * made on this page load — another device's, read since, or one from before a reload — and no set the
 * page keeps need name it, since a to-do read lists only what is not Done. Looking again for every
 * message already Done (`doneHeldHere`) found none of those and asked the chat service nothing, which
 * undid #300 reply-marks-read there. Only a Done actually on its way is waited for now.
 */
test("`#311 sent-read-waits-for-done`: under Hide read without threads a reply to a message Done on another device, or before a reload, asks the chat service at once; one whose Done here is held waits, and asks nothing once it fails", async () => {
  const target = "1000000000000000003";
  /** A Reply on message 3 sent under Hide read, its POST held; another device's Done read meanwhile; then the POST confirmed. */
  const elsewhere = async (script = SCRIPT) => {
    const page = newPage(new Map(), script);
    page.upstreamReadMarkSupported = true;
    const asked = serverFloor(page);
    await signIn(page);
    await showDiscord(page, conversation());
    await chooseReadMode(page, "Hide read");
    assert.ok(shownIds(page).includes(target), "the premise is the message listed under Hide read");
    const succeeds = page.replyResponse;
    let release = null;
    page.replyResponse = (...args) => new Promise((resolve) => { release = () => resolve(succeeds(...args)); });
    await replyButton(rowWithId(page, target)).click();
    await page.el("reply-text").setValue("answering");
    const sending = page.el("reply-send").click();
    await page.settle();
    assert.ok(release, "the premise is the reply's POST held");
    page.dealtWith.add(target);
    await reReadChannel(page);
    assert.equal(shownIds(page).includes(target), false, "the premise is the Done made elsewhere taking the message off the list");
    release();
    await sending;
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.equal(page.repliesPosted.length, 1, "the premise is the reply sent and confirmed");
    return { page, asked };
  };
  /** Hide read kept over a reload; message 3 Done on the server since; a send an earlier page saved, Retried and confirmed. */
  const reloaded = async (script = SCRIPT) => {
    const first = newPage();
    await signIn(first);
    await showDiscord(first, conversation());
    await chooseReadMode(first, "Hide read");
    first.storage.set("vibe-talk.outgoing-messages", JSON.stringify([{ id: "outgoing-earlier-page", channel: CHANNEL.id,
      text: "answering", remaining: "answering", state: "failed", threadId: null, replyTo: target, marksRead: target,
      createdAt: 1, postedCount: 0, parts: [], observedParts: [], detail: "refused" }]));
    let asked = [];
    const page = newPage(first.storage, script, (p) => {
      p.messages = conversation();
      p.upstreamReadMarkSupported = true;
      p.dealtWith.add(target);
      asked = serverFloor(p);
    });
    await reopenedChannel(page);
    assert.equal(page.el("todo-filter-label").text(), "Hide read", "the premise is Hide read kept over the reload");
    assert.equal(shownIds(page).includes(target), false, "the premise is the message Done, off the list");
    assert.equal(outgoingRows(page).length, 1, "the premise is the saved send");
    await outgoingRetry(outgoingRows(page)[0]).click();
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.equal(page.repliesPosted.length, 1, "the premise is the reply sent and confirmed");
    return { page, asked };
  };
  for (const [how, run] of [["Done on another device", elsewhere], ["Done before a reload", reloaded]]) {
    const { page, asked } = await run();
    assert.deepStrictEqual(page.upstreamReadCalls, [{ message_id: target, after_reply: true }],
      `${how}: the chat service was not asked to mark the message the reply answers`);
    assert.deepStrictEqual(asked, [target], `${how}: the chat service's marker did not move`);
    assert.deepStrictEqual(page.dismissCalls, [], `${how}: the send wrote a second Done over a saved one`);
    assert.match(page.el("status").textContent, /^Marked read here and in Discord\./, `${how}: the mark in the chat service was not said`);
    assert.equal(page.el("error").hidden, true, `${how}: the mark put up the error banner: ${page.el("error").textContent}`);
  }

  /**
   * A Reply on message 3 sent under Hide read, its POST held; its Done pressed meanwhile, which takes it
   * off the list, and held on its way; the POST confirmed; then the Done answered `answer`, or saved.
   */
  const heldHere = async (answer, script = SCRIPT) => {
    const page = newPage(new Map(), script);
    page.upstreamReadMarkSupported = true;
    const asked = serverFloor(page);
    await signIn(page);
    await showDiscord(page, conversation());
    await chooseReadMode(page, "Hide read");
    const succeeds = page.replyResponse;
    let send = null;
    page.replyResponse = (...args) => new Promise((resolve) => { send = () => resolve(succeeds(...args)); });
    await replyButton(rowWithId(page, target)).click();
    await page.el("reply-text").setValue("answering");
    const sending = page.el("reply-send").click();
    await page.settle();
    assert.ok(send, "the premise is the reply's POST held");
    let done = null;
    page.inboxWriteHook = async (kind) => (kind === "dismiss"
      ? new Promise((resolve) => { done = () => resolve(answer); }) : null);
    const pressed = doneButton(rowWithId(page, target)).click();
    await page.settle();
    assert.ok(done, "the premise is the Done on its way");
    assert.equal(shownIds(page).includes(target), false, "the premise is the Done taking the message off the list");
    send();
    await sending;
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.equal(page.repliesPosted.length, 1, "the premise is the reply sent and confirmed");
    const whileSaving = [...page.upstreamReadCalls];
    done();
    await pressed.catch(() => {});
    for (let i = 0; i < 3; i += 1) await page.settle();
    return { page, asked, whileSaving };
  };
  const failing = json(502, { error: "store_error", detail: "the store is read-only" });
  const failed = await heldHere(failing);
  assert.deepStrictEqual(failed.whileSaving, [], "the chat service was asked before the Done was saved");
  assert.deepStrictEqual(failed.asked, [], "the chat service's marker was moved for a Done that failed");
  assert.equal(failed.page.el("error").hidden, false, "a Done that failed said nothing");
  const saved = await heldHere(null);
  assert.deepStrictEqual(saved.whileSaving, [], "the chat service was asked before the Done was saved");
  assert.deepStrictEqual(saved.page.upstreamReadCalls, [{ message_id: target, after_reply: true }],
    "the saved Done was not marked in the chat service as a reply's mark");
  assert.deepStrictEqual(saved.page.dismissCalls, [{ messages: [target] }], "the send wrote a second Done");

  // THE CONTROLS. Looking again for every message Done already, a Done on its way or not, asks the
  // chat service nothing for a Done made elsewhere or before the reload: no set the page keeps names
  // it. And taking a message off the list for Done when looking again — the reading that decides it is
  // Done in the first place — asks for one whose Done here failed, which no read has put back.
  const lookedAlways = brokenScript(SENT_SAVED_ALREADY, "");
  for (const [how, run] of [["Done on another device", elsewhere], ["Done before a reload", reloaded]]) {
    const control = await run(lookedAlways);
    assert.deepStrictEqual(control.page.upstreamReadCalls, [],
      `${how}: looking again for every message Done already still asked the chat service, so this test cannot see it`);
  }
  assert.deepStrictEqual((await heldHere(failing, lookedAlways)).asked, [], "the control changed what a failed Done asks");
  const offList = brokenScript("  return inColumn(columns.main, () => ids.every((id) => doneState().archived.has(String(id))));\n",
    "  return inColumn(columns.main, () => todoMode || ids.every((id) => doneState().archived.has(String(id))));\n");
  const offListFailed = await heldHere(failing, offList);
  assert.deepStrictEqual(offListFailed.whileSaving, [], "the control waited no longer");
  assert.deepStrictEqual(offListFailed.asked, [target],
    "looking again by the list asked nothing after the failed Done, so this test cannot see it");
});

// --- `#326 refused-undo-restores-done` ----------------------------------------------------------
//
// Found in the final reviews of #306 done-stale-across-views and #311 sent-read-waits-for-done. A Done
// pressed, then Undo while the Done is still being saved: the Undo's write waits behind the Done's. Each
// refusal took back exactly what its own act had changed, so with both refused the Undo's took the
// message back to Done — a Done the server had refused as well. Every view drew it Done, the next write
// saved it into the device's snapshot, and a Reply on it wrote no Done and asked the chat service, whose
// marker cannot be moved back, to mark it read. A refusal now takes a message back to what the server
// holds for it, by this page's writes: the last of them it saved, or what it held before the first.

/** The refusal every test of this section answers a write with. */
const STORE_REFUSES = () => json(502, { error: "store_error", detail: "the store is read-only" });

/** The line in `settleInboxWrite` that leaves a refused write's take-back out where the server holds what it made, for negative controls. */
const REFUSAL_KEEPS_SERVER = ["      else if (write.server === done) stands.add(id);\n", ""];
/** ...and the one that leaves it out while a later write of the message is on its way. */
const REFUSAL_KEEPS_LATER = ["      if (write.writes > 0) stands.add(id);\n      else if", "      if"];

/** The lines that bring the Undo chip up to date as a Done is saved whatever it offers since, for a negative control. */
const CHIP_FOLLOWS_ANY = ["  if (offered()) {\n    const still = new Set(lastDismissal.messages);\n", "  {\n    const still = new Set(requested);\n"];

/**
 * Root 201 of `threadData`, Main and All held, the chat service's marker supported, marked Done in Main
 * with its Done button, its write held on its way; Undo pressed meanwhile, from the chip, its write queued
 * behind; with `again`, Done pressed on it once more from its row, queued behind that; then the writes
 * answered in turn by `answers` (null for saved). Answers the page and the server's floor's asks of the
 * chat service.
 */
async function doneThenUndo(answers, script = SCRIPT, again = false) {
  let asked = null;
  const { page } = await mainAndAllHeld(script, (fresh) => { fresh.upstreamReadMarkSupported = true; asked = serverFloor(fresh); });
  const later = answers.slice(1);
  let release = null;
  page.inboxWriteHook = async (kind) => {
    if (kind === "dismiss" && release === null) return new Promise((resolve) => { release = () => resolve(answers[0]); });
    return later.length > 0 ? later.shift() : null;
  };
  // Not waited for: the button's act ends with its write, which is held.
  const acts = [doneButton(channelRow(page, "201")).click()];
  await page.settle();
  assert.ok(release, "the premise is the Done on its way");
  assert.equal(doneDrawn(page, "201"), "true", "the premise is the Done drawn at once");
  acts.push(page.el("undo-dismiss").click());
  await page.settle();
  assert.equal(doneDrawn(page, "201"), "false", "the premise is the Undo drawn at once");
  if (again) {
    acts.push(doneButton(channelRow(page, "201")).click());
    await page.settle();
    assert.equal(doneDrawn(page, "201"), "true", "the premise is the second Done drawn at once");
  }
  assert.deepStrictEqual(page.restoreCalls, [], "the premise is the Undo waiting behind the Done");
  release();
  for (const act of acts) await act.catch(() => {});
  for (let i = 0; i < 4; i += 1) await page.settle();
  assert.deepStrictEqual(page.restoreCalls, [{ messages: ["201"] }], "the premise is the Undo written after the Done");
  assert.equal(page.dismissCalls.length, again ? 2 : 1, "the premise is each Done written once");
  assert.equal(page.el("error").hidden, answers.every((answer) => answer === null), "the premise is each refusal said");
  page.inboxWriteHook = null;
  return { page, asked };
}

/** Every order of `count` writes each refused or saved, and its name: "refuse/save" and so on. */
const answerOrders = (count) => Array.from({ length: 2 ** count }, (_, n) => {
  const answers = Array.from({ length: count }, (__, at) => ((n >> (count - 1 - at)) & 1 ? null : STORE_REFUSES()));
  return [answers, answers.map((answer) => (answer ? "refuse" : "save")).join("/")];
});

test("`#326 refused-undo-restores-done`: a Done and the Undo pressed while it was saved, each refused or saved, leave root 201 as the server holds it in every view, in the snapshot and for a Reply", async () => {
  for (const [answers, says] of answerOrders(2)) {
    const { page, asked } = await doneThenUndo(answers);
    const held = page.dealtWith.has("201");
    assert.equal(held, answers[0] === null && answers[1] !== null, `${says}: the premise is the server's Done for 201`);
    assert.equal(doneDrawn(page, "201"), String(held), `${says}: Main draws 201 otherwise than the server holds it`);
    assert.equal(doneButton(channelRow(page, "201")).text(), held ? "Unarchive" : "Done", `${says}: Main's button offers the wrong act`);
    // Undo was pressed: the chip offers to put back nothing, whichever answer came last.
    assert.equal(page.el("undo-dismiss").hidden, true, `${says}: the Undo chip came back after Undo was pressed`);
    await pickThread(page, "flat");
    assert.equal(doneDrawn(page, "201"), String(held), `${says}: All draws 201 otherwise than the server holds it`);
    await pickThread(page, "main");
    // A later write, which saves the snapshot from the page's copy.
    await doneButton(channelRow(page, "203")).click();
    await page.settle();
    assert.equal(savedDone(page, "201"), held, `${says}: a later write saved 201 into the snapshot otherwise than the server holds it`);
    // A Reply on 201: a Done written first where the server holds none, and only then the chat service asked.
    const before = page.dismissCalls.length;
    await replyAndSend(page, "201", "on it");
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.equal(page.repliesPosted.at(-1).body.reply_to, "201", `${says}: the premise is the reply sent and confirmed`);
    assert.deepStrictEqual(page.dismissCalls.slice(before), held ? [] : [{ messages: ["201"] }],
      `${says}: the reply's Done was written otherwise than the server's Done says`);
    assert.equal(page.dealtWith.has("201"), true, `${says}: the reply left 201 without a Done on the server`);
    const dismissed = page.requests.map((one, at) => (one.endsWith("/dismiss") ? at : -1)).filter((at) => at >= 0).at(-1);
    const marked = page.requests.findIndex((one) => one.endsWith("/upstream-read"));
    assert.ok(marked > dismissed, `${says}: the chat service was asked before 201's Done was on the server`);
    assert.deepStrictEqual(asked, ["201"], `${says}: the chat service's marker was not moved once, for 201`);
  }

  // THE CONTROLS. A refusal taking back what its own act changed, whatever the server holds: refused
  // twice, the Undo's take-back draws 201 Done, saves it, and the Reply writes no Done and asks the
  // service for a message the server holds none for.
  const { page, asked } = await doneThenUndo([STORE_REFUSES(), STORE_REFUSES()], brokenScript(...REFUSAL_KEEPS_SERVER));
  assert.equal(page.dealtWith.has("201"), false);
  assert.equal(doneDrawn(page, "201"), "true", "without the server's state kept the Undo's refusal took 201 back to not Done, so this test cannot see it");
  await doneButton(channelRow(page, "203")).click();
  await page.settle();
  assert.equal(savedDone(page, "201"), true, "without the server's state kept the snapshot left 201 out, so this test's claim about it is untested");
  const before = page.dismissCalls.length;
  await replyAndSend(page, "201", "on it");
  for (let i = 0; i < 3; i += 1) await page.settle();
  assert.deepStrictEqual(page.dismissCalls.slice(before), [], "without the server's state kept the reply wrote a Done, so this test cannot see it");
  assert.deepStrictEqual(asked, ["201"], "without the server's state kept the chat service was not asked, so this test cannot see it");
  assert.equal(page.dealtWith.has("201"), false);
  // The Done's answer bringing the chip up to date whatever it offers since: saved twice, it offers to
  // put back 201, which is back already.
  const chip = await doneThenUndo([null, null], brokenScript(...CHIP_FOLLOWS_ANY));
  assert.equal(chip.page.el("undo-dismiss").hidden, false, "without the chip's own check it stayed away, so this test cannot see it");
});

test("`#326 refused-undo-restores-done`: a Done, its Undo and a Done again, pressed while the first was saved, each refused or saved, leave root 201 as the server holds it", async () => {
  for (const [answers, says] of answerOrders(3)) {
    const { page } = await doneThenUndo(answers, SCRIPT, true);
    const held = page.dealtWith.has("201");
    // The last write the server saved decides it; none saved, it holds none.
    const last = [true, false, true].filter((_, at) => answers[at] === null).at(-1);
    assert.equal(held, last === true, `${says}: the premise is the server's Done for 201`);
    assert.equal(doneDrawn(page, "201"), String(held), `${says}: Main draws 201 otherwise than the server holds it`);
    await pickThread(page, "flat");
    assert.equal(doneDrawn(page, "201"), String(held), `${says}: All draws 201 otherwise than the server holds it`);
    await doneButton(channelRow(page, "203")).click();
    await page.settle();
    assert.equal(savedDone(page, "201"), held, `${says}: a later write saved 201 into the snapshot otherwise than the server holds it`);
  }
  // THE CONTROLS. Without a later write's act left standing, the first Done's refusal takes 201 back to
  // not Done under the second Done, whose saving then draws nothing; without the server's state kept,
  // the second Done's refusal takes it back to not Done over the first one saved.
  const unlater = await doneThenUndo([STORE_REFUSES(), STORE_REFUSES(), null], brokenScript(...REFUSAL_KEEPS_LATER), true);
  assert.equal(unlater.page.dealtWith.has("201"), true);
  assert.equal(doneDrawn(unlater.page, "201"), "false",
    "without the later write's act left standing 201 was drawn Done, so this test cannot see it");
  const unkept = await doneThenUndo([null, STORE_REFUSES(), STORE_REFUSES()], brokenScript(...REFUSAL_KEEPS_SERVER), true);
  assert.equal(unkept.page.dealtWith.has("201"), true);
  assert.equal(doneDrawn(unkept.page, "201"), "false", "without the server's state kept 201 was drawn Done, so this test cannot see it");
});

// A Done or an Undo that fails in the page before its write is queued — a slip in drawing it, no write
// at all — used to leave everything noted before it as it was: the act drawn and held here with no write
// on its way, a wait on it (`doneSaved`) that never ended, and the ledger counting a write still on its
// way, so every later refusal of that message took nothing back. A plain `finally` that only ended the
// wait would have been worse: a send looking again would have found the Done held, and asked the chat
// service to mark a message read whose Done was never sent. It is taken back as a refusal is.

/** web/voice.js `script` with a slip just before `line`: an exception no write caused, the `nth` time it is reached. */
function slipBefore(script, line, nth = 1) {
  assert.ok(script.includes(line), `the slip goes before ${JSON.stringify(line)}, which web/voice.js no longer contains`);
  return script.replace(line,
    `if ((globalThis.slipReached = (globalThis.slipReached || 0) + 1) === ${nth}) throw new Error("a slip in the page");\n${line}`);
}

/** The last line a Done draws before its write is queued, and the same for an Undo. */
const BEFORE_DONE_WRITE = "    setStatus(`Saving ${requested.length} local change";
const BEFORE_UNDO_WRITE = "    setStatus(`Restoring ${restored.length} message";
/** The line after an Undo's notes, before its `try`: a slip there is one nothing takes back, as before. */
const UNDO_OUTSIDE_TRY = "  // `#229 desktop-two-column`. Put back in the other column as well.\n";
/**
 * A Done's `catch` letting a slip through untouched, as when the lines before the write were outside the
 * `try`, and its `finally` ending the wait on it (`saved`) or, as then, not: for negative controls.
 */
const DONE_CATCH = "  } catch (error) {\n    // Only what the server did not take is taken back: the batches before the one that failed are\n";
const DONE_SLIP_UNTAKEN = [DONE_CATCH, `${DONE_CATCH}    if (error.message === "a slip in the page") throw error;\n`];
const DONE_SAVED = "    saved();\n  }\n  settleInboxWrite(requested, true, new Set(requested));\n";
const DONE_SLIP_UNSAVED = [DONE_SAVED, DONE_SAVED.replace("    saved();", "    if (globalThis.slipReached !== 1) saved();")];
/** The ledger settled for a slip too, in each act's `catch`: for negative controls, left out for a slip only. */
const DONE_SLIP_SETTLES = ["    const stands = settleInboxWrite(requested, true, kept, sent);\n",
  "    const stands = error.message === \"a slip in the page\" ? new Set() : settleInboxWrite(requested, true, kept, sent);\n"];
const UNDO_SLIP_SETTLES = ["    const stands = settled ? new Set() : settleInboxWrite(restored, false, kept, sent);\n",
  "    const stands = settled || error.message === \"a slip in the page\" ? new Set() : settleInboxWrite(restored, false, kept, sent);\n"];
/** ...and settled as if the server had refused it, a write made before it still on its way. */
const DONE_SLIP_REFUSED = ["settleInboxWrite(requested, true, kept, sent)", "settleInboxWrite(requested, true, kept, true)"];
const UNDO_SLIP_REFUSED = ["settleInboxWrite(restored, false, kept, sent)", "settleInboxWrite(restored, false, kept, true)"];

/**
 * Root 201 of `threadData`, Main and All held, the chat service's marker supported, marked Done in Main
 * with its Done button on `script`, which slips before the write is queued. Answers the page and the
 * server's floor's asks of the chat service.
 */
async function slippedDone(script) {
  let asked = null;
  const { page } = await mainAndAllHeld(script, (fresh) => { fresh.upstreamReadMarkSupported = true; asked = serverFloor(fresh); });
  await doneButton(channelRow(page, "201")).click().catch(() => {});
  for (let i = 0; i < 3; i += 1) await page.settle();
  assert.deepStrictEqual(page.dismissCalls, [], "the premise is a Done whose write was never sent");
  return { page, asked };
}

/** A Reply on 201, sent and confirmed. Answers the Done writes made from then on. */
async function replyOn201(page) {
  const before = page.dismissCalls.length;
  await replyAndSend(page, "201", "on it");
  for (let i = 0; i < 3; i += 1) await page.settle();
  assert.equal(page.repliesPosted.at(-1).body.reply_to, "201", "the premise is the reply sent and confirmed");
  return page.dismissCalls.slice(before);
}

test("`#326 refused-undo-restores-done`: a Done that fails in the page before its write is sent is taken back as a refused one: in every view, a later refusal taken back too, and a Reply writes its own Done before the chat service is asked", async () => {
  const { page, asked } = await slippedDone(slipBefore(SCRIPT, BEFORE_DONE_WRITE));
  assert.equal(page.el("error").hidden, false, "the slip was not said");
  assert.equal(doneDrawn(page, "201"), "false", "Main draws a Done whose write was never sent");
  assert.equal(doneButton(channelRow(page, "201")).text(), "Done", "Main's button offers to put back a Done never sent");
  await pickThread(page, "flat");
  assert.equal(doneDrawn(page, "201"), "false", "All draws a Done whose write was never sent");
  await pickThread(page, "main");
  // A Done of it again, refused: taken back, with no other write of it on its way.
  page.inboxWriteHook = async (kind) => (kind === "dismiss" ? STORE_REFUSES() : null);
  await doneButton(channelRow(page, "201")).click().catch(() => {});
  for (let i = 0; i < 3; i += 1) await page.settle();
  page.inboxWriteHook = null;
  assert.deepStrictEqual(page.dismissCalls, [{ messages: ["201"] }], "the premise is the second Done written, and refused");
  assert.equal(doneDrawn(page, "201"), "false", "a refused Done after the slip stayed drawn: the slipped write is still counted on its way");
  // A Reply on it: its own Done written first, and only then the chat service asked, once.
  assert.deepStrictEqual(await replyOn201(page), [{ messages: ["201"] }], "the reply wrote no Done of its own");
  assert.equal(page.dealtWith.has("201"), true);
  const dismissed = page.requests.map((one, at) => (one.endsWith("/dismiss") ? at : -1)).filter((at) => at >= 0).at(-1);
  assert.ok(page.requests.findIndex((one) => one.endsWith("/upstream-read")) > dismissed,
    "the chat service was asked before 201's Done was on the server");
  assert.deepStrictEqual(asked, ["201"], "the chat service's marker was not moved once, for 201");

  // THE CONTROLS. A slip nothing takes back, as when the lines before the write were outside the `try`:
  // 201 stays drawn Done with no write sent, and the Reply's wait for that Done never ends, so it neither
  // writes a Done nor asks the chat service.
  const loose = await slippedDone(slipBefore(brokenScriptEach(DONE_SLIP_UNTAKEN, DONE_SLIP_UNSAVED), BEFORE_DONE_WRITE));
  assert.equal(doneDrawn(loose.page, "201"), "true", "a slip nothing takes back was taken back, so this test cannot see one left");
  assert.deepStrictEqual(await replyOn201(loose.page), [], "with the slip left the reply wrote a Done, so this test cannot see its wait");
  assert.deepStrictEqual(loose.asked, [], "with the slip left the reply's wait ended, so this test cannot see it hang");
  // The wait ended by a plain `finally`, the Done not taken back: the Reply finds 201 held Done, writes
  // nothing, and asks the chat service to mark a message whose Done was never sent.
  const ended = await slippedDone(slipBefore(brokenScript(...DONE_SLIP_UNTAKEN), BEFORE_DONE_WRITE));
  assert.deepStrictEqual(await replyOn201(ended.page), [], "with the wait ended the reply wrote a Done, so this test cannot see it");
  assert.deepStrictEqual(ended.asked, ["201"], "with the wait ended and the Done kept the service was not asked, so this test cannot see the marker moved");
  assert.equal(ended.page.dealtWith.has("201"), false);
  // The ledger not settled for the slip: the second Done's refusal finds a write of 201 still on its way,
  // and takes nothing back.
  const counted = await slippedDone(slipBefore(brokenScript(...DONE_SLIP_SETTLES), BEFORE_DONE_WRITE));
  assert.equal(doneDrawn(counted.page, "201"), "false", "the premise is the slip taken back");
  counted.page.inboxWriteHook = async (kind) => (kind === "dismiss" ? STORE_REFUSES() : null);
  await doneButton(channelRow(counted.page, "201")).click().catch(() => {});
  for (let i = 0; i < 3; i += 1) await counted.page.settle();
  assert.equal(doneDrawn(counted.page, "201"), "true", "with the ledger left counting the slip the refusal was taken back, so this test cannot see it");
});

/** Root 201 of `threadData`, Main and All held, marked Done and saved, then Undo pressed from the chip on `script`, which slips before its write. */
async function slippedUndo(script) {
  const { page } = await mainAndAllHeld(script);
  await doneButton(channelRow(page, "201")).click();
  await page.settle();
  assert.deepStrictEqual(page.dismissCalls, [{ messages: ["201"] }], "the premise is a Done saved, to undo");
  await page.el("undo-dismiss").click().catch(() => {});
  for (let i = 0; i < 3; i += 1) await page.settle();
  assert.deepStrictEqual(page.restoreCalls, [], "the premise is an Undo whose write was never sent");
  return page;
}

test("`#326 refused-undo-restores-done`: an Undo that fails in the page before its write is sent is taken back as a refused one: Done in every view, as the server holds it, and a later refusal taken back too", async () => {
  const page = await slippedUndo(slipBefore(SCRIPT, BEFORE_UNDO_WRITE));
  assert.equal(page.el("error").hidden, false, "the slip was not said");
  assert.equal(doneDrawn(page, "201"), "true", "Main draws 201 put back by an Undo never sent");
  await pickThread(page, "flat");
  assert.equal(doneDrawn(page, "201"), "true", "All draws 201 put back by an Undo never sent");
  await pickThread(page, "main");
  assert.equal(doneDrawn(page, "201"), "true", "Main draws 201 put back by an Undo never sent, once shown again");
  // Put back from its row, refused: Done again, with no other write of it on its way.
  page.inboxWriteHook = async (kind) => (kind === "restore" ? STORE_REFUSES() : null);
  await doneButton(channelRow(page, "201")).click().catch(() => {});
  for (let i = 0; i < 3; i += 1) await page.settle();
  page.inboxWriteHook = null;
  assert.deepStrictEqual(page.restoreCalls, [{ messages: ["201"] }], "the premise is the put back written, and refused");
  assert.equal(doneDrawn(page, "201"), "true", "a refused put back after the slip stayed drawn: the slipped write is still counted on its way");
  await doneButton(channelRow(page, "203")).click();
  await page.settle();
  assert.equal(savedDone(page, "201"), true, "a later write saved 201 into the snapshot otherwise than the server holds it");

  // THE CONTROLS. A slip outside the `try`: 201 put back in the page's copy while the server holds its
  // Done, and drawn so once Main is shown again. The ledger not settled for the slip: the put back's
  // refusal finds a write of 201 still on its way, and leaves it drawn put back.
  const loose = await slippedUndo(slipBefore(SCRIPT, UNDO_OUTSIDE_TRY));
  await pickThread(loose, "flat");
  await pickThread(loose, "main");
  assert.equal(doneDrawn(loose, "201"), "false", "a slip outside the `try` was taken back, so this test cannot see one left");
  const counted = await slippedUndo(slipBefore(brokenScript(...UNDO_SLIP_SETTLES), BEFORE_UNDO_WRITE));
  assert.equal(doneDrawn(counted, "201"), "true", "the premise is the slip taken back");
  counted.inboxWriteHook = async (kind) => (kind === "restore" ? STORE_REFUSES() : null);
  await doneButton(channelRow(counted, "201")).click().catch(() => {});
  for (let i = 0; i < 3; i += 1) await counted.settle();
  assert.equal(doneDrawn(counted, "201"), "false", "with the ledger left counting the slip the refusal was taken back, so this test cannot see it");
});

test("`#326 refused-undo-restores-done`: a Done or an Undo that fails in the page while an earlier write of the message is still on its way takes back exactly what it changed, and that write's answer decides", async () => {
  /**
   * Root 201 of `threadData`, Main and All held, on `script`. With `kind` "restore", its Done held on its
   * way and Undo pressed meanwhile from the chip; with "dismiss", its Done saved, then its put back from
   * the row held on its way and Done pressed again from the row; the second act slipping before its
   * write. Then the held write answered `answer` (null for saved). Answers the page and how 201 was drawn
   * just after the slip.
   */
  const run = async (kind, answer, script) => {
    const { page } = await mainAndAllHeld(script);
    const earlier = kind === "restore" ? "dismiss" : "restore";
    if (kind === "dismiss") {
      await doneButton(channelRow(page, "201")).click();
      await page.settle();
    }
    let release = null;
    page.inboxWriteHook = async (asked) => (asked === earlier && release === null
      ? new Promise((resolve) => { release = () => resolve(answer); }) : null);
    const first = doneButton(channelRow(page, "201")).click();
    await page.settle();
    assert.ok(release, `the premise is the ${earlier} on its way`);
    assert.equal(doneDrawn(page, "201"), String(earlier === "dismiss"), `the premise is the ${earlier} drawn at once`);
    await (kind === "restore" ? page.el("undo-dismiss") : doneButton(channelRow(page, "201"))).click().catch(() => {});
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.equal(page.el("error").hidden, false, "the premise is the slip said");
    const afterSlip = doneDrawn(page, "201");
    release();
    await first.catch(() => {});
    for (let i = 0; i < 4; i += 1) await page.settle();
    page.inboxWriteHook = null;
    assert.deepStrictEqual(kind === "restore" ? page.restoreCalls : page.dismissCalls.slice(1), [],
      `the premise is the ${kind} never written`);
    return { page, afterSlip };
  };
  for (const kind of ["restore", "dismiss"]) {
    const line = kind === "restore" ? BEFORE_UNDO_WRITE : BEFORE_DONE_WRITE;
    // The Done saved first reaches its line too.
    const nth = kind === "restore" ? 1 : 2;
    for (const answer of [null, STORE_REFUSES()]) {
      const says = `the ${kind} slipped, the earlier write ${answer === null ? "saved" : "refused"}`;
      const { page, afterSlip } = await run(kind, answer, slipBefore(SCRIPT, line, nth));
      assert.equal(afterSlip, String(kind === "restore"), `${says}: the slip left its own act drawn over the earlier write's`);
      const held = page.dealtWith.has("201");
      assert.equal(doneDrawn(page, "201"), String(held), `${says}: Main draws 201 otherwise than the server holds it`);
      await pickThread(page, "flat");
      assert.equal(doneDrawn(page, "201"), String(held), `${says}: All draws 201 otherwise than the server holds it`);
      await doneButton(channelRow(page, "203")).click();
      await page.settle();
      assert.equal(savedDone(page, "201"), held, `${says}: a later write saved 201 into the snapshot otherwise than the server holds it`);
    }
  }
  // THE CONTROLS: a slip settled as if the server had refused it finds the earlier write still on its way,
  // so takes nothing back, and its own act stays drawn once that write is saved.
  const undo = await run("restore", null, slipBefore(brokenScript(...UNDO_SLIP_REFUSED), BEFORE_UNDO_WRITE));
  assert.equal(undo.page.dealtWith.has("201"), true);
  assert.equal(doneDrawn(undo.page, "201"), "false", "the slipped Undo settled as refused was taken back, so this test cannot see it");
  const done = await run("dismiss", null, slipBefore(brokenScript(...DONE_SLIP_REFUSED), BEFORE_DONE_WRITE, 2));
  assert.equal(done.page.dealtWith.has("201"), false);
  assert.equal(doneDrawn(done.page, "201"), "true", "the slipped Done settled as refused was taken back, so this test cannot see it");
});

test("`#326 refused-undo-restores-done`: a put back from its row that the server refuses leaves the Undo chip offering a Done made while it was on its way", async () => {
  const run = async (script = SCRIPT) => {
    const { page } = await mainAndAllHeld(script);
    await doneButton(channelRow(page, "201")).click();
    await page.settle();
    // 201 put back from its row, its write held: the chip offered 201 alone, so it goes.
    let release = null;
    page.inboxWriteHook = async (kind) => (kind === "restore" && release === null
      ? new Promise((resolve) => { release = () => resolve(STORE_REFUSES()); }) : null);
    const putBack = doneButton(channelRow(page, "201")).click();
    await page.settle();
    assert.ok(release, "the premise is the put back on its way");
    assert.equal(page.el("undo-dismiss").hidden, true, "the premise is the chip gone with 201 put back");
    // A Done on 203 meanwhile, its write queued behind: the chip offers it.
    const done = doneButton(channelRow(page, "203")).click();
    await page.settle();
    assert.equal(page.el("undo-dismiss").hidden, false, "the premise is the chip offering the Done on 203");
    release();
    await putBack.catch(() => {});
    await done;
    for (let i = 0; i < 4; i += 1) await page.settle();
    page.inboxWriteHook = null;
    assert.equal(page.el("error").hidden, false, "the premise is the put back refused, and said");
    assert.equal(doneDrawn(page, "201"), "true", "the premise is 201 Done again, as the server holds it");
    assert.equal(page.el("undo-dismiss").hidden, false, "the chip went with the refusal");
    await page.el("undo-dismiss").click();
    await page.settle();
    return page;
  };
  const page = await run();
  assert.deepStrictEqual(page.restoreCalls.at(-1), { messages: ["203"] }, "the chip's Undo put back another message than the Done made last");
  assert.equal(doneDrawn(page, "203"), "false");
  assert.equal(doneDrawn(page, "201"), "true");
  // THE CONTROL: the refusal bringing the chip back to what it offered before the put back, whatever it
  // offers since: it offers 201 again, and the Done on 203 has no Undo.
  const control = await run(brokenScript("    if (lastDismissal === offer) {", "    if (true) {"));
  assert.deepStrictEqual(control.restoreCalls.at(-1), { messages: ["201"] },
    "without the chip's own check the Undo put back 203, so this test cannot see it");
});

// --- `#327 done-races-remaining` ----------------------------------------------------------------
//
// Found in the final reviews of #306 done-stale-across-views and #311 sent-read-waits-for-done, four
// smaller races of a Done still on its way, each older than both:
//   (1) a thread opened on the right for the first time while a Done made on Main was saved held nothing
//       for the message, and drew it as its read did, not Done, even once the Done was saved;
//   (2) Through here did not wait for a Done still on its way on a row it covers before moving the chat
//       service's marker, which cannot be moved back;
//   (3) an Undo pressed while a sent reply's own Done (#300 reply-marks-read) was saved still had the
//       chat service asked;
//   (4) under Hide read in a channel without threads, a Done the server refused kept its row off the
//       list until the next read.

/** The lines of `noteArchived` that give a copy the store's state for a message the read does not speak for, for a negative control. */
const COPY_TAKES_STORE = ["  if (threadingSupported && channelStore.current().channel === String(selectedChannelId())) {\n    for (const id of readLedger.writtenIds()) {",
  "  if (false) {\n    for (const id of readLedger.writtenIds()) {"];
/** The wait of Through here for a Done on its way on a row it covers, for a negative control. */
const THROUGH_WAITS = [
  "    if (!(await doneSaved(waitingFor, () => String(selectedChannelId()) === channel && doneHeldHere(waitingFor)))) {",
  "    if (false) {"];
/** Through here waiting only for the rows on screen, as first written, for a negative control. */
const THROUGH_WAITS_ON_SCREEN = ["  const waitingFor = [...new Set([...covered, ...onTheirWay])];\n",
  "  const waitingFor = covered;\n"];
/** The look of a confirmed send at its message once its own Done is saved, for a negative control. */
const SENT_LOOKS_AGAIN = ["  return doneSaved(ids, () => String(selectedChannelId()) === channel && doneHeldHere(ids));\n}\n",
  "  return true;\n}\n"];
/** The rows a refused Done put back under Hide read without threads, for a negative control. */
const ROWS_PUT_BACK = ["        putBackRowsOff(listed, back);\n", ""];

/**
 * Two columns over the channel of `threadData`, the cards on the right and no thread opened there yet;
 * a Done on root 201 in Main, or for `kind` "restore" an Undo of a Done on it saved first, its write held
 * on its way; thread one & two opened on the right from 201's tag meanwhile, for the first time; then the
 * write answered `answer` (null for saved). With `oneColumn`, the act is made in All in one column, and two
 * columns turned on while it is on its way, before the thread is opened. Answers the page and how the
 * right column drew 201 while the write was on its way.
 */
async function openedWhileSaving(kind, answer, script = SCRIPT, oneColumn = false) {
  const page = oneColumn ? await threadPageOn(script) : (await twoColumnServer(null, script)).page;
  assert.deepStrictEqual(sideIds(page), [], "the premise is no thread on the right yet");
  if (kind === "restore") {
    await doneButton(channelRow(page, "201")).click();
    await page.settle();
    assert.deepStrictEqual(page.dismissCalls, [{ messages: ["201"] }], "the premise is a Done saved, to undo");
  }
  let release = null;
  page.inboxWriteHook = async (asked) => (asked === kind && release === null
    ? new Promise((resolve) => { release = () => resolve(answer); }) : null);
  const pressed = doneButton(channelRow(page, "201")).click();
  await page.settle();
  assert.ok(release, `the premise is the ${kind} on its way`);
  assert.equal(doneDrawn(page, "201"), String(kind === "dismiss"), "the premise is the act drawn at once");
  if (oneColumn) {
    await twoColumns(page);
    assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "the premise is Main on the left beside the cards");
  }
  await threadBadge(channelRow(page, "201")).click();
  await page.settle();
  assert.deepStrictEqual(sideIds(page), ["201", "202"], "the premise is the thread opened on the right");
  const whileSaving = sideDoneDrawn(page, "201");
  release();
  await pressed.catch(() => {});
  for (let i = 0; i < 4; i += 1) await page.settle();
  assert.equal(page.el("error").hidden, answer === null, `the premise is the ${kind} ${answer === null ? "saved" : "refused, and said"}`);
  page.inboxWriteHook = null;
  return { page, whileSaving };
}

test("`#327 done-races-remaining`: a thread opened on the right for the first time while a Done or an Undo made on the left is saved draws it as made, and as the server holds it once answered", async () => {
  for (const [oneColumn, where] of [[false, "made on Main beside the cards"], [true, "made in All before two columns"]]) {
    for (const kind of ["dismiss", "restore"]) {
      const made = String(kind === "dismiss");
      for (const answer of [null, STORE_REFUSES()]) {
        const says = `${where}, ${kind} ${answer === null ? "saved" : "refused"}`;
        const { page, whileSaving } = await openedWhileSaving(kind, answer, SCRIPT, oneColumn);
        assert.equal(whileSaving, made, `${says}: the thread opened on the right drew 201 as its read did, not as the act on the left made it`);
        const held = page.dealtWith.has("201");
        assert.equal(sideDoneDrawn(page, "201"), String(held), `${says}: the right column draws 201 otherwise than the server holds it`);
        assert.equal(doneDrawn(page, "201"), String(held), `${says}: Main draws 201 otherwise than the server holds it`);
        // A later write from the right, which saves the snapshot from the right column's copy.
        await doneButton(sideRow(page, "202")).click();
        await page.settle();
        assert.equal(savedDone(page, "201"), held, `${says}: a later write from the right saved 201 otherwise than the server holds it`);
      }
    }
  }
  // THE CONTROL: without a copy taking the store's state for a message its read does not speak for, the
  // thread opened on the right draws a Done on its way as not Done, and still does once it is saved.
  for (const oneColumn of [false, true]) {
    const { page, whileSaving } = await openedWhileSaving("dismiss", null, brokenScript(...COPY_TAKES_STORE), oneColumn);
    assert.equal(whileSaving, "false", "without the store's state the thread drew the Done on its way, so this test cannot see it");
    assert.equal(sideDoneDrawn(page, "201"), "false", "without the store's state the thread drew the saved Done, so this test cannot see it");
  }
});

/**
 * Three messages of `backlog` in a channel without threads, the chat service's marker supported; the
 * first marked Done with its Done button, its write held on its way; then Through here pressed on row
 * `boundary` meanwhile; then the first Done answered `answer` (null for saved), every later write saved.
 * Answers the page, the ids, what was asked of the chat service while the first Done was on its way, and
 * the server's floor's asks of the service.
 */
async function throughWhileSaving(answer, boundary, script = SCRIPT) {
  const page = newPage(new Map(), script);
  page.upstreamReadMarkSupported = true;
  const asked = serverFloor(page);
  await signIn(page);
  const ids = (await showDiscord(page, backlog(3))).map((row) => row.getAttribute("data-id"));
  let release = null;
  page.inboxWriteHook = async (kind) => (kind === "dismiss" && release === null
    ? new Promise((resolve) => { release = () => resolve(answer); }) : null);
  const pressed = doneButton(rowWithId(page, ids[0])).click();
  await page.settle();
  assert.ok(release, "the premise is the first Done on its way");
  const through = readThroughButton(rowWithId(page, ids[boundary])).click();
  await page.settle();
  const whileSaving = [...page.upstreamReadCalls];
  release();
  await pressed.catch(() => {});
  await through;
  for (let i = 0; i < 4; i += 1) await page.settle();
  page.inboxWriteHook = null;
  return { page, ids, whileSaving, asked };
}

test("`#327 done-races-remaining`: Through here waits for a Done still on its way on a row it covers before it moves the chat service's marker, and moves it not at all once that Done is refused", async () => {
  // Boundary 2 writes its own Done of rows 1 and 2; boundary 0 covers only the row whose Done is on its way.
  for (const boundary of [2, 0]) {
    const saved = await throughWhileSaving(null, boundary);
    assert.deepStrictEqual(saved.whileSaving, [], `through ${boundary}: the chat service was asked before the Done it covers was saved`);
    assert.deepStrictEqual(saved.page.upstreamReadCalls, [{ message_id: saved.ids[boundary] }],
      `through ${boundary}: the chat service was not asked once the Done it covers was saved`);
    assert.deepStrictEqual(saved.asked, [saved.ids[boundary]]);
    const dismissed = saved.page.requests.map((one, at) => (one.endsWith("/dismiss") ? at : -1)).filter((at) => at >= 0).at(-1);
    assert.ok(saved.page.requests.findIndex((one) => one.endsWith("/upstream-read")) > dismissed,
      `through ${boundary}: the chat service was asked before every Done it covers was written`);
    assert.match(saved.page.el("status").textContent, /read here and in Discord\.$/);

    const refused = await throughWhileSaving(STORE_REFUSES(), boundary);
    assert.deepStrictEqual(refused.whileSaving, [], `through ${boundary}: the chat service was asked before the Done it covers was answered`);
    assert.deepStrictEqual(refused.page.upstreamReadCalls, [], `through ${boundary}: the chat service was asked after a Done it covers was refused`);
    assert.deepStrictEqual(refused.asked, []);
    assert.equal(rowWithId(refused.page, refused.ids[0]).getAttribute("data-archived"), "false", "the premise is the refused Done taken back");
    assert.match(refused.page.el("status").textContent, boundary === 0
      ? /^Not marked read in Discord: a Done it covers was refused or undone\.$/
      : /^Marked 2 messages read here, not in Discord: a Done it covers was refused or undone\.$/,
      `through ${boundary}: the status line does not say why the chat service was not asked`);
    assert.equal(refused.page.el("error").hidden, false, "the premise is the refusal said");
  }
  // THE CONTROL: without the wait, the chat service is asked after a refused Done it covers, and asked at
  // once, while the Done is still on its way, when Through here has nothing of its own to write.
  const late = await throughWhileSaving(STORE_REFUSES(), 2, brokenScript(...THROUGH_WAITS));
  assert.deepStrictEqual(late.asked, [late.ids[2]], "without the wait the chat service was not asked after the refusal, so this test cannot see it");
  const early = await throughWhileSaving(STORE_REFUSES(), 0, brokenScript(...THROUGH_WAITS));
  assert.deepStrictEqual(early.whileSaving, [{ message_id: early.ids[0] }],
    "without the wait the chat service was not asked while the Done was on its way, so this test cannot see it");
});

/**
 * Three messages of `backlog` under Hide read, the chat service's marker supported; the first shown
 * marked Done with its Done button, its write held on its way, so Hide read has taken its row off
 * the list; then Through here pressed on the last row shown meanwhile; then the first Done answered
 * `answer` (null for saved). Answers the page, the held message, the boundary, what was asked of
 * the chat service while the first Done was on its way, and the server's floor's asks.
 */
async function throughWhileSavingHidden(answer, script = SCRIPT) {
  const page = newPage(new Map(), script);
  page.upstreamReadMarkSupported = true;
  const asked = serverFloor(page);
  await signIn(page);
  await showDiscord(page, backlog(3));
  await turnTodoOn(page);
  const id = shownIds(page)[0];
  let release = null;
  page.inboxWriteHook = async (kind) => (kind === "dismiss" && release === null
    ? new Promise((resolve) => { release = () => resolve(answer); }) : null);
  const pressed = doneButton(rowWithId(page, id)).click();
  await page.settle();
  assert.ok(release, "the premise is the first Done on its way");
  assert.ok(!shownIds(page).includes(id), "the premise is Hide read took the held Done's row off the list");
  const boundary = shownIds(page).at(-1);
  const through = readThroughButton(rowWithId(page, boundary)).click();
  for (let i = 0; i < 4; i += 1) await page.settle();
  const whileSaving = [...page.upstreamReadCalls];
  release();
  await pressed.catch(() => {});
  await through;
  for (let i = 0; i < 4; i += 1) await page.settle();
  page.inboxWriteHook = null;
  return { page, id, boundary, whileSaving, asked };
}

test("`#327 done-races-remaining`: under Hide read, Through here also waits for a Done still on its way whose row it took off the list, which its marker would pass, and moves the marker not at all once that Done is refused", async () => {
  // Found in the review of this lane (2026-10-19): the wait above was only for the rows on screen,
  // and under Hide read a Done takes its row off at once, so Through here never waited for it and
  // its marker passed a message whose Done was then refused and put back here unread.
  const refused = await throughWhileSavingHidden(STORE_REFUSES());
  assert.deepStrictEqual(refused.whileSaving, [], "the chat service was asked before the held Done was answered");
  assert.deepStrictEqual(refused.page.upstreamReadCalls, [], "the chat service was asked although a Done its marker passes was refused");
  assert.deepStrictEqual(refused.asked, []);
  assert.equal(refused.page.dealtWith.has(refused.id), false, "the premise is the held Done refused");
  assert.ok(shownIds(refused.page).includes(refused.id), "the premise is the refused Done's row back on the list");
  assert.match(refused.page.el("status").textContent,
    /^Marked 2 messages read here, not in Discord: a Done it covers was refused or undone\.$/,
    "the status line does not say why the chat service was not asked");

  const saved = await throughWhileSavingHidden(null);
  assert.deepStrictEqual(saved.whileSaving, [], "the chat service was asked before the held Done was answered");
  assert.deepStrictEqual(saved.asked, [saved.boundary], "the chat service was not asked once, after the held Done was saved");
  assert.equal(saved.page.dealtWith.has(saved.id), true, "the premise is the held Done saved");

  // THE CONTROL: waiting only for the rows on screen, as first written, the marker passes the
  // message whose Done was refused.
  const control = await throughWhileSavingHidden(STORE_REFUSES(), brokenScript(...THROUGH_WAITS_ON_SCREEN));
  assert.equal(control.page.dealtWith.has(control.id), false, "the premise is the held Done refused");
  assert.deepStrictEqual(control.asked, [control.boundary],
    "waiting only for the rows on screen the chat service was not asked, so this test cannot see it");
});

/**
 * Message 3 of `conversation`, the chat service's marker supported: a Reply on it sent and confirmed, whose
 * own Done is held on its way; Undo pressed meanwhile, from the chip; then that Done saved and the Undo
 * answered `undo` (null for saved). Answers the page, the target and the server's floor's asks.
 */
async function undoWhileSentDoneSaving(undo, script = SCRIPT) {
  const target = "1000000000000000003";
  const page = newPage(new Map(), script);
  page.upstreamReadMarkSupported = true;
  const asked = serverFloor(page);
  await signIn(page);
  await showDiscord(page, conversation());
  let release = null;
  page.inboxWriteHook = async (kind) => {
    if (kind === "dismiss" && release === null) return new Promise((resolve) => { release = () => resolve(null); });
    return kind === "restore" ? undo : null;
  };
  await replyAndSend(page, target, "answering");
  assert.equal(page.repliesPosted.at(-1).body.reply_to, target, "the premise is the reply sent and confirmed");
  assert.ok(release, "the premise is the reply's own Done on its way");
  assert.equal(rowWithId(page, target).getAttribute("data-archived"), "true", "the premise is the reply's Done drawn");
  const undoing = page.el("undo-dismiss").click();
  await page.settle();
  assert.equal(rowWithId(page, target).getAttribute("data-archived"), "false", "the premise is the Undo drawn at once");
  release();
  await undoing;
  for (let i = 0; i < 4; i += 1) await page.settle();
  assert.deepStrictEqual(page.restoreCalls, [{ messages: [target] }], "the premise is the Undo written");
  page.inboxWriteHook = null;
  return { page, target, asked };
}

test("`#327 done-races-remaining`: an Undo pressed while a sent reply's own Done is saved asks the chat service nothing, and a refused one asks it once, after", async () => {
  const undone = await undoWhileSentDoneSaving(null);
  assert.equal(undone.page.dealtWith.has(undone.target), false, "the premise is the Undo saved");
  assert.deepStrictEqual(undone.page.upstreamReadCalls, [], "a message put back while the reply's Done was saved was read in the chat service");
  assert.deepStrictEqual(undone.asked, []);
  assert.equal(rowWithId(undone.page, undone.target).getAttribute("data-archived"), "false");
  // The Undo refused: the reply's Done stands on the server, and the service is asked once that is known.
  const kept = await undoWhileSentDoneSaving(STORE_REFUSES());
  assert.equal(kept.page.dealtWith.has(kept.target), true, "the premise is the Undo refused");
  assert.equal(rowWithId(kept.page, kept.target).getAttribute("data-archived"), "true");
  assert.deepStrictEqual(kept.page.upstreamReadCalls, [{ message_id: kept.target, after_reply: true }],
    "a reply whose Done stands after a refused Undo was not read in the chat service");
  const restored = kept.page.requests.findIndex((one) => one.endsWith("/restore"));
  assert.ok(kept.page.requests.findIndex((one) => one.endsWith("/upstream-read")) > restored,
    "the chat service was asked before the Undo was answered");
  // THE CONTROL: without the look once its own Done is saved, the send asks the service for a message put back.
  const control = await undoWhileSentDoneSaving(null, brokenScript(...SENT_LOOKS_AGAIN));
  assert.deepStrictEqual(control.asked, [control.target], "without the look the service was not asked, so this test cannot see it");
});

/**
 * `conversation` under Hide read in a channel without threads; message `id` marked Done with its Done
 * button, its write held on its way, which takes its row off the list at once; `meanwhile`; then the Done
 * refused. Answers the page, the list before the Done and the requests made after it.
 */
async function refusedUnderHideRead(id, meanwhile = null, script = SCRIPT) {
  const page = newPage(new Map(), script);
  await signIn(page);
  await showDiscord(page, conversation());
  await chooseReadMode(page, "Hide read");
  const before = shownIds(page);
  assert.equal(before.length, 3, "the premise is three messages to deal with");
  let release = null;
  page.inboxWriteHook = async (kind) => (kind === "dismiss"
    ? new Promise((resolve) => { release = () => resolve(STORE_REFUSES()); }) : null);
  const requests = page.requests.length;
  const pressed = doneButton(rowWithId(page, id)).click();
  await page.settle();
  assert.ok(release, "the premise is the Done on its way");
  assert.equal(shownIds(page).includes(id), false, "the premise is the Done taking its row off at once");
  if (meanwhile) await meanwhile(page);
  release();
  await pressed.catch(() => {});
  for (let i = 0; i < 4; i += 1) await page.settle();
  assert.equal(page.el("error").hidden, false, "the premise is the refusal said");
  return { page, before, made: page.requests.slice(requests) };
}

test("`#327 done-races-remaining`: under Hide read without threads a Done the server refused puts its row back at once, where it stood, with no read", async () => {
  for (const id of ["1000000000000000001", "1000000000000000002", "1000000000000000003"]) {
    const { page, before, made } = await refusedUnderHideRead(id);
    assert.deepStrictEqual(shownIds(page), before, `the row of ${id} a refused Done took off is not back where it stood`);
    assert.equal(rowWithId(page, id).getAttribute("data-archived"), "false");
    assert.equal(doneButton(rowWithId(page, id)).text(), "Done", "the row put back offers no Done");
    assert.match(page.el("channel-summary").text(), /^3 of/, "the head of the list does not count the row put back");
    assert.deepStrictEqual(made.filter((one) => !one.endsWith("/dismiss")), [], "the row waited on a read to come back");
  }
  // A message arriving while the Done is on its way stays below the row put back.
  const arrival = message({ id: "1000000000000000004", content: "a newer one" });
  const arrived = await refusedUnderHideRead("1000000000000000003", async (page) => {
    await deliver(page, page.stream(), sseMessage(arrival));
    assert.ok(shownIds(page).includes(arrival.id), "the premise is the arrival listed");
  });
  assert.deepStrictEqual(shownIds(arrived.page), [...arrived.before, arrival.id], "the row put back is not above the arrival");
  // THE CONTROL: without the rows put back, the row stays off the list until the next read.
  const control = await refusedUnderHideRead("1000000000000000002", null, brokenScript(...ROWS_PUT_BACK));
  assert.deepStrictEqual(shownIds(control.page), ["1000000000000000001", "1000000000000000003"],
    "without the rows put back the row came back, so this test cannot see it");
});

test("`#327 done-races-remaining`: under Hide read without threads a refused Done puts its row back where it stood among the rows a to-do read drew while it was on its way", async () => {
  const poll = async (page) => {
    page.expireTimers(DISCORD_POLL_MS);
    for (let i = 0; i < 3; i += 1) await page.settle();
  };
  // The server still lists the row (the Done not taken yet): the poll draws it, and nothing is put back twice.
  for (const id of ["1000000000000000001", "1000000000000000002", "1000000000000000003"]) {
    const { page, before } = await refusedUnderHideRead(id, async (p) => {
      await poll(p);
      assert.ok(shownIds(p).includes(id), "the premise is a to-do read that lists the row");
    });
    assert.deepStrictEqual(shownIds(page), before, `the row of ${id} is not where it stood after the poll, once`);
    assert.equal(rowWithId(page, id).getAttribute("data-archived"), "false");
  }
  // The server took the Done and its answer was lost: the poll leaves the row out, and the page, told the
  // Done failed, puts it back between the rows the poll drew afresh, which it stood between.
  for (const id of ["1000000000000000001", "1000000000000000002", "1000000000000000003"]) {
    const { page, before } = await refusedUnderHideRead(id, async (p) => {
      p.dealtWith.add(id);
      await poll(p);
      assert.deepStrictEqual(shownIds(p), shownIds(p).filter((one) => one !== id), "the premise is a to-do read without the row");
      assert.equal(shownIds(p).length, 2, "the premise is a to-do read without the row");
    });
    assert.deepStrictEqual(shownIds(page), before, `the row of ${id} is not back where it stood among the rows the poll drew`);
  }
  // Neither row it stood between is listed any more (dealt with elsewhere), and the poll brought an older
  // and a newer message: it goes in by when it was sent.
  const older = "1000000000000000000";
  const newer = "1000000000000000004";
  const lone = async (script) => (await refusedUnderHideRead("1000000000000000002", async (p) => {
    for (const one of ["1000000000000000001", "1000000000000000002", "1000000000000000003"]) p.dealtWith.add(one);
    const at = (one, by) => new Date(Date.parse(one.timestamp) + by).toISOString();
    p.messages = [message({ id: older, content: "an older one", timestamp: at(p.messages[0], -60000) }), ...p.messages,
      message({ id: newer, content: "a newer one", timestamp: at(p.messages.at(-1), 60000) })];
    await poll(p);
    assert.deepStrictEqual(shownIds(p), [older, newer], "the premise is a to-do read of two other messages");
  }, script)).page;
  assert.deepStrictEqual(shownIds(await lone(SCRIPT)), [older, "1000000000000000002", newer],
    "the row put back among rows it never stood beside is not in time order");
  // THE CONTROL: placed by the rows of the list as it stood, by element, before the rows that came since,
  // as first written: every row the poll drew came since, and the row goes in at the top.
  const unplaced = brokenScript("    list.insertBefore(row, next);\n",
    "    list.insertBefore(row, [...list.children].find((one) => !listed.includes(one)) || null);\n");
  const misplaced = await refusedUnderHideRead("1000000000000000003", async (p) => {
    p.dealtWith.add("1000000000000000003");
    await poll(p);
  }, unplaced);
  assert.deepStrictEqual(shownIds(misplaced.page), ["1000000000000000003", "1000000000000000001", "1000000000000000002"],
    "placed by element the row still went back where it stood, so this test cannot see it");
  assert.deepStrictEqual(shownIds(await lone(unplaced)), ["1000000000000000002", older, newer],
    "placed by element the lone row still went in by time, so this test cannot see it");
});

/**
 * Three messages of `backlog` in a channel without threads, the chat service's marker supported: Through
 * here pressed on the last, its Done held on its way; Undo pressed meanwhile from the chip, its write
 * queued behind; then that Done saved and the Undo answered `undo` (null for saved). Answers the page,
 * the ids and the server's floor's asks of the chat service.
 */
async function undoWhileThroughSaving(undo, script = SCRIPT) {
  const page = newPage(new Map(), script);
  page.upstreamReadMarkSupported = true;
  const asked = serverFloor(page);
  await signIn(page);
  const ids = (await showDiscord(page, backlog(3))).map((row) => row.getAttribute("data-id"));
  let release = null;
  page.inboxWriteHook = async (kind) => {
    if (kind === "dismiss" && release === null) return new Promise((resolve) => { release = () => resolve(null); });
    return kind === "restore" ? undo : null;
  };
  const through = readThroughButton(rowWithId(page, ids[2])).click();
  await page.settle();
  assert.ok(release, "the premise is Through here's Done on its way");
  const undoing = page.el("undo-dismiss").click();
  await page.settle();
  assert.equal(rowWithId(page, ids[2]).getAttribute("data-archived"), "false", "the premise is the Undo drawn at once");
  release();
  await through.catch(() => {});
  await undoing.catch(() => {});
  for (let i = 0; i < 4; i += 1) await page.settle();
  assert.deepStrictEqual(page.restoreCalls, [{ messages: ids }], "the premise is the Undo written, after the Done");
  page.inboxWriteHook = null;
  return { page, ids, asked };
}

test("`#327 done-races-remaining`: Through here whose own Done is undone while it is saved moves the chat service's marker not at all, and once, after, when the Undo is refused", async () => {
  const undone = await undoWhileThroughSaving(null);
  assert.deepStrictEqual(undone.asked, [], "the chat service was asked to mark read what the reader put back while it was saved");
  assert.deepStrictEqual(undone.page.upstreamReadCalls, []);
  assert.equal(rowWithId(undone.page, undone.ids[2]).getAttribute("data-archived"), "false");
  assert.match(undone.page.el("status").textContent, /^Not marked read in Discord: a Done it covers was refused or undone\.$/,
    "the status line does not say why the chat service was not asked");
  const kept = await undoWhileThroughSaving(STORE_REFUSES());
  assert.equal(rowWithId(kept.page, kept.ids[2]).getAttribute("data-archived"), "true", "the premise is the Undo refused");
  assert.deepStrictEqual(kept.asked, [kept.ids[2]], "a Through here whose Done stands after a refused Undo did not move the marker");
  const restored = kept.page.requests.findIndex((one) => one.endsWith("/restore"));
  assert.ok(kept.page.requests.findIndex((one) => one.endsWith("/upstream-read")) > restored,
    "the chat service was asked before the Undo was answered");
  // THE CONTROL: without the wait and the look, the marker moves through what the reader put back.
  const control = await undoWhileThroughSaving(null, brokenScript(...THROUGH_WAITS));
  assert.deepStrictEqual(control.asked, [control.ids[2]], "without the wait the chat service was not asked, so this test cannot see it");
});

/**
 * Message 3 of `conversation`, the chat service's marker supported: a Reply on it sent and confirmed, whose
 * own Done is held on its way; Undo pressed meanwhile from the chip, on `script`, which slips before the
 * Undo's write; then that Done saved. Answers the page, the target and the server's floor's asks.
 */
async function slipWhileSentDoneSaving(script) {
  const target = "1000000000000000003";
  const page = newPage(new Map(), script);
  page.upstreamReadMarkSupported = true;
  const asked = serverFloor(page);
  await signIn(page);
  await showDiscord(page, conversation());
  let release = null;
  page.inboxWriteHook = async (kind) => (kind === "dismiss" && release === null
    ? new Promise((resolve) => { release = () => resolve(null); }) : null);
  await replyAndSend(page, target, "answering");
  assert.equal(page.repliesPosted.at(-1).body.reply_to, target, "the premise is the reply sent and confirmed");
  assert.ok(release, "the premise is the reply's own Done on its way");
  await page.el("undo-dismiss").click().catch(() => {});
  for (let i = 0; i < 3; i += 1) await page.settle();
  assert.equal(page.el("error").hidden, false, "the premise is the slip said");
  release();
  for (let i = 0; i < 4; i += 1) await page.settle();
  assert.deepStrictEqual(page.restoreCalls, [], "the premise is the Undo never written");
  page.inboxWriteHook = null;
  return { page, target, asked };
}

test("`#327 done-races-remaining`: an Undo that fails in the page before its write, pressed while a sent reply's own Done is saved, leaves no wait hanging: the chat service is asked once, after that Done", async () => {
  const { page, target, asked } = await slipWhileSentDoneSaving(slipBefore(SCRIPT, BEFORE_UNDO_WRITE));
  assert.equal(page.dealtWith.has(target), true);
  assert.equal(rowWithId(page, target).getAttribute("data-archived"), "true", "the slipped Undo was not taken back over the reply's Done");
  assert.deepStrictEqual(asked, [target], "the reply's mark never came, or came otherwise than once");
  const dismissed = page.requests.map((one, at) => (one.endsWith("/dismiss") ? at : -1)).filter((at) => at >= 0).at(-1);
  assert.ok(page.requests.findIndex((one) => one.endsWith("/upstream-read")) > dismissed,
    "the chat service was asked before the reply's Done was on the server");
  // THE CONTROL: a slip outside the `try`, once the put back is noted as on its way: nothing ends that,
  // and the reply's mark waits for it for good.
  const loose = await slipWhileSentDoneSaving(slipBefore(SCRIPT, UNDO_OUTSIDE_TRY));
  assert.deepStrictEqual(loose.asked, [], "with the slip left the reply's mark came, so this test cannot see it hang");
});

// Found in the review of the above (2026-10-19), and made by it: a put back on its way takes the place
// of a Done of the same message still on its way in `doneSaving` (`noteDoneSaving`). An Undo that slipped
// in the page before its write ended its own wait at once, and left nothing there for the Done it undid,
// still on its way: a sent reply, Through here and Shift+E then asked the chat service at once, before
// that Done was answered, and so also when the server then refused it. The chat service's marker cannot
// be moved back. A wait now ends only once each one it took the place of has settled too.

/** `noteDoneSaving` ending its wait without the ones it took the place of, as first written: for negative controls. */
const WAIT_ENDS_ALONE = ["    if (before.length === 0) end();\n    else Promise.all(before).then(end);\n", "    end();\n"];

/** What asks the chat service once a Done is saved, each on message `id`, the chat service's marker supported. */
const ASKS_ONCE_SAVED = {
  "a sent reply": (page, id) => replyAndSend(page, id, "answering"),
  "Through here": (page, id) => readThroughButton(rowWithId(page, id)).click(),
  "Shift+E": (page) => pressHere(page, "E", { shiftKey: true }),
};

/**
 * The chat service's marker supported: message `id` marked Done with its Done button, its write held on its
 * way — the first of `backlog`, or for Shift+E message 3, selected and opened (`openThree`); Undo pressed
 * meanwhile from the chip, on `script` with a slip before the Undo's write; then `ask` on the message (one of
 * `ASKS_ONCE_SAVED`); then the Done answered `answer` (null for saved). Answers the page, the message, what
 * was asked of the chat service while the Done was on its way, and the server's floor's asks.
 */
async function askAfterSlippedUndo(ask, answer, script = SCRIPT) {
  const slipping = slipBefore(script, BEFORE_UNDO_WRITE);
  let asked = null;
  const arrange = (p) => { p.upstreamReadMarkSupported = true; asked = serverFloor(p); };
  let page;
  let id;
  if (ask === "Shift+E") {
    ({ page } = await openThree(slipping, arrange));
    id = "3";
  } else {
    page = newPage(new Map(), slipping);
    arrange(page);
    await signIn(page);
    id = (await showDiscord(page, backlog(3)))[0].getAttribute("data-id");
  }
  const dismissed = page.dismissCalls.length;
  let release = null;
  page.inboxWriteHook = async (kind) => (kind === "dismiss" && release === null
    ? new Promise((resolve) => { release = () => resolve(answer); }) : null);
  const done = doneButton(rowWithId(page, id)).click();
  await page.settle();
  assert.ok(release, "the premise is the Done on its way");
  await page.el("undo-dismiss").click().catch(() => {});
  for (let i = 0; i < 3; i += 1) await page.settle();
  assert.equal(page.el("error").hidden, false, "the premise is the Undo's slip said");
  assert.deepStrictEqual(page.restoreCalls, [], "the premise is the Undo never written");
  assert.equal(rowWithId(page, id).getAttribute("data-archived"), "true", "the premise is the slipped Undo taken back over the Done");
  if (ask === "Shift+E") assert.equal(selectedId(page), id, "the premise is the message still selected");
  const asking = ASKS_ONCE_SAVED[ask](page, id);
  for (let i = 0; i < 6; i += 1) await page.settle();
  const whileSaving = [...page.upstreamReadCalls];
  release();
  await done.catch(() => {});
  await asking;
  for (let i = 0; i < 6; i += 1) await page.settle();
  assert.deepStrictEqual(page.dismissCalls.slice(dismissed), [{ messages: [id] }], `the premise is the one Done written, ${ask} writing none`);
  page.inboxWriteHook = null;
  return { page, id, whileSaving, asked };
}

test("`#327 done-races-remaining`: an Undo that slips in the page before its write, pressed while the Done it undoes is on its way, leaves that Done waited for: a sent reply, Through here and Shift+E ask the chat service nothing until it is answered, nothing once it is refused, and once, after, when it is saved", async () => {
  for (const ask of Object.keys(ASKS_ONCE_SAVED)) {
    const refused = await askAfterSlippedUndo(ask, STORE_REFUSES());
    assert.deepStrictEqual(refused.whileSaving, [], `${ask}: the chat service was asked before the Done was answered`);
    assert.deepStrictEqual(refused.page.upstreamReadCalls, [], `${ask}: the chat service was asked for a Done the server refused`);
    assert.deepStrictEqual(refused.asked, []);
    assert.equal(refused.page.dealtWith.has(refused.id), false, "the premise is the Done refused");
    assert.equal(rowWithId(refused.page, refused.id).getAttribute("data-archived"), "false", "the refused Done was not taken back");

    const saved = await askAfterSlippedUndo(ask, null);
    assert.deepStrictEqual(saved.whileSaving, [], `${ask}: the chat service was asked before the Done was answered`);
    assert.deepStrictEqual(saved.asked, [saved.id], `${ask}: the chat service was not asked once, after the Done was saved`);
    assert.equal(saved.page.dealtWith.has(saved.id), true, "the premise is the Done saved");
    assert.equal(rowWithId(saved.page, saved.id).getAttribute("data-archived"), "true");
  }
  // THE CONTROL: the Undo's wait ended without the Done's, as first written: each asks the chat service while
  // the Done is on its way, and for a Done the server then refuses.
  for (const ask of Object.keys(ASKS_ONCE_SAVED)) {
    const control = await askAfterSlippedUndo(ask, STORE_REFUSES(), brokenScript(...WAIT_ENDS_ALONE));
    assert.deepStrictEqual(control.whileSaving.map((call) => call.message_id), [control.id],
      `${ask}: with the wait ended alone the chat service was not asked while the Done was on its way, so this test cannot see it`);
    assert.deepStrictEqual(control.asked, [control.id],
      `${ask}: with the wait ended alone the chat service was not asked for the refused Done, so this test cannot see it`);
  }
});

// Found in the same review, the same race as `#327 done-races-remaining` part (3) but for Shift+E's own
// Done, and older than that lane (`#370 shift-e-undo-marker`): `readAndCollapse` on a message not Done
// yet marked it Done and then asked the chat service without looking again, so an Undo pressed while
// that Done was on its way, the Undo itself saved, still moved the marker for a message put back.
// Shift+E now looks again once its Done is saved, as a sent reply does since `#327`.

/** The line that looks again after Shift+E's own Done, as first written (only for a Done made before): for negative controls. */
const SHIFT_E_LOOKS_AGAIN = ["  if (!(await doneSaved(ids))) return;\n", "  if (wasDone && !(await doneSaved(ids))) return;\n"];

/**
 * The chat service's marker supported: message 3 selected and opened (`openThree`), Shift+E pressed
 * with its Done held on its way; with `undo`, the Undo chip pressed meanwhile; then the Done answered
 * saved, and every write let settle. Answers the page and the server floor's asks.
 */
async function shiftEWithHeldDone(undo, script = SCRIPT) {
  let asked = null;
  const arrange = (p) => { p.upstreamReadMarkSupported = true; asked = serverFloor(p); };
  const { page } = await openThree(script, arrange);
  let release = null;
  page.inboxWriteHook = async (kind) => (kind === "dismiss" && release === null
    ? new Promise((resolve) => { release = () => resolve(null); }) : null);
  const pressing = pressHere(page, "E", { shiftKey: true });
  await page.settle();
  assert.ok(release, "the premise is Shift+E's Done on its way");
  // Not awaited yet: the Undo's write is queued behind the Done still on its way.
  const undoing = undo ? page.el("undo-dismiss").click().catch(() => {}) : null;
  for (let i = 0; i < 3; i += 1) await page.settle();
  release();
  await pressing;
  if (undoing) await undoing;
  for (let i = 0; i < 6; i += 1) await page.settle();
  page.inboxWriteHook = null;
  return { page, asked };
}

test("`#370 shift-e-undo-marker`: an Undo pressed while Shift+E's own Done is on its way, the Undo saved, leaves the chat service unasked; with no Undo it is asked once, after the Done is saved", async () => {
  const undone = await shiftEWithHeldDone(true);
  assert.deepStrictEqual(undone.page.restoreCalls, [{ messages: ["3"] }], "the premise is the Undo written");
  assert.equal(undone.page.dealtWith.has("3"), false, "the premise is the message put back on the server");
  assert.equal(rowWithId(undone.page, "3").getAttribute("data-archived"), "false", "the premise is the message drawn put back");
  assert.deepStrictEqual(undone.page.upstreamReadCalls, [], "the chat service was asked for a message put back meanwhile");
  assert.deepStrictEqual(undone.asked, [], "the server's floor was asked for a message put back meanwhile");

  const kept = await shiftEWithHeldDone(false);
  assert.deepStrictEqual(kept.page.restoreCalls, [], "the premise is no Undo written");
  assert.equal(kept.page.dealtWith.has("3"), true, "the premise is the Done saved");
  assert.deepStrictEqual(kept.asked, ["3"], "the chat service was not asked once, after Shift+E's Done was saved");

  // THE CONTROL: Shift+E looked again only for a Done made before it, as first written: the Undo
  // saved, the chat service is still asked for the message put back.
  const control = await shiftEWithHeldDone(true, brokenScript(...SHIFT_E_LOOKS_AGAIN));
  assert.deepStrictEqual(control.page.restoreCalls, [{ messages: ["3"] }], "the premise is the Undo written");
  assert.equal(control.page.dealtWith.has("3"), false, "the premise is the message put back on the server");
  assert.deepStrictEqual(control.asked, ["3"],
    "with the look again missing the chat service was not asked for the message put back, so this test cannot see it");
});
