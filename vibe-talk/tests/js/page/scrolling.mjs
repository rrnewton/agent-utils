// The app page's tests: the reader's place: scrollback, pulling past either end, and the jump to the newest.
//
// One feature's file of the suite tests/js/voice_page.test.mjs runs, against the fixture in
// ./fixture.mjs. Run with the rest of the suite, or alone: node --test tests/js/page/scrolling.mjs.

import assert from "node:assert/strict";

import { BOTTOM_SLACK_PX, CHANNEL, CHARS_PER_LINE, CSS_CODE, CSS_UNCONDITIONAL, DESKTOP_QUERY,
  DISCORD_POLL_MS, HTML_CODE, LINE_PX, PAGE_CODE, PAGE_ELEMENTS, PAGE_IDS, PULL_ARM_PX,
  PULL_WAIT_MS, RELAY_FENCE_OPEN, SCRIPT, SCRIPT_CODE, SCROLLBACK_PX, SCROLL_VIEWPORT_PX,
  SHORT_MESSAGE, assertMarkupContains, assistantSays, atBottomOf, brokenModule, brokenScript,
  channelSummaryText, countingChannel, cssBlock, cssBlockIn, cssBlockOutside, cssRules, deliver,
  errorResponse, fillTranscript, focused, foldButton, gate, helpEntry, holdChannelReads,
  inTwoColumns, json, jumpSaysArrived, jumpShown, longMessage, manyThreadColumns, markupHolds,
  markupPlace, message, newPage, outgoingRetry, outgoingRows, pagedChannel, parkMidway, passTime,
  press, pullDown, reReadChannel, readerOn, relayed, reopenedChannel, replyButton, replyInStream,
  rowWithId, savedOutgoing, scrollToGap, sentWithoutKey, setReadNew, settingsGroup, showDiscord,
  shownIds, sideAtFoot, signIn, sourceConstant, speakRelay, sseMessage, sseUpdate, startTalking,
  tallChannel, test, threadPage, touchAt, upstreamReadButton, withLiveChannel, youSay } from "./fixture.mjs";

test("a turn arriving FOLLOWS the newest line for a reader already at the bottom", async () => {
  // The positive half. Without it, every "the view held still" test below is satisfied by a page
  // that never scrolls at all.
  const page = newPage();
  await startTalking(page);
  const area = fillTranscript(page);
  assert.ok(atBottomOf(area), "arrivals should have left the reader at the bottom");
  const before = area.scrollTop;

  assistantSays(page, longMessage("newest"));

  assert.ok(area.scrollTop > before, "the transcript did not follow the newest line");
  assert.ok(atBottomOf(area), "it followed, but not all the way to the newest line");
  assert.equal(page.el("jump-newest").hidden, true, "nothing is off screen to jump to");
});

test("YOUR OWN TURN DOES NOT YANK YOU TO THE BOTTOM once you have scrolled up", async () => {
  // This page has no typed-reply control yet, so `user_transcript` IS "sending a reply" here: it
  // is the arrival the reader themselves caused. It must not move the view either.
  const parked = async (script) => {
    const page = newPage(new Map(), script);
    await startTalking(page);
    const area = fillTranscript(page);
    area.scrollTop = 120; // reading something near the top of the history
    youSay(page, longMessage("mine"));
    return { page, area };
  };

  const { page, area } = await parked();
  assert.equal(area.scrollTop, 120, "the reader's own turn dragged them to the bottom");
  assert.equal(
    page.el("jump-newest").hidden,
    false,
    "the view held still and said nothing, so the reader cannot tell a turn arrived"
  );

  // The control: the OLD rule, an unconditional scroll on every arrival. It has to fail.
  const control = await parked(
    brokenScript(
      'function followIfPinned(pinned, turns = 0, view = "voice") {\n  if (pinned && currentView === view) {',
      'function followIfPinned(pinned, turns = 0, view = "voice") {\n  if (true) {'
    )
  );
  assert.notEqual(
    control.area.scrollTop,
    120,
    "the negative control did not reproduce the defect, so this test cannot detect it either"
  );
});

test("COLLAPSING A MESSAGE ABOVE THE VIEWPORT LEAVES THE READER LOOKING AT THE SAME LINE", async () => {
  // The case a browser's own scroll anchoring does not cover: the thing that changed height is
  // entirely above what the reader can see, so nothing the browser anchors to has moved on screen
  // and it does not compensate. Everything below it slides up under the eye.
  const run = async (script) => {
    const page = newPage(new Map(), script);
    await startTalking(page);
    const area = fillTranscript(page);

    // Open one of the earliest answers, so collapsing it later is a real change of height.
    const early = page.el("transcript").children[1];
    const fold = foldButton(early);
    assert.ok(fold, "the first long answer arrived with no fold control");
    await fold.click();
    assert.equal(early.getAttribute("data-collapsed"), "false", "the fold did not open");

    // Park well down the list, with that message off the top of the screen.
    area.scrollTop = Math.round((area.scrollHeight - area.clientHeight) * 0.7);
    assert.ok(
      early.getBoundingClientRect().bottom < 0,
      "the message being collapsed is still on screen, so this is not the case under test"
    );
    const anchor = page.el("transcript").children.find((li) => li.getBoundingClientRect().bottom > 0);
    assert.ok(anchor, "nothing is on screen to anchor to");
    const before = anchor.getBoundingClientRect().top;

    await fold.click(); // collapse it again, from above the viewport

    assert.equal(early.getAttribute("data-collapsed"), "true", "the fold did not close");
    return { page, moved: anchor.getBoundingClientRect().top - before };
  };

  const { moved } = await run();
  assert.equal(moved, 0, `the line the reader was looking at moved ${moved}px`);

  // The control: the same page with the restore deleted. The whole test rests on the fixture being
  // able to SEE the difference, so it has to be shown seeing it.
  const control = await run(
    brokenScript(
      "  area.scrollTop += mark.anchor.getBoundingClientRect().top - mark.top;",
      "  void mark;"
    )
  );
  assert.notEqual(
    control.moved,
    0,
    "with the scroll restore deleted the view still did not move, so this fixture cannot tell " +
      "an anchored page from an unanchored one"
  );
});

test("COLLAPSE ALL puts the list back in one tap, without moving the reader", async () => {
  const page = newPage();
  await startTalking(page);
  const area = fillTranscript(page);
  const longOnes = page.el("transcript").children.filter((li) => foldButton(li));
  assert.ok(longOnes.length >= 3, "not enough long messages to be worth collapsing");

  assert.equal(page.el("collapse-all").hidden, true, "nothing is expanded, so there is no offer");
  for (const li of longOnes) {
    await foldButton(li).click();
  }
  assert.equal(page.el("collapse-all").hidden, false, "the offer never appeared");

  area.scrollTop = Math.round((area.scrollHeight - area.clientHeight) * 0.7);
  const anchor = page.el("transcript").children.find((li) => li.getBoundingClientRect().bottom > 0);
  const before = anchor.getBoundingClientRect().top;

  await page.el("collapse-all").click();

  assert.ok(
    longOnes.every((li) => li.getAttribute("data-collapsed") === "true"),
    "collapse all left something open"
  );
  assert.equal(
    anchor.getBoundingClientRect().top,
    before,
    "collapsing everything moved the reader"
  );
  assert.equal(page.el("collapse-all").hidden, true, "the offer stayed after it was taken");
});

test("JUMP TO NEWEST says a turn arrived off screen, and takes you there", async () => {
  const page = newPage();
  await startTalking(page);
  const area = fillTranscript(page);
  assert.equal(page.el("jump-newest").hidden, true, "the chip is up while the reader is pinned");

  area.scrollTop = 120;
  assistantSays(page, longMessage("while you were reading"));
  assert.equal(page.el("jump-newest").hidden, false, "a turn arrived off screen and said nothing");
  assert.equal(area.scrollTop, 120, "the chip appeared AND the page moved anyway");

  await page.el("jump-newest").click();
  assert.ok(atBottomOf(area), "the chip did not take the reader to the newest line");
  assert.equal(page.el("jump-newest").hidden, true, "the chip stayed after it was taken");
});

test("reaching the bottom yourself puts the chip away, without a tap", async () => {
  const page = newPage();
  await startTalking(page);
  const area = fillTranscript(page);
  area.scrollTop = 120;
  assistantSays(page, longMessage("newest"));
  assert.equal(page.el("jump-newest").hidden, false);

  // A thumb, not a control: the reader scrolls down to the bottom themselves.
  area.scrollTop = area.scrollHeight - area.clientHeight;
  await area.dispatch("scroll");

  assert.equal(page.el("jump-newest").hidden, true, "the chip outstayed the reason for it");
});

test("ONE COLLAPSING IDIOM: the transcript and the channel fold a long message identically", async () => {
  // The property the issue asks for, stated as a comparison rather than as two separate checks —
  // two separate checks are satisfied by two implementations that have drifted apart.
  const page = newPage();
  await startTalking(page);
  assistantSays(page, longMessage("spoken"));
  const spoken = page.el("transcript").children.at(-1);

  page.messages = [message({ id: "1", content: longMessage("posted") })];
  await page.el("view-switch").click();
  await page.settle();
  const posted = page.el("discord-log").children.at(-1);

  for (const [where, li] of [
    ["the transcript", spoken],
    ["the channel", posted],
  ]) {
    const fold = foldButton(li);
    assert.ok(fold, `${where} rendered a long message with no fold control`);
    assert.equal(fold.tagName, "button", `${where}'s fold control is not a button`);
    assert.equal(li.getAttribute("data-collapsed"), "true", `${where} did not arrive collapsed`);
    const body = li.descendants().find((node) => node.hasClass("body"));
    assert.ok(body.hasClass("clamped"), `${where} collapsed the wrong element`);
    assert.equal(fold.getAttribute("aria-expanded"), "false", `${where} misreports its state`);
  }

  assert.equal(
    foldButton(spoken).className,
    foldButton(posted).className,
    "the two lists style their fold controls differently"
  );
  assert.equal(
    foldButton(spoken).textContent,
    foldButton(posted).textContent,
    "the two lists word their fold controls differently"
  );

  // And it is literally one function — not implementations that agree today — called once by each
  // row builder: a turn, a channel message, and (`#270 voice-transcript-markers`) a frozen send card,
  // which is a row of the transcript like any turn.
  assert.match(SCRIPT_CODE, /function foldable\(/, "the folding idiom is not a shared function");
  const calls = (SCRIPT_CODE.match(/foldable\(/g) || []).length - 1;
  assert.equal(calls, 3, `foldable() is called from ${calls} places; each row builder needs it once`);
});

test("a short message gets no fold control at all, in either list", async () => {
  // A control that would reveal nothing, on every line of the list, is chrome charging rent.
  const page = newPage();
  await startTalking(page);
  assistantSays(page, SHORT_MESSAGE);
  const spoken = page.el("transcript").children.at(-1);

  page.messages = [message({ id: "1", content: SHORT_MESSAGE })];
  await page.el("view-switch").click();
  await page.settle();
  const posted = page.el("discord-log").children.at(-1);

  for (const [where, li] of [
    ["the transcript", spoken],
    ["the channel", posted],
  ]) {
    assert.equal(foldButton(li), undefined, `${where} put a fold control on a short message`);
    assert.equal(li.getAttribute("data-collapsed"), null, `${where} marked a short message`);
    const body = li.descendants().find((node) => node.hasClass("body"));
    assert.ok(!body.hasClass("clamped"), `${where} clamped a message with nothing to hide`);
  }
});

test("expanding one message leaves its neighbours exactly as they were", async () => {
  // Otherwise "expand" is a mode, and the list you come back to is not the list you left.
  const page = newPage();
  await startTalking(page);
  fillTranscript(page);
  const longOnes = page.el("transcript").children.filter((li) => foldButton(li));
  assert.ok(longOnes.length >= 3, "not enough long messages to have neighbours");
  assert.ok(
    longOnes.every((li) => li.getAttribute("data-collapsed") === "true"),
    "long messages did not ARRIVE collapsed"
  );

  const fold = foldButton(longOnes[1]);
  await fold.click();

  assert.equal(longOnes[1].getAttribute("data-collapsed"), "false");
  assert.equal(fold.getAttribute("aria-expanded"), "true");
  assert.ok(
    longOnes.filter((_li, i) => i !== 1).every((li) => li.getAttribute("data-collapsed") === "true"),
    "expanding one message expanded its neighbours too"
  );

  await fold.click();
  assert.equal(longOnes[1].getAttribute("data-collapsed"), "true", "the fold does not close again");
});

test("the collapsed height is really LINES — the reader's count, three until they say otherwise — and the chips are outside the faded region", () => {
  // The fixture cannot prove a line clamp — it has no renderer — so the clamp is asserted as a
  // declaration on the selector that has to carry it. `overflow: hidden` is checked by name
  // because without it the box shows every line and the clamp does nothing at all.
  const clamped = cssBlock(".body.clamped");
  assert.match(clamped, /-webkit-line-clamp:\s*var\(--collapsed-lines\)/, "the clamp is not the reader's lines");
  assert.match(cssBlock(":root"), /--collapsed-lines:\s*3\b/, "the page has no default collapsed-lines count");
  assert.match(clamped, /overflow:\s*hidden/, "without this the clamp shows every line anyway");
  assert.match(clamped, /-webkit-box-orient:\s*vertical/);

  // #scroll-area carries a mask-image gradient that fades its top edge. A chip inside it would be
  // faded with the content, and would scroll away with it.
  assert.match(cssBlock("#scroll-area"), /mask-image/, "the fade this reasoning depends on is gone");
  assertMarkupContains("screen-main", "scroll-tools");
  assert.throws(
    () => assertMarkupContains("scroll-area", "scroll-tools"),
    /no longer nests/,
    "the chips are inside the scrolling element, where the mask fades them"
  );
  assert.match(cssBlock("#screen-main"), /position:\s*relative/, "the chips have nothing to sit on");
});

test("SWITCHING VIEWS AND COMING BACK KEEPS YOUR PLACE", async () => {
  // The one path that was genuinely broken. Both panes share one scroll container, so leaving a
  // view loses its position unless somebody writes it down — and nobody did: every switch called
  // `scrollToNewest`. A glance at the transcript cost a reader forty messages of scrolling.
  const page = newPage();
  await signIn(page);
  const area = await walkedBackChannel(page);
  const parked = parkMidway(page);

  await page.el("view-switch").click();
  await page.settle();
  await page.el("view-switch").click();
  await page.settle();

  assert.equal(area.scrollTop, parked, "coming back to the channel lost the reader's place");
});

test("...but a FIRST entry still lands on the newest message", async () => {
  // The reasoning the unconditional jump was built on was sound for a first entry and wrong for
  // every one after it. Arriving at the TOP of a long channel means scrolling past everything
  // already read to reach what you opened it for.
  //
  // A PLAIN first entry, not `walkedBackChannel`: that helper scrolls on its own, so removing the
  // jump entirely left the earlier version of this test green — it was asserting the helper's
  // behaviour rather than the page's.
  const page = newPage();
  await signIn(page);
  const area = page.el("scroll-area");
  area.scrollTop = 0;
  await showDiscord(page, tallChannel());

  assert.ok(area.scrollHeight > area.clientHeight, "the fixture cannot scroll, so this proves nothing");
  assert.ok(atBottomOf(area), "a first entry did not land on the newest message");
});

test("a message ARRIVING does not move a reader who is elsewhere", async () => {
  const page = newPage();
  // A TALL channel: with one message there is nothing to scroll, "parked" is the bottom, and the
  // test would pass by accident while proving nothing.
  const stream = await withLiveChannel(page, tallChannel());
  const area = page.el("scroll-area");
  const parked = parkMidway(page);
  assert.ok(parked > 0, "the fixture is not tall enough to have a place to lose");

  await deliver(page, stream, sseMessage(message({ id: "200", content: "new" })));

  assert.equal(area.scrollTop, parked, "an arriving message dragged the reader to the bottom");
  // ...and it says something arrived, because silently leaving them put is the other half of it.
  assert.equal(page.el("jump-newest").hidden, false, "nothing said a message had arrived");
});

test("the POLL does not move a reader who is elsewhere", async () => {
  const page = newPage();
  await signIn(page);
  const area = await walkedBackChannel(page);
  const parked = parkMidway(page);

  await reReadChannel(page);

  assert.equal(area.scrollTop, parked, "the 45-second poll moved the reader");
});

test("SENDING A REPLY does not move you — the whole complaint, pinned", async () => {
  const page = newPage();
  await signIn(page);
  const area = await walkedBackChannel(page);
  const parked = parkMidway(page);

  await page.el("reply-text").setValue("restarted it");
  await page.el("reply-send").click();
  await page.settle();

  assert.equal(area.scrollTop, parked, "replying threw away the reader's place in the backlog");
});

// --- ...AND THE SAME FOR THE VOICE TRANSCRIPT ---------------------------------------------------
//
// The audit above pinned every path in the CHANNEL and recorded, honestly, that it had not reached
// the other list: deleting `showView`'s jump left the whole suite green, because on the channel
// path the entry read lands on the newest anyway. So the line was load-bearing for the transcript
// and nothing was watching it. This is that half.
//
// TWO OF THESE ARE ALREADY HERE and are not repeated: "a turn arriving FOLLOWS the newest line for
// a reader already at the bottom" and "JUMP TO NEWEST says a turn arrived off screen, and takes you
// there" are both about the transcript, both use a transcript tall enough to scroll, and between
// them pin the two arrival cases — follow when pinned, hold still and raise the chip when not. What
// was missing is everything about ENTERING and LEAVING this view.
//
// Writing them found two defects, neither of which the channel has:
//
//   * A SAVED scrollTop IS NOT "KEEP ME ON THE NEWEST". A call goes on talking while you read a
//     backlog, so the transcript is the one list that grows while nobody is in it. Coming back put
//     the reader at the offset the bottom used to be at — several turns above the end, with nothing
//     saying so. `viewScroll` remembers whether they were on the newest now, which is the rule the
//     channel's own re-read has always settled by.
//   * A TURN ARRIVING ASKED THE WRONG LIST whether the reader was at the bottom. Both panes share
//     one #scroll-area, so a turn landing while the reader was at the bottom of the CHANNEL
//     scrolled the channel and put the CHANNEL's chip away, and never raised the transcript's.

/** A stored conversation long enough that restoring it really overflows the scroll area. */
const tallConversation = (count = 12) =>
  Array.from({ length: count }, (_unused, i) => ({
    speaker: i % 2 === 0 ? "you" : "agent",
    text: longMessage(`turn ${i + 1}`),
    at_ms: 1_700_000_000_000 + i * 60_000,
  }));

/** A live call with a transcript that overflows, and a tall channel waiting behind the switch. */
async function talkingBesideAChannel(page) {
  page.messages = tallChannel();
  await startTalking(page);
  return fillTranscript(page);
}

/** The turn at the end of the transcript, as a rectangle the reader can be asked to look at. */
function newestTurnOnScreen(page) {
  const rows = page.el("transcript").children;
  const rect = rows[rows.length - 1].getBoundingClientRect();
  const viewport = page.el("scroll-area").clientHeight;
  return { rect, viewport, visible: rect.bottom > 0 && rect.top < viewport };
}

test("a FIRST entry into the voice transcript lands on the newest turn", async () => {
  // The transcript's counterpart to "the channel view opens on the NEWEST message". A stored
  // conversation is what makes a first entry have anything in it at all: the page opens on this
  // view with nothing in the list, and restoring an earlier conversation is what fills it.
  //
  // No helper that scrolls: `signIn` is the page's own load path and touches no scroll position.
  const page = newPage();
  page.storedTurns.set("conv_earlier", tallConversation());
  const area = page.el("scroll-area");

  await signIn(page);
  await page.settle();

  assert.ok(
    area.scrollHeight > area.clientHeight * 2,
    `the restored transcript is ${area.scrollHeight} tall in a ${area.clientHeight} box, so it ` +
      "does not overflow and landing at the bottom would prove nothing"
  );
  assert.ok(atBottomOf(area), "the transcript opened at the top of the restored conversation");
  const newest = newestTurnOnScreen(page);
  assert.ok(
    newest.visible,
    `the newest turn occupies ${newest.rect.top}..${newest.rect.bottom} of a 0..${newest.viewport} ` +
      "viewport, so the reader was taken somewhere it cannot be seen"
  );
});

test("SWITCHING VIEWS AND COMING BACK KEEPS YOUR PLACE IN THE TRANSCRIPT TOO", async () => {
  // `viewScroll` holds both views and only the channel half was tested. Glancing at the backlog
  // must not cost a reader their place in what the assistant said ten turns ago either.
  const page = newPage();
  const area = await talkingBesideAChannel(page);
  const parked = parkMidway(page);
  assert.ok(parked > 0, "the transcript is not tall enough to have a place to lose");

  await page.el("view-switch").click();
  await page.settle();
  assert.equal(page.tab(), "discord", "the switch did not reach the channel");
  await page.el("view-switch").click();
  await page.settle();
  assert.equal(page.tab(), "voice");

  assert.equal(area.scrollTop, parked, "coming back to the transcript lost the reader's place");
});

test("...but a reader who left ON THE NEWEST TURN comes back to the NEWEST TURN", async () => {
  // The defect a saved scrollTop cannot see, and the one line in `showView` this whole audit could
  // not reach until now. The call goes on talking while the reader is over on the channel, so the
  // transcript GROWS while they are away — and "where I was" for a reader parked on the newest line
  // is the newest line, not the number of pixels the newest line used to sit at.
  const page = newPage();
  const area = await talkingBesideAChannel(page);
  assert.ok(atBottomOf(area), "the reader is not on the newest turn, so this is not the case here");

  await page.el("view-switch").click(); // over to the channel...
  await page.settle();
  assert.equal(page.tab(), "discord", "the switch did not reach the channel");
  area.scrollTop = 0; // ...and up to the top of the backlog, far from the transcript's own bottom
  for (const n of [1, 2, 3]) {
    assistantSays(page, longMessage(`while you were away ${n}`));
  }

  await page.el("view-switch").click(); // and back
  await page.settle();
  assert.equal(page.tab(), "voice");

  assert.ok(
    area.scrollHeight > area.clientHeight * 2,
    "the transcript does not overflow, so landing at the bottom would prove nothing"
  );
  assert.ok(
    atBottomOf(area),
    `coming back left the reader at ${area.scrollTop} of a transcript that now ends at ` +
      `${area.scrollHeight - area.clientHeight} — where the newest turn USED to be, not where it is`
  );
  const newest = newestTurnOnScreen(page);
  assert.ok(
    newest.visible,
    `the newest turn occupies ${newest.rect.top}..${newest.rect.bottom} of a 0..${newest.viewport} ` +
      "viewport, so the reader was taken somewhere it cannot be seen"
  );
  assert.equal(page.el("jump-newest").hidden, true, "the reader IS on the newest turn");
});

test("A TURN ARRIVING WHILE YOU READ THE CHANNEL SAYS SO WHEN YOU COME BACK", async () => {
  // Both panes share one #scroll-area, so `atBottom` answers about whichever list is UP. A turn
  // landing while the reader sat at the bottom of the channel was therefore read as "the reader is
  // pinned to the transcript": it scrolled the channel, put the CHANNEL's chip away, and left the
  // transcript's unraised. The reader came back to their place with three turns below it and
  // nothing on the screen admitting they existed.
  const page = newPage();
  const area = await talkingBesideAChannel(page);
  const parked = parkMidway(page);
  assert.ok(parked > 0, "the transcript is not tall enough to have a place to lose");

  await page.el("view-switch").click();
  await page.settle();
  assert.equal(page.tab(), "discord", "the switch did not reach the channel");
  assert.ok(atBottomOf(area), "the reader is not at the bottom of the channel, which is the case");
  const inTheChannel = area.scrollTop;

  assistantSays(page, longMessage("while you were reading the backlog"));

  assert.equal(area.scrollTop, inTheChannel, "a voice turn moved the channel the reader was on");

  await page.el("view-switch").click();
  await page.settle();

  assert.equal(area.scrollTop, parked, "coming back to the transcript lost the reader's place");
  assert.equal(
    page.el("jump-newest").hidden,
    false,
    "a turn arrived while the reader was away and nothing on the transcript said so"
  );
});

test("A REPLY THAT ONLY HALF POSTED KEEPS THE REST OF YOUR TEXT", async () => {
  // The server splits a long reply and can get halfway. It answers 207 for that — which `fetch`
  // reports as `ok`, so a page that only checked `response.ok` would treat it as success, clear
  // the draft, and destroy the only copy of the remainder. Sending again would then post the first
  // half twice.
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [message({ id: "1000000000000000001" })]);
  await replyButton(rows[0]).click();
  await page.settle();

  const typed = "first half. second half.";
  page.el("reply-text").value = typed;
  await page.el("reply-text").dispatch("input", {});

  page.replyResponse = async () =>
    json(207, {
      error: "partially_posted",
      detail: "discord returned HTTP 500",
      posted: 1,
      unsent: "second half.",
    });

  await page.el("reply-send").click();
  await page.settle();

  // Submission returns to history immediately. The remainder belongs to its outgoing message,
  // leaving the editor free for another draft without losing what the server did not accept.
  assert.equal(page.screen(), "main");
  assert.equal(page.el("reply-text").value, "");
  const held = outgoingRows(page)[0];
  assert.match(held.text(), /second half\./);
  assert.doesNotMatch(held.text(), /first half\./);
  assert.match(held.text(), /1 part/i);
  assert.equal(savedOutgoing(page)[0].remaining, "second half.");
  assert.equal(savedOutgoing(page)[0].replyTo, "1000000000000000001");

  page.replyResponse = async () => json(200, {
    posted: message({ id: "9000000000000000002", content: "second half." }),
  });
  await outgoingRetry(held).click();
  await page.settle();
  assert.deepEqual(sentWithoutKey(page.repliesPosted[1].body), {
    text: "second half.", reply_to: "1000000000000000001",
  }, "retry repeated the already accepted prefix");
});

test("...and a reply that fully posted clears the draft and shows every part", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [message({ id: "1000000000000000001" })]);
  const before = page.el("discord-log").children.length;
  await replyButton(rows[0]).click();
  await page.settle();
  page.el("reply-text").value = "all of it";
  await page.el("reply-text").dispatch("input", {});

  page.replyResponse = async () =>
    json(200, {
      posted: message({ id: "9000000000000000001", content: "part one" }),
      parts: [
        message({ id: "9000000000000000001", content: "part one" }),
        message({ id: "9000000000000000002", content: "part two" }),
      ],
    });

  await page.el("reply-send").click();
  await page.settle();

  assert.equal(
    page.el("discord-log").children.length,
    before + 2,
    "only one part of a split reply reached the channel view"
  );
  const kept = JSON.parse(page.storage.get("vibe-talk.voice.drafts") || "{}");
  assert.equal(kept["1000000000000000001"], undefined, "a fully sent reply left its draft behind");
});

test("the channel spends its width on words, not on insets", async () => {
  // 15% of a 393-pixel phone, on every row, bought what the speaker COLOUR now buys.
  const mine = cssBlock('#discord-log li.discord-message[data-who="me"]');
  assert.match(mine, /--speaker-inset/, "the phone rule still hard-codes a percentage inset");
  assert.doesNotMatch(mine, /margin-left:\s*15%/, "the 15% inset survived");
});

test("the channel view shows the READER's clock, not the server's and not UTC", async () => {
  const page = newPage();
  await signIn(page);
  // `#52 operator-timezone` had the server convert once, into `server.timezone`, and the page
  // print whatever it was handed. That is right for the VOICE AGENT, which has to speak a time and
  // cannot ask a browser. It is wrong for a phone: `server.timezone` defaults to UTC, so an
  // operator who never configured one reads every message in UTC while holding a device that
  // knows its own zone. The browser's zone is not a guess.
  //
  // `spoken_time` here is deliberately UNLIKE anything the server sends — the real form is a bare
  // `21:31`. A fixture that matched it would make this test pass whether the page used it or not.
  const lines = await showDiscord(page, [
    message({ timestamp: "2026-08-19T04:31:00.000Z", spoken_time: "2026-08-18 21:31 PDT" }),
  ]);
  const text = lines[0].text();
  assert.doesNotMatch(text, /PDT/, "the page is still printing the SERVER's configured zone");
  assert.doesNotMatch(text, /2026-08-19T04:31/, "the page rendered the raw ISO stamp");
  assert.match(text, /\d{1,2}:\d{2}/, "no clock reached the row at all");
});

test("a bot author is labelled as one", async () => {
  const page = newPage();
  await signIn(page);
  const lines = await showDiscord(page, [message({ author: "MyDiscordBot", author_is_bot: true })]);
  assert.match(lines[0].text(), /MyDiscordBot \(bot\)/);
});

test("THE CHANNEL WALKS BACK USING THE CURSOR THE SERVER HANDED BACK", async () => {
  // Not arithmetic on an offset, and not a second read of the same window: the id the previous
  // step named is the id that goes out, which is the whole contract of the cursored route.
  const page = newPage();
  await signIn(page);
  const all = pagedChannel(page, { steps: 3, size: 4 });
  await showDiscord(page, []);

  const log = page.el("discord-log");
  assert.equal(log.children.length, 4, "the first read is one page, not the whole channel");
  assert.equal(page.el("load-older").hidden, false, "nothing says there is more above");

  await page.el("load-older").click();
  await page.settle();

  assert.equal(log.children.length, 8, "the older step did not arrive");
  // Oldest first, and the older step is ABOVE the newer one — a channel read upside down would
  // otherwise satisfy a count.
  assert.deepStrictEqual(
    log.children.map((li) => li.getAttribute("data-id")),
    all.slice(4, 12).map((m) => m.id)
  );
  // The cursor really was the one handed back: the oldest id of the first page.
  assert.match(
    page.pagesServed[1],
    new RegExp(`[?&]before=${all[8].id}(&|$)`),
    `it asked for ${page.pagesServed[1]}, not for the oldest id of the page it was handed`
  );
  assert.doesNotMatch(page.pagesServed[0], /before=/, "the first read must not carry a cursor");
});

test("LOADING OLDER MESSAGES LEAVES THE READER LOOKING AT THE SAME LINE", async () => {
  // Prepending is a mutation ABOVE the viewport, which is the one case a browser's own scroll
  // anchoring does not cover — the same case as collapsing something the reader scrolled past, and
  // handled by the same helper rather than by a second mechanism.
  const run = async (script) => {
    const page = newPage(new Map(), script);
    await signIn(page);
    page.channelPage = async (path) => {
      const before = /[?&]before=/.test(String(path));
      const body = Array.from({ length: 8 }, (_u, i) =>
        message({ id: String((before ? 2000 : 3000) + i), content: longMessage(`m${i}`) })
      );
      return json(200, { channel: CHANNEL, messages: body, has_more: true, next_before: "1999" });
    };
    await showDiscord(page, []);
    const area = page.el("scroll-area");
    assert.ok(area.scrollHeight > area.clientHeight * 2, "the channel does not overflow");
    area.scrollTop = Math.round((area.scrollHeight - area.clientHeight) * 0.5);
    const anchor = page.el("discord-log").children.find(
      (li) => li.getBoundingClientRect().bottom > 0
    );
    const before = anchor.getBoundingClientRect().top;

    await page.el("load-older").click();
    await page.settle();

    return { page, area, moved: anchor.getBoundingClientRect().top - before };
  };

  const { page, area, moved } = await run();
  assert.equal(page.el("discord-log").children.length, 16, "the older step did not arrive");
  assert.equal(moved, 0, `the line the reader was looking at moved ${moved}px`);

  // WHAT THIS ASSERTION IS, EXACTLY. `#74 scroll-test-strength`, finding 1, recorded here rather
  // than left for the next reader to rediscover: `getBoundingClientRect().top` in this fixture is
  // `offsetTop() - scrollTop` and the page's restore is `scrollTop += top - before`, so the two
  // substituted into each other make the post-mutation top identically `before`. Read on its own,
  // `moved === 0` therefore says "the restore formula was applied, against the same anchor" — a
  // real claim, and a smaller one than the sentence in the test's name.
  //
  // Two things make the rest of the claim real, and both are below. The fixture CLAMPS scrollTop
  // the way a browser does, so a restore that scrolls past the end of the content is refused and
  // the anchor moves; and the position is asserted to be strictly INSIDE the range, so `moved ===
  // 0` cannot have been reached by being pinned against either end of it.
  const furthest = area.scrollHeight - area.clientHeight;
  assert.ok(
    area.scrollTop > 0 && area.scrollTop < furthest,
    `the restore landed at ${area.scrollTop} of 0..${furthest} — against an end of the range, ` +
      "which is a position the clamp could have produced from any overshoot at all"
  );

  // THE CONTROL. Without it the fixture could be one that never moves scrollTop at all.
  const control = await run(
    brokenScript(
      "  area.scrollTop += mark.anchor.getBoundingClientRect().top - mark.top;",
      "  void mark;"
    )
  );
  assert.notEqual(
    control.moved,
    0,
    "with the restore deleted the view still did not move, so this test cannot tell an anchored " +
      "page from an unanchored one"
  );

  // THE SECOND CONTROL, and the one that answers the reading above: "against the same anchor" is
  // the load-bearing half of what `moved === 0` proves, so a page that applies the very same
  // formula against a DIFFERENT element has to fail here. It is not a hypothetical mistake — the
  // first child of the log is a perfectly natural thing to measure, and after a prepend it is not
  // the same row it was.
  const wrongAnchor = await run(
    brokenScript(
      "  area.scrollTop += mark.anchor.getBoundingClientRect().top - mark.top;",
      '  area.scrollTop += el("discord-log").children[0].getBoundingClientRect().top - mark.top;'
    )
  );
  assert.notEqual(
    wrongAnchor.moved,
    0,
    "the restore measured against a different element and the reader still did not move, so this " +
      "test cannot see WHICH anchor the formula was applied to"
  );
});

test("THE LAST STEP OF THE WALK DOES NOT JERK THE READER EITHER", async () => {
  // The step nobody photographs. Reaching the beginning HIDES #load-older, and that control is a
  // sibling ABOVE #discord-log inside #scroll-area — so taking its height away is the same
  // mutation as prepending, made in the same place, and it has to be inside the same anchor. A
  // page that anchors the rows and then hides the button afterwards moves the reader by the height
  // of a button on the one step where they have finally arrived somewhere.
  const page = newPage();
  await signIn(page);
  pagedChannel(page, { steps: 2, size: 8, content: (i) => longMessage(`m${i}`) });
  await showDiscord(page, []);

  const area = page.el("scroll-area");
  assert.ok(area.scrollHeight > area.clientHeight * 2, "the channel does not overflow");
  assert.equal(page.el("load-older").hidden, false, "nothing says there is more above");
  const grip = page.el("load-older").getBoundingClientRect().height;
  assert.ok(grip > 0, "the control has no modelled height, so hiding it could not move anything");

  area.scrollTop = Math.round((area.scrollHeight - area.clientHeight) * 0.5);
  const anchor = page
    .el("discord-log")
    .children.find((li) => li.getBoundingClientRect().bottom > 0);
  const before = anchor.getBoundingClientRect().top;

  await page.el("load-older").click();
  await page.settle();

  assert.equal(page.el("discord-log").children.length, 16, "the older step did not arrive");
  assert.equal(page.el("load-older").hidden, true, "the walk did not reach the beginning");
  const moved = anchor.getBoundingClientRect().top - before;
  assert.equal(
    moved,
    0,
    `arriving at the beginning of the channel moved the line the reader was looking at ${moved}px`
  );
});

test("reaching the top loads the next step without a tap, and only one at a time", async () => {
  const page = newPage();
  await signIn(page);
  pagedChannel(page, { steps: 4, size: 4 });
  await showDiscord(page, []);
  const area = page.el("scroll-area");

  area.scrollTop = 0;
  await area.dispatch("scroll");
  // A phone fires a great many scroll events; a second step must not be started on top of the
  // first, or the same page arrives twice and the cursor jumps a step.
  await area.dispatch("scroll");
  await area.dispatch("scroll");
  await page.settle();

  assert.equal(page.el("discord-log").children.length, 8, "the automatic step ran more than once");
  assert.equal(page.pagesServed.length, 2, `${page.pagesServed.length} reads went out, not 2`);

  // ...and the same guard on the CONTROL, which the automatic path does not exercise: two taps
  // before the first has answered must still be one step, or the same page arrives twice and the
  // cursor skips one.
  const first = page.el("load-older").click();
  const second = page.el("load-older").click();
  await first;
  await second;
  await page.settle();
  assert.equal(page.pagesServed.length, 3, "a second tap started a second step on top of the first");
  assert.equal(page.el("discord-log").children.length, 12);
});

test("A SCROLL THAT IS NOT NEAR THE TOP LOADS NOTHING", async () => {
  // The other edge of the automatic trigger, and the one the "reaching the top" test above cannot
  // see: it sets scrollTop to exactly zero, so it stays green for ANY positive trigger, including
  // one wider than the whole list. At that width every scroll event anywhere — a flick at the
  // BOTTOM of the channel — takes another step back, and a reader who nudges the list once walks
  // the entire history without asking for any of it.
  const page = newPage();
  await signIn(page);
  pagedChannel(page, { steps: 4, size: 8, content: (i) => longMessage(`m${i}`) });
  await showDiscord(page, []);
  const area = page.el("scroll-area");

  const bottom = area.scrollHeight - area.clientHeight;
  assert.ok(
    bottom > sourceConstant("OLDER_TRIGGER_PX"),
    `the modelled channel is only ${bottom}px of scroll, which is inside the trigger — this test ` +
      "would pass on any page at all"
  );
  area.scrollTop = bottom;
  await area.dispatch("scroll");
  await page.settle();

  assert.equal(
    page.pagesServed.length,
    1,
    "a scroll at the BOTTOM of the list fetched older messages"
  );
  assert.equal(page.el("discord-log").children.length, 8, "the list grew without being asked");

  // ...and one step up from the bottom, still far from the top, is still not a request.
  area.scrollTop = Math.round(bottom / 2);
  await area.dispatch("scroll");
  await page.settle();
  assert.equal(
    page.pagesServed.length,
    1,
    "a scroll in the MIDDLE of the list fetched older messages"
  );
});

test("reaching the beginning of the channel stops offering more, and says the count", async () => {
  const page = newPage();
  await signIn(page);
  pagedChannel(page, { steps: 2, size: 3 });
  await showDiscord(page, []);
  assert.match(
    channelSummaryText(page),
    /older ones are not loaded/,
    "it claimed a total before the walk had reached the beginning"
  );

  await page.el("load-older").click();
  await page.settle();

  assert.equal(page.el("load-older").hidden, true, "it still offers a step past the beginning");
  // ...and NOW the number is the channel's own, because there is nothing above it. That is the
  // `#62 message-count-accuracy` rule, and this is the first time this page can satisfy it.
  assert.match(channelSummaryText(page), /^6 messages from lead team$/);
});

test("AND THE COUNT SURVIVES THE NEXT POLL, WHICH NOBODY ASKED FOR", async () => {
  // A truthful count that lasts forty-five seconds is not a truthful count. The poll re-reads the
  // NEWEST page, whose `has_more` is true — it is the newest page of a channel with more behind it
  // — and a summary written from that payload rather than from what is actually loaded goes back
  // to "older ones are not loaded" while #load-older stays hidden. The page then claims more
  // exists and offers no way to reach it, which is the two halves of the same answer disagreeing.
  const page = newPage();
  await signIn(page);
  pagedChannel(page, { steps: 2, size: 3 });
  await showDiscord(page, []);
  await page.el("load-older").click();
  await page.settle();
  assert.match(channelSummaryText(page), /^6 messages from lead team$/);

  assert.equal(page.expireTimers(DISCORD_POLL_MS), 1, "no poll was armed to test against");
  await page.settle();

  assert.equal(
    page.el("discord-log").children.length,
    6,
    "the poll discarded what the reader had walked back to"
  );
  assert.equal(page.el("load-older").hidden, true, "the poll re-offered a step past the beginning");
  assert.match(
    channelSummaryText(page),
    /^6 messages from lead team$/,
    `a poll nobody asked for rewrote the count to: ${channelSummaryText(page)}`
  );

  // ...and the Refresh button is the same re-read by hand, so it must not do it either.
  await reReadChannel(page);
  assert.match(channelSummaryText(page), /^6 messages from lead team$/);
  assert.equal(page.el("load-older").hidden, true);
});

test("a step that FAILS keeps what is already on screen, and does not hang up", async () => {
  const page = newPage();
  const socket = await startTalking(page);
  pagedChannel(page, { steps: 3, size: 4 });
  await showDiscord(page, []);
  page.channelPage = errorResponse(502, "discord_error", "discord returned HTTP 500");

  await page.el("load-older").click();
  await page.settle();

  assert.equal(page.el("discord-log").children.length, 4, "a failed step emptied the channel");
  assert.match(page.el("error").textContent, /502/, "the failure was swallowed");
  assert.equal(page.el("load-older").disabled, false, "the control is stuck reporting a step");
  assert.equal(page.tracks[0].stops, 0, "a failed step released the microphone");
  assert.notEqual(socket.readyState, 3, "a failed step hung up the call");
});

test("A REFRESH DOES NOT THROW AWAY WHAT THE READER WALKED BACK TO", async () => {
  // The interaction that makes paging and polling hard to have at once: a re-read of the NEWEST
  // page must not silently discard the older ones, or a background poll would delete the history
  // out from under somebody reading it every forty-five seconds.
  const page = newPage();
  await signIn(page);
  pagedChannel(page, { steps: 3, size: 4 });
  await showDiscord(page, []);
  await page.el("load-older").click();
  await page.settle();
  assert.equal(page.el("discord-log").children.length, 8);

  await reReadChannel(page);

  assert.equal(
    page.el("discord-log").children.length,
    8,
    "the refresh replaced eight loaded messages with the newest four"
  );
  assert.equal(
    page.el("discord-log").children[0].getAttribute("data-id"),
    "999",
    "the oldest row the reader had walked back to is not at the top any more"
  );
  // The ids span a LENGTH change, so this also fails on a page that compares snowflakes as plain
  // strings: `"999" < "1003"` is false lexicographically and true numerically.
  assert.deepStrictEqual(
    page.el("discord-log").children.map((li) => li.getAttribute("data-id")),
    ["999", "1000", "1001", "1002", "1003", "1004", "1005", "1006"]
  );
});

test("AND THE STEP AFTER A REFRESH GOES FURTHER BACK, NOT OVER THE SAME GROUND", async () => {
  // The half the test above stops one move short of. Keeping the rows is not enough: the CURSOR
  // has to survive the re-read as well, because the newest page hands back a cursor belonging to
  // ITSELF — `before=1003` — and adopting it points the walk at four messages that are already on
  // screen. The reader taps Older messages, waits, and gets four duplicated rows and no progress,
  // which is worse than the refresh having discarded them: the page looks like it is walking and
  // is not. A background poll does this every forty-five seconds, unasked.
  const page = newPage();
  await signIn(page);
  pagedChannel(page, { steps: 3, size: 4 });
  await showDiscord(page, []);
  await page.el("load-older").click();
  await page.settle();
  await reReadChannel(page);

  await page.el("load-older").click();
  await page.settle();

  assert.deepStrictEqual(
    page.el("discord-log").children.map((li) => li.getAttribute("data-id")),
    ["995", "996", "997", "998", "999", "1000", "1001", "1002", "1003", "1004", "1005", "1006"],
    "the step after a refresh re-read messages already on screen instead of walking further back"
  );
  assert.match(
    page.pagesServed[page.pagesServed.length - 1],
    /[?&]before=999(&|$)/,
    `the step after the refresh asked for ${page.pagesServed[page.pagesServed.length - 1]}, which ` +
      "is a cursor the refresh handed back rather than the one the walk had reached"
  );
});

test("a channel that is WHOLE replaces on a refresh rather than accumulating", async () => {
  // The other side of the keep rule. `has_more: false` means this page IS the channel, so anything
  // else on screen is stale — a deleted message, or the previous channel's. Keeping it would be
  // the "Refresh appended instead of replacing" defect wearing a paging costume.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "1", content: "one" })]);
  page.channelPage = async () =>
    json(200, { channel: CHANNEL, messages: [message({ id: "2", content: "two" })], has_more: false });

  await reReadChannel(page);

  const lines = page.el("discord-log").children;
  assert.equal(lines.length, 1, "a whole-channel read kept a message that no longer exists");
  assert.match(lines[0].text(), /two/);
});

test("switching channel starts the walk again rather than stepping back from a stranger", async () => {
  // A cursor is a message id in ONE channel. Carrying it across would ask the server to step back
  // from a message that is not there, which the route answers with an error the reader cannot act
  // on — and would be a confusing one, because they only changed channel.
  //
  // Three REAL channels since `#18 offline-message-cache`: going back to a channel this device has
  // saved rightly draws that channel's own rows and cursor, so a "change" to the same channel no
  // longer stands in for a change to a stranger.
  const page = newPage();
  const second = { id: "1110000000000000002", label: "second", writable: true };
  const third = { id: "1110000000000000003", label: "third", writable: true };
  page.channels = [{ ...CHANNEL }, second, third];
  await signIn(page);
  pagedChannel(page, { steps: 3, size: 4 });
  await showDiscord(page, []);
  await page.el("load-older").click();
  await page.settle();
  assert.equal(page.el("discord-log").children.length, 8);

  page.el("discord-channel").value = second.id;
  await page.el("discord-channel").dispatch("change");
  await page.settle();

  assert.equal(page.el("discord-log").children.length, 4, "the other channel's rows stayed");
  assert.doesNotMatch(
    page.pagesServed[page.pagesServed.length - 1],
    /before=/,
    "the new channel was read from a cursor belonging to the old one"
  );

  // ...and the reset holds even when the new channel's read FAILS. Leaving the old channel's
  // cursor armed is the dangerous case: a scroll to the top would then ask this server to step
  // back from a message that is not in this channel, and the reader only changed channel.
  page.channelPage = errorResponse(502, "discord_error", "nope");
  page.el("discord-channel").value = third.id;
  await page.el("discord-channel").dispatch("change");
  await page.settle();
  assert.equal(
    page.el("load-older").hidden,
    true,
    "after a failed channel change the page still offers to step back from the old channel"
  );
  // ...and the SUMMARY goes with the rows it counts. The list is emptied by the change and the
  // summary is only ever rewritten inside a read that succeeded, so leaving it alone means an
  // empty screen headed "6 messages from lead team" — a confident count of messages that are not
  // there, about a channel the reader has left.
  assert.equal(page.el("discord-log").children.length, 0, "a failed change left rows on screen");
  assert.deepStrictEqual(
    page.el("channel-summary").children.map((line) => line.text()),
    [],
    "a failed change of channel left the previous channel's count standing over an empty list"
  );
});

/** Two fingers on the glass, which is a pinch or a two-thumb scroll and never a pull. */
const twoFingers = (first, second) => ({
  touches: [
    { clientY: first, clientX: 0 },
    { clientY: second, clientX: 0 },
  ],
});

/** The channel view, walked back one step so there is history above the reader. */
async function walkedBackChannel(page) {
  pagedChannel(page, { steps: 4, size: 4, content: (i) => longMessage(`m${i}`) });
  await showDiscord(page, []);
  await page.el("load-older").click();
  await page.settle();
  assert.equal(page.el("discord-log").children.length, 8, "the walk back did not happen");
  return page.el("scroll-area");
}

test("A PULL PAST THE TOP REFRESHES, AND SAYS WHAT IT FOUND", async () => {
  const page = newPage();
  await signIn(page);
  const area = await walkedBackChannel(page);
  area.scrollTop = 0;
  const readsBefore = page.pagesServed.length;

  await pullDown(page, PULL_ARM_PX);

  assert.equal(page.pagesServed.length, readsBefore + 1, "the pull fetched nothing");
  // The NEWEST page, not another step backwards: a cursor here would be the gesture reading the
  // wrong end of the channel.
  assert.doesNotMatch(
    page.pagesServed[page.pagesServed.length - 1],
    /before=/,
    "the pull walked further BACK instead of fetching what is new"
  );
  // `keepPosition: false`. A pull happens at the top of the history, and keeping your place there
  // means staying at the oldest thing loaded — the opposite of what was asked for.
  assert.ok(atBottomOf(area), "the pull left the reader at the top instead of at the newest line");
  assert.equal(page.el("pull-refresh").hidden, true, "the affordance stayed on the screen");
  // A refresh that finds nothing must not be indistinguishable from a refresh that never
  // happened, which is the complaint the issue opens with.
  assert.match(page.el("status").textContent, /refreshed/, "the refresh said nothing at all");
  assert.match(page.el("status").textContent, /nothing new/);
});

test("...and says so differently when something HAS arrived", async () => {
  const page = newPage();
  await signIn(page);
  const all = pagedChannel(page, { steps: 3, size: 4 });
  await showDiscord(page, []);
  page.el("scroll-area").scrollTop = 0;
  // The channel gains a message while the reader is looking at it, exactly as it does in life.
  all.push(message({ id: "9995", content: "the arm64 runner came back by itself" }));

  await pullDown(page, PULL_ARM_PX);

  assert.match(page.el("status").textContent, /something new/, "a real arrival was not reported");
  assert.ok(
    page.el("discord-log").children.some((li) => li.getAttribute("data-id") === "9995"),
    "the message the pull fetched is not on the screen"
  );
});

test("THE AFFORDANCE ARMS BEFORE THE FINGER LIFTS, AND REPORTS THE FETCH IN FLIGHT", async () => {
  const page = newPage();
  await signIn(page);
  await walkedBackChannel(page);
  const area = page.el("scroll-area");
  area.scrollTop = 0;
  const affordance = page.el("pull-refresh");
  assert.equal(affordance.hidden, true, "the affordance stands on the screen at rest");

  // Short of the threshold: the gesture is recognised and says so, but a release would do nothing.
  await area.dispatch("touchstart", touchAt(120));
  await area.dispatch("touchmove", touchAt(120 + PULL_ARM_PX - 1));
  assert.equal(affordance.hidden, false, "a drag past the top said nothing at all");
  assert.equal(affordance.getAttribute("data-state"), "pull");
  assert.match(affordance.textContent, /Pull/);

  // Past it: armed, and it says so BEFORE the finger lifts — a gesture that only reports itself
  // afterwards cannot be abandoned.
  await area.dispatch("touchmove", touchAt(120 + PULL_ARM_PX));
  assert.equal(affordance.getAttribute("data-state"), "armed");
  assert.match(affordance.textContent, /Release/);

  // Changing your mind takes the offer away rather than leaving it standing: dragging back up
  // past where the finger landed abandons the gesture, and the affordance goes with it. Without
  // this the strip stays on the screen saying "Pull to refresh" through a scroll that is now
  // going the other way.
  await area.dispatch("touchmove", touchAt(110));
  assert.equal(affordance.hidden, true, "an abandoned pull left its offer on the screen");
  assert.equal(affordance.textContent, "", "the abandoned affordance kept its wording");
  // ...and the release then does nothing at all.
  const readsBefore = page.pagesServed.length;
  await area.dispatch("touchend");
  await page.settle();
  assert.equal(page.pagesServed.length, readsBefore, "an abandoned pull refreshed anyway");

  // Now the real one, and while the fetch is in flight. The read is HELD open here rather than
  // answered, because "there is a visible affordance while it is in flight" is a claim about the
  // window between the release and the answer, and a fixture that answers instantly has none.
  await area.dispatch("touchstart", touchAt(120));
  await area.dispatch("touchmove", touchAt(120 + PULL_ARM_PX));
  assert.equal(affordance.getAttribute("data-state"), "armed");

  let answer = null;
  page.channelPage = () => new Promise((resolve) => {
    answer = resolve;
  });
  const lifted = area.dispatch("touchend");
  await page.settle();
  assert.equal(affordance.hidden, false, "nothing on the screen says the refresh is happening");
  assert.equal(affordance.getAttribute("data-state"), "busy");
  assert.match(affordance.textContent, /Refreshing/);

  answer(json(200, { channel: CHANNEL, messages: page.messages }));
  await lifted;
  await page.settle();
  assert.equal(affordance.hidden, true, "the affordance was left saying the refresh is in flight");
});

test("A PULL THAT DOES NOT REACH THE THRESHOLD REFRESHES NOTHING", async () => {
  // The other edge of the same gesture. Without this, arming at zero pixels would pass every test
  // above — and every flick at the top of the channel would throw the reader to the newest line.
  const page = newPage();
  await signIn(page);
  const area = await walkedBackChannel(page);
  area.scrollTop = 0;
  const readsBefore = page.pagesServed.length;

  await pullDown(page, PULL_ARM_PX - 1);

  assert.equal(page.pagesServed.length, readsBefore, "a short drag refreshed the channel");
  assert.equal(page.el("pull-refresh").hidden, true, "the affordance was left on the screen");
  assert.equal(area.scrollTop, 0, "a short drag moved the reader");
});

test("A DRAG THAT STARTS INSIDE THE HISTORY IS A SCROLL, NOT A REFRESH", async () => {
  // The reading that keeps `#65 scrollback-paging` working: with content still above the viewport
  // the drag has somewhere to scroll to, so it is a scroll however far it travels.
  const page = newPage();
  await signIn(page);
  const area = await walkedBackChannel(page);
  area.scrollTop = Math.round((area.scrollHeight - area.clientHeight) * 0.5);
  const parked = area.scrollTop;
  const readsBefore = page.pagesServed.length;

  await pullDown(page, PULL_ARM_PX * 3);

  assert.equal(page.pagesServed.length, readsBefore, "a drag mid-history refreshed the channel");
  assert.equal(page.el("pull-refresh").hidden, true, "a drag mid-history offered to refresh");
  assert.equal(area.scrollTop, parked, "the reader was moved by a gesture that did nothing");

  // ...and it does not BECOME a pull by arriving at the top, which is the case that decides where
  // the reading has to be taken. A flick started a little below the top runs the list out within
  // a frame — so the FIRST touchmove the page sees already reports a scroll position of zero. If
  // the gesture were judged from where the finger IS rather than from where it LANDED, that
  // ordinary flick would refresh the channel and throw the reader to the newest line for it.
  area.scrollTop = 30;
  await area.dispatch("touchstart", touchAt(120));
  area.scrollTop = 0; // the flick has already run the list out, as a real one does
  await area.dispatch("touchmove", touchAt(120 + PULL_ARM_PX * 2));
  await area.dispatch("touchend");
  await page.settle();

  assert.equal(
    page.pagesServed.length,
    readsBefore,
    "a scroll that merely REACHED the top turned itself into a refresh"
  );
  assert.equal(page.el("pull-refresh").hidden, true, "the affordance appeared during a scroll");
});

test("THE PULL AND THE STEP BACK ARE THE SAME CONTAINER'S TWO ENDS, AND DO NOT FIGHT", async () => {
  // Both directions of the contention, because either one alone is satisfiable by switching the
  // other feature off.
  const page = newPage();
  await signIn(page);
  const area = await walkedBackChannel(page);
  area.scrollTop = 0;

  // ONE: a scroll event arriving under a finger that is pulling must not take a step BACK. The
  // reader is asking for the newest; growing the history they are trying to leave, mid-gesture,
  // is the fight this design exists to avoid.
  await pullDown(page, PULL_ARM_PX, { lift: false });
  const readsBefore = page.pagesServed.length;
  await area.dispatch("scroll");
  await page.settle();
  assert.equal(page.pagesServed.length, readsBefore, "the pull was answered with older messages");

  // ...and the step back is only STOOD ASIDE, not switched off: the finger lifts, and the very
  // next scroll at the top takes it.
  await area.dispatch("touchcancel");
  await area.dispatch("scroll");
  await page.settle();
  assert.equal(
    page.pagesServed.length,
    readsBefore + 1,
    "the automatic step back never came back after a pull"
  );
  assert.match(page.pagesServed[page.pagesServed.length - 1], /before=/);

  // TWO: the other direction. A step that is already in flight owns the top of the list, so a
  // finger landing on it does not arm a pull — the answer to that fetch is about to prepend
  // content above the viewport, and a refresh would throw the reader away from it.
  let answer = null;
  page.channelPage = (path) => {
    page.pagesServed.push(String(path));
    return new Promise((resolve) => {
      answer = resolve;
    });
  };
  area.scrollTop = 0;
  await area.dispatch("scroll");
  await page.settle();
  const during = page.pagesServed.length;
  assert.ok(answer, "no older step is in flight, so this half of the test proves nothing");

  // The release is deliberately NOT awaited here. On a page that armed this pull it would park on
  // a fetch nothing is ever going to answer, and a hanging test reports as a hanging suite rather
  // than as the defect it is; the assertions below are what has to notice.
  await pullDown(page, PULL_ARM_PX * 2, { lift: false });
  const lifted = page.el("scroll-area").dispatch("touchend");
  await page.settle();

  assert.equal(page.pagesServed.length, during, "a pull started on top of a step already in flight");
  assert.equal(page.el("pull-refresh").hidden, true, "the affordance armed during a step back");
  answer(json(200, { channel: CHANNEL, messages: [], has_more: false, next_before: null }));
  await lifted;
  await page.settle();
});

/**
 * A channel with MORE HISTORY ABOVE, arrived at the way a browser really arrives.
 *
 * Every earlier test in this section reaches the pullable state by ASSIGNING `scrollTop = 0` and
 * never dispatching the `scroll` event that a browser guarantees — so the suite was green about a
 * state a reader on a paged channel could not be in. This drags the list to its top with a finger
 * down and lets the page hear the scroll, which is the case `#68` is actually for: the channel is
 * hours out of date AND it has a history.
 */
async function draggedToTopOf(page, y) {
  const area = page.el("scroll-area");
  area.scrollTop = Math.round((area.scrollHeight - area.clientHeight) * 0.4);
  assert.ok(
    area.scrollTop > sourceConstant("OLDER_TRIGGER_PX"),
    "the fixture's channel is too short to drag through"
  );
  await area.dispatch("touchstart", touchAt(y));
  // The browser scrolls the content under the finger and reports it. THIS is the event `#65
  // scrollback-paging` listens on, and it arrives while the finger is still on the glass.
  area.scrollTop = 0;
  await area.dispatch("scroll");
  await page.settle();
  return area;
}

test("A CHANNEL THAT STILL HAS HISTORY CAN BE PULLED — THE CASE THE ISSUE IS ABOUT", async () => {
  // The defect this replaces: arriving at the top of a paged channel fired the automatic step
  // back, `preservingScroll` restored the reader to a positive offset, and so `scrollTop === 0`
  // at touchstart — the old precondition for a pull — was a state that could not occur until the
  // whole channel had been walked back. The gesture existed only on channels that did not need it.
  const page = newPage();
  await signIn(page);
  pagedChannel(page, { steps: 4, size: 4, content: (i) => longMessage(`m${i}`) });
  await showDiscord(page, []);
  const log = page.el("discord-log");
  assert.equal(log.children.length, 4, "the channel did not load its newest page");
  assert.equal(page.el("load-older").hidden, false, "this channel has no history, so it is not the case");

  const area = await draggedToTopOf(page, 300);

  // The step back stood aside while the finger was down — which is what leaves the reader AT the
  // top, where an overscroll is possible at all.
  assert.equal(log.children.length, 4, "the step back fired under the finger and moved the ground");
  assert.equal(area.scrollTop, 0, "the reader was pushed off the top mid-drag");
  const readsBefore = page.pagesServed.length;

  // The list has run out under the finger, so from here on the drag is an overscroll: the first
  // move anchors the edge, and the travel after it is the pull.
  await area.dispatch("touchmove", touchAt(320));
  await area.dispatch("touchmove", touchAt(320 + PULL_ARM_PX));
  assert.equal(
    page.el("pull-refresh").getAttribute("data-state"),
    "armed",
    "a pull on a channel with history above it cannot be armed at all"
  );

  await area.dispatch("touchend");
  await page.settle();

  assert.equal(page.pagesServed.length, readsBefore + 1, "the pull fetched nothing");
  assert.doesNotMatch(
    page.pagesServed[page.pagesServed.length - 1],
    /before=/,
    "the pull was answered with OLDER messages instead of with what is new"
  );
  assert.match(page.el("status").textContent, /refreshed/, "the refresh said nothing at all");
  assert.ok(atBottomOf(area), "the pull left the reader at the top instead of at the newest line");
  // ...and the channel still has a history, so this really was the paged case and not the
  // walked-to-the-end one the old gesture needed.
  assert.equal(page.el("load-older").hidden, false, "the channel ran out of history on the way");
});

test("THE STEP BACK IS SUSPENDED UNDER A FINGER, NOT SWITCHED OFF", async () => {
  // The other half of the same rule, and the one that keeps `#65 scrollback-paging` automatic. A
  // suspension that forgot the step would make the walk back need a second gesture to re-announce
  // itself — the reader is already at the top and cannot scroll further to produce one.
  const page = newPage();
  await signIn(page);
  pagedChannel(page, { steps: 4, size: 4, content: (i) => longMessage(`m${i}`) });
  await showDiscord(page, []);
  const log = page.el("discord-log");

  const area = await draggedToTopOf(page, 300);
  assert.equal(log.children.length, 4, "the step back fired under the finger");

  // The finger lifts without pulling: no overscroll, so this was a scroll to the top, and the
  // step the reader asked for by getting there is taken now.
  await area.dispatch("touchend");
  await page.settle();

  assert.equal(log.children.length, 8, "lifting the finger lost the step back entirely");
  assert.match(page.pagesServed[page.pagesServed.length - 1], /before=/, "that was not a step back");
  assert.equal(page.el("pull-refresh").hidden, true, "a scroll to the top offered to refresh");
  // ...and the anchored prepend put the reader back on the line they were reading, which is why
  // the top is not where they are afterwards. `#47 scrollback-stability`.
  assert.ok(area.scrollTop > 0, "the reader was left at the top with four new pages above them");

  // A browser cancelling the gesture is the same lift as far as the owed step is concerned.
  const again = newPage();
  await signIn(again);
  pagedChannel(again, { steps: 4, size: 4, content: (i) => longMessage(`m${i}`) });
  await showDiscord(again, []);
  const cancelled = await draggedToTopOf(again, 300);
  assert.equal(again.el("discord-log").children.length, 4);
  await cancelled.dispatch("touchcancel");
  await again.settle();
  assert.equal(
    again.el("discord-log").children.length,
    8,
    "a cancelled touch swallowed the step back the reader had already asked for"
  );
});

test("A DRAG ACROSS THE LIST IS NOT A PULL, HOWEVER FAR IT DRIFTS DOWN", async () => {
  // A thumb swiping sideways at the edge of the screen — the platform's own back gesture — carries
  // tens of pixels of downward drift, and a gesture that read only clientY armed a refresh on it.
  const page = newPage();
  await signIn(page);
  const area = await walkedBackChannel(page);
  area.scrollTop = 0;
  const readsBefore = page.pagesServed.length;

  await area.dispatch("touchstart", touchAt(300, 20));
  await area.dispatch("touchmove", touchAt(300 + PULL_ARM_PX * 2, 20 + PULL_ARM_PX * 4));

  assert.equal(page.el("pull-refresh").hidden, true, "a sideways swipe offered to refresh");
  // ...and it stays refused for the rest of the touch. A drag that arcs back to vertical at the
  // far end of its travel must not arm on the way: the reader is swiping across, not down.
  await area.dispatch("touchmove", touchAt(300 + PULL_ARM_PX * 3, 20 + PULL_ARM_PX * 4));
  assert.equal(page.el("pull-refresh").hidden, true, "a sideways swipe armed by curving downward");

  await area.dispatch("touchend");
  await page.settle();
  assert.equal(page.pagesServed.length, readsBefore, "a sideways swipe refreshed the channel");
});

test("A SECOND FINGER DOES NOT RE-ANCHOR THE TRAVEL", async () => {
  // `touches[0]` changes meaning when a second finger lands, and rebuilding the gesture from it
  // restarts the measurement from wherever finger one has got to — so a drag that had travelled
  // almost far enough could arm on a pinch, from halfway.
  const page = newPage();
  await signIn(page);
  const area = await walkedBackChannel(page);
  area.scrollTop = 0;
  const readsBefore = page.pagesServed.length;

  await area.dispatch("touchstart", touchAt(300));
  await area.dispatch("touchmove", touchAt(300 + PULL_ARM_PX - 1));
  assert.equal(page.el("pull-refresh").getAttribute("data-state"), "pull", "the drag is not recognised");

  await area.dispatch("touchstart", twoFingers(300 + PULL_ARM_PX - 1, 520));
  assert.equal(page.el("pull-refresh").hidden, true, "a second finger left the offer standing");

  await area.dispatch("touchmove", twoFingers(300 + PULL_ARM_PX * 2, 520));
  assert.equal(page.el("pull-refresh").hidden, true, "a two-finger gesture armed a refresh");

  await area.dispatch("touchend");
  await page.settle();
  assert.equal(page.pagesServed.length, readsBefore, "a pinch refreshed the channel");
});

test("NOTHING LIVES ABOVE THE MESSAGES, and the poll and the pull are what re-read", async () => {
  // `#55 voice-desktop-app` kept a Refresh button on the reasoning that a desktop has no pull
  // gesture, and that the two acts differ: Refresh keeps your place, the pull asks for what is new.
  // Both halves were true and the placement was still wrong. The button lived INSIDE #scroll-area,
  // above the message list, so on a long channel it sat behind an unbounded scrollback — and
  // `#65 scrollback-paging` made scrolling toward it PREPEND more history, so it receded as you
  // approached. `#83 channel-selector-in-bar` had already found exactly this for the channel
  // picker and moved that out; Refresh and the archive filter were left behind.
  //
  // WHAT REPLACES IT. The channel re-reads itself every DISCORD_POLL_MS without being asked, and
  // that re-read still keeps the reader's place — the property the button was credited with. The
  // pull is there for "now, please". The archive filter, which changes what the list IS, moved to
  // the control bar rather than being deleted.
  assert.equal(PAGE_IDS.has("refresh-discord"), false, "a control still sits above the messages");
  assert.equal(
    markupHolds("scroll-area", "todo-filter"),
    false,
    "the archive filter is still stranded above an unbounded scrollback"
  );
  assert.ok(PAGE_IDS.has("todo-filter"), "the archive filter was deleted rather than moved");

  const page = newPage();
  await signIn(page);
  const area = await walkedBackChannel(page);

  area.scrollTop = Math.round((area.scrollHeight - area.clientHeight) * 0.5);
  const parked = area.scrollTop;
  await reReadChannel(page);
  assert.equal(area.scrollTop, parked, "the poll stopped keeping the reader's place");

  area.scrollTop = 0;
  await pullDown(page, PULL_ARM_PX);
  assert.ok(atBottomOf(area), "the gesture stopped taking the reader to the newest message");
});

test("the pull affordance is a sibling of the scroll area, not a passenger inside it", () => {
  // Same argument as the chips and the width handle: #scroll-area carries the fade-under-the-header
  // mask, and this thing appears at exactly the edge where that mask is strongest — inside, it
  // would announce itself by dissolving, and it would scroll away from the gesture producing it.
  assert.equal(
    markupHolds("scroll-area", "pull-refresh"),
    false,
    "the affordance is inside the scrolling element, where the header mask fades it out"
  );
  assert.equal(PAGE_ELEMENTS.get("pull-refresh").hidden, true, "it ships standing on the screen");
  assert.equal(PAGE_ELEMENTS.get("pull-refresh").text, "", "it ships carrying a state's wording");
  const rule = cssBlock("#pull-refresh");
  assert.match(rule, /position:\s*absolute/, "it takes a row of the page instead of floating");
  // A report, never a control: the finger that summoned it is mid-drag on the list underneath.
  assert.match(rule, /pointer-events:\s*none/, "the affordance can swallow the gesture under it");
  assert.match(
    cssBlock('#pull-refresh[data-state="armed"]'),
    /var\(--accent\)/,
    "armed looks exactly like not-yet-armed, so there is nothing to release on"
  );
  // The gesture depends on the browser NOT taking the overscroll for its own page refresh.
  assert.match(cssBlock("#scroll-area"), /overscroll-behavior:\s*contain/);
});

// --- pulling UP past the newest message ---------------------------------------------------------
//
// `#188 pull-refresh-bottom`. The owner reads at the newest end of the channel, and "I want pull UP
// to trigger refresh even IF messages were loaded in the past." The top's pull answered only a
// reader at the top, which on an empty list is everyone — so it seemed to work until the channel
// had loaded, and then the reader was a whole history away from it. Bouncing against the bottom did
// nothing and showed nothing.
//
// The same gesture at the other end, so the tests below check what differs: it must BEGIN with the
// reader parked at the newest line, so a fling that merely reaches the end is a scroll; it keeps the
// reader's place; it waits for a read already in flight instead of adding one; and it says what it
// found at the foot of the list, where the reader is looking, before it goes.

/** How long a pull up leaves its result on the screen. Derived, never restated. */
const PULL_RESULT_MS = sourceConstant("PULL_RESULT_MS");

/** How far the affordance follows the finger, however far the finger goes. */
const PULL_RISE_PX = sourceConstant("PULL_RISE_PX");

/**
 * One pull UP on the channel list: land at `from`, drag `travel` pixels toward the top of the
 * screen, and lift — the drag that, at the newest line, runs past the end of the list.
 */
async function pullUp(page, travel, { lift = true, from = 300, target = undefined } = {}) {
  const area = page.el("scroll-area");
  await area.dispatch("touchstart", { ...touchAt(from), target });
  await area.dispatch("touchmove", touchAt(from - travel));
  if (lift) {
    await area.dispatch("touchend");
    await page.settle();
  }
}

/**
 * A channel long enough to scroll, read, and left where the owner reads it: at the newest line.
 * Returns the channel's whole history too, so a test can make something arrive.
 */
async function parkedAtNewest(page) {
  const all = pagedChannel(page, { steps: 2, size: 12, content: (i) => longMessage(`m${i}`) });
  await showDiscord(page, []);
  const area = page.el("scroll-area");
  assert.ok(area.scrollHeight > area.clientHeight * 2, "the channel does not overflow");
  area.scrollTop = area.scrollHeight;
  assert.ok(atBottomOf(area), "the reader is not at the newest line");
  return { area, all };
}

/** The pixels the affordance has risen by, as `renderPull` wrote them. */
const riseOf = (page) => Number.parseInt(page.el("pull-refresh").style.getPropertyValue("--pull-rise"), 10);

test("A PULL UP PAST THE NEWEST MESSAGE REFRESHES, WITH THE CHANNEL ALREADY LOADED", async () => {
  // The case the owner reported, exactly: messages loaded in the past, the reader at the bottom.
  const page = newPage();
  await signIn(page);
  const { area, all } = await parkedAtNewest(page);
  const readsBefore = page.pageReads;
  all.push(message({ id: "9995", content: "the arm64 runner came back by itself" }));

  await pullUp(page, PULL_ARM_PX);

  assert.equal(page.pageReads, readsBefore + 1, "a pull up past the newest message fetched nothing");
  assert.doesNotMatch(
    page.pagesServed[page.pagesServed.length - 1],
    /before=/,
    "the pull up walked BACK instead of fetching what is new"
  );
  assert.ok(shownIds(page).includes("9995"), "the message the pull up fetched is not on the screen");
  // The reader was at the newest line and is at the new newest line: keeping their place, there,
  // IS following the newest.
  assert.ok(atBottomOf(area), "the refresh left the reader above the message it fetched");

  // ...and it says so where the reader is looking, then goes.
  const affordance = page.el("pull-refresh");
  assert.equal(affordance.hidden, false, "a pull up refreshed and said nothing at the foot of the list");
  assert.equal(affordance.getAttribute("data-edge"), "end", "the result was reported at the top");
  assert.equal(affordance.getAttribute("data-state"), "done");
  assert.match(affordance.textContent, /^Updated \d{2}:\d{2} · new messages$/);
  assert.equal(page.expireTimers(PULL_RESULT_MS), 1, "nothing was armed to take the result away");
  assert.equal(affordance.hidden, true, "the result stayed over the newest line");
  assert.equal(affordance.textContent, "", "the collapsed affordance kept its wording");
});

test("...and a pull up that finds nothing new says THAT, rather than looking like no refresh", async () => {
  const page = newPage();
  await signIn(page);
  await parkedAtNewest(page);
  const readsBefore = page.pageReads;

  await pullUp(page, PULL_ARM_PX);

  assert.equal(page.pageReads, readsBefore + 1, "the pull up fetched nothing");
  assert.match(page.el("pull-refresh").textContent, /^Updated \d{2}:\d{2} · nothing new$/);
});

test("THE FOOT OF THE LIST SAYS PULL, THEN RELEASE, THEN REFRESHING — AND FOLLOWS THE FINGER", async () => {
  const page = newPage();
  await signIn(page);
  const { area } = await parkedAtNewest(page);
  const affordance = page.el("pull-refresh");
  assert.equal(affordance.hidden, true, "the affordance stands on the screen at rest");

  await area.dispatch("touchstart", touchAt(300));
  await area.dispatch("touchmove", touchAt(300 - (PULL_ARM_PX - 1)));
  assert.equal(affordance.hidden, false, "a drag past the newest message said nothing at all");
  assert.equal(affordance.getAttribute("data-edge"), "end", "a pull up was drawn at the top edge");
  assert.equal(affordance.getAttribute("data-state"), "pull");
  assert.match(affordance.textContent, /Pull/);
  // The freshness pill is a screen away from the foot of the list, so — unlike a pull at the top,
  // which takes its place — this one leaves it up: "Offline · …" is the reason to pull at all.
  assert.equal(page.el("channel-freshness").hidden, false, "a pull up hid how current the channel is");
  const short = riseOf(page);
  assert.ok(short > 0 && short < PULL_ARM_PX - 1, `the affordance rose ${short}px for a ${PULL_ARM_PX - 1}px pull`);

  await area.dispatch("touchmove", touchAt(300 - PULL_ARM_PX));
  assert.equal(affordance.getAttribute("data-state"), "armed");
  assert.match(affordance.textContent, /Release/);
  const armedAt = riseOf(page);

  // RESISTED: twice the travel is not twice the rise, and no pull climbs past PULL_RISE_PX.
  await area.dispatch("touchmove", touchAt(300 - PULL_ARM_PX * 4));
  const far = riseOf(page);
  assert.ok(far > armedAt, "the affordance stopped following the finger");
  assert.ok(far - armedAt < PULL_ARM_PX * 3, "the affordance moved as far as the finger did");
  assert.ok(far < PULL_RISE_PX, `the affordance climbed ${far}px, past its ${PULL_RISE_PX}px reach`);

  // Changing your mind takes the offer away: back down past where the pull began.
  await area.dispatch("touchmove", touchAt(310));
  assert.equal(affordance.hidden, true, "an abandoned pull up left its offer on the screen");
  const readsBefore = page.pageReads;
  await area.dispatch("touchend");
  await page.settle();
  assert.equal(page.pageReads, readsBefore, "an abandoned pull up refreshed anyway");

  // Now the real one, held in flight: "it shows the refresh happening" is a claim about the window
  // between the release and the answer, and a fixture that answers at once has none.
  await area.dispatch("touchstart", touchAt(300));
  await area.dispatch("touchmove", touchAt(300 - PULL_ARM_PX));
  const reads = holdChannelReads(page);
  const lifted = area.dispatch("touchend");
  await page.settle();
  assert.equal(affordance.hidden, false, "nothing at the foot of the list says the refresh is happening");
  assert.equal(affordance.getAttribute("data-state"), "busy");
  assert.equal(affordance.getAttribute("data-edge"), "end");
  assert.match(affordance.textContent, /Refreshing/);
  // A tap while it is in flight is not a pull, and must not take the report down with it.
  await area.dispatch("touchstart", touchAt(200));
  await area.dispatch("touchend");
  assert.equal(affordance.getAttribute("data-state"), "busy", "a tap took down the refresh in flight");

  reads.release();
  await lifted;
  await page.settle();
  assert.equal(affordance.getAttribute("data-state"), "done", "the refresh never said it had finished");
  assert.match(affordance.textContent, /^Updated \d{2}:\d{2}/);
});

test("A PULL UP THAT FAILS SAYS SO AT THE FOOT, AND IN THE ERROR PANEL, AND KEEPS THE ROWS", async () => {
  // The owner's other half: "especially while the page says it is offline". A pull that fails has
  // to look different from one that worked, at the place the reader is looking.
  const page = newPage();
  await signIn(page);
  await parkedAtNewest(page);
  const rows = shownIds(page);
  const serve = page.channelPage;
  page.channelPage = errorResponse(502, "discord_error", "the provider is down");

  await pullUp(page, PULL_ARM_PX);

  const affordance = page.el("pull-refresh");
  assert.equal(affordance.getAttribute("data-edge"), "end");
  assert.equal(affordance.getAttribute("data-state"), "failed", "a failed pull up reported success");
  assert.equal(affordance.textContent, "Refresh failed");
  assert.deepStrictEqual(shownIds(page), rows, "a failed refresh took the rows away");
  assert.match(page.el("channel-freshness").textContent, /^Refresh failed · showing messages from/);
  assert.match(page.el("error").textContent, /provider is down/, "the failure is reported nowhere lasting");
  assert.equal(page.expireTimers(PULL_RESULT_MS), 1);
  assert.equal(affordance.hidden, true, "the failure stayed over the newest line");

  // OFFLINE is a failure too, and the next pull — with the network back — recovers in place.
  page.channelPage = async () => {
    throw new TypeError("Failed to fetch");
  };
  await pullUp(page, PULL_ARM_PX);
  assert.equal(affordance.getAttribute("data-state"), "failed", "an offline pull up reported success");
  assert.match(page.el("channel-freshness").textContent, /^Offline · showing messages saved/);
  page.channelPage = serve;
  await pullUp(page, PULL_ARM_PX);
  assert.equal(affordance.getAttribute("data-state"), "done", "the pull up did not recover once online");
  assert.match(page.el("channel-freshness").textContent, /^(Live · u|U)pdated \d{2}:\d{2}$/);
});

test("A PULL UP THAT DOES NOT REACH THE THRESHOLD REFRESHES NOTHING", async () => {
  const page = newPage();
  await signIn(page);
  const { area } = await parkedAtNewest(page);
  const parked = area.scrollTop;
  const readsBefore = page.pageReads;

  await pullUp(page, PULL_ARM_PX - 1);

  assert.equal(page.pageReads, readsBefore, "a short drag at the newest line refreshed the channel");
  assert.equal(page.el("pull-refresh").hidden, true, "the affordance was left on the screen");
  assert.equal(area.scrollTop, parked, "a short drag moved the reader");
});

test("ONLY A READER PARKED AT THE NEWEST LINE PULLS UP — A FLING THAT REACHES IT IS A SCROLL", async () => {
  const page = newPage();
  await signIn(page);
  const { area } = await parkedAtNewest(page);
  const furthest = area.scrollHeight - area.clientHeight;
  const readsBefore = page.pageReads;

  // In the middle of the history the drag has somewhere to scroll to, however far it goes.
  area.scrollTop = Math.round(furthest * 0.5);
  await pullUp(page, PULL_ARM_PX * 3);
  assert.equal(page.pageReads, readsBefore, "a drag mid-history refreshed the channel");
  assert.equal(page.el("pull-refresh").hidden, true, "a drag mid-history offered to refresh");

  // A FLING that runs the list out: the finger landed in the history, the list reached its end on
  // the way, and the finger kept going. At the top that has to count (rule 2 — paging makes the
  // top unreachable otherwise), but nothing pages in at the bottom, so here it is a scroll. Only a
  // reader who stopped at the newest line and pulled again is asking.
  area.scrollTop = Math.round(furthest * 0.5);
  await area.dispatch("touchstart", touchAt(500));
  area.scrollTop = area.scrollHeight; // the drag has run the list out, as a fast one does
  await area.dispatch("touchmove", touchAt(400));
  // ...and the finger keeps going, well past the threshold, measured from where the list ran out.
  await area.dispatch("touchmove", touchAt(400 - PULL_ARM_PX * 2));
  assert.equal(page.el("pull-refresh").hidden, true, "a fling that reached the newest line offered to refresh");
  await area.dispatch("touchend");
  await page.settle();
  assert.equal(page.pageReads, readsBefore, "a fling that merely reached the newest line refreshed");

  // Parked a little short of the end still counts as parked — `atBottom`'s slack — but only the
  // travel past the REAL end is pull. The pixels before it scrolled the list.
  const shortBy = Math.floor(BOTTOM_SLACK_PX / 2);
  area.scrollTop = furthest - shortBy;
  assert.ok(atBottomOf(area) && area.scrollTop < furthest, "the fixture is not parked short of the end");
  await area.dispatch("touchstart", touchAt(500));
  await area.dispatch("touchmove", touchAt(500 - shortBy)); // scrolling the last few pixels
  assert.equal(page.el("pull-refresh").hidden, true, "the last of the scroll was taken for a pull");
  area.scrollTop = furthest;
  await area.dispatch("touchmove", touchAt(500 - shortBy - 1)); // ...and the list has run out here
  await area.dispatch("touchmove", touchAt(500 - shortBy - 1 - (PULL_ARM_PX - 1)));
  assert.equal(
    page.el("pull-refresh").getAttribute("data-state"),
    "pull",
    "the travel spent scrolling to the end was counted toward the pull"
  );
  await area.dispatch("touchmove", touchAt(500 - shortBy - 1 - PULL_ARM_PX));
  assert.equal(page.el("pull-refresh").getAttribute("data-state"), "armed");
  await area.dispatch("touchend");
  await page.settle();
  assert.equal(page.pageReads, readsBefore + 1, "a reader parked a little short of the end could not pull");
});

test("A READ ALREADY IN FLIGHT IS THE REFRESH — A PULL UP NEVER PUTS A SECOND ONE ON THE WIRE", async () => {
  const page = newPage();
  await signIn(page);
  const { area } = await parkedAtNewest(page);

  const affordance = page.el("pull-refresh");
  // ONE: the poll is already reading when the finger lands. That read is answering the question,
  // so no second one goes out — but the pull is still OFFERED, and its release still says it is
  // refreshing and then what it found. On a stalled connection this is the state the page sits in
  // for as long as the stall lasts, and a pull that showed nothing in it would be the owner's
  // "I don't get a refresh that shows in the UI" all over again.
  let reads = holdChannelReads(page);
  assert.ok(page.expireTimers(DISCORD_POLL_MS) > 0, "no channel poll was armed");
  await page.settle();
  const during = page.pageReads;
  assert.equal(reads.held.length, 1, "the poll did not start a read, so this proves nothing");
  // The poll's "Loading messages…" line arrived in flow ABOVE the log. A browser's own scroll
  // anchoring keeps a reader at the end through that; this fixture does not model it, so it is
  // done here — otherwise the list would no longer be at its end and this would not be a pull.
  area.scrollTop = area.scrollHeight;
  // Time passes, so that a result stamped with the last read's time is told from this one's.
  page.setClock(page.clock() + 7 * 60 * 1000);
  await area.dispatch("touchstart", touchAt(300));
  await area.dispatch("touchmove", touchAt(300 - (PULL_ARM_PX - 1)));
  assert.equal(affordance.hidden, false, "a pull up over a read in flight showed nothing at all");
  assert.equal(affordance.getAttribute("data-state"), "pull");
  assert.equal(affordance.getAttribute("data-edge"), "end");
  await area.dispatch("touchmove", touchAt(300 - PULL_ARM_PX * 2));
  assert.equal(affordance.getAttribute("data-state"), "armed", "a pull up over a read never armed");
  const lifted = area.dispatch("touchend");
  await page.settle();
  assert.equal(affordance.getAttribute("data-state"), "busy", "the release said nothing was happening");
  assert.match(affordance.textContent, /Refreshing/);
  assert.equal(page.pageReads, during, "a pull up started a second read beside the poll's");
  reads.release();
  await page.settle();
  assert.ok(page.expireTimers(PULL_WAIT_MS) > 0, "the release was not waiting on the read in flight");
  await lifted;
  await page.settle();
  assert.equal(page.el("channel-loading").hidden, true, "the poll's read never finished");
  assert.equal(page.pageReads, during, "a read went on the wire after the one the release waited for");
  assert.equal(affordance.getAttribute("data-state"), "done", "the read it waited on was not reported");
  // ...stamped with when THAT read landed, which is when the freshness pill says too.
  const stamped = /^Updated (\d{2}:\d{2}) · nothing new$/.exec(affordance.textContent);
  assert.ok(stamped, `the foot said ${JSON.stringify(affordance.textContent)}`);
  assert.match(page.el("channel-freshness").textContent, new RegExp(`pdated ${stamped[1]}$`));
  assert.equal(page.expireTimers(PULL_RESULT_MS), 1);

  // The TOP is still not offered over a read in flight: its release asks for a read directly, which
  // could only queue behind this one and return at once, reporting a refresh that had not happened.
  reads = holdChannelReads(page);
  assert.ok(page.expireTimers(DISCORD_POLL_MS) > 0, "no channel poll was armed");
  await page.settle();
  const polling = page.pageReads;
  assert.equal(reads.held.length, 1, "the poll did not start a read, so this proves nothing");
  area.scrollTop = 0;
  await pullDown(page, PULL_ARM_PX * 2, { lift: false });
  assert.equal(affordance.hidden, true, "a pull at the top armed on top of a read in flight");
  const topLifted = area.dispatch("touchend");
  await page.settle();
  assert.equal(page.pageReads, polling, "a pull at the top started a second read beside the poll's");
  reads.release();
  await topLifted;
  await page.settle();
  area.scrollTop = area.scrollHeight;

  // TWO: the poll starts AFTER the finger lands. The release waits for that read and reports it.
  await area.dispatch("touchstart", touchAt(300));
  reads = holdChannelReads(page);
  assert.ok(page.expireTimers(DISCORD_POLL_MS) > 0, "no channel poll was armed");
  await page.settle();
  const polled = page.pageReads;
  // The poll's "Loading messages…" line arrives in flow ABOVE the log. A browser's own scroll
  // anchoring keeps a reader at the end through that; this fixture does not model it, so it is
  // done here — otherwise the list would no longer be at its end and this would not be a pull.
  area.scrollTop = area.scrollHeight;
  await area.dispatch("touchmove", touchAt(300 - PULL_ARM_PX));
  assert.equal(page.el("pull-refresh").getAttribute("data-state"), "armed");
  const released = area.dispatch("touchend");
  await page.settle();
  assert.equal(page.el("pull-refresh").getAttribute("data-state"), "busy", "the release did not wait");
  assert.equal(page.pageReads, polled, "the release queued a second read behind the poll's");

  reads.release();
  await page.settle();
  assert.ok(page.expireTimers(PULL_WAIT_MS) > 0, "the release was not waiting on the read in flight");
  await released;
  await page.settle();
  assert.equal(page.pageReads, polled, "a read went on the wire after the one the release waited for");
  assert.equal(page.el("pull-refresh").getAttribute("data-state"), "done", "the waited-on read was not reported");
  assert.match(page.el("pull-refresh").textContent, /^Updated \d{2}:\d{2}/);
});

test("IN A THREAD, A PULL UP REFRESHES THAT THREAD — AND A SHORT LIST IS PULLED BY DIRECTION", async () => {
  // A thread of two messages cannot scroll, so the list is at its top AND its newest line at once.
  // There the direction decides, and this checks the half `#68` could never reach: UP.
  const page = await threadPage();
  const picker = page.el("thread-select");
  picker.value = `thread:${page.threads[0].id}`;
  await picker.dispatch("change");
  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["201", "202"], "the thread did not open");
  const area = page.el("scroll-area");
  assert.ok(area.scrollTop === 0 && atBottomOf(area), "the thread scrolls, so it is not the short case");
  const reads = page.timelineCalls.length;
  page.messages.push(message({
    id: "205",
    content: "a second answer in the thread",
    thread: { ...page.messages[2].thread },
  }));

  const affordance = page.el("pull-refresh");
  await area.dispatch("touchstart", touchAt(300));
  await area.dispatch("touchmove", touchAt(300 + PULL_ARM_PX));
  assert.equal(affordance.getAttribute("data-edge"), "top", "a pull DOWN was taken for a pull up");
  assert.equal(affordance.getAttribute("data-state"), "armed");
  assert.equal(page.el("channel-freshness").hidden, true, "two pills stacked at the top");
  // Straight back up past where the finger landed, without lifting: now it is the foot's pull.
  await area.dispatch("touchmove", touchAt(300 - PULL_ARM_PX));
  assert.equal(affordance.getAttribute("data-edge"), "end", "a pull UP was taken for a pull down");
  assert.equal(affordance.getAttribute("data-state"), "armed");
  assert.equal(page.el("channel-freshness").hidden, false, "the freshness pill stayed hidden");
  await area.dispatch("touchend");
  await page.settle();

  assert.equal(page.timelineCalls.length, reads + 1, "the pull up did not read anything");
  const read = new URL(page.timelineCalls.at(-1), "http://fixture.test");
  assert.equal(read.searchParams.get("view"), "thread", "the pull up read something other than the thread");
  assert.equal(read.searchParams.get("thread_id"), page.threads[0].id, "the pull up read another thread");
  assert.equal(read.searchParams.get("before"), null, "the pull up walked back instead of refreshing");
  // `remote-ops-audit`: and it says so to the server, which counts its reads by why they were made.
  assert.equal(read.searchParams.get("reason"), "pull", "the pull up did not say it was one");
  assert.deepStrictEqual(shownIds(page), ["201", "202", "205"], "the thread's new answer is not on the screen");
  assert.equal(page.el("thread-heading").hidden, false, "refreshing the thread closed it");
  assert.equal(affordance.getAttribute("data-state"), "done");
});

/**
 * Pull up past the threshold and lift, leaving the refresh in flight. Returns `{ lifted }`, the
 * release still running — wrapped, because an async function returning it bare would wait for it.
 */
async function pullUpAndHold(page) {
  await pullUp(page, PULL_ARM_PX, { lift: false });
  const lifted = page.el("scroll-area").dispatch("touchend");
  await page.settle();
  assert.equal(page.el("pull-refresh").getAttribute("data-state"), "busy", "the pull up never started");
  return { lifted };
}

test("A PULL UP REPORTS ONLY ON THE VIEW IT WAS PULLED OVER, AND ONLY A READ SINCE", async () => {
  // On a slow read the reader does not wait: they tap the other tab. The read they pulled for is
  // then folded away for later, and the page restores the freshness of the tab they went to. A
  // result said at the foot from that would be "Updated" at a time they never refreshed at, with
  // "new messages" for rows that came with the other tab.
  const page = await threadPage();
  await page.el("channel-view-flat").click();
  await page.settle();
  await page.el("channel-view-main").click();
  await page.settle();
  const area = page.el("scroll-area");
  const affordance = page.el("pull-refresh");
  area.scrollTop = area.scrollHeight;
  // Time passes between reads, so that a stamp from an earlier read is told from a new one.
  page.setClock(page.clock() + 7 * 60 * 1000);

  // ONE: to a tab the page already holds, so the switch costs no read of its own.
  let reads = holdChannelReads(page, "timeline");
  let { lifted } = await pullUpAndHold(page);
  assert.equal(reads.held.length, 1, "the pull up's read is not the one held, so this proves nothing");
  await page.el("channel-view-flat").click();
  await page.settle();
  reads.release();
  await lifted;
  await page.settle();
  assert.equal(
    affordance.hidden,
    true,
    `All was told ${JSON.stringify(affordance.textContent)} about the refresh pulled on Main`
  );

  // TWO: away and straight back while the read hangs, and then it fails. The tab on screen IS the
  // one pulled, but nothing has landed for it since the release: the page restored the stamp of the
  // read BEFORE, and "Updated" at that time would report a refresh that never happened.
  await page.el("channel-view-main").click();
  await page.settle();
  area.scrollTop = area.scrollHeight;
  page.setClock(page.clock() + 7 * 60 * 1000);
  reads = holdChannelReads(page, "timeline");
  ({ lifted } = await pullUpAndHold(page));
  assert.equal(reads.held.length, 1, "the pull up's read is not the one held, so this proves nothing");
  await page.el("channel-view-flat").click();
  await page.settle();
  await page.el("channel-view-main").click();
  await page.settle();
  reads.fail();
  await lifted;
  await page.settle();
  assert.equal(
    affordance.hidden,
    true,
    `a refresh that never landed was reported as ${JSON.stringify(affordance.textContent)}`
  );

  // ...while one that lands after the reader comes back is reported as usual.
  reads = holdChannelReads(page, "timeline");
  ({ lifted } = await pullUpAndHold(page));
  reads.release();
  await lifted;
  await page.settle();
  assert.equal(affordance.getAttribute("data-state"), "done", "a refresh that landed was not reported");
});

test("...and a pull up left for another CHANNEL says nothing about the one now on screen", async () => {
  const page = newPage();
  const second = { id: "1110000000000000002", label: "second", writable: true };
  page.channels = [{ ...CHANNEL }, second];
  await signIn(page);
  await parkedAtNewest(page);
  const affordance = page.el("pull-refresh");
  page.setClock(page.clock() + 7 * 60 * 1000);

  const reads = holdChannelReads(page);
  const { lifted } = await pullUpAndHold(page);
  assert.equal(reads.held.length, 1, "the pull up's read is not the one held, so this proves nothing");
  page.el("discord-channel").value = second.id;
  await page.el("discord-channel").dispatch("change");
  await page.settle();
  // The new channel's read queues behind the pulled one, and runs inside the wait for it.
  reads.release();
  await lifted;
  await page.settle();
  assert.match(page.pagesServed.at(-1), new RegExp(second.id), "the second channel was never read");
  assert.match(page.el("channel-freshness").textContent, /pdated \d{2}:\d{2}$/, "the channel is not current");
  assert.equal(
    affordance.hidden,
    true,
    `the second channel was told ${JSON.stringify(affordance.textContent)} about the first one's refresh`
  );
});

test("A PULL UP IS THE CHANNEL'S ALONE — NOT THE VOICE VIEW'S, AND NOT A DRAFT'S THAT SCROLLS", async () => {
  const page = newPage();
  await signIn(page);
  // The voice view shares #scroll-area, and an empty transcript is at its newest line. Its pull
  // would re-read a channel that is not on screen.
  assert.equal(page.tab(), "voice");
  await pullUp(page, PULL_ARM_PX * 2);
  assert.equal(page.pageReads, 0, "a pull up on the voice view read the channel");
  assert.equal(page.el("pull-refresh").hidden, true, "a pull up on the voice view offered to refresh");

  const { area } = await parkedAtNewest(page);
  const readsBefore = page.pageReads;
  // The composer sits at the foot of the channel. Dragging through a draft too long for its box is
  // the reader reaching the end of their own message, and the list stays at its end throughout.
  const longDraft = { tagName: "TEXTAREA", scrollHeight: 240, clientHeight: 60 };
  await pullUp(page, PULL_ARM_PX * 2, { target: longDraft });
  assert.equal(page.pageReads, readsBefore, "scrolling a long draft refreshed the channel");
  assert.equal(page.el("pull-refresh").hidden, true, "scrolling a long draft offered to refresh");
  // ...but a draft that fits is just part of the list, and a pull that starts on it is a pull.
  const shortDraft = { tagName: "TEXTAREA", scrollHeight: 60, clientHeight: 60 };
  await pullUp(page, PULL_ARM_PX, { target: shortDraft });
  assert.equal(page.pageReads, readsBefore + 1, "a pull up starting on the composer did nothing");
  assert.ok(atBottomOf(area));
});

test("the foot's affordance clears the chips, spins only while fetching, and never blocks a scroll", () => {
  const end = cssBlock('#pull-refresh[data-edge="end"]');
  assert.match(end, /top:\s*auto/, "a pull up's affordance is still pinned to the top edge");
  // Where the status line stands, and above the floating chips by their measured height.
  assert.match(end, /bottom:\s*calc\([^;]*var\(--scroll-tools-clearance/, "it can cover a chip");
  assert.match(cssBlock("#pull-refresh"), /white-space:\s*nowrap/, "\"Updated 07:15 · nothing new\" wraps");
  assert.match(
    cssBlock('#pull-refresh[data-state="busy"]::before'),
    /animation:\s*outgoing-spin/,
    "nothing spins while the refresh is in flight"
  );
  // Beside the rule, as this sheet's other spinners keep theirs — so not `mediaBody`, which finds
  // only the first of the several reduced-motion blocks.
  assert.match(
    CSS_CODE,
    /@media \(prefers-reduced-motion: reduce\)\s*\{\s*#pull-refresh\[data-state="busy"\]::before\s*\{\s*animation:\s*none/,
    "the spinner ignores a reader who asked for less motion"
  );
  // PASSIVE, all four: the gesture only reads touches, and a listener that might cancel makes the
  // browser hold every scroll that starts on the list until this script has answered.
  assert.match(SCRIPT_CODE, /const PASSIVE = \{ passive: true \};/);
  for (const type of ["touchstart", "touchmove", "touchend", "touchcancel"]) {
    const line = SCRIPT_CODE.split("\n").find((text) => text.includes(`addEventListener("${type}"`));
    assert.ok(line, `nothing listens for ${type}`);
    assert.match(line, /, PASSIVE\);\s*$/, `the ${type} listener can hold up a scroll`);
  }
});

/** A channel tall enough to actually scroll: folded messages are capped at three lines each. */
const channelOf = (n, extra = 0) =>
  Array.from({ length: n + extra }, (_, i) =>
    message({ id: String(i + 1), content: longMessage(`m${i + 1}`) })
  );

// --- the "Newest" chip belongs to the list it is about ------------------------------------------
//
// Both lists live inside one #scroll-area, so a single page-wide flag gets this wrong in two
// directions at once: a voice turn raises a chip while you are reading the channel, offering to
// jump you to the bottom of a list nothing arrived in — and a channel message arriving during a
// background poll raises nothing at all, which is the very defect the chip exists to prevent.

test("a message arriving in the CHANNEL while you have scrolled up raises the chip", async () => {
  const page = newPage();
  await signIn(page);
  countingChannel(page, channelOf(12));
  await page.el("view-switch").click();
  await page.settle();

  const area = page.el("scroll-area");
  area.scrollTop = 0; // reading older messages, not pinned to the bottom

  page.messages = channelOf(12, 1);
  page.expireTimers(45000);
  await page.settle();

  assert.equal(page.reads, 2, "the poll did not fire");
  assert.equal(area.scrollTop, 0, "a background poll moved the reader");
  assert.equal(
    page.el("jump-newest").hidden,
    false,
    "the channel gained a message and said nothing — the reader cannot tell"
  );
});

test("a poll that returns the SAME messages does not offer to jump anywhere", async () => {
  const page = newPage();
  await signIn(page);
  countingChannel(page, channelOf(12));
  await page.el("view-switch").click();
  await page.settle();

  const area = page.el("scroll-area");
  area.scrollTop = 0;
  page.expireTimers(45000); // same page.messages: nothing new
  await page.settle();

  assert.equal(page.reads, 2);
  // Up the list, so the way down is offered (`#207 scrollback-jump`), and offered as nothing more.
  assert.equal(page.el("jump-newest-label").textContent, "Newest");
  assert.equal(
    jumpSaysArrived(page),
    false,
    "an unchanged channel raised an arrival; every 45 seconds it would nag about nothing"
  );
});

test("a VOICE turn does not raise a chip over the channel list", async () => {
  const page = newPage();
  const socket = await startTalking(page);
  countingChannel(page, channelOf(12));
  await page.el("view-switch").click(); // now reading the channel
  await page.settle();
  assert.equal(page.tab(), "discord");

  const area = page.el("scroll-area");
  area.scrollTop = 0;

  socket.onmessage({
    data: JSON.stringify({
      type: "user_transcript",
      user_transcription_event: { user_transcript: longMessage("spoken while reading the channel") },
    }),
  });

  // The channel's own jump is up, because the reader is at the top of the channel (`#207
  // scrollback-jump`) — and it says nothing arrived, because nothing arrived in the channel.
  assert.equal(
    jumpSaysArrived(page),
    false,
    "a transcript turn was announced on the jump over the CHANNEL"
  );

  // ...and the offer is waiting for them when they go back to the list it was about.
  await page.el("view-switch").click();
  await page.settle();
  assert.equal(page.tab(), "voice");
});

const JUMP_COUNT_MAX = sourceConstant("JUMP_COUNT_MAX");

/** What the jump says, and what it is called, in one value so a failure shows both. */
const jumpWords = (page) =>
  `${page.el("jump-newest-label").textContent} / ${page.el("jump-newest").getAttribute("aria-label")}`;

/** Halfway up whatever list is on screen: a reader well into its history, far from both ends. */
const halfway = (page) => {
  const area = page.el("scroll-area");
  return Math.round((area.scrollHeight - area.clientHeight) / 2);
};

test("THE JUMP IS UP WHENEVER THE CHANNEL IS SCROLLED BACK, and gone at the newest message", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, tallChannel());
  const area = page.el("scroll-area");
  assert.ok(area.scrollHeight > area.clientHeight * 2, "the channel does not overflow, so this proves nothing");
  assert.equal(jumpShown(page), false, "the jump is up over a reader on the newest message");

  await scrollToGap(page, halfway(page));
  assert.equal(jumpShown(page), true, "a reader halfway up the channel was offered no way back");
  assert.equal(jumpWords(page), "Newest / Jump to the newest message");
  assert.equal(jumpSaysArrived(page), false, "nothing arrived, and the jump says something did");

  await scrollToGap(page, 0);
  assert.equal(jumpShown(page), false, "the jump outstayed the scrollback it was offered for");
});

test("...and the same over the voice transcript", async () => {
  const page = newPage();
  await startTalking(page);
  fillTranscript(page);
  assert.equal(jumpShown(page), false, "the jump is up over a reader on the newest turn");

  await scrollToGap(page, halfway(page));
  assert.equal(jumpShown(page), true, "a reader halfway up the transcript was offered no way back");
  assert.equal(jumpWords(page), "Newest / Jump to the newest message");

  await scrollToGap(page, 0);
  assert.equal(jumpShown(page), false, "the jump outstayed the scrollback it was offered for");
});

test("a list that fits on the screen offers no jump, and one that shrinks to fit takes it away", async () => {
  // Nothing to scroll is the newest message on screen: a control offering to go there is inert.
  const page = newPage();
  await startTalking(page);
  youSay(page, SHORT_MESSAGE);
  assistantSays(page, SHORT_MESSAGE);
  const area = page.el("scroll-area");
  assert.ok(area.scrollHeight <= area.clientHeight, "the transcript overflows, so this is not the case");
  await area.dispatch("scroll");
  assert.equal(jumpShown(page), false, "a transcript of two short turns offered a jump");

  // A channel the reader is up, which a re-read then finds is two short messages: the list shrank
  // under them to less than a screen, so there is no scrollback left to come back from.
  countingChannel(page, channelOf(12));
  await page.el("view-switch").click();
  await page.settle();
  await scrollToGap(page, halfway(page));
  assert.equal(jumpShown(page), true, "the premise: a reader up the channel is offered the jump");
  page.messages = [message({ id: "1", content: SHORT_MESSAGE }), message({ id: "2", content: SHORT_MESSAGE })];
  page.expireTimers(DISCORD_POLL_MS);
  await page.settle();
  assert.ok(area.scrollHeight <= area.clientHeight, "the channel still overflows, so this is not the case");
  assert.equal(jumpShown(page), false, `a channel that fits on the screen still offers "${jumpWords(page)}"`);
});

test("the jump does not flicker at its boundary: it comes at one distance and goes at another", async () => {
  // A single distance is a boundary a resting thumb sits on and a decaying fling crosses on every
  // frame, and the control would blink in the corner. Two distances with a band between them.
  assert.ok(
    SCROLLBACK_PX - BOTTOM_SLACK_PX >= LINE_PX,
    `SCROLLBACK_PX ${SCROLLBACK_PX} leaves no band above BOTTOM_SLACK_PX ${BOTTOM_SLACK_PX}`
  );
  const run = async (script) => {
    const page = newPage(new Map(), script);
    await signIn(page);
    await showDiscord(page, tallChannel());
    const seen = [];
    const at = async (gap) => {
      await scrollToGap(page, gap);
      seen.push(jumpShown(page) ? "up" : "-");
    };
    // Up from the newest message, across both distances...
    for (const gap of [BOTTOM_SLACK_PX + 1, SCROLLBACK_PX, SCROLLBACK_PX + 1]) await at(gap);
    // ...a fling decaying back and forth through the band, and a thumb resting in it...
    for (const gap of [SCROLLBACK_PX - 2, SCROLLBACK_PX + 3, SCROLLBACK_PX - 1, BOTTOM_SLACK_PX + 1, SCROLLBACK_PX]) {
      await at(gap);
    }
    // ...down to the newest, and up into the band again from there.
    for (const gap of [BOTTOM_SLACK_PX, BOTTOM_SLACK_PX + 1, SCROLLBACK_PX, BOTTOM_SLACK_PX + 5, SCROLLBACK_PX - 1]) {
      await at(gap);
    }
    return seen;
  };
  assert.deepStrictEqual(await run(SCRIPT), [
    "-", "-", "up",
    "up", "up", "up", "up", "up",
    "-", "-", "-", "-", "-",
  ]);

  // THE CONTROL: one distance instead of two. It toggles inside the band, and this has to see it.
  const control = await run(brokenScript("    } else if (gap > SCROLLBACK_PX) {", "    } else {"));
  assert.ok(control.slice(8).includes("up"), "the test cannot tell one distance from two");
});

test("an arrival below says HOW MANY on the same control, and taking it clears the count", async () => {
  const page = newPage();
  await startTalking(page);
  const area = fillTranscript(page);
  const parked = halfway(page);
  await scrollToGap(page, parked);
  assert.equal(jumpWords(page), "Newest / Jump to the newest message");

  for (const n of [1, 2, 3]) assistantSays(page, longMessage(`while you were reading ${n}`));
  assert.equal(area.scrollHeight - area.clientHeight - area.scrollTop > parked, true, "the turns did not land below");
  assert.equal(jumpWords(page), "3 new / Jump to the newest message, 3 new");
  assert.equal(jumpSaysArrived(page), true, "the count is not in the arrival's accent");
  // ONE control. The arrival changed what it says, and raised nothing beside it.
  assert.equal([...HTML_CODE.matchAll(/id="jump-newest"/g)].length, 1);

  await page.el("jump-newest").click();
  assert.ok(atBottomOf(area), "the tap did not land on the newest turn");
  assert.equal(jumpShown(page), false, "the jump stayed after it was taken");
  await scrollToGap(page, parked);
  assert.equal(jumpWords(page), "Newest / Jump to the newest message", "the count came back after it was taken");

  // ...and reaching the newest by hand clears it as surely as the tap does.
  assistantSays(page, longMessage("one more"));
  assert.equal(page.el("jump-newest-label").textContent, "1 new");
  await scrollToGap(page, 0);
  await scrollToGap(page, parked + LINE_PX);
  assert.equal(jumpWords(page), "Newest / Jump to the newest message", "scrolling to the newest left the count up");
});

test("the channel counts what arrived on the stream and in a poll, and says New when it cannot count", async () => {
  const live = newPage();
  const stream = await withLiveChannel(live, tallChannel());
  await scrollToGap(live, halfway(live));
  await deliver(live, stream, sseMessage(message({ id: "200", content: "the first of two" })));
  await deliver(live, stream, sseMessage(message({ id: "201", content: "and the second" })));
  assert.equal(jumpWords(live), "2 new / Jump to the newest message, 2 new");

  const polled = newPage();
  await signIn(polled);
  countingChannel(polled, channelOf(12));
  await polled.el("view-switch").click();
  await polled.settle();
  await scrollToGap(polled, halfway(polled));
  polled.messages = channelOf(12, 3);
  polled.expireTimers(DISCORD_POLL_MS);
  await polled.settle();
  assert.equal(jumpWords(polled), "3 new / Jump to the newest message, 3 new");
  // A count that runs past what the chip spells out says so rather than growing the chip.
  polled.messages = channelOf(12, 3 + JUMP_COUNT_MAX);
  polled.expireTimers(DISCORD_POLL_MS);
  await polled.settle();
  assert.equal(polled.el("jump-newest-label").textContent, `${JUMP_COUNT_MAX}+ new`);

  // The message that was newest is not in the next read at all — more arrived than one read holds,
  // or it went — so how many is unknown, and the jump says only that something is there.
  const lost = newPage();
  await signIn(lost);
  countingChannel(lost, channelOf(12));
  await lost.el("view-switch").click();
  await lost.settle();
  await scrollToGap(lost, halfway(lost));
  lost.messages = Array.from({ length: 12 }, (_, i) =>
    message({ id: String(500 + i), content: longMessage(`later ${i}`) }));
  lost.expireTimers(DISCORD_POLL_MS);
  await lost.settle();
  assert.equal(jumpWords(lost), "New / Jump to the newest message, something new below");
  assert.equal(jumpSaysArrived(lost), true);
});

test("a restore that lands up the history shows the jump at once; one that lands on the newest never does", async () => {
  // Every one of these puts the reader somewhere WITHOUT a scroll event — a browser fires one
  // later, the fixture never does — so each is the page deciding from where it put them.
  const page = newPage();
  await signIn(page);
  const area = page.el("scroll-area");
  area.scrollTop = 0; // the rows are drawn into a list scrolled to its top...
  await showDiscord(page, tallChannel());
  assert.ok(atBottomOf(area), "the premise: a first entry lands on the newest message");
  assert.equal(jumpShown(page), false, "...and a first entry that lands on the newest offered the jump");

  // Away and back from the newest message: back on the newest, and nothing offered.
  await page.el("view-switch").click();
  await page.settle();
  await page.el("view-switch").click();
  await page.settle();
  assert.equal(jumpShown(page), false, "coming back to the newest message offered a jump to it");

  // Away and back from halfway up: back halfway up, and the jump is there on arrival.
  await scrollToGap(page, halfway(page));
  const parked = area.scrollTop;
  await page.el("view-switch").click();
  await page.settle();
  await page.el("view-switch").click();
  await page.settle();
  assert.equal(area.scrollTop, parked, "the premise: coming back keeps the reader's place");
  assert.equal(jumpShown(page), true, "coming back to a place up the channel showed no way back");
  assert.equal(jumpWords(page), "Newest / Jump to the newest message");

  // Off the main screen there is no list to jump in; back on it, the place is put back and so is it.
  await page.el("open-settings").click();
  assert.equal(jumpShown(page), false, "the jump stood over Settings");
  await page.el("close-settings").click();
  assert.equal(jumpShown(page), true, "back from Settings, the jump did not come back with the place");
  const row = page.el("discord-log").children.find((li) => li.getBoundingClientRect().bottom > 0);
  // `#410 inline-reply-editor`. A reply opens in the list, which stays on screen with its jump.
  await replyButton(row).click();
  await page.settle();
  assert.ok(replyInStream(page));
  assert.equal(jumpShown(page), true, "a reply open in the list took the jump away");
  await page.el("reply-cancel").click();
  await page.settle();
  assert.equal(area.scrollTop, parked, "the premise: putting the reply down leaves the reader where they were");
  assert.equal(jumpShown(page), true, "back from replying, the jump did not stay with the place");
});

test("a reload that puts the reader back up the channel shows the jump; one on the newest does not", async () => {
  const channel = tallChannel();
  const reload = async (park) => {
    const first = newPage();
    await signIn(first);
    await showDiscord(first, channel);
    park(first);
    await first.setVisibility("hidden");
    const page = newPage(first.storage, SCRIPT, (p) => { p.messages = channel; });
    // In the script's own task, drawn from the device, before any read has answered.
    const drawn = jumpShown(page);
    await reopenedChannel(page);
    return { drawn, read: jumpShown(page), page };
  };
  const midway = await reload((first) => { first.el("scroll-area").scrollTop = halfway(first); });
  assert.ok(!atBottomOf(midway.page.el("scroll-area")), "the premise: the reload put the reader back mid-channel");
  assert.equal(midway.drawn, true, "the reopen put the reader up the channel and offered no way back");
  assert.equal(midway.read, true, "the reopen's read took the jump away from a reader still up the channel");

  const newest = await reload(() => {});
  assert.ok(atBottomOf(newest.page.el("scroll-area")), "the premise: the reload put the reader on the newest");
  assert.equal(newest.drawn, false, "a reopen on the newest message offered a jump to it");
  assert.equal(newest.read, false, "the reopen's read offered a jump to the newest message");
});

test("the list changing size under a reader who has not scrolled decides the jump again", async () => {
  // None of these is a scroll, so a browser fires no scroll event for any of them and the listener
  // never hears of it: the window resized, a row below the reader growing. The fixture's `resized`
  // is all a browser does for them. Without it, a reader the window grew under is on the newest
  // line with a jump offering to take them there.
  const run = async (script) => {
    const seen = {};
    const page = newPage(new Map(), script, (p) => p.enableResizeObserver());
    await signIn(page);
    await showDiscord(page, tallChannel());
    const area = page.el("scroll-area");
    assert.equal(jumpShown(page), false, "the premise: on the newest message, no jump");
    // The window loses more than SCROLLBACK_PX and the reader, unmoved, is up the history.
    area.clientHeight -= SCROLLBACK_PX + LINE_PX;
    assert.ok(area.scrollHeight - area.clientHeight - area.scrollTop > SCROLLBACK_PX, "the premise: now up");
    page.resized(area);
    page.expireTimers(0);
    seen.windowShrank = jumpShown(page);
    // Down from halfway up to just inside the band, so the jump is up by the scroll listener's own
    // decision; then the window gains 30px and the reader, unmoved, is on the newest line.
    await scrollToGap(page, halfway(page));
    await scrollToGap(page, BOTTOM_SLACK_PX + 16);
    assert.equal(jumpShown(page), true, "the premise: come down into the band from above, the jump is up");
    area.clientHeight += 30;
    assert.ok(atBottomOf(area), "the premise: the taller window put the reader on the newest line");
    page.resized(area);
    page.expireTimers(0);
    seen.windowGrew = jumpShown(page);

    // A pane rather than the list's own box: the newest turn, re-wrapped taller (a narrower column, a
    // font arriving) with nothing drawn by the page, under a reader on the newest line.
    const talk = newPage(new Map(), script, (p) => p.enableResizeObserver());
    await startTalking(talk);
    fillTranscript(talk);
    assert.equal(jumpShown(talk), false, "the premise: on the newest turn, no jump");
    talk.el("transcript").children.at(-1).textContent += " and more".repeat(CHARS_PER_LINE);
    talk.resized(talk.el("pane-voice"));
    talk.expireTimers(0);
    seen.rowGrewBelow = jumpShown(talk);
    return seen;
  };
  assert.deepStrictEqual(await run(SCRIPT), { windowShrank: true, windowGrew: false, rowGrewBelow: true });

  // THE CONTROL: the list and its panes not watched. Each is left as the last scroll decided it, and
  // all three have to see that.
  const control = await run(brokenScript(
    '  for (const box of [el("scroll-area"), ...el("scroll-area").children]) geometry.observe(box);\n', ""
  ));
  assert.deepStrictEqual(control, { windowShrank: false, windowGrew: true, rowGrewBelow: false });
});

test("a desk's dock growing keeps a reader on the newest turn there, and moves nobody else", async () => {
  // `#218 desktop-dock`. On a desk the dock is one row of controls or two, and a call going live is a
  // second row: 42px taken from the bottom of the list, past the slack, so the reader who had just
  // pressed Talk on the newest turn was no longer on it and the call's next turn was not followed.
  // The fixture lays nothing out, so the test does what a browser does: the dock is the new height,
  // the list has lost that much, and whatever watches the list is told.
  const GROWTH = 42;
  const run = async (script) => {
    const page = newPage(new Map(), script, (p) => p.enableResizeObserver());
    const area = await startTalking(page).then(() => fillTranscript(page));
    const dock = page.el("dock");
    dock.offsetHeight = 50;
    // The first report has nothing before it: the dock as the page has it.
    page.resized(area);
    const grow = (by) => {
      dock.offsetHeight += by;
      area.clientHeight -= by;
      page.resized(area);
    };
    assert.ok(atBottomOf(area), "the premise: the reader is on the newest turn");
    grow(GROWTH);
    const newest = atBottomOf(area);
    // Up the history, the dock growing again: the reader stays on the line they were reading.
    await scrollToGap(page, halfway(page));
    const up = area.scrollTop;
    grow(20);
    const upHeld = area.scrollTop === up;
    // A WINDOW losing height is not the dock growing, and leaves a reader where they are (`#207`).
    await scrollToGap(page, 0);
    area.clientHeight -= GROWTH;
    page.resized(area);
    const windowLeft = !atBottomOf(area);
    // Away to Settings and back, which on a phone grows the dock by the whole tile as the pane
    // returns. The list is hidden while Settings is up, so it reports nothing to lose.
    await scrollToGap(page, BOTTOM_SLACK_PX + 10);
    await page.el("open-settings").click();
    await page.settle();
    area.clientHeight = 0;
    page.resized(area);
    dock.offsetHeight += 100;
    area.clientHeight = SCROLL_VIEWPORT_PX - 100;
    await page.el("close-settings").click();
    await page.settle();
    page.resized(area);
    return { newest, upHeld, windowLeft, back: area.scrollHeight - area.clientHeight - area.scrollTop };
  };
  const real = await run(SCRIPT);
  assert.equal(real.newest, true, "the dock grew under a reader on the newest turn and left them above it");
  assert.equal(real.upHeld, true, "the dock grew under a reader up the history and moved them");
  assert.equal(real.windowLeft, true, "a window losing height pulled the reader down to the newest turn");

  // THE CONTROL: nothing keeps the reader. The growth leaves them above the newest turn — further than
  // the slack, so this is the defect and not a rounding — and everything else comes out the same,
  // the return from Settings included: what the page does there, it did before this.
  const control = await run(brokenScript(
    "    if (grew > 0 && atBottom(area, BOTTOM_SLACK_PX + grew)) {\n      area.scrollTop = area.scrollHeight;\n    }\n",
    ""
  ));
  assert.ok(GROWTH > BOTTOM_SLACK_PX, "the premise: a second row is more than the slack");
  assert.deepStrictEqual(control, { ...real, newest: false });
});

test("a tap on the jump only scrolls: no refresh, no fold, nothing read aloud", async () => {
  const page = newPage();
  await signIn(page);
  countingChannel(page, channelOf(12));
  await page.el("view-switch").click();
  await page.settle();
  const area = page.el("scroll-area");
  await scrollToGap(page, halfway(page));
  const rows = () => page.el("discord-log").children.map((li) =>
    `${li.className} ${JSON.stringify([...li.attributes].sort())}`);
  const before = { rows: rows(), reads: page.reads, speak: page.speakCalls.length, players: page.players.length };

  await page.el("jump-newest").click();
  await page.settle();

  assert.ok(atBottomOf(area), "the tap did not land on the newest message");
  assert.equal(page.reads, before.reads, "the tap read the channel: it was taken for the pull up past the end");
  assert.equal(page.el("pull-refresh").hidden, true, "the tap put the pull's affordance up");
  assert.equal(page.el("pull-refresh").getAttribute("data-state"), "idle");
  assert.deepStrictEqual(rows(), before.rows, "the tap folded, marked or otherwise changed a row");
  assert.equal(page.speakCalls.length, before.speak, "the tap read a message aloud");
  assert.equal(page.players.length, before.players, "the tap started a player");
  // Why none of them can hear it: it is outside the list, so no row's tap and no list gesture is
  // ever given its touch.
  assert.equal(markupHolds("scroll-area", "jump-newest"), false, "the jump is inside the list it scrolls");
  assertMarkupContains("scroll-tools", "jump-newest");
});

test("each list has its own jump: scrolled back in the channel, the transcript shows its own state", async () => {
  const page = newPage();
  await talkingBesideAChannel(page);
  await page.el("view-switch").click();
  await page.settle();
  assert.equal(page.tab(), "discord");
  await scrollToGap(page, halfway(page));
  assert.equal(jumpWords(page), "Newest / Jump to the newest message", "the premise: up the channel");

  await page.el("view-switch").click(); // to the transcript, left on its newest turn
  await page.settle();
  assert.equal(page.tab(), "voice");
  assert.equal(jumpShown(page), false, "the channel's scrollback was shown over the transcript's newest turn");

  // The transcript up its own history, and a turn arriving below.
  await scrollToGap(page, halfway(page));
  assistantSays(page, longMessage("said while you read back"));
  assert.equal(jumpWords(page), "1 new / Jump to the newest message, 1 new");

  await page.el("view-switch").click(); // the channel: its own place, its own words
  await page.settle();
  assert.equal(jumpWords(page), "Newest / Jump to the newest message", "the transcript's arrival followed the reader to the channel");
  assert.equal(jumpShown(page), true);
  await page.el("view-switch").click(); // and back: the transcript's arrival is still waiting there
  await page.settle();
  assert.equal(jumpWords(page), "1 new / Jump to the newest message, 1 new");
});

test("the jump is the FIRST thing in the chip row, drawn at its left end with the others at the right", () => {
  // DOM order is the layout's input: `margin-inline-end: auto` on the first item is what sends every
  // later one to the right, and `wrap-reverse` is what keeps that first line at the bottom.
  const tools = HTML_CODE.slice(HTML_CODE.indexOf('id="scroll-tools"'));
  assert.equal(/\bid="([^"]+)"/.exec(tools.slice('id="scroll-tools"'.length))[1], "jump-newest",
    "the jump is not the first thing in the chip row");
  for (const id of ["jump-bookmark", "summarise", "undo-dismiss", "expand-all", "collapse-all"]) {
    assertMarkupContains("scroll-tools", id);
    assert.ok(markupPlace("jump-newest").at < markupPlace(id).at, `#${id} comes before the jump`);
  }
  // `#240 reading-bookmark`. The jump to the bookmark is DIRECTLY right of it, and while it is up it
  // takes the auto margin from the jump beside it, so the two stand together at the left.
  const afterNewest = tools.slice(tools.indexOf("</button>") + "</button>".length);
  assert.equal(/\bid="([^"]+)"/.exec(afterNewest)[1], "jump-bookmark", "the bookmark's jump is not the next chip after Newest");
  assert.match(cssBlock("#jump-newest:has(+ #jump-bookmark:not([hidden]))"), /margin-inline-end:\s*0/);
  assert.match(cssBlock("#jump-bookmark"), /margin-inline-end:\s*auto/);
  assert.match(cssBlock("#jump-bookmark::after"), /inset:\s*min\(0px,\s*calc\(\(100% - 2\.75rem\) \/ 2\)\)/);
  assert.match(cssBlock('#jump-bookmark[data-direction="down"] .bookmark-arrow'), /rotate\(180deg\)/);
  const row = cssRules(CSS_CODE, "#scroll-tools")[0];
  assert.match(row, /left:\s*calc\(0\.75rem \+ env\(safe-area-inset-left\)\)/, "the row does not reach the left, inside the safe area");
  assert.match(row, /right:\s*calc\(0\.75rem \+ env\(safe-area-inset-right\)\)/, "the row does not reach the right, inside the safe area");
  assert.match(row, /flex-wrap:\s*wrap-reverse/, "a wrapped row would lift the jump off the bottom line");
  assert.match(row, /justify-content:\s*flex-end/, "the other chips are not kept at the right");
  assert.match(cssBlock("#jump-newest"), /margin-inline-end:\s*auto/, "nothing sends the other chips to the right");
  // Hit at 44px whatever it is drawn at.
  assert.match(cssBlock("#jump-newest::after"), /inset:\s*min\(0px,\s*calc\(\(100% - 2\.75rem\) \/ 2\)\)/);
  assert.match(cssBlock("#jump-newest[data-arrivals]"), /color:\s*var\(--accent\)/, "an arrival does not read differently");
  // On a desk the row spans the reading column, not the window.
  const desk = cssBlockIn("(min-width: 900px) and (pointer: fine)", "#scroll-tools");
  assert.match(desk, /width:\s*calc\(min\(var\(--reading-width\), 100%\) - 1\.2rem\)/);
  assert.match(desk, /transform:\s*translateX\(-50%\)/);
  // Named before any script has run.
  assert.match(HTML_CODE, /<button id="jump-newest"[^>]*aria-label="Jump to the newest message"/);
});

// --- `#280 scroll-stability`: a read that lands, or a message that arrives, never moves the reader
//
// The owner, 2026-10-13: "I swear I just saw an instance where it jumped me to the bottom of the screen
// in the main message view (two column mode)... I don't think I touched anything. I think it was
// something updating underneath." A read took the reader's place as it went OUT and settled to it when
// it landed, seconds later behind a bridge: a reader on the newest line when it went who had scrolled up
// since was thrown back to the bottom. And on a channel with threads a message arriving live went in
// before anybody asked whether the reader was on the newest line, so a reader who was on it was never
// followed.
// tests/scroll_stability_browser.py measures both in Chromium; these hold the rules.

/** A channel of long messages, taller than the list, with or without threads. */
function stabilityChannel(page, threads) {
  page.threadingSupported = threads;
  return Array.from({ length: 14 }, (_unused, index) =>
    message({ id: `277${String(index).padStart(2, "0")}`, content: longMessage(`message ${index}`) }));
}

for (const threads of [true, false]) {
  const kind = threads ? "with threads" : "without threads";

  test(`${kind}: a read sent while the reader was on the newest line, landing after they scrolled up, leaves them where they are`, async () => {
    const page = newPage();
    const messages = stabilityChannel(page, threads);
    const stream = await withLiveChannel(page, messages);
    const area = page.el("scroll-area");
    assert.ok(atBottomOf(area), "the channel did not open on its newest line");
    // The read an edit asks for, held on the wire.
    const held = gate(threads ? page.timeline : page.channelPage);
    if (threads) page.timeline = held.respond;
    else page.channelPage = held.respond;
    const edited = { ...messages[3], content: longMessage("message 3, edited") };
    page.messages = [...messages.slice(0, 3), edited, ...messages.slice(4)];
    await deliver(page, stream, sseUpdate("edit-277", edited));
    // Up the history while it is out.
    area.scrollTop = Math.round(area.scrollHeight / 3);
    const before = readerOn(page);
    assert.ok(!atBottomOf(area), "the reader did not leave the newest line");
    held.open();
    await page.settle();
    await page.settle();
    assert.match(page.el("discord-log").text(), /message 3, edited/, "the held read never landed");
    assert.ok(!atBottomOf(area), "the read that landed threw the reader back to the newest line");
    assert.deepStrictEqual(readerOn(page), before, "the read that landed moved the row the reader was on");
  });

  test(`${kind}: a message arriving live is followed by a reader on the newest line, and moves nobody up the list`, async () => {
    const page = newPage();
    const messages = stabilityChannel(page, threads);
    const stream = await withLiveChannel(page, messages);
    const area = page.el("scroll-area");
    assert.ok(atBottomOf(area), "the channel did not open on its newest line");
    const first = message({ id: "27790", content: longMessage("arrived live") });
    page.messages = [...messages, first];
    await deliver(page, stream, sseMessage(first));
    await page.settle();
    assert.ok(rowWithId(page, "27790"), "the live message never reached the list");
    assert.ok(atBottomOf(area), "a reader on the newest line was left above the message that arrived");

    area.scrollTop = Math.round(area.scrollHeight / 3);
    const before = readerOn(page);
    const second = message({ id: "27791", content: longMessage("arrived live again") });
    page.messages = [...page.messages, second];
    await deliver(page, stream, sseMessage(second));
    await page.settle();
    assert.ok(rowWithId(page, "27791"), "the second live message never reached the list");
    assert.deepStrictEqual(readerOn(page), before, "a message arriving moved a reader up the list");
    assert.ok(jumpSaysArrived(page), "nothing offered the reader up the list the message that arrived");
  });
}

test("a held mutation refresh cannot paint the old channel under a new selection", async () => {
  const page = newPage();
  const other = { id: "2220000000000000002", label: "other team", writable: true };
  page.channels = [{ ...CHANNEL }, other];
  page.upstreamReadMarkSupported = true;
  const fromA = message({ id: "a-message", channel_id: CHANNEL.id, content: "private to A" });
  const fromB = message({ id: "b-message", channel_id: other.id, content: "current in B" });
  const stream = await withLiveChannel(page, [fromA]);

  let releaseA = null;
  let held = true;
  page.channelPage = async (path) => {
    if (held) {
      held = false;
      return new Promise((resolve) => {
        releaseA = () => resolve(json(200, { channel: CHANNEL, messages: [fromA] }));
      });
    }
    assert.match(String(path), new RegExp(other.id), "the queued read did not follow the selector");
    return json(200, { channel: other, messages: [fromB] });
  };

  stream.push(sseUpdate("held-a-update", fromA));
  await page.settle();
  assert.ok(releaseA, "the mutation did not start the held channel-A read");

  page.el("discord-channel").value = other.id;
  await page.el("discord-channel").dispatch("change");
  assert.deepStrictEqual(shownIds(page), [], "changing channel did not clear the old rows");

  releaseA();
  await page.settle();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), [fromB.id], "channel A rendered beneath channel B");
  assert.doesNotMatch(page.el("discord-log").text(), /private to A/);

  const control = upstreamReadButton(page.el("discord-log").children[0]);
  await control.click();
  await page.settle();
  assert.deepStrictEqual(page.upstreamReadCalls, [{ message_id: fromB.id }]);
  assert.deepStrictEqual(page.upstreamReadPaths, [
    `/api/v1/channels/${other.id}/upstream-read`,
  ]);
});

test("HOSTILE MESSAGE TEXT NEVER BECOMES MARKUP ON THE LIVE PATH EITHER", async () => {
  // The same payloads as the fetched path, re-run here on purpose. A live feed is not a reason to
  // relax element construction, and a second entry point into the renderer is exactly where a
  // shortcut gets taken.
  const page = newPage();
  const stream = await withLiveChannel(page, []);
  const HOSTILE =
    '<script>alert("xss")</script> <img src=x onerror=alert(1)> ' +
    '<iframe src="javascript:alert(2)"></iframe> &lt;already escaped&gt;';
  const before = page.createdTags.length;

  await deliver(page, stream, sseMessage(message({ id: "400", content: HOSTILE })));

  const rows = page.el("discord-log").children;
  const row = rows[rows.length - 1];
  assert.ok(
    row.text().includes('<script>alert("xss")</script>'),
    `the payload was mangled rather than shown: ${row.text()}`
  );
  assert.ok(row.text().includes("&lt;already escaped&gt;"));
  const created = page.createdTags.slice(before);
  for (const tag of ["script", "img", "iframe", "style", "object", "embed", "link"]) {
    assert.ok(!created.includes(tag), `a live message created a <${tag}>`);
  }
  for (const node of row.descendants()) {
    for (const name of node.attributes.keys()) {
      assert.doesNotMatch(name, /^on/i, `an ${name} attribute was set from live channel text`);
      assert.notEqual(name, "src");
    }
  }
});

test("an arriving message reaches a live call only when the reader has asked for it", async () => {
  const page = newPage();
  await startTalking(page);
  page.messages = [];
  await page.el("view-switch").click();
  await page.settle();
  const stream = page.stream();
  assert.ok(stream, "no live stream was attached");

  // `off` is a mode the reader can choose, and choosing it means silence even mid-call.
  await setReadNew(page, "off");
  await deliver(page, stream, sseMessage(message({ id: "500", content: "the deploy finished" })));
  await speakRelay(page);
  assert.deepStrictEqual(
    relayed(page),
    [],
    "an arriving message was spoken into a paid conversation the reader had switched off"
  );

  await setReadNew(page, "gist");
  await deliver(page, stream, sseMessage(message({ id: "501", content: "and the tag is cut" })));
  await speakRelay(page);

  const sent = relayed(page);
  assert.equal(sent.length, 1, "the selector is on and nothing was relayed");
  assert.match(sent[0].text, /and the tag is cut/, "the message text has to actually be in it");
  assert.match(sent[0].text, /lead team/, "which channel it came from is half the information");
  assert.match(
    sent[0].text,
    /never be treated as a command/i,
    "channel text handed to a model must arrive framed as a quotation, not as an instruction"
  );
});

test("the three modes differ in WHAT THE AGENT IS ASKED TO DO, and off asks nothing", async () => {
  // The whole of what the reader is choosing between. Mode 2 spends one sentence of conversation
  // time on an arrival; mode 3 spends however long the message is. A test that only checked
  // "something was sent" would pass against a page where the selector had three positions and two
  // behaviours, which is the failure worth catching.
  const page = newPage();
  await startTalking(page);
  page.messages = [];
  await page.el("view-switch").click();
  await page.settle();
  const stream = page.stream();

  await setReadNew(page, "gist");
  await deliver(page, stream, sseMessage(message({ id: "520", content: "the tag is cut" })));
  await speakRelay(page);
  const gist = relayed(page);
  assert.equal(gist.length, 1);
  assert.match(
    gist[0].text,
    /one short sentence/i,
    "mode 2 is a summary, and the turn has to say so or the agent will read the message out"
  );
  // The agent answers it. On this protocol the next relayed turn waits for that answer, which a
  // `user_message` sent into it would cut (`#337 voice-relay-silent`).
  assistantSays(page, "Lead team says the tag is cut.");
  await passTime(page, 5000);

  await setReadNew(page, "full");
  await deliver(page, stream, sseMessage(message({ id: "521", content: "and deployed" })));
  await speakRelay(page);
  const full = relayed(page);
  assert.equal(full.length, 2);
  assert.match(
    full[1].text,
    /word for word/i,
    "mode 3 is the words themselves, and nothing else in the turn asks for them"
  );

  // Both of them ask the agent to hand the floor back. This is an interruption of a conversation
  // the reader is already having, not an invitation to open a topic.
  for (const turn of full) {
    assert.match(turn.text, /Then stop\./, `a relayed turn left the floor: ${turn.text}`);
  }

  // The task sentence comes FIRST, ahead of the quoted text. Placed after it, the instruction
  // would sit behind whatever a third party wrote, which is the one ordering the framing exists
  // to prevent.
  for (const turn of full) {
    assert.ok(
      turn.text.indexOf("Then stop.") < turn.text.indexOf(RELAY_FENCE_OPEN),
      `quoted channel text is the last instruction in the turn: ${turn.text}`
    );
  }
});

test("WHAT IS RELAYED IS THE PREPARED BODY, not the raw one", async () => {
  // The reported symptom, in the place it actually happened. `read new` builds its turn HERE, in
  // the browser, so it was the one path to the agent with no preparation on it at all: a message
  // mentioning a snowflake got read out as nineteen spoken digits. The server now sends the
  // prepared body beside the raw one and this quotes that.
  const page = newPage();
  await startTalking(page);
  page.messages = [];
  await page.el("view-switch").click();
  await page.settle();
  const stream = page.stream();

  await setReadNew(page, "full");
  await deliver(
    page,
    stream,
    sseMessage(
      message({
        id: "530",
        content: "**deploy** by 1000000000000000009 at 2026-08-24T04:31:00Z",
        spoken_content: "deploy by UID ending in 009 three hours ago",
      })
    )
  );
  await speakRelay(page);

  const [turn] = relayed(page);
  assert.ok(turn, "nothing was relayed at all");
  assert.match(turn.text, /UID ending in 009/, "the prepared body is not what was quoted");
  assert.doesNotMatch(
    turn.text,
    /1000000000000000009/,
    "a nineteen-digit id reached the agent, which is the whole complaint"
  );
  assert.doesNotMatch(turn.text, /\*\*/, "markdown asterisks reached a thing that reads aloud");

  // And the ROW still shows what was written. The letter is only recoverable because the real id
  // is on screen beside it; a page that showed the prepared body in both places would have made
  // the substitution lossy instead of merely quiet.
  await page.el("view-switch").click();
  await page.settle();
  const shown = page.el("discord-log").text();
  assert.match(shown, /1000000000000000009/, "the screen lost the id the letter stands for");
});

test("a server too old to prepare a body still gets its messages read", async () => {
  // The fallback, stated as a behaviour rather than as an `||`. `spoken_content` is a new field;
  // an older server sends none, and the failure mode to avoid is silence — a reader who is
  // listening cannot tell "nothing arrived" apart from "it arrived and was dropped". Reading it
  // badly is the right answer, and this is the only place that says so.
  const page = newPage();
  await startTalking(page);
  page.messages = [];
  await page.el("view-switch").click();
  await page.settle();
  const stream = page.stream();

  await setReadNew(page, "full");
  await deliver(page, stream, sseMessage(message({ id: "531", content: "the tag is cut" })));
  await speakRelay(page);

  const [turn] = relayed(page);
  assert.ok(turn, "a message with no prepared body was silently dropped");
  assert.match(turn.text, /the tag is cut/, "the raw body is the fallback and it was not used");
});

test("A BURST OF ARRIVALS IS ONE SPOKEN TURN, NOT ONE EACH", async () => {
  // A coding agent posting four lines in two seconds is the ordinary case this feature exists for.
  // One `user_message` each would be four spoken turns in a row, each interrupting the last, with
  // no gap for the reader to answer in — and four turns billed.
  const page = newPage();
  await startTalking(page);
  page.messages = [];
  await page.el("view-switch").click();
  await page.settle();
  const stream = page.stream();
  await setReadNew(page, "full");

  for (const [id, content] of [
    ["1200", "starting the build"],
    ["1201", "tests are running"],
    ["1202", "one failure in the parser"],
  ]) {
    await deliver(page, stream, sseMessage(message({ id, content })));
  }
  assert.deepStrictEqual(
    relayed(page),
    [],
    "an arrival was spoken before the burst window closed, so a burst cannot be collected at all"
  );

  await speakRelay(page);
  const sent = relayed(page);
  assert.equal(sent.length, 1, `${sent.length} turns for one burst of three messages`);
  for (const content of ["starting the build", "tests are running", "one failure in the parser"]) {
    assert.match(sent[0].text, new RegExp(content), `${content} was dropped from the one turn`);
  }
  assert.match(
    sent[0].text,
    /these 3 new chat messages/,
    "a turn about three messages has to say there are three, or the agent reads them as one"
  );
});

test("switching the selector off during the burst window cancels what was held", async () => {
  // The window is short, but it is long enough for the reader to reach the button — and having
  // pressed Off, hearing the thing you just switched off is the worst reading of the control.
  const page = newPage();
  await startTalking(page);
  page.messages = [];
  await page.el("view-switch").click();
  await page.settle();
  const stream = page.stream();
  await setReadNew(page, "gist");

  await deliver(page, stream, sseMessage(message({ id: "1300", content: "held, then cancelled" })));
  await setReadNew(page, "off");
  await speakRelay(page);

  assert.deepStrictEqual(relayed(page), [], "the held burst was spoken after the reader said no");
});

test("the bar button cycles the three modes and both controls agree about which one", async () => {
  // Two controls over one value, which is the failure this is here to catch: a reader who sets the
  // select and then looks at the bar must not see the old mode looking back at them.
  assertMarkupContains("bar-pack", "read-new");
  const page = newPage();
  await startTalking(page);

  // `#126 read-new-selector`. The default is mode 2, at the owner's request, so this is what an
  // untouched page is doing.
  assert.equal(page.el("read-new").getAttribute("data-mode"), "gist");
  assert.equal(page.el("relay-to-agent").value, "gist");

  await page.el("read-new").click();
  assert.equal(page.el("read-new").getAttribute("data-mode"), "full");
  assert.equal(page.el("relay-to-agent").value, "full", "the select did not follow the button");

  await page.el("read-new").click();
  assert.equal(page.el("read-new").getAttribute("data-mode"), "off", "the cycle does not wrap");
  assert.equal(page.el("relay-to-agent").value, "off");

  await page.el("read-new").click();
  assert.equal(page.el("read-new").getAttribute("data-mode"), "gist");

  await setReadNew(page, "full");
  assert.equal(
    page.el("read-new").getAttribute("data-mode"),
    "full",
    "the button did not follow the select"
  );

  // The label and the tooltip both change, because a three-state button whose face is constant is
  // a button whose state cannot be read.
  const faces = new Set();
  for (const _ of [0, 1, 2]) {
    faces.add(`${page.el("read-new-label").text()}|${page.el("read-new").getAttribute("title")}`);
    await page.el("read-new").click();
  }
  assert.equal(faces.size, 3, "two of the three modes look identical on the bar");
  assert.match(
    page.el("read-new").getAttribute("aria-label") || "",
    /new messages/i,
    "a screen reader is told the tooltip's next action and not the state it is in"
  );
});

test("the mode outlives a reload, and an earlier explicit refusal is not overturned", async () => {
  // The key is SHARED with the boolean this replaced, on purpose. "off" is still a mode, so a
  // reader who had turned the old switch off stays off; moving to a new key would have read as
  // "never chose anything" and the new default is ON, silently overturning a refusal. "on" is not
  // a mode any more and falls through to the default, which is the right answer rather than a
  // coincidence: "on" meant relay, and summarising is what relaying now means.
  const store = new Map();
  const page = newPage(store);
  await startTalking(page);
  await setReadNew(page, "full");
  assert.equal(
    newPage(store).el("relay-to-agent").value,
    "full",
    "the chosen mode did not survive a reload"
  );

  const stored = (value) => newPage(new Map([["vibe-talk.voice.relay", value]]));
  assert.equal(
    stored("off").el("read-new").getAttribute("data-mode"),
    "off",
    "a reader who had switched the old relay off was switched back on by the rename"
  );
  assert.equal(
    stored("on").el("read-new").getAttribute("data-mode"),
    "gist",
    "the old boolean 'on' has to land on a mode that speaks"
  );
  assert.equal(stored("loud").el("read-new").getAttribute("data-mode"), "gist");
});

test("a message this server posted is shown but never relayed back to the agent", async () => {
  // The feedback loop: `ops::reply` posts as the bot, the poller reads it back, and relaying it
  // would have the agent hear its own answer as news and answer it. That bills, in a loop.
  const page = newPage();
  await startTalking(page);
  page.messages = [];
  await page.el("view-switch").click();
  await page.settle();
  const stream = page.stream();
  await setReadNew(page, "gist");

  await deliver(
    page,
    stream,
    sseMessage(message({ id: "600", author: "vibe-talk", content: "posted on your behalf" }), {
      self_posted: true,
    })
  );

  assert.equal(
    [...page.el("discord-log").children].filter((li) => li.getAttribute("data-id") === "600")
      .length,
    1,
    "our own reply still belongs in the channel view — it is the RELAY that is suppressed"
  );
  await speakRelay(page);
  assert.deepStrictEqual(relayed(page), [], "the agent was told about its own reply");
});

test("THE REPLAY TAIL IS SHOWN BUT NEVER ANNOUNCED TO A LIVE CALL AS NEWS", async () => {
  // The burst. Every attach that carries no `Last-Event-ID` — a fresh sign-in, a channel change,
  // the reconnect after an `event: reset` — opens with the server's whole replay tail, up to two
  // hundred messages it published while this page was somewhere else. Rendering them is right.
  // Relaying them says "a message was just posted" about text that may be hours old, several
  // times in a row, into a conversation billed by the minute: existing history labelled as new,
  // which is the exact failure the server's seeding tick exists to prevent.
  const page = newPage();
  await startTalking(page);
  page.messages = [];
  await page.el("view-switch").click();
  await page.settle();
  const stream = page.stream();
  await setReadNew(page, "gist");

  for (const id of ["1000", "1001", "1002"]) {
    await deliver(
      page,
      stream,
      sseMessage(message({ id, content: `said while you were away ${id}` }), { replayed: true })
    );
  }

  assert.equal(
    page.el("discord-log").children.length,
    3,
    "the tail still belongs on screen — it is the RELAY that is suppressed, not the message"
  );
  await speakRelay(page);
  assert.deepStrictEqual(
    relayed(page),
    [],
    "a stale replay tail was spoken into a paid conversation as though it had just been said"
  );

  // The control, without which "relay nothing, ever" would pass this test.
  await deliver(
    page,
    stream,
    sseMessage(message({ id: "1003", content: "and this one really did just arrive" }))
  );
  await speakRelay(page);
  const sent = relayed(page);
  assert.equal(sent.length, 1, "a live arrival must still reach the agent");
  assert.match(sent[0].text, /really did just arrive/);
});

test("with no call in progress an arriving message is rendered and relayed to nobody", async () => {
  const page = newPage();
  const stream = await withLiveChannel(page, []);
  await setReadNew(page, "gist");

  await deliver(page, stream, sseMessage(message({ id: "700", content: "nobody is listening" })));

  assert.equal(page.sockets.length, 0, "there was no conversation to relay into");
  assert.equal(page.el("discord-log").children.length, 1, "and it still has to be on screen");
});

test("a dropped stream reconnects, and resumes from the last id it saw", async () => {
  const page = newPage();
  const stream = await withLiveChannel(page, []);
  await deliver(page, stream, sseMessage(message({ id: "800", content: "before the drop" })));

  stream.drop();
  await page.settle();
  assert.equal(page.streamOpens.length, 1, "it reconnected without waiting at all");

  assert.ok(page.expireTimers(sourceConstant("LIVE_RETRY_MS")) > 0, "no retry was scheduled");
  await page.settle();

  assert.equal(page.streamOpens.length, 2, "a dropped stream died silently instead of retrying");
  assert.equal(
    page.streamOpens[1].lastEventId,
    "800",
    "the reconnect must say where it got to, or the server has to guess"
  );
  const resumed = page.stream();
  await deliver(page, resumed, sseMessage(message({ id: "801", content: "after the drop" })));
  assert.match(page.el("discord-log").text(), /after the drop/);
});

test("an `event: reset` re-reads the channel instead of resuming short", async () => {
  // The server sends this when a subscriber fell further behind than its replay tail, so there IS
  // a gap and the stream cannot fill it. Carrying on would leave a hole the page cannot see.
  const page = newPage();
  const stream = await withLiveChannel(page, [message({ id: "900", content: "one" })]);
  await deliver(page, stream, sseMessage(message({ id: "901", content: "two" })));
  let reReads = 0;
  const served = page.channelPage;
  page.channelPage = async (path, options) => {
    reReads += 1;
    return served(path, options);
  };

  await deliver(page, stream, 'event: reset\ndata: {"missed":9}\n\n');

  assert.equal(reReads, 1, "a reset must send the page back to /page for the whole window");
  stream.drop();
  await page.settle();
  page.expireTimers(sourceConstant("LIVE_RETRY_MS"));
  await page.settle();
  assert.equal(
    page.streamOpens[1].lastEventId,
    null,
    "resuming from an id on the far side of a gap asks the server to replay what it just said it " +
      "cannot"
  );
});

test("a keep-alive comment is not mistaken for a message", async () => {
  const page = newPage();
  const stream = await withLiveChannel(page, []);
  await deliver(page, stream, ":\n\n");
  await deliver(page, stream, ": ping\n\n");
  assert.equal(page.el("discord-log").children.length, 0, "a keep-alive became a row");
});

test("a frame split across two chunks is still one message", async () => {
  // A chunk boundary falls wherever the network puts it, not where a message ends. This is the
  // failure that only shows up against a real socket and never against a canned array.
  const page = newPage();
  const stream = await withLiveChannel(page, []);
  const frame = sseMessage(message({ id: "1000", content: "split down the middle" }));
  const cut = Math.floor(frame.length / 2);

  await deliver(page, stream, frame.slice(0, cut));
  assert.equal(page.el("discord-log").children.length, 0, "half a frame became a row");
  await deliver(page, stream, frame.slice(cut));

  assert.equal(page.el("discord-log").children.length, 1);
  assert.match(page.el("discord-log").text(), /split down the middle/);
});

test("the stream carries its credential in a header, never in the URL", async () => {
  // `EventSource` cannot send a header, and the usual workaround is a token in the query string —
  // which puts a bearer credential in something every proxy logs and the browser keeps in history.
  // That is exactly what `redact()` and `/api/v1/signed-url`'s `no-store` exist to prevent.
  const page = newPage();
  await withLiveChannel(page, []);
  const opened = page.streamOpens[0];
  assert.match(opened.authorization || "", /^Bearer write-token-/);
  assert.doesNotMatch(opened.path, /token|authorization|bearer/i, opened.path);
  assert.ok(!opened.path.includes(page.storage.get("vibe-talk.token")), opened.path);
  assert.doesNotMatch(
    PAGE_CODE,
    /new EventSource/,
    "EventSource cannot carry the bearer header; see the comment above openChannelStream"
  );
});

test("signing out stops the stream instead of leaving a credential open", async () => {
  const page = newPage();
  const stream = await withLiveChannel(page, []);

  await page.el("forget-token").click();
  stream.drop();
  await page.settle();
  page.expireTimers(sourceConstant("LIVE_RETRY_MS"));
  await page.settle();

  assert.equal(
    page.streamOpens.length,
    1,
    "a signed-out page reconnected to a channel stream with a token it no longer has"
  );
});

test("Settings tells the truth about live updates in all three of its states", async () => {
  // OFF on the server, ON and attached, and ON but not attached are three different problems with
  // three different fixes, and all three look identical from the channel view: a list that is not
  // changing.
  const off = newPage();
  off.livePollSeconds = 0;
  off.liveDelivery = "off";
  await signIn(off);
  assert.match(off.el("live-state").textContent, /OFF on this server/);
  assert.equal(off.streamOpens.length, 0, "nothing should attach when the server is not watching");

  const on = newPage();
  await withLiveChannel(on, []);
  assert.match(on.el("live-state").textContent, /every 30 seconds/);
  assert.doesNotMatch(on.el("live-state").textContent, /not connected/);

  on.stream().drop();
  await on.settle();
  assert.match(
    on.el("live-state").textContent,
    /not connected to the stream/,
    "a page whose stream has dropped must not keep claiming it is live"
  );

  const pushed = newPage();
  pushed.livePollSeconds = 0;
  pushed.liveDelivery = "push";
  await signIn(pushed);
  assert.equal(pushed.streamOpens.length, 1, "push-only delivery did not attach the SSE stream");
  assert.match(pushed.el("live-state").textContent, /by adapter push/);
  // `#332 settings-voice-agent-section` shortened the line, and kept this clause in it: "listening"
  // alone would read as a statement that the adapter is healthy, which nothing here can know.
  assert.match(pushed.el("live-state").textContent, /adapter's health is not checked/);
  assert.doesNotMatch(pushed.el("live-state").textContent, /every 0 seconds/);
  pushed.stream().drop();
  await pushed.settle();
  assert.match(pushed.el("live-state").textContent, /by adapter push, but not connected right now/,
    "a push page whose stream has dropped must not keep claiming it is listening");
});

test("Settings says plainly what the relay costs and what it cannot do", () => {
  // `#85 voice-desktop-review` moved the PARAGRAPHS to their own screen and left the CONTROL here.
  // The disclosure is therefore asserted in two halves, and the second half is what keeps the move
  // honest: the prose still exists, and the `?` beside this control is what reaches it. A test that
  // only looked at the help screen would pass just as well if nothing linked to it.
  // `#347 settings-prose` named the group for its control, and made its one-line summary the tooltip of
  // its title and its `?`, which is where "voice vendor" and "by default" are read from now.
  const settings = settingsGroup("Arriving messages during a call");
  assert.match(settings, /id="relay-to-agent"/);
  // ALL THREE modes are named on the screen, and none of them is `selected` in the markup. The
  // live value comes out of storage, so a markup default would be a second opinion about the mode
  // that disagrees with the real one after a reload.
  for (const mode of ["off", "gist", "full"]) {
    assert.match(
      settings,
      new RegExp(`value="${mode}"`),
      `the ${mode} mode has no option the reader can choose`
    );
  }
  assert.ok(
    !/<option[^>]*\bselected\b/.test(settings.slice(settings.indexOf('id="relay-to-agent"'))),
    "the markup states a mode of its own, which is a second truth about a stored setting"
  );
  assert.match(
    settings,
    /data-help="live-messages"/,
    "the group offers no way to reach what it costs"
  );
  assert.match(settings, /voice vendor/, "the cost is not even summarised beside the control");
  // THE DEFAULT IS NOW ON, at the owner's request, which reverses what this screen used to say. A
  // setting that spends money without being asked for on the day has to be legible as the default
  // rather than discovered from a bill.
  assert.match(
    settings,
    /by default/,
    "the screen no longer says which of the three modes an untouched page is in"
  );

  const block = helpEntry("live-messages");
  assert.match(block, /voice vendor/, "where the channel text goes has to be said, not implied");
  assert.match(
    block,
    /one<\/strong> spoken turn/,
    "a burst becoming one turn rather than one each is behaviour the reader will notice, so the " +
      "screen has to account for the pause before the agent speaks"
  );
  assert.match(
    block,
    /while this page is open/,
    "the socket lives in this browser, so closing the tab ends the relay; the screen must say so"
  );
  assert.match(
    block,
    /polling or push\s+delivery/,
    "the two live delivery modes are not explained"
  );
  assert.match(
    block,
    /catch-up is never announced/,
    "the reader has to be told that a message which landed while the feed was reconnecting is on " +
      "screen but was not spoken, or the silence reads as the relay being broken"
  );
});

test("the relay is cut to a budget rather than sending a whole essay into a call", async () => {
  const page = newPage();
  await startTalking(page);
  page.messages = [];
  await page.el("view-switch").click();
  await page.settle();
  await setReadNew(page, "gist");

  const long = "a very long sentence about the overnight run. ".repeat(60);
  await deliver(page, page.stream(), sseMessage(message({ id: "1100", content: long })));

  await speakRelay(page);
  const sent = relayed(page);
  assert.equal(sent.length, 1);
  const budget = sourceConstant("RELAY_MAX_CHARS");
  // The quoted line only, not the rest of the turn: the task sentence and the fences are this
  // page's own words and are not what the budget is about.
  const quoted = sent[0].text
    .split("\n")
    .find((line) => line.includes("said: "))
    .split("said: ")[1];
  assert.ok(quoted.length > 0, `the quoted body is missing: ${sent[0].text}`);
  assert.ok(
    quoted.length <= budget,
    `${quoted.length} characters of a ${long.length}-character message were relayed, which is ` +
      `not a budget of ${budget}`
  );
  assert.match(quoted, /…$/, "a cut line has to look cut");
});

// --- `#281 view-keys` with `#280 scroll-stability` --------------------------------------------------

test("`#281 view-keys` x `#280 scroll-stability`: Ctrl+1 to Ctrl+4 show each view where it is read — the thread list at its foot, Main where the reader left it — and nothing moves it after, the picker's new name wrapping the dock included", async () => {
  // In Chromium (tests/scroll_stability_browser.py) the view picker is as wide as what it says, and at a
  // desk "Thread list" in place of "Main" wrapped the dock onto a second row after the cards had been
  // opened at their foot: the thread list opened 43px above it, and nothing took it the rest of the way.
  // This fixture lays nothing out, so the test does what the browser did: a picker saying the thread list
  // or two columns takes a second row, GROWTH px, from the foot of both columns' lists, at once, and what
  // watches them is told once the page has done (`settled`), as a browser reports sizes after a layout.
  const GROWTH = 42;
  /** The picker as wide as what it says, from the page's first line: saying the thread list or two columns
   * wraps the dock, which takes GROWTH px from the foot of both lists. */
  const wrapDock = (p) => {
    const picker = p.el("thread-select");
    const lay = (value) => {
      const growth = value === "threads" || value === "columns" ? GROWTH : 0;
      p.el("dock").offsetHeight = 50 + growth;
      p.el("scroll-area").clientHeight = SCROLL_VIEWPORT_PX - growth;
      p.el("side-scroll").clientHeight = SCROLL_VIEWPORT_PX - growth;
    };
    let said = String(picker.value);
    lay(said);
    Object.defineProperty(picker, "value", {
      configurable: true,
      get: () => said,
      set: (value) => {
        said = String(value);
        lay(said);
      },
    });
  };
  const run = async (script) => {
    const { page } = await manyThreadColumns(16, (p) => {
      p.enableResizeObserver();
      wrapDock(p);
    }, script);
    const picker = page.el("thread-select");
    const main = page.el("scroll-area");
    const side = page.el("side-scroll");
    const settled = async () => {
      for (const box of [main, side, ...main.children, ...side.children]) page.resized(box);
      page.expireTimers(0);
      // The glide's wait, were anything waiting: a view key arms none.
      page.expireTimers(sourceConstant("COLUMN_GLIDE_MS"));
      await page.settle();
    };
    const key = async (digit) => {
      await press(page, focused(page) || main, digit, { ctrlKey: true });
      await page.settle();
    };
    await settled();
    assert.ok(sideAtFoot(page), "the premise: two columns opened their cards at the foot");
    await key("1");
    await settled();
    // Up Main, one column: the thread list over it, and back.
    main.scrollTop = Math.round(main.scrollHeight / 3);
    await main.dispatch("scroll");
    const kept = readerOn(page);
    assert.ok(!atBottomOf(main), "the premise: the reader is up Main");
    await key("3");
    assert.equal(page.el("screen-main").getAttribute("data-columns"), "list", "Ctrl+3 did not show the thread list");
    const listFoot = sideAtFoot(page);
    await settled();
    const listFootAfter = sideAtFoot(page);
    // A card drawn taller with nothing redrawn, under a reader at the foot of the thread list.
    const card = page.el("side-cards").children[page.el("side-cards").children.length - 1];
    card.style.height = "160px";
    page.resized(page.el("side-list"));
    await page.settle();
    const cardGrewFoot = sideAtFoot(page);
    card.style.height = "";
    page.resized(page.el("side-list"));
    await settled();
    await key("1");
    const mainKept = JSON.stringify(readerOn(page)) === JSON.stringify(kept);
    await settled();
    const mainKeptAfter = JSON.stringify(readerOn(page)) === JSON.stringify(kept);
    await key("2");
    assert.equal(picker.value, "flat", "Ctrl+2 did not show All");
    const all = readerOn(page);
    await settled();
    const allHeld = JSON.stringify(readerOn(page)) === JSON.stringify(all);
    await key("3");
    const listFromAll = sideAtFoot(page);
    await settled();
    await key("4");
    assert.ok(inTwoColumns(page), "Ctrl+4 did not bring two columns");
    const cardsFoot = sideAtFoot(page);
    const left = readerOn(page);
    await settled();
    const twoHeld = sideAtFoot(page) && JSON.stringify(readerOn(page)) === JSON.stringify(left);
    return { listFoot, listFootAfter, cardGrewFoot, mainKept, mainKeptAfter, allHeld, listFromAll, cardsFoot, twoHeld };
  };
  const real = await run(SCRIPT);
  assert.deepStrictEqual(real, {
    listFoot: true, listFootAfter: true, cardGrewFoot: true, mainKept: true, mainKeptAfter: true, allHeld: true,
    listFromAll: true, cardsFoot: true, twoHeld: true,
  });

  // THE CONTROLS. The cards placed before the picker says the thread list, as before: from Main, whose left
  // column has no view to change and so draws no picker before the cards are placed, the dock's second row
  // leaves the thread list above its foot as the key draws it. Once the browser reports the sizes, the right
  // column's dock-growth keeper takes it the rest of the way (the next test), so what the placement after
  // the picker spares is the list drawn off its foot and then moved. (From All the left column goes to Main
  // behind the list first, which draws the picker before the cards are placed.)
  const placedEarly = await run(brokenScript(
    "  } else {\n    // Where the card the list's place was on has gone since, the place two columns had.\n" +
    "    placeCardAnchor(place, true);\n  }\n",
    "  }\n"));
  assert.deepStrictEqual(placedEarly, { ...real, listFoot: false });
  // The right column's reader kept on its foot in two columns only: a card growing under a reader at the
  // foot of the thread list leaves them above it.
  const twoOnly = await run(brokenScript(
    "if (!sideColumnUp() || currentScreen !== \"main\" || currentView !== \"discord\") return false;",
    "if (!columnsShown || currentScreen !== \"main\" || currentView !== \"discord\") return false;"));
  assert.deepStrictEqual(twoOnly, { ...real, cardGrewFoot: false });
});

test("`#281 view-keys` x `#280 scroll-stability`: the dock growing under the cards after they were placed — its last button coming in after a reload, a channel's longer name — keeps a reader at their foot on it, in the thread list and beside Main, and moves nobody else", async () => {
  // In Chromium (tests/scroll_stability_browser.py) a reload in the thread list at a desk put the cards at
  // their foot, and about 30 ms later the dock's last button came in. In the thread list the dock is only as
  // wide as the reading column, so that wrapped it onto a second row, 42px taken from the foot of the cards,
  // and the list stayed 43px above its foot: the right column's dock-growth keeper (`#229`) kept a thread on
  // the right and returned for the cards, and the cards' own foot keeper is for their content, not the
  // column. The fixture lays nothing out, so the test does what the browser did: the dock is the new height,
  // the cards' column has lost that much, and whatever watches it is told, with nothing redrawn.
  const GROWTH = 42;
  const run = async (script) => {
    const { page } = await manyThreadColumns(16, (p) => p.enableResizeObserver(), script);
    const dock = page.el("dock");
    const side = page.el("side-scroll");
    const grow = (by) => {
      dock.offsetHeight += by;
      side.clientHeight -= by;
      page.resized(side);
    };
    const scrollSide = async (top) => {
      side.scrollTop = top;
      await side.dispatch("scroll");
    };
    const footGap = () => side.scrollHeight - side.clientHeight - side.scrollTop;
    dock.offsetHeight = 50;
    side.clientHeight = SCROLL_VIEWPORT_PX;
    await press(page, focused(page) || page.el("scroll-area"), "3", { ctrlKey: true });
    await page.settle();
    assert.equal(page.el("screen-main").getAttribute("data-columns"), "list", "Ctrl+3 did not show the thread list");
    // The first report has nothing before it: the dock and the column as the page has them.
    page.resized(side);
    assert.ok(sideAtFoot(page), "the premise: the thread list opened at its foot");
    assert.ok(side.scrollHeight > side.clientHeight + 200, "the premise: the cards run past the column's foot");
    grow(GROWTH);
    const listFoot = sideAtFoot(page);
    // Within the slack of the foot, the dock growing again: taken the rest of the way.
    await scrollSide(side.scrollHeight - side.clientHeight - (BOTTOM_SLACK_PX - 10));
    grow(20);
    const nearFoot = sideAtFoot(page);
    // Up the cards: the reader stays on the card they were reading.
    await scrollSide(100);
    grow(20);
    const upHeld = side.scrollTop === 100;
    // A WINDOW losing height is not the dock growing, and leaves a reader at the foot where they are (`#207`).
    await scrollSide(side.scrollHeight);
    side.clientHeight -= GROWTH;
    page.resized(side);
    const windowLeft = footGap() === GROWTH;
    // Beside Main the same: a channel's longer name wrapping the dock under a reader at the foot of the cards.
    await press(page, focused(page) || page.el("scroll-area"), "4", { ctrlKey: true });
    await page.settle();
    assert.ok(inTwoColumns(page), "Ctrl+4 did not bring two columns");
    page.resized(side);
    await scrollSide(side.scrollHeight);
    assert.ok(sideAtFoot(page), "the premise: the cards beside Main are at their foot");
    grow(GROWTH);
    const twoFoot = sideAtFoot(page);
    return { listFoot, nearFoot, upHeld, windowLeft, twoFoot };
  };
  const real = await run(SCRIPT);
  assert.deepStrictEqual(real, { listFoot: true, nearFoot: true, upHeld: true, windowLeft: true, twoFoot: true });

  // THE CONTROL: the keeper for a thread on the right only, as before. The dock growing leaves the cards
  // above their foot, in the thread list and beside Main — further than the slack, so this is the defect and
  // not a rounding — and the reader up the cards and the window losing height come out the same.
  const threadOnly = await run(brokenScript(
    "    if (sideColumnUp() && currentView === \"discord\" && columns.side.mode === \"list\") {\n" +
    "      if (!cardsOpening && grew > 0 && atBottom(area, BOTTOM_SLACK_PX + grew)) followFoot(area);\n" +
    "      return;\n    }\n",
    ""));
  assert.ok(GROWTH > BOTTOM_SLACK_PX, "the premise: a second row is more than the slack");
  assert.deepStrictEqual(threadOnly, { ...real, listFoot: false, nearFoot: false, twoFoot: false });
});

// --- `#338 column-overscroll`: room past the right column's foot -------------------------------------
//
// The owner, 2026-10-15: at a desk the wheel and the page keys go on past the foot of the right column into
// blank room, until the last card's top is at half the column's height. tests/column_overscroll_browser.py
// walks it in Chromium, where the wheel, the browser's clamp at the end of a scroller and frames are real.
// These hold the page's rules for it one at a time in the model above, which lays out heights and clamps
// a position set past the end, but scrolls nothing by itself: where a browser would scroll for a key, the
// test does, after the page has had the key.

/**
 * `#338 column-overscroll`. The one layout question the page asks of the room: whether web/voice.css gives
 * it a box, which it does at a desk alone, the right column drawn. Left out of the ordinary fixture, so the
 * rest of the suite runs with the room never offered, as on a phone.
 */
const drawOverscroll = (page) => {
  page.el("side-overscroll").getClientRects = () =>
    (page.wideDesk && page.el("side-column").getClientRects().length > 0 ? [{ top: 0, bottom: 0 }] : []);
};

/** The room's height as the page gave it, in px: 0 while it is closed. */
const overscrollPx = (page) => Number.parseFloat(page.el("side-overscroll").style.height || "0") || 0;

test("`#338 column-overscroll`: the room past the right column's foot is drawn at a desk alone, is the last thing in its scroller, is hidden from assistive technology, and is watched by neither observer of its neighbours", async () => {
  assert.match(cssBlockIn(DESKTOP_QUERY, "#side-scroll > #side-overscroll"), /display:\s*block/,
    "the desktop block does not draw the room");
  assert.equal(cssBlockOutside(DESKTOP_QUERY, "#side-scroll > #side-overscroll"), "",
    "the room is drawn outside the desktop block too");
  assert.match(cssRules(CSS_UNCONDITIONAL, "#side-overscroll").join("\n"), /display:\s*none/,
    "the room is not taken away everywhere else");
  // The last child of #side-scroll but the room a packed card holds (`#459 pack-keeps-card-still`), by the
  // fixture's tree, which is checked against the markup as it is built: the scroller closes after them, and
  // the column's jump follows.
  assert.match(HTML_CODE, /<div id="side-overscroll" aria-hidden="true"><\/div>\s*(?:<!--[\s\S]*?-->\s*)?<div id="side-overextend" aria-hidden="true"><\/div>\s*<\/div>\s*<div id="side-tools"/,
    "the room is not the last thing in the right column's scroller, or is not hidden from assistive technology");
  const { page } = await manyThreadColumns(4, (p) => p.enableResizeObserver());
  const room = page.el("side-overscroll");
  const watching = page.resizeObservers.filter((observer) => observer.targets.includes(page.el("side-list")));
  assert.ok(watching.length >= 2, "the premise: the right column's rows are watched by its jump's observer and its foot keeper");
  assert.ok(watching.every((observer) => !observer.targets.includes(room)), "an observer of the rows watches the room");
});

test("`#338 column-overscroll`: at a desk PgDn or Space at the foot of the right column's cards opens room past it, as far as brings the last card's top to the middle; in it the reader is at the newest; End, the Newest chip, going back up past the foot and resting at the foot close it; and a held key, a key up the list, or a column with no box for it open none", async () => {
  const IDLE_MS = sourceConstant("OVERSCROLL_IDLE_MS");
  const run = async (script) => {
    const { page } = await manyThreadColumns(16, drawOverscroll, script);
    const side = page.el("side-scroll");
    const room = page.el("side-overscroll");
    const last = () => page.el("side-cards").children.at(-1);
    const scrollSide = async (top) => {
      side.scrollTop = top;
      await side.dispatch("scroll");
      await page.settle();
    };
    // A key with the focus on the column, and the scroll a browser makes for it, to the end of what it can.
    const pageKey = async (key, fields = {}) => {
      await press(page, side, key, fields);
      if (!fields.repeat && (key === "PageDown" || key === " ")) await scrollSide(side.scrollHeight);
    };
    assert.ok(sideAtFoot(page), "the premise: the cards opened at their foot");
    assert.ok(last().getBoundingClientRect().top > SCROLL_VIEWPORT_PX / 2 + LINE_PX,
      "the premise: the last card's top is below the middle at the foot");
    const out = {};
    await pageKey("PageDown");
    out.opened = overscrollPx(page) > 0;
    out.middle = last().getBoundingClientRect().top;
    // A little way into the room, further from its end than the slack: still at the newest (the jump over a
    // thread on the right, which reads the same measure, is tests/column_overscroll_browser.py's JUMP).
    await scrollSide(side.scrollHeight - overscrollPx(page) - side.clientHeight + 10);
    out.inRoom = overscrollPx(page) - 10 > BOTTOM_SLACK_PX;
    out.atNewest = page.pure("atBottom")(side);
    await press(page, side, "End");
    out.endClosed = overscrollPx(page) === 0 && sideAtFoot(page);
    await pageKey(" ");
    out.spaceOpened = overscrollPx(page) > 0;
    await page.el("side-jump-newest").click();
    await page.settle();
    out.chipClosed = overscrollPx(page) === 0 && sideAtFoot(page);
    await pageKey("PageDown");
    await scrollSide(side.scrollHeight - overscrollPx(page) - side.clientHeight - 50);
    out.backClosed = overscrollPx(page) === 0;
    await scrollSide(side.scrollHeight);
    await pageKey("PageDown", { repeat: true });
    out.heldOpened = overscrollPx(page) > 0;
    await scrollSide(100);
    await pageKey("PageDown");
    out.upOpened = overscrollPx(page) > 0;
    await scrollSide(side.scrollHeight);
    const drawn = room.getClientRects;
    room.getClientRects = () => [];
    await pageKey("PageDown");
    out.noBoxOpened = overscrollPx(page) > 0;
    room.getClientRects = drawn;
    // Opened at the foot and not gone into: closed once the reader has rested there.
    await press(page, side, "PageDown");
    const waiting = overscrollPx(page) > 0;
    page.expireTimers(IDLE_MS);
    out.idleClosed = waiting && overscrollPx(page) === 0 && sideAtFoot(page);
    return out;
  };
  const real = await run(SCRIPT);
  assert.deepStrictEqual(real, {
    opened: true, middle: SCROLL_VIEWPORT_PX / 2, inRoom: true, atNewest: true, endClosed: true,
    spaceOpened: true, chipClosed: true, backClosed: true, heldOpened: false, upOpened: false, noBoxOpened: false,
    idleClosed: true,
  });

  // THE CONTROLS. The room not counted as the foot: in it the reader is up the list.
  const notFoot = await run(brokenScript(
    "  return area.scrollHeight - room - area.scrollTop - (area.clientHeight || 0);\n",
    "  return area.scrollHeight - area.scrollTop - (area.clientHeight || 0);\n"));
  assert.deepStrictEqual(notFoot, { ...real, atNewest: false });
  // End and the chip landing where the room already is, rather than closing it: the reader stays in it.
  const kept = await run(brokenScript(
    "  if (area === leftEl(\"side-scroll\")) closeOverscroll();\n  area.scrollTop = area.scrollHeight;\n}",
    "  area.scrollTop = area.scrollHeight;\n}").replace(
    "  if (el(\"scroll-area\") === leftEl(\"side-scroll\")) closeOverscroll();\n", "").replace(
    "  el(\"jump-newest\").addEventListener(\"click\", inThisColumn(() => {\n    closeOverscroll();\n",
    "  el(\"jump-newest\").addEventListener(\"click\", inThisColumn(() => {\n"));
  assert.deepStrictEqual(kept, { ...real, endClosed: false, chipClosed: false });
});

test("`#338 column-overscroll` x `#280 scroll-stability`: THE LAST CARD SHRINKING under a reader at the end of the room past the right column's foot, with nothing scrolled, is fitted to as its size is reported and left where the reader had it, its top at the middle; and the room closes once the cards left are too short to scroll past their foot even with it", async () => {
  const run = async (script) => {
    const { page } = await manyThreadColumns(16, (p) => {
      p.enableResizeObserver();
      drawOverscroll(p);
    }, script);
    const side = page.el("side-scroll");
    const last = page.el("side-cards").children.at(-1);
    // As a browser lays out a change of size: the position clamped to the new end first, then the sizes told.
    const laidOut = () => {
      side.scrollTop = side.scrollTop;
      page.resized(page.el("side-list"));
      page.expireTimers(0);
    };
    await press(page, side, "PageDown");
    side.scrollTop = side.scrollHeight;
    await side.dispatch("scroll");
    const middle = last.getBoundingClientRect().top;
    assert.equal(middle, SCROLL_VIEWPORT_PX / 2, "the premise: PgDn and the scroll for it left the last card's top at the middle");
    const tall = last.height();
    last.style.height = `${tall - 2 * LINE_PX}px`;
    side.scrollTop = side.scrollTop;
    const dropped = last.getBoundingClientRect().top - middle;
    page.resized(page.el("side-list"));
    page.expireTimers(0);
    const held = last.getBoundingClientRect().top - middle;
    const atEnd = side.scrollTop === side.scrollHeight - side.clientHeight;
    // Every card but the last two gone, as a redraw could leave them: the two and the room fit in the column.
    for (const row of page.el("side-cards").children.slice(0, -2)) row.hidden = true;
    laidOut();
    const shortClosed = overscrollPx(page) === 0;
    return { dropped, held, atEnd, shortClosed };
  };
  const real = await run(SCRIPT);
  assert.deepStrictEqual(real, { dropped: 2 * LINE_PX, held: 0, atEnd: true, shortClosed: true });

  // THE CONTROLS. The foot keeper not holding the reader in the room: the room is fitted only after, and the
  // card is left where the browser's clamp dropped it, the room now longer than where the reader is.
  const unheld = await run(brokenScript("    holdOverscrollReader(area);\n    const was = footing;\n",
    "    const was = footing;\n"));
  assert.deepStrictEqual(unheld, { ...real, held: 2 * LINE_PX, atEnd: false });
  // The room kept whatever the list's length: blank room left under two cards that do not scroll.
  const kept = await run(brokenScript(
    "  if (area.scrollHeight - (area.clientHeight || 0) > footTop(area) + 1) return true;\n  closeOverscroll();\n  return false;\n",
    "  return true;\n"));
  assert.deepStrictEqual(kept, { ...real, shortClosed: false });
});

// Found in the review of the above (2026-10-19): two of the composer's keepers (`#340
// composer-grow-box`) still put a list at its foot with `scrollTop = scrollHeight`. In the right
// column that now reaches to the end of the room, and `atBottom` counts a reader anywhere in the
// room as at the foot, so ticking Grow, or the side box growing under the reader's typing, took a
// reader part-way into the room to its end. Both now keep the reader by `followFoot`, as the other
// followers do.

test("`#338 column-overscroll` x `#340 composer-grow-box`: A READER PART-WAY INTO the room past the right column's foot is not moved by the side box growing under their typing, nor by Grow unticked; both keepers leave them where they are, the room open", async () => {
  const run = async (script) => {
    const { page } = await manyThreadColumns(16, (p) => {
      p.enableResizeObserver();
      drawOverscroll(p);
    }, script);
    const side = page.el("side-scroll");
    const box = page.el("side-compose-text");
    const scrollSide = async (top) => {
      side.scrollTop = top;
      await side.dispatch("scroll");
      await page.settle();
    };
    let boxPx = 80;
    box.height = () => boxPx;
    page.el("side-compose-grow").getClientRects = () => [{ top: 0, bottom: 24 }];
    await page.el("side-compose-grow").setChecked(true);
    box.focus();
    page.resized(box);
    // Into the room, and only part-way: 30px past the foot of a room far deeper than that.
    await press(page, side, "PageDown");
    await scrollSide(side.scrollHeight);
    const room = overscrollPx(page);
    assert.ok(room > 80, `the premise: the room is deep enough to be part-way into, ${room}px`);
    const foot = side.scrollHeight - room - side.clientHeight;
    await scrollSide(foot + 30);
    const place = side.scrollTop;
    assert.equal(page.pure("atBottom")(side), true, "the premise: in the room the reader counts as at the foot");
    // The box grows under their typing, its size reported as a browser reports it. (The focus is
    // theirs again: the page keys above took it to the list, and the keeper acts only on the box typed in.)
    box.focus();
    boxPx = 200;
    page.resized(box);
    await page.settle();
    const grewMoved = side.scrollTop - place;
    // Grow unticked, in either column: every list at its foot is put back at it.
    await page.el("side-compose-grow").setChecked(false);
    await page.settle();
    const untickMoved = side.scrollTop - place;
    return { grewMoved, untickMoved, roomOpen: overscrollPx(page) > 0 };
  };
  const real = await run(SCRIPT);
  assert.deepStrictEqual(real, { grewMoved: 0, untickMoved: 0, roomOpen: true });

  // THE CONTROLS. Each keeper putting the list at `scrollHeight` again, as both did: the reader is
  // taken on to the end of the room by that keeper alone.
  const grewJump = await run(await brokenModule([{ file: "src/view/composer/grow.ts",
    find: "      if (grew > 0 && deps.atBottom(area, deps.bottomSlack + grew)) {\n        deps.followFoot(area);\n",
    replace: "      if (grew > 0 && deps.atBottom(area, deps.bottomSlack + grew)) {\n        area.scrollTop = area.scrollHeight;\n" }]));
  assert.ok(grewJump.grewMoved > 1, `with the growing box's keeper at scrollHeight the reader was not moved, so this test cannot see it: ${JSON.stringify(grewJump)}`);
  const untickJump = await run(await brokenModule([{ file: "src/view/composer/grow.ts",
    find: "      for (const area of footed) deps.followFoot(area);\n",
    replace: "      for (const area of footed) area.scrollTop = area.scrollHeight;\n" }]));
  assert.equal(untickJump.grewMoved, 0, "the premise: the growing box's keeper still holds in this control");
  assert.ok(untickJump.untickMoved > 1, `with Grow's keeper at scrollHeight the reader was not moved, so this test cannot see it: ${JSON.stringify(untickJump)}`);
});
