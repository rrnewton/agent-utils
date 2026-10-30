// The app page's tests: what a call proposes to post: the approval card, its markers, the spoken yes and the recap.
//
// One feature's file of the suite tests/js/voice_page.test.mjs runs, against the fixture in
// ./fixture.mjs. Run with the rest of the suite, or alone: node --test tests/js/page/call-sending.mjs.

import assert from "node:assert/strict";

import { CAPTURE_RESUME_MS, CHANNEL, CLEAR_ARMED_MS, DEFAULT_POST_PREFIX, HTML, HTML_CODE, MINTED,
  OTHER_CHANNEL, PAGE_CODE, PAGE_ELEMENTS, RECORD_EPOCH, REDRAFT_WAIT_MS, REPLAY_PREAMBLE, SCRIPT,
  SEAM_DETAIL_MAX_WORDS, SEAM_DETAIL_MIN_WORDS, SETTINGS, SHIPPED_SYSTEM_PROMPT, SPOKEN_SETTLE_MS,
  TRANSCRIPT_PAGE_LIMIT, assistantSays, backFacts, boundaryLines, boundaryWords, brokenScript,
  compose, cssBlock, cssBlockIn, deliver, fillTranscript, framesOfType, framesSent, helpEntry,
  hosted, hostedAudio, hostedHears, hostedSays, initiationFrame, json, longMessage, message,
  neutralReadBack, newPage, openReplyOn, optionalSourceConstant, promptsSent, proposal,
  proposedDuringACall, readBackDuringACall, recordingCall, relayed, reloadOf, resumeFrame,
  rowWithId, seamChip, seamDetail, seamLabel, seamRecapDetail, sentCards, session_lines, setReadNew,
  settingsGroup, settleRedraftWait, settleSpokenYes, showDiscord, signIn, sourceConstant,
  sourceStringTable, speakRelay, speech, spokenDraft, sseMessage, startChatting, startNeutralCall,
  startTalking, startTalkingNamed, state, statusText, test, threadData, turnDone, untilDeepLink,
  vendorAccepts, voicePostPrefixView, voicePromptView, watchTurns, withEarlierConversation, words,
  youSay } from "./fixture.mjs";

const proposalWatches = (page) => page.requests.filter((r) => r.startsWith("GET /api/v1/post-proposals"));

test("a proposed post is shown verbatim, and Send commits exactly that proposal, once, as the UI", async () => {
  const page = newPage();
  const pending = await proposedDuringACall(page);

  assert.equal(page.el("post-confirm-text").textContent, pending.text);
  // `#269 approval-card-inline`: what it answers in words, and never the id.
  assert.equal(page.el("post-confirm-where").textContent, "To lead team · as a reply to an earlier message");
  assert.match(page.el("post-confirm-expiry").textContent, /^Waiting for you · lapses in 120 s$/);
  assert.ok(!page.createdTags.includes("b"), "the proposed text became markup");
  assert.ok(!page.renderedText().includes(pending.handle), "the handle was put on screen");
  assert.deepEqual(page.proposalCommits, [], "something was committed before Send");

  await page.el("post-confirm-send").click();
  await page.settle();
  assert.deepEqual(page.proposalCommits, [{
    handle: pending.handle,
    channel_id: pending.channel_id,
    text: pending.text,
    reply_to: pending.reply_to,
    confirmed_by: "ui",
  }]);
  assert.equal(page.el("post-confirm").hidden, true, "the card outlived the post");
  assert.match(page.el("status").textContent, /Posted to lead team/);
  assert.equal(page.el("error").hidden, true, page.el("error").textContent);

  await page.el("post-confirm-send").click();
  await page.settle();
  assert.equal(page.proposalCommits.length, 1, "a second tap committed again");
});

test("Don't send withdraws the proposal and commits nothing", async () => {
  const page = newPage();
  const pending = await proposedDuringACall(page, proposal({ reply_to: null }));
  assert.equal(page.el("post-confirm-where").textContent, "To lead team");

  await page.el("post-confirm-cancel").click();
  await page.settle();
  assert.deepEqual(page.proposalCancels, [{ handle: pending.handle }]);
  assert.deepEqual(page.proposalCommits, []);
  assert.equal(page.el("post-confirm").hidden, true);
  assert.equal(page.proposal, null, "the server still holds the withdrawn proposal");
});

test("a refused confirmation takes the card away and says this tap posted nothing", async () => {
  for (const [status, code, said] of [
    [410, "proposal_expired", /lapsed before Send reached the server\. Nothing was posted/],
    [409, "proposal_superseded", /replaced that proposal.*Nothing was posted/],
    [409, "proposal_used", /already been settled.*This tap posted nothing/],
  ]) {
    const page = newPage();
    await proposedDuringACall(page);
    page.commitProposal = async () => {
      page.settleProposal();
      return json(status, { error: code, detail: "server sentence" });
    };
    await page.el("post-confirm-send").click();
    await page.settle();
    assert.equal(page.el("post-confirm").hidden, true, `${code} left a Send button up`);
    assert.equal(page.el("error").hidden, false, `${code} was not reported`);
    assert.match(page.el("error").textContent, said);
  }
});

test("a send that never reached the server keeps the card, because nothing was spent", async () => {
  const page = newPage();
  await proposedDuringACall(page);
  page.commitProposal = async () => { throw new TypeError("Failed to fetch"); };
  await page.el("post-confirm-send").click();
  await page.settle();
  assert.equal(page.el("post-confirm").hidden, false);
  assert.equal(page.el("post-confirm-send").disabled, false, "the retry was left disabled");
  assert.match(page.el("error").textContent, /Could not reach vibe-talk/);
});

test("a confirmed post that failed says nothing was posted, and never shows the token", async () => {
  for (const [posted, said] of [[0, /^Nothing was posted to lead team: /], [1, /^Only part of the message reached lead team: /]]) {
    const page = newPage();
    await proposedDuringACall(page);
    page.commitProposal = async () => {
      page.settleProposal();
      return json(207, {
        error: "partially_posted",
        detail: "chat request failed for write-token-aaaaaaaaaaaaaaaa",
        posted,
        unsent: "on it",
      });
    };
    await page.el("post-confirm-send").click();
    await page.settle();
    assert.equal(page.el("post-confirm").hidden, true);
    assert.match(page.el("error").textContent, said);
    assert.ok(!page.el("error").textContent.includes("write-token-aaaaaaaaaaaaaaaa"), "the token reached the screen");
  }
});

test("redrawing the card never extends the lapse the server set", async () => {
  const page = newPage();
  await proposedDuringACall(page, proposal({ expires_in_ms: 10000 }));
  page.setClock(page.clock() + 8000);
  page.commitProposal = async () => { throw new TypeError("Failed to fetch"); };
  await page.el("post-confirm-send").click();
  await page.settle();
  assert.equal(page.el("post-confirm").hidden, false);
  assert.match(page.el("post-confirm-expiry").textContent, /lapses in 2 s/);
});

test("a card that appears where none was shown holds Send too, without saying the text changed", async () => {
  // `#49 post-confirmation-card-hold`. A tap on its way to what was under that spot, a row or the
  // transcript, must not land on a Send the owner has not yet seen.
  const page = newPage();
  await startTalking(page);
  await page.settle();
  const pending = proposal();
  page.propose(pending);
  await page.settle();
  assert.equal(page.el("post-confirm").hidden, false, "the proposal never reached the card");
  assert.equal(page.el("post-confirm-send").disabled, true, "Send was live on the card's first frame");
  // `#269 approval-card-inline`: the card is the next message in the list now, where a tap aimed at
  // the last row may land on either button, so Don't send is held with Send.
  assert.equal(page.el("post-confirm-cancel").disabled, true, "Don't send was live on the card's first frame");
  assert.doesNotMatch(page.el("post-confirm-expiry").textContent, /Changed/, "a first card said it changed");

  await page.el("post-confirm-send").click();
  await page.el("post-confirm-cancel").click();
  await page.settle();
  assert.deepEqual(page.proposalCommits, [], "a tap during the hold committed the card");
  assert.deepEqual(page.proposalCancels, [], "a tap during the hold withdrew the card");

  assert.equal(page.expireTimers(2000), 1, "no hold was armed");
  assert.equal(page.el("post-confirm-send").disabled, false, "Send stayed held");
  assert.equal(page.el("post-confirm-cancel").disabled, false, "Don't send stayed held");
  await page.el("post-confirm-send").click();
  await page.settle();
  assert.deepEqual(page.proposalCommits.map((c) => c.handle), [pending.handle]);
});

test("a newer proposal replaces the card, holds Send, says so, and then Send commits the newer one", async () => {
  const page = newPage();
  await proposedDuringACall(page);
  const newer = proposal({ serial: 8, handle: "nEwEr-0123456789abcde", text: "actually, tomorrow" });
  page.propose(newer);
  await page.settle();
  assert.equal(page.el("post-confirm-text").textContent, "actually, tomorrow");
  assert.equal(page.el("post-confirm-send").disabled, true, "Send was live the moment the words changed");
  assert.match(page.el("post-confirm-expiry").textContent, /^Changed: read again · lapses in \d+ s$/);

  // A tap aimed at the old words lands during the hold and sends nothing.
  await page.el("post-confirm-send").click();
  await page.settle();
  assert.deepEqual(page.proposalCommits, [], "a tap during the hold committed the replacement");

  assert.equal(page.expireTimers(2000), 1, "no hold was armed");
  assert.equal(page.el("post-confirm-send").disabled, false, "Send stayed held");
  assert.match(page.el("post-confirm-expiry").textContent, /^Changed:/, "the change mark did not outlast the hold");
  await page.el("post-confirm-send").click();
  await page.settle();
  assert.deepEqual(page.proposalCommits.map((c) => [c.handle, c.text]), [[newer.handle, newer.text]]);
});

test("a proposal carrying the sign-in token is neither shown nor sendable", async () => {
  const page = newPage();
  await proposedDuringACall(page, proposal({ text: "here you go: write-token-aaaaaaaaaaaaaaaa" }));
  assert.ok(!page.el("post-confirm-text").textContent.includes("write-token-aaaaaaaaaaaaaaaa"));
  assert.equal(page.el("post-confirm-send").disabled, true);
  assert.equal(page.el("post-confirm-expiry").textContent, "Has your sign-in token · tap Don't send");
  await page.el("post-confirm-send").click();
  await page.settle();
  assert.deepEqual(page.proposalCommits, []);
});

test("the card counts down to the lapse, and a lapsed card cannot send", async () => {
  const page = newPage();
  await proposedDuringACall(page, proposal({ expires_in_ms: 2000 }));
  assert.match(page.el("post-confirm-expiry").textContent, /lapses in 2 s/);
  page.setClock(page.clock() + 1000);
  assert.equal(page.expireTimers(1000), 1, "no countdown tick was armed");
  assert.match(page.el("post-confirm-expiry").textContent, /lapses in 1 s/);
  page.setClock(page.clock() + 1000);
  page.expireTimers(1000);
  assert.equal(page.el("post-confirm-expiry").textContent, "Lapsed — nothing was posted.");
  assert.equal(page.el("post-confirm-send").disabled, true);
  assert.equal(page.expireTimers(1000), 0, "the countdown kept ticking past the lapse");
});

// --- `#269 approval-card-inline`: the card as the tentative next message ----------------------------

test("`#269 approval-card-inline`: the card is the next message under the transcript, under the channel, and docked on other screens", async () => {
  const page = newPage();
  await proposedDuringACall(page);
  const card = page.el("post-confirm");
  assert.equal(card.parentNode, page.el("pane-voice"), "the card is not in the call's pane");
  assert.equal(card.nextSibling, page.el("voice-composer"), "the card is not just above the call's box");
  assert.equal(card.getAttribute("data-place"), "voice");
  assert.ok(page.el("screen-main").hasAttribute("data-draft"), "the stylesheet cannot make room for the card");
  // Not modal and takes no focus: a group named by its heading and described by what it sends.
  assert.match(HTML, /<section id="post-confirm" role="group" aria-labelledby="post-confirm-title"/);
  assert.doesNotMatch(HTML_CODE, /alertdialog/, "the card still calls itself a modal dialog");
  // A turn spoken while the draft waits lands in the transcript's own list, above the card: the card
  // is never a row of the list, so nothing that walks the turns meets it.
  assistantSays(page, "anything else for the channel?");
  await page.settle();
  const transcript = page.el("transcript");
  assert.ok(!transcript.children.includes(card), "the card became a row of the transcript");
  const deep = (node) => [node.textContent, ...node.children.map(deep)].join(" ");
  assert.match(deep(transcript.children[transcript.children.length - 1]), /anything else for the channel/);
  const kids = page.el("pane-voice").children;
  assert.ok(kids.indexOf(transcript) < kids.indexOf(card), "a turn landed under the card");

  await showDiscord(page, [message({ id: "300", content: "is the build green yet" })]);
  assert.equal(card.parentNode, page.el("pane-discord"), "the card did not follow the owner to the channel");
  assert.equal(card.nextSibling, page.el("channel-composer"), "the card is not just above the channel's box");
  assert.equal(card.getAttribute("data-place"), "channel");
  assert.equal(card.hidden, false);
  await page.el("view-switch").click();
  await page.settle();
  assert.equal(card.parentNode, page.el("pane-voice"), "the card did not come back to the call");

  for (const [open, close] of [["open-settings", "close-settings"], ["open-help", "close-help"]]) {
    if (open === "open-help") await page.el("open-settings").click();
    await page.el(open).click();
    await page.settle();
    assert.equal(card.parentNode, page.el("frame-body"), `the card did not dock on ${page.screen()}`);
    assert.equal(card.getAttribute("data-place"), "dock");
    assert.equal(card.hidden, false, `the draft is out of sight on ${page.screen()}`);
    await page.el(close).click();
    if (close === "close-help") await page.el("close-settings").click();
    await page.settle();
  }
  assert.equal(page.screen(), "main");
  assert.equal(card.nextSibling, page.el("voice-composer"));

  // Docked or not, it is the same card: Send from Settings commits it, once.
  await page.el("open-settings").click();
  await page.el("post-confirm-send").click();
  await page.settle();
  assert.equal(page.proposalCommits.length, 1);
  assert.equal(card.hidden, true);
  // The room it made is kept a moment, so a second tap a little late does not land in a message
  // box that sprang back up under it; then it goes.
  assert.ok(page.el("screen-main").hasAttribute("data-draft"), "the box sprang back at once");
  assert.equal(page.expireTimers(1000), 1, "nothing takes the room back");
  assert.ok(!page.el("screen-main").hasAttribute("data-draft"), "the room for the card outlived it");
});

test("`#269 approval-card-inline`: beside a reply open in the list the card's Send is outlined, so it is never taken for the reply's", async () => {
  // `#410 inline-reply-editor`. Reply opens in the list the card ends, so the card stays there; its
  // filled Send is outlined while the reply's box, with a filled Send of its own, is open.
  const page = newPage();
  await proposedDuringACall(page);
  const card = page.el("post-confirm");
  await openReplyOn(page, [message({ id: "300", content: "is the build green yet" })]);
  assert.equal(card.getAttribute("data-place"), "channel");
  assert.equal(card.hasAttribute("data-beside-reply"), true, "the card's Send is not outlined beside the reply");
  assert.match(cssBlock('#post-confirm[data-beside-reply] #post-confirm-send'), /background:\s*transparent/,
    "the card's Send is as filled as the reply's own");
  await page.el("reply-cancel").click();
  await page.settle();
  assert.equal(card.getAttribute("data-place"), "channel");
  assert.equal(card.hasAttribute("data-beside-reply"), false, "the card still thinks a reply is beside it");
});

test("`#269 approval-card-inline`: the card's small text and a frozen card's notes are legible on the owner's tile", () => {
  // `--muted` and `--warn` on `--mine` are 4.1:1 and 3.9:1 in dark, under small text's 4.5:1.
  const dark = cssBlock(":root");
  // `#555 own-row-dark-contrast`: lighter in dark (and darker in light) since, so a reaction's count on
  // its chip's tint over the owner's row reads at 4.5:1 too.
  assert.match(dark, /--muted-on-mine:\s*#b4bcc8/);
  assert.match(dark, /--warn-on-mine:\s*#e8a27a/);
  assert.match(cssBlockIn("(prefers-color-scheme: light)", ":root"), /--muted-on-mine:\s*#525b68/, "light has no on-tile muted");
  for (const selector of ["#post-confirm-title", "#post-confirm-where", "#post-confirm-expiry", ".sent-how"]) {
    assert.match(cssBlock(selector), /color:\s*var\(--muted-on-mine\)/, `${selector} is drawn in --muted on the tile`);
  }
  assert.match(cssBlock('.messages li.sent-card[data-outcome="partial"] .sent-note'), /color:\s*var\(--warn-on-mine\)/);
});

test("`#269 approval-card-inline`: a reader at the newest line follows the card down, and one reading older turns is told a draft waits", async () => {
  const pinnedPage = newPage();
  await startTalking(pinnedPage);
  const pinnedArea = fillTranscript(pinnedPage);
  pinnedArea.scrollTop = pinnedArea.scrollHeight;
  pinnedPage.propose(proposal());
  await pinnedPage.settle();
  assert.equal(
    pinnedArea.scrollTop,
    pinnedArea.scrollHeight - pinnedArea.clientHeight,
    "a reader at the newest line was left above the card"
  );

  const page = newPage();
  await startTalking(page);
  const area = fillTranscript(page);
  area.scrollTop = 120;
  page.propose(proposal());
  await page.settle();
  assert.equal(page.el("post-confirm").hidden, false);
  assert.equal(area.scrollTop, 120, "the card pulled a reader away from what he was reading");
  assert.equal(page.el("jump-newest").hidden, false, "nothing says a draft is waiting below");
  assert.equal(page.el("jump-newest-label").textContent, "Draft to send");
  assert.equal(page.el("jump-newest").getAttribute("aria-label"), "Jump to the draft waiting to be sent");
  await page.el("jump-newest").click();
  await page.settle();
  assert.equal(area.scrollTop, area.scrollHeight - area.clientHeight, "the jump did not go to the draft");
});

test("`#269 approval-card-inline`: Send says where, and the line under the words names what it answers, never an id", async () => {
  const page = newPage();
  await proposedDuringACall(page, proposal({
    reply_to: "1122334455667788990",
    reply_to_author: "Alex",
    reply_to_excerpt: "is the build green yet…",
    thread_name: "release",
  }));
  assert.equal(page.el("post-confirm-send-label").textContent, "Send to lead team");
  assert.equal(page.el("post-confirm-send").getAttribute("aria-label"), "Send to lead team");
  assert.equal(
    page.el("post-confirm-where").textContent,
    "To lead team · replying to Alex: “is the build green yet…” · in thread “release”"
  );
  assert.match(page.el("post-confirm-expiry").textContent, /^Waiting for you · lapses in \d+ s$/);
  for (const [pending, said] of [
    [proposal({ reply_to: null }), "To lead team"],
    [proposal({ reply_to: "1122334455667788990", reply_to_author: "Alex" }), "To lead team · replying to Alex"],
    [proposal({ reply_to: "1122334455667788990" }), "To lead team · as a reply to an earlier message"],
  ]) {
    page.propose({ ...pending, serial: pending.serial + 100, handle: `${pending.handle}-${said.length}` });
    await page.settle();
    const where = page.el("post-confirm-where").textContent;
    assert.equal(where, said);
    assert.doesNotMatch(where, /\d{10}/, "an id reached the card");
  }
  // A long name is cut on screen and whole for a screen reader.
  const long = "the very long channel name nobody would ever say out loud in one breath";
  page.propose(proposal({ serial: 300, handle: "lOnG-0123456789abcdef", channel_name: long }));
  await page.settle();
  assert.equal(page.el("post-confirm-send").getAttribute("aria-label"), `Send to ${long}`);
  assert.match(cssBlock("#post-confirm-send-label"), /text-overflow:\s*ellipsis/);
});

test("`#269 approval-card-inline`: while a draft is up the message box is one line, and its own Send hides until something is typed", () => {
  assert.match(cssBlock("#screen-main[data-draft] #compose-text"), /height:\s*2\.75rem/);
  assert.match(cssBlock("#screen-main[data-draft] #compose-text:placeholder-shown ~ #send-text"),
    /display:\s*none/, "the box's Send stays up beside the card's");
  assert.match(cssBlock("#screen-main[data-draft] #channel-compose-text:placeholder-shown ~ #channel-send"),
    /display:\s*none/, "the channel box's Send stays up beside the card's");
  assert.match(cssBlock("#screen-main[data-draft] #pane-voice > #post-confirm"), /margin-top:\s*auto/);
  assert.match(cssBlock("#pane-voice > #post-confirm"), /margin-left:\s*15%/,
    "the card is not at the owner's own inset, so freezing it would move it sideways");
});

test("`#269 approval-card-inline`: the status line is one line high whatever it says, and the buttons share one row", () => {
  // A minimum height let a long status wrap and push Don't send down as a spoken yes settled.
  const line = cssBlock("#post-confirm-expiry");
  assert.match(line, /(^|[;\s])height:\s*1\.45em/, "the status line is not held to one line's height");
  assert.match(line, /white-space:\s*nowrap/, "the status line wraps");
  assert.match(line, /text-overflow:\s*ellipsis/, "a status too long for the card is not cut");
  assert.doesNotMatch(line, /min-height/, "a minimum lets the status line grow");
  assert.match(cssBlock(".post-confirm-actions"), /flex-wrap:\s*nowrap/, "Send can wrap under Don't send");
  // Every sentence the line shows fits a 360px phone's call pane (measured in Chromium): short.
  const card = /const POST_CARD_STATUS = \{([\s\S]*?)\n\};/.exec(SCRIPT);
  assert.ok(card, "the card's status lines are no longer in one place");
  const said = [...card[1].matchAll(/["`]([^"`]+)["`]/g)].map((m) => m[1].replace(/\$\{seconds\}/, "118"));
  assert.ok(said.length >= 5, `found ${said.length} status lines`);
  for (const sentence of said) {
    assert.ok(sentence.length <= 40, `"${sentence}" is ${sentence.length} characters, too long for one line`);
  }
});

test("`#269 approval-card-inline`: docked, the card clears the home indicator and only its words scroll", () => {
  // NOT VERIFIABLE BY SCREENSHOT: Chromium's emulation reports a zero inset. Off the main screen the
  // control pane, which carries the bottom inset everywhere else, is hidden and the docked card is
  // the last thing on screen, so without its own bottom inset Send sits under the home indicator.
  const dock = cssBlock("#frame-body > #post-confirm");
  assert.match(dock, /padding:[^;]*env\(safe-area-inset-bottom\)/, "the docked card ignores the bottom inset");
  assert.match(dock, /env\(safe-area-inset-left\)/, "the docked card ignores the left inset");
  assert.match(dock, /env\(safe-area-inset-right\)/, "the docked card ignores the right inset");
  // Never a scroll box as a whole: that put where it goes, its status, Don't send and Send below its
  // own fold once More opened a long draft. Its words are what scroll.
  assert.doesNotMatch(dock, /overflow(-y)?:\s*(auto|scroll)/, "the docked card scrolls as a whole");
  assert.doesNotMatch(dock, /max-height/, "the docked card is capped, so what is under its words can be cut off");
  assert.match(cssBlock('#post-confirm[data-place="dock"] #post-confirm-text'), /overflow-y:\s*auto/,
    "opened, a long docked draft has nowhere to go");
  assert.match(cssBlock('#post-confirm[data-place="dock"] #post-confirm-text'), /max-height:[^;]*100dvh/,
    "the docked words are not bounded by the room the screen has");
});

test("`#269 approval-card-inline`: a long draft is folded with More, and More opens it in place", async () => {
  const page = newPage();
  const text = `${"long words ".repeat(60)}the end`;
  await proposedDuringACall(page, proposal({ text }));
  const card = page.el("post-confirm");
  const more = page.el("post-confirm-more");
  assert.equal(more.hidden, false, "a long draft has no way to read the rest");
  assert.ok(card.hasAttribute("data-folded"));
  assert.equal(more.getAttribute("aria-expanded"), "false");
  assert.equal(page.el("post-confirm-text").textContent, text, "folding shortened what is confirmed");
  await more.click();
  assert.ok(!card.hasAttribute("data-folded"), "More did not open the draft");
  assert.equal(more.getAttribute("aria-expanded"), "true");
  await more.click();
  assert.ok(card.hasAttribute("data-folded"), "Less did not fold it again");
  // A short draft has nothing to fold.
  page.propose(proposal({ serial: 12, handle: "sHoRt-0123456789abcde", text: "on it" }));
  await page.settle();
  assert.equal(more.hidden, true);
  assert.ok(!card.hasAttribute("data-folded"));
});

test("`#269 approval-card-inline`: a spoken yes settling says so on the card, with how to stop it", async () => {
  const page = newPage();
  const socket = await readBackDuringACall(page);
  page.expireTimers(2000);
  hostedHears(socket, "yes");
  await page.settle();
  assert.equal(page.el("post-confirm-expiry").textContent, "Heard yes — tap Don't send to stop");
  // The countdown's tick does not write over it.
  page.expireTimers(1000);
  assert.equal(page.el("post-confirm-expiry").textContent, "Heard yes — tap Don't send to stop");
  await page.el("post-confirm-cancel").click();
  await page.settle();
  assert.deepEqual(page.proposalCommits, []);
  assert.equal(page.proposalCancels.length, 1);
});

test("`#269 approval-card-inline`: the card going with the focus in it never puts the focus in the message box on a touch screen", async () => {
  // A withdrawal the server refused leaves no line in the transcript to take the focus: the list
  // takes it on a touch screen, and the box on a mouse or keys.
  for (const [kind, expected] of [["touch", "scroll-area"], ["mouse", "compose-text"]]) {
    const page = newPage();
    await proposedDuringACall(page);
    const serving = page.postProposals;
    page.postProposals = async (path, options) => path === "/api/v1/post-proposals/cancel"
      ? json(409, { error: "proposal_used", detail: "already settled" })
      : serving(path, options);
    const cancel = page.el("post-confirm-cancel");
    await page.documentEvent("pointerdown", { target: cancel, pointerType: kind });
    cancel.focus();
    await cancel.click();
    await page.settle();
    assert.equal(page.el("post-confirm").hidden, true);
    assert.equal(
      page.document.activeElement,
      page.el(expected),
      `after a ${kind} tap the focus went to ${page.document.activeElement && page.document.activeElement.id}`
    );
  }
  // `#270 voice-transcript-markers`. One the server took leaves the draft's "Not sent" line where
  // the card was, and the focus goes to it on either: never into the box on a touch screen.
  for (const kind of ["touch", "mouse"]) {
    const page = newPage();
    await proposedDuringACall(page);
    const cancel = page.el("post-confirm-cancel");
    await page.documentEvent("pointerdown", { target: cancel, pointerType: kind });
    cancel.focus();
    await cancel.click();
    await page.settle();
    const line = notSentLines(page)[0];
    assert.ok(line, `${kind}: Don't send left no line`);
    assert.equal(page.document.activeElement, line,
      `after a ${kind} tap the focus went to ${page.document.activeElement && page.document.activeElement.id}`);
  }
});

test("`#269 approval-card-inline`: a new draft is said once on the status line, and a changed one says it changed", async () => {
  const page = newPage();
  await proposedDuringACall(page);
  assert.equal(page.el("status").textContent, "Draft to lead team is waiting: Send or Don't send.");
  page.propose(proposal({ serial: 8, handle: "nEwEr-0123456789abcde", text: "actually, tomorrow" }));
  await page.settle();
  assert.equal(page.el("status").textContent, "The assistant changed the draft to lead team — read it again.");
});

// --- `#270 voice-transcript-markers`: frozen send cards -------------------------------------------

/** Every word under `node`, its own and its children's. */
const wordsOf = (node) => [node.textContent, ...node.children.map(wordsOf)].join(" ");

test("`#270 voice-transcript-markers`: a tapped Send freezes into a card in the transcript, filed from the server's own copy", async () => {
  const page = newPage();
  await recordingCall(page);
  page.propose(proposal({ reply_to: null }));
  await page.settle();
  page.expireTimers(2000);
  const send = page.el("post-confirm-send");
  send.focus();
  await send.click();
  await watchTurns(page);
  const cards = sentCards(page);
  assert.equal(cards.length, 1, "the send left no frozen card, or two");
  const card = cards[0];
  assert.equal(page.el("transcript").children.at(-1), card, "the frozen card is not the newest row");
  assert.ok(card.hasClass("mine"), "the frozen card does not stand where a turn of the owner's does");
  assert.match(wordsOf(card), /Sent to lead team/);
  assert.match(wordsOf(card), /approved by tap/);
  assert.match(wordsOf(card), /second line/, "the frozen card lost the words that went");
  assert.ok(!page.el("transcript").children.some((li) => li.className === "seam"), "a seam said it again");
  // Filed once, as the server's copy: the claim names the send and carries no words.
  assert.deepEqual(page.claims.map(({ speaker, text, send: seq }) => ({ speaker, text, send: seq })),
    [{ speaker: "sent", text: "", send: 1 }]);
  assert.equal(page.claims[0].id, "conv_cards");
  assert.equal(page.storedTurns.get("conv_cards").at(-1).speaker, "sent");
  // The focus went with the card into the transcript, not into the message box.
  assert.ok(page.document.activeElement && page.document.activeElement.hasClass("sent-card"),
    `the focus went to ${page.document.activeElement && page.document.activeElement.id}`);
  assert.ok(!wordsOf(card).includes("Not kept"), "a filed card says it was not kept");
});

test("`#270 voice-transcript-markers`: a bug drawing the frozen card never turns a send that posted into 'nothing was posted'", async () => {
  // The card is drawn inside the commit's `try`, after the server has posted. Were a drawing bug to
  // reach the commit's `catch`, it would say the send posted nothing and tell the agent it was NOT
  // sent. It is thrown on its own instead, and the send memory still draws the card.
  const drawingBug = () => brokenScript(
    "function freezeSentCard(proposal, confirmedBy, outcome, answer) {\n",
    "function freezeSentCard(proposal, confirmedBy, outcome, answer) {\n  throw new Error(\"a drawing bug\");\n",
  );
  const tapped = newPage(new Map(), drawingBug());
  await recordingCall(tapped);
  tapped.propose(proposal({ reply_to: null }));
  await tapped.settle();
  tapped.expireTimers(2000);
  await tapped.el("post-confirm-send").click();
  await watchTurns(tapped);
  assert.equal(tapped.el("status").textContent, "Posted to lead team.");
  assert.doesNotMatch(tapped.el("error").textContent, /Nothing was posted|posted nothing/);
  assert.equal(tapped.el("post-confirm").hidden, true, "the posted draft stayed on the card");
  assert.throws(() => tapped.expireTimers(0), /a drawing bug/, "the drawing bug was swallowed");
  // The send memory reports the send at the watch's next read, and draws its card once.
  await watchTurns(tapped);
  assert.equal(sentCards(tapped).length, 1, "the send memory did not draw the card the commit could not");
  assert.match(wordsOf(sentCards(tapped)[0]), /approved by tap/);

  // A spoken yes: the agent is told the message was SENT.
  const spoken = newPage(new Map(), drawingBug());
  const socket = await readBackDuringACall(spoken);
  hostedHears(socket, "yes");
  await settleSpokenYes(spoken);
  assert.equal(spoken.proposalCommits.length, 1);
  assert.doesNotMatch(spoken.el("error").textContent, /posted nothing/);
  const notices = pageNotices(socket);
  assert.equal(notices.length, 1, notices.join("\n"));
  assert.match(notices[0], /^Voice page: the message you read back was SENT to lead team\./);
});

test("`#270 voice-transcript-markers`: a send from the send memory is drawn once when it is this page's, and never when it is not", async () => {
  const page = newPage();
  page.setClock(1_700_000_100_000);
  await recordingCall(page);
  await watchTurns(page);
  const watched = proposalWatches(page).filter((r) => r.includes("sent_after="));
  assert.ok(watched.length > 0, "the watch never read the send memory");

  // A direct send that began inside this call: "sent without asking", filed once.
  page.publishSend(proposal({ serial: 21, handle: "dIrEcT", text: "rebase onto main" }), "direct");
  await watchTurns(page);
  assert.equal(sentCards(page).length, 1);
  assert.match(wordsOf(sentCards(page)[0]), /sent without asking/);
  assert.deepEqual(page.claims.map((c) => c.send), [1]);

  // The same send again draws nothing more, and files nothing more.
  page.resendAll = true;
  page.propose(proposal({ serial: 40, handle: "wAkE" }));
  await watchTurns(page);
  page.resendAll = false;
  page.propose(null);
  await watchTurns(page);
  assert.equal(sentCards(page).length, 1, "one send was drawn twice");
  assert.equal(page.claims.length, 1);

  // A send that began before this call, from another page, and was never shown here: not ours.
  page.publishSend(proposal({ serial: 22, handle: "eLsEwHeRe", text: "from another tab" }), "direct",
    { begun_at_ms: 1_600_000_000_000 });
  await watchTurns(page);
  assert.equal(sentCards(page).length, 1, "another page's send was drawn here");
});

/**
 * The fake server restarts: its memory is empty, its proposals number from one again, its sends
 * number from its clock, above anything the last run handed out, and it is another run.
 */
function restartServer(page, instance = "cnVuLXR3bw") {
  page.sendMemory = [];
  page.sendsThrough = 1_700_000_000_000;
  page.serverInstance = instance;
}

/**
 * Hang up, then call again into the vendor conversation `conversationId`, on the same page;
 * `between` runs after the hang-up has settled and before the next call starts.
 */
async function callAgain(page, conversationId, between = () => {}) {
  await page.el("hang-up").click();
  await watchTurns(page);
  between();
  await page.el("talk").click();
  const socket = page.sockets.at(-1);
  socket.onopen();
  socket.onmessage({ data: JSON.stringify({
    type: "conversation_initiation_metadata",
    conversation_initiation_metadata_event: { conversation_id: conversationId, agent_output_audio_format: "pcm_16000" },
  }) });
  await watchTurns(page);
  return socket;
}

/** Each row of the transcript, as a card, a seam ending a conversation, or its class. */
const rowShapes = (page) => page.el("transcript").children.map((li) => (li.hasClass("sent-card") ? "CARD" : li.className) +
  (li.getAttribute("data-ends") ? `[ends ${li.getAttribute("data-ends")}]` : ""));

test("`#270 voice-transcript-markers`: after the server restarts, a send that reuses a drawn serial is still drawn", async () => {
  const page = newPage();
  await recordingCall(page);
  await watchTurns(page);
  page.publishSend(proposal({ serial: 3, handle: "bEfOrE", text: "before the restart" }), "direct");
  await watchTurns(page);
  assert.equal(sentCards(page).length, 1);
  restartServer(page);
  page.publishSend(proposal({ serial: 3, handle: "aFtEr", text: "after the restart" }), "direct");
  await watchTurns(page, 6);
  assert.equal(sentCards(page).length, 2, "a new send was taken for one already drawn because its serial came round again");
  assert.match(wordsOf(sentCards(page)[1]), /after the restart/);
});

test("`#270 voice-transcript-markers`: a tapped send after a restart is drawn and filed in the call it was tapped in, not the one its serial was first used in", async () => {
  // Proposal serials start again at one in every run of the server, and the page stays open across
  // a restart. Keyed by serial alone, the next run's serial 3 was taken for the card already drawn:
  // the commit's own answer drew nothing, and the send memory then placed it above the end of the
  // FIRST call and filed it there.
  for (const firstFate of ["sent", "withdrawn"]) {
    const page = newPage();
    await recordingCall(page, "conv_one");
    await watchTurns(page);
    page.propose(proposal({ serial: 3, handle: "bEfOrE-0123456789abcd", reply_to: null, text: "before the restart" }));
    await page.settle();
    page.expireTimers(2000);
    await page.el(firstFate === "sent" ? "post-confirm-send" : "post-confirm-cancel").click();
    await watchTurns(page);
    assert.equal(sentCards(page).length, firstFate === "sent" ? 1 : 0);

    restartServer(page);
    await callAgain(page, "conv_two");
    page.propose(proposal({ serial: 3, handle: "aFtEr-0123456789abcde", reply_to: null, text: "after the restart" }));
    await watchTurns(page);
    page.expireTimers(2000);
    await page.el("post-confirm-send").click();
    await watchTurns(page, 8);

    const cards = sentCards(page);
    assert.equal(cards.length, firstFate === "sent" ? 2 : 1, `${firstFate}: ${rowShapes(page).join(" | ")}`);
    assert.match(wordsOf(cards.at(-1)), /after the restart/);
    assert.equal(page.el("transcript").children.at(-1), cards.at(-1),
      `${firstFate}: the new send is not the newest row of the call it was tapped in: ${rowShapes(page).join(" | ")}`);
    assert.equal(page.claims.at(-1).id, "conv_two", `${firstFate}: the new send was filed in the earlier conversation`);
    assert.equal(page.claims.filter((claim) => claim.id === "conv_two").length, 1);
  }
});

test("`#270 voice-transcript-markers`: two reads whose answers cross never take the cursor back, so nothing is drawn twice", async () => {
  const page = newPage();
  page.sendsThrough = 10;
  await recordingCall(page);
  await watchTurns(page);
  page.publishSend(proposal({ serial: 21, handle: "dIrEcT", text: "rebase onto main" }), "direct");
  await watchTurns(page);
  assert.equal(sentCards(page).length, 1);
  // An answer from earlier arrives late, from the same run: its count is behind every cursor this
  // page has sent.
  const late = 6;
  const serving = page.postProposals;
  let crossed = false;
  page.postProposals = async (path, options) => {
    const answer = await serving(path, options);
    if (crossed || !path.includes("wait=")) return answer;
    crossed = true;
    const body = JSON.parse(await answer.text());
    return json(200, { ...body, sends: [], sends_through: late });
  };
  page.propose(proposal({ serial: 40, handle: "wAkE" }));
  await watchTurns(page);
  page.propose(null);
  await watchTurns(page, 8);
  assert.ok(crossed, "the fixture never crossed an answer");
  const reads = proposalWatches(page);
  assert.ok(!reads.some((read) => read.includes(`sent_after=${late}`)), `the cursor went back: ${reads.join(", ")}`);
  assert.equal(sentCards(page).length, 1, "a send was drawn twice after the cursor went back");
  assert.equal(page.claims.length, 1);
});

test("`#270 voice-transcript-markers`: a send the long poll brings before the commit's own answer is drawn once", async () => {
  // The server wakes the long poll the moment a send settles, before the commit's answer leaves.
  const page = newPage();
  await recordingCall(page);
  page.propose(proposal({ reply_to: null }));
  await page.settle();
  page.expireTimers(2000);
  const answering = page.commitProposal;
  let release = null;
  page.commitProposal = async (body) => {
    const answer = await answering(body);
    await new Promise((resolve) => { release = resolve; });
    return answer;
  };
  const tapped = page.el("post-confirm-send").click();
  await watchTurns(page, 6);
  assert.ok(release, "the commit never reached the fixture");
  assert.equal(sentCards(page).length, 1, "the send memory did not draw the send while its answer was on its way");
  release();
  await tapped;
  await watchTurns(page, 6);
  assert.equal(sentCards(page).length, 1, "the commit's answer drew the same send again");
  assert.equal(page.claims.length, 1);
});

test("`#270 voice-transcript-markers`: a send that began after a call ended, and before the next began, is not that call's", async () => {
  const page = newPage();
  page.serverNowMs = 1_700_000_000_000;
  await recordingCall(page, "conv_one");
  await watchTurns(page);
  // The first call ends at 10 s on the server's clock, and the next begins at 60 s.
  page.serverNowMs = 1_700_000_010_000;
  await callAgain(page, "conv_two", () => { page.serverNowMs = 1_700_000_060_000; });
  // Another tab's direct send, begun in the gap between the two calls.
  page.publishSend(proposal({ serial: 23, handle: "gAp", text: "from another tab" }), "direct",
    { begun_at_ms: 1_700_000_030_000 });
  await watchTurns(page);
  assert.equal(sentCards(page).length, 0, `a send from between the calls was drawn: ${rowShapes(page).join(" | ")}`);
  assert.deepEqual(page.claims, []);
});

test("`#270 voice-transcript-markers`: a draft shown in the next call before the voice service names it is filed in that call, not the last one", async () => {
  const page = newPage();
  await recordingCall(page, "conv_first");
  await page.el("hang-up").click();
  await watchTurns(page);
  await page.el("talk").click();
  const socket = page.sockets.at(-1);
  socket.onopen();
  await watchTurns(page);
  page.propose(proposal({ reply_to: null }));
  await watchTurns(page);
  socket.onmessage({ data: JSON.stringify({
    type: "conversation_initiation_metadata",
    conversation_initiation_metadata_event: { conversation_id: "conv_second", agent_output_audio_format: "pcm_16000" },
  }) });
  await page.settle();
  page.expireTimers(2000);
  await page.el("post-confirm-send").click();
  await watchTurns(page, 6);
  assert.deepEqual(page.claims.map((claim) => claim.id), ["conv_second"], "the send was filed in the call before");
  assert.equal(page.el("transcript").children.at(-1), sentCards(page)[0],
    `the card was drawn above the last call's end: ${fateShapes(page).join(" | ")}`);
});

test("`#270 voice-transcript-markers`: a draft shown before the voice service named the conversation is filed in it", async () => {
  const page = newPage();
  page.transcriptRecordsSupported = true;
  const socket = await startTalking(page);
  await page.settle();
  page.propose(proposal({ reply_to: null }));
  await page.settle();
  socket.onmessage({ data: JSON.stringify({
    type: "conversation_initiation_metadata",
    conversation_initiation_metadata_event: { conversation_id: "conv_named_late", agent_output_audio_format: "pcm_16000" },
  }) });
  await page.settle();
  page.expireTimers(2000);
  await page.el("post-confirm-send").click();
  await watchTurns(page);
  assert.equal(sentCards(page).length, 1);
  assert.deepEqual(page.claims.map((claim) => claim.id), ["conv_named_late"]);
  assert.ok(!wordsOf(sentCards(page)[0]).includes("Not kept"), "the card says it was not kept");
});

test("`#270 voice-transcript-markers`: a commit whose answer was lost is drawn when the send memory says it went", async () => {
  const page = newPage();
  await recordingCall(page);
  page.propose(proposal());
  await page.settle();
  page.expireTimers(2000);
  const pending = page.proposal;
  page.commitProposal = async () => {
    // The server took it and posted it; its answer never arrived.
    page.settleProposal();
    page.fileSend(pending, "tap");
    throw new TypeError("Failed to fetch");
  };
  await page.el("post-confirm-send").click();
  await watchTurns(page, 6);
  assert.equal(sentCards(page).length, 1, "the send memory's word on the lost commit drew no card");
  assert.match(wordsOf(sentCards(page)[0]), /approved by tap/);
  assert.equal(page.el("post-confirm").hidden, true, "the card stayed up for a send that went");
  assert.equal(page.claims.length, 1);
});

test("`#270 voice-transcript-markers`: the send memory's cursor goes on every read, a retry after an error included", async () => {
  const page = newPage();
  await recordingCall(page);
  await watchTurns(page);
  const failing = page.postProposals;
  let failed = false;
  page.postProposals = async (path, options) => {
    if (!failed && path.includes("wait=")) {
      failed = true;
      return json(500, { error: "storage_error", detail: "down" });
    }
    return failing(path, options);
  };
  page.propose(proposal());
  await watchTurns(page);
  page.expireTimers(3000);
  await watchTurns(page);
  const reads = proposalWatches(page);
  assert.ok(failed, "the fixture never failed a read");
  const retry = reads[reads.indexOf(reads.find((r) => r.includes("wait="))) + 1] || reads.at(-1);
  assert.ok(reads.slice(1).every((r) => r.includes("sent_after=")), `a read went without the cursor: ${reads.join(", ")}`);
  assert.ok(retry.includes("sent_after="));
});

test("`#270 voice-transcript-markers`: a send that settles between the last waiting read and Hang up is drawn above the line that ends the call", async () => {
  const page = newPage();
  await recordingCall(page, "conv_last");
  await watchTurns(page);
  // Settled, but the watch is parked and is not woken: Hang up's last read is what brings it.
  const settle = page.proposalWaiters.splice(0);
  page.sendsThrough += 1;
  page.sendMemory.push({
    text: "landed at the last moment",
    detail: {
      serial: 31, send_seq: page.sendsThrough, channel_id: CHANNEL.id, channel_name: "lead team", parts_posted: 1,
      parts_total: 1, unsent_chars: 0, outcome: "sent", how: "direct", origin: "mcp", text_truncated: false,
      text_chars: 25, begun_at_ms: page.clock(), sent_at_ms: page.clock(),
    },
  });
  await page.el("hang-up").click();
  await watchTurns(page);
  for (const wake of settle) wake();
  await watchTurns(page);
  const rows = page.el("transcript").children;
  const card = sentCards(page)[0];
  assert.ok(card, "the last moment's send was never drawn");
  const boundary = rows.find((li) => li.getAttribute("data-ends") === "conv_last");
  assert.ok(boundary, "the line that ends the call does not say which call it ends");
  assert.ok(rows.indexOf(card) < rows.indexOf(boundary), "the send was drawn below the end of its call");
  assert.equal(page.claims.at(-1).id, "conv_last");
});

test("`#270 voice-transcript-markers`: a claim the server refuses keeps the card and never stops the recording", async () => {
  for (const [status, noted] of [[404, true], [409, false], [400, true], [413, true]]) {
    const page = newPage();
    await recordingCall(page);
    page.claimStatus = status;
    page.propose(proposal({ reply_to: null }));
    await page.settle();
    page.expireTimers(2000);
    await page.el("post-confirm-send").click();
    await watchTurns(page);
    assert.equal(sentCards(page).length, 1, `${status} took the card away`);
    assert.equal(/Not kept in the stored transcript/.test(wordsOf(sentCards(page)[0])), noted, String(status));
    // The next turn is still recorded.
    const before = page.storeCalls.length;
    youSay(page, "and the next thing");
    page.sockets[0].onmessage({ data: JSON.stringify({ type: "agent_response", agent_response_event: { agent_response: "ok" } }) });
    await watchTurns(page);
    assert.ok(page.storeCalls.length > before, `${status} stopped the recording`);
  }
});

test("`#270 voice-transcript-markers`: a server that does not take records is sent no claim, no records=1 and no cursor", async () => {
  const page = newPage();
  await startTalkingNamed(page, "conv_old");
  await page.settle();
  page.propose(proposal({ reply_to: null }));
  await page.settle();
  page.expireTimers(2000);
  await page.el("post-confirm-send").click();
  await watchTurns(page);
  assert.equal(sentCards(page).length, 1, "the live card should still freeze on screen");
  assert.ok(!wordsOf(sentCards(page)[0]).includes("Not kept"), "an old server's card nags about keeping");
  assert.deepEqual(page.claims, []);
  assert.ok(!proposalWatches(page).some((r) => r.includes("sent_after")), "the cursor went to an old server");
  assert.ok(!page.transcriptCalls.some((r) => r.includes("records=1")), "records=1 went to an old server");

  // And a transcript read really is made, without records: a reload with a stored conversation.
  const reread = newPage();
  reread.storedTurns.set("conv_old", [{ speaker: "you", text: "what happened overnight", at_ms: 1_700_000_000_000 }]);
  await signIn(reread);
  await reread.settle();
  assert.ok(reread.transcriptCalls.length > 0, "the page never read the stored transcript");
  assert.ok(!reread.transcriptCalls.some((r) => r.includes("records=1")), "records=1 went to an old server");
});

test("`#270 voice-transcript-markers`: a frozen card ticks only a whole send, and a partial one shows only what went", async () => {
  const page = newPage();
  await recordingCall(page);
  page.claimStatus = 404;
  page.propose(proposal({ reply_to: null, text: "first part —\nsecond 😀 part" }));
  await page.settle();
  page.expireTimers(2000);
  page.commitProposal = async () => {
    page.settleProposal();
    return json(207, { error: "partially_posted", detail: "chat request failed", posted: 1, unsent: "second 😀 part",
      uncertain: false, serial: 7 });
  };
  await page.el("post-confirm-send").click();
  await watchTurns(page);
  const card = sentCards(page)[0];
  assert.ok(card, "the partial send left no card");
  const said = wordsOf(card);
  assert.match(said, /Partly sent to lead team/);
  assert.match(said, /first part —/);
  assert.doesNotMatch(said, /second/, "the card shows words that were not sent");
  // "second 😀 part" is thirteen characters as the server counts them, fourteen UTF-16 units.
  assert.match(said, /the rest \(13 characters\) was not sent/);
  const mark = card.children[0].children.find((kid) => kid.hasClass("sent-mark"));
  assert.equal(mark.textContent, "!", "a partial send is ticked as if it all went");

  // Restored, each outcome has its own glyph: the tick only for a send that went whole.
  const restored = newPage();
  restored.transcriptRecordsSupported = true;
  restored.storedTurns.set("conv_marks", ["sent", "unconfirmed", "partial"].map((outcome, at) => ({
    speaker: "sent", text: `ship it ${outcome}`, at_ms: 1_700_000_000_000 + at,
    detail: {
      kind: "sent", serial: at + 1, send_seq: at + 1, channel_id: CHANNEL.id, channel_name: "lead team",
      parts_posted: outcome === "sent" ? 1 : 0, parts_total: 1, unsent_chars: 0, outcome, how: "tap",
      origin: "commit", text_truncated: false, text_chars: 9, begun_at_ms: 1, sent_at_ms: 1,
    },
  })));
  await signIn(restored);
  await restored.settle();
  const marks = sentCards(restored).map((li) => li.children[0].children.find((kid) => kid.hasClass("sent-mark")).textContent);
  assert.deepEqual(marks, ["✓", "?", "!"]);
});

test("`#270 voice-transcript-markers`: a restored record asks for records and draws its sends as the same frozen cards", async () => {
  const page = newPage();
  page.transcriptRecordsSupported = true;
  page.storedTurns.set("conv_earlier", [
    { speaker: "agent", text: "To lead team: ship it. Send it?", at_ms: 1_700_000_000_000 },
    { speaker: "you", text: "yes", at_ms: 1_700_000_000_001 },
    { speaker: "marker", text: "", at_ms: 1_700_000_000_002, detail: { kind: "not_sent", why: "lapsed", serial: 3 } },
    {
      speaker: "sent", text: "ship it", at_ms: 1_700_000_000_003,
      detail: {
        kind: "sent", serial: 4, send_seq: 2, channel_id: CHANNEL.id, channel_name: "lead team", reply_to: "11",
        reply_to_author: "Alex", reply_to_excerpt: "is it green", parts_posted: 1, parts_total: 2, unsent_chars: 40,
        outcome: "partial", how: "voice", origin: "commit", text_truncated: false, text_chars: 7,
        begun_at_ms: 1_700_000_000_000, sent_at_ms: 1_700_000_000_003,
      },
    },
  ]);
  await signIn(page);
  await page.settle();
  assert.ok(page.transcriptCalls.some((r) => r.includes("records=1")), "the record was read without its records");
  const cards = sentCards(page);
  assert.equal(cards.length, 1, "the stored send was not drawn as a frozen card");
  const said = wordsOf(cards[0]);
  assert.match(said, /Partly sent to lead team/);
  assert.match(said, /approved by voice/);
  assert.match(said, /replying to Alex: “is it green”/);
  assert.match(said, /Part 1 of 2 went; the rest \(40 characters\) was not sent\./);
  assert.ok(!said.includes("11"), "the reply's id reached the screen");
  // The boundary says which call it ends. (The lapsed draft's marker is its "Not sent" line, which
  // `#270`'s draft-fate tests pin.)
  const boundary = page.el("transcript").children.find((li) => li.className === "seam" && !li.getAttribute("data-not-sent"));
  assert.equal(boundary.getAttribute("data-ends"), "conv_earlier");
});

test("`#270 voice-transcript-markers`: Open in channel takes the owner to the message the card sent", async () => {
  const page = newPage();
  await recordingCall(page);
  page.propose(proposal({ reply_to: null }));
  await page.settle();
  page.expireTimers(2000);
  await page.el("post-confirm-send").click();
  await watchTurns(page);
  const card = sentCards(page)[0];
  const open = card.children.find((kid) => kid.hasClass("sent-open"));
  assert.ok(open, "a card for a message with an id has no way to open it");
  const id = String(page.storedTurns.get("conv_cards").at(-1).detail.message_id);
  page.messages = [message({ id, channel_id: CHANNEL.id, content: "on it" })];
  await open.click();
  await watchTurns(page, 8);
  assert.equal(page.tab(), "discord", "Open in channel did not open the channel");
  const row = page.el("discord-log").children.find((li) => String(li.getAttribute("data-ids") || "").includes(id));
  assert.ok(row && row.getAttribute("data-landed") === "true", "the message was not brought into view");
});

// --- #580 open-in-chat-jump --------------------------------------------------------------------
//
// The owner, of a voice card's Open in channel: "when I click it I want to jump directly to that
// message. But instead I see a little chip pop up that says the messages in the zero 14 channel and I
// have to manually switch over ... pressing the button should jump to the right view and to the
// message ... the all view with all chronological messages be the default ... centered and
// highlighted on the message that was linked."

/**
 * A page on the call's view whose stored call holds a frozen card for a message the voice agent sent
 * to OTHER_CHANNEL — `message`, in `thread` when given — while the channel picker shows CHANNEL.
 * OTHER_CHANNEL holds `threadData`'s two threads and a dozen long fillers after them.
 */
async function sentElsewherePage({ message: id = "204", thread = "spaces/A/threads/another", view = null } = {}) {
  const stored = new Map(view ? [["vibe-talk.voice.open-in-view", view]] : []);
  const page = newPage(stored, SCRIPT, (p) => p.enableHistory());
  const other = { ...CHANNEL, id: OTHER_CHANNEL.id, label: OTHER_CHANNEL.label };
  page.channels = [{ ...CHANNEL }, other];
  page.threadingSupported = true;
  const data = threadData();
  const fillers = Array.from({ length: 12 }, (_unused, i) => message({ id: `${310 + i}`, content: longMessage(`filler ${i}`) }));
  const byChannel = {
    [CHANNEL.id]: { messages: [message({ id: "100", content: "here on lead team" })], threads: [] },
    [other.id]: { messages: [...data.messages, ...fillers], threads: data.threads },
  };
  const serve = page.timeline;
  page.timeline = async (path, options) => {
    const channel = page.channels.find((c) => String(path).includes(`/channels/${encodeURIComponent(c.id)}/`));
    assert.ok(channel, `the page read a channel this fixture has no cast for: ${path}`);
    page.messages = byChannel[channel.id].messages;
    page.threads = byChannel[channel.id].threads;
    const answer = await serve(path, options);
    return json(answer.status, { ...JSON.parse(await answer.text()), channel });
  };
  page.transcriptRecordsSupported = true;
  page.storedTurns.set("conv_sent", [{
    speaker: "sent", text: "rebase onto main", at_ms: 1_700_000_000_000,
    detail: {
      kind: "sent", serial: 1, send_seq: 1, channel_id: other.id, channel_name: other.label,
      message_id: id, ...(thread ? { thread_id: thread } : {}),
      parts_posted: 1, parts_total: 1, unsent_chars: 0, outcome: "sent", how: "tap",
      origin: "commit", text_truncated: false, text_chars: 16, begun_at_ms: 1, sent_at_ms: 1,
    },
  }]);
  await signIn(page);
  await page.settle();
  assert.equal(page.tab(), "voice", "fixture: the page did not open on the call's view");
  assert.equal(page.el("discord-channel").value, CHANNEL.id, "fixture: the picker is not on the other channel");
  return page;
}

/** The card's Open in channel button. */
const sentOpenButton = (page) => {
  const card = sentCards(page)[0];
  assert.ok(card, "fixture: no frozen card was drawn");
  const open = card.children.find((kid) => kid.hasClass("sent-open"));
  assert.ok(open, "the card has no Open in channel");
  return open;
};

test("`#580 open-in-chat-jump`: Open in channel on the call's view switches channel, lands in All lit on the message, and one Back returns to the call's view", async () => {
  const page = await sentElsewherePage();
  const length = page.history.length;
  await sentOpenButton(page).click();
  await untilDeepLink(page, () => rowWithId(page, "204")?.getAttribute("data-landed") === "true");
  assert.equal(page.tab(), "discord", "Open in channel did not open the channel's view");
  assert.equal(page.el("discord-channel").value, OTHER_CHANNEL.id, "the channel was not switched");
  assert.equal(page.el("thread-select").value, "flat", "the jump did not land in All");
  assert.equal(page.el("thread-heading").hidden, true, "All was asked for, and a thread was opened");
  assert.equal(rowWithId(page, "204").getAttribute("data-landed"), "true", "the message was not lit");
  assert.doesNotMatch(statusText(page), /choose it in the channel picker/);
  assert.equal(page.history.length - length, 1, "the jump did not push exactly one entry");
  assert.deepStrictEqual(backFacts(page), { depth: 1, tag: "jump" });

  await page.browserBack();
  await untilDeepLink(page, () => page.tab() === "voice");
  assert.equal(page.left, false, "Back after the jump left the app");
  assert.equal(page.tab(), "voice", "Back did not return to the call's view");
  assert.equal(statusText(page), "Back on the call's view.");
  assert.deepStrictEqual(backFacts(page), { depth: 0, tag: "top" });
  assert.equal(sentCards(page).length, 1, "the card is gone from the call's view");
});

test("`#580 open-in-chat-jump`: the message landed on is centred in the list, where the list can put it there", async () => {
  const page = await sentElsewherePage({ message: "316", thread: null });
  await sentOpenButton(page).click();
  await untilDeepLink(page, () => rowWithId(page, "316")?.getAttribute("data-landed") === "true");
  const area = page.el("scroll-area");
  const room = { top: page.pure("floatingClearance(el('scroll-area'))"), bottom: area.getBoundingClientRect().bottom };
  const box = rowWithId(page, "316").getBoundingClientRect();
  const offCentre = Math.abs((box.top + box.bottom) / 2 - (room.top + room.bottom) / 2);
  assert.ok(offCentre <= 12, `the message is ${offCentre}px off the middle of the list`);
});

test("`#580 open-in-chat-jump`: with Main chosen in Settings, a reply opens in its thread and a root message in Main", async () => {
  const reply = await sentElsewherePage({ view: "main" });
  assert.equal(reply.el("open-in-view").value, "main");
  await sentOpenButton(reply).click();
  await untilDeepLink(reply, () => rowWithId(reply, "204")?.getAttribute("data-landed") === "true");
  assert.equal(reply.el("thread-select").value, "thread:spaces/A/threads/another", "the reply's thread did not open");

  const root = await sentElsewherePage({ message: "200", thread: null, view: "main" });
  await sentOpenButton(root).click();
  await untilDeepLink(root, () => rowWithId(root, "200")?.getAttribute("data-landed") === "true");
  assert.equal(root.el("thread-select").value, "main", "a root message did not land in Main");
  assert.equal(rowWithId(root, "200").getAttribute("data-landed"), "true");

  // The default is All, and the choice is one of the synced settings.
  const fresh = newPage();
  await signIn(fresh);
  assert.equal(fresh.el("open-in-view").value, "flat", "Open in channel does not land in All by default");
  // Absent means All, and stays absent until the reader chooses: a default written here would sync
  // over another device's choice.
  assert.equal(fresh.storage.has("vibe-talk.voice.open-in-view"), false, "the default was written as a choice");
  assert.ok(SETTINGS.SYNCED_SETTING_KEYS.includes(fresh.pure("OPEN_IN_VIEW_KEY")), "the choice is not synced");
  fresh.el("open-in-view").value = "main";
  await fresh.el("open-in-view").dispatch("change");
  await fresh.settle();
  assert.equal(fresh.storage.get("vibe-talk.voice.open-in-view"), "main");
});

// --- `#269 approval-card-inline` and `#270 voice-transcript-markers`: what becomes of a draft ---------

/** The "Not sent" lines in the transcript, in order. */
const notSentLines = (page) => page.el("transcript").children.filter((li) => li.getAttribute("data-not-sent"));

/** Every draft-not-sent marker the page kept, as `{id, marker}`, across conversations. */
const keptMarkers = (page) => [...page.storedTurns].flatMap(([id, held]) =>
  held.filter((turn) => turn.speaker === "marker" && turn.detail && turn.detail.kind === "not_sent")
    .map((turn) => ({ id, marker: turn.detail })));

/** Each row's shape, with a "Not sent" line's reason: `rowShapes`, for the draft-fate tests. */
const fateShapes = (page) => page.el("transcript").children.map((li) => li.getAttribute("data-not-sent")
  ? `NOT SENT ${li.getAttribute("data-not-sent")}`
  : (li.hasClass("sent-card") ? "CARD" : li.className) + (li.getAttribute("data-ends") ? `[ends ${li.getAttribute("data-ends")}]` : ""));

/** A draft of this call's on the card, past its hold. */
async function draftUp(page, overrides = {}) {
  const pending = proposal({ reply_to: null, ...overrides });
  page.propose(pending);
  await watchTurns(page);
  page.expireTimers(2000);
  return pending;
}

test("`#270 voice-transcript-markers`: Don't send leaves a kept 'Not sent' line once the server withdrew the draft, and none when it did not", async () => {
  const page = newPage();
  await recordingCall(page, "conv_withdrawn");
  const pending = await draftUp(page);
  await page.el("post-confirm-cancel").click();
  await watchTurns(page);
  const lines = notSentLines(page);
  assert.equal(lines.length, 1, fateShapes(page).join(" | "));
  assert.equal(seamLabel(lines[0]), "Not sent");
  assert.match(seamDetail(lines[0]), /^You tapped Don't send, so the draft was withdrawn and nothing was posted\./);
  assert.equal(page.el("transcript").children.at(-1), lines[0], "the line is not where the card was");
  assert.deepEqual(keptMarkers(page), [{ id: "conv_withdrawn", marker: { kind: "not_sent", why: "withdrawn", serial: pending.serial } }]);

  // Answered `proposal_used`: settled some other way before the tap arrived. What that was is the
  // send memory's or the proposal's fate's to say, not this tap's.
  const used = newPage();
  await recordingCall(used, "conv_used");
  await draftUp(used);
  const serving = used.postProposals;
  used.postProposals = async (path, options) => path === "/api/v1/post-proposals/cancel"
    ? json(409, { error: "proposal_used", detail: "already settled" })
    : serving(path, options);
  await used.el("post-confirm-cancel").click();
  await watchTurns(used);
  assert.equal(used.el("post-confirm").hidden, true);
  assert.deepEqual(notSentLines(used), [], "a withdrawal the server refused said the draft was withdrawn");
  assert.deepEqual(keptMarkers(used), []);
});

test("`#270 voice-transcript-markers`: a draft the watch takes down says it lapsed only when the server says it expired", async () => {
  for (const [what, fate] of [["lapsed", "expired"], ["replaced", "superseded"], ["committed elsewhere", "committed"], ["withdrawn elsewhere", "cancelled"]]) {
    const page = newPage();
    await recordingCall(page, "conv_fates");
    const pending = await draftUp(page);
    if (fate === "superseded") page.propose(proposal({ serial: 8, handle: "nEwEr-0123456789abcde", reply_to: null, text: "newer" }));
    else page.settleProposal(fate);
    await watchTurns(page);
    const lines = notSentLines(page);
    if (fate !== "expired") {
      assert.deepEqual(lines, [], `${what}: ${fateShapes(page).join(" | ")}`);
      assert.deepEqual(keptMarkers(page), [], what);
      continue;
    }
    assert.equal(page.el("post-confirm").hidden, true);
    assert.equal(lines.length, 1, fateShapes(page).join(" | "));
    assert.match(seamDetail(lines[0]), /^The draft lapsed before anyone sent it, so nothing was posted\./);
    assert.equal(page.el("status").textContent, "The draft to lead team lapsed; nothing was posted.");
    assert.deepEqual(keptMarkers(page), [{ id: "conv_fates", marker: { kind: "not_sent", why: "lapsed", serial: pending.serial } }]);
  }
});

test("`#269 approval-card-inline`: Hang up with a draft waiting retires it with a kept 'Not sent' line inside the call, but not while its send is on its way", async () => {
  const page = newPage();
  await recordingCall(page, "conv_hung");
  const pending = await draftUp(page);
  await page.el("hang-up").click();
  await watchTurns(page);
  assert.equal(page.el("post-confirm").hidden, true, "a Send button outlived the call it was hung up on");
  assert.deepEqual(fateShapes(page).slice(-2), ["NOT SENT ended", "seam[ends conv_hung]"]);
  const live = seamDetail(notSentLines(page)[0]);
  // True even if a reloaded page sends the draft before it lapses: Hang up put it away on this page only.
  assert.equal(live, "You hung up with a draft still waiting, so it was not posted then, and the page stopped offering it.");
  assert.deepEqual(keptMarkers(page), [{ id: "conv_hung", marker: { kind: "not_sent", why: "ended", serial: pending.serial } }]);

  // A reload draws the same line, in the same words, in the same place.
  const reloaded = newPage();
  reloaded.transcriptRecordsSupported = true;
  reloaded.storedTurns.set("conv_hung", page.storedTurns.get("conv_hung").map((turn) => ({ ...turn })));
  await signIn(reloaded);
  await reloaded.settle();
  assert.ok(reloaded.transcriptCalls.some((read) => read.includes("records=1")));
  assert.deepEqual(fateShapes(reloaded).slice(-2), ["NOT SENT ended", "seam[ends conv_hung]"]);
  assert.equal(seamDetail(notSentLines(reloaded)[0]), live);
  assert.equal(seamLabel(notSentLines(reloaded)[0]), "Not sent");

  // A send on its way when he hung up is settled by its answer: the card it froze into, inside the
  // call, and no line saying it was not sent.
  const busy = newPage();
  await recordingCall(busy, "conv_busy");
  await draftUp(busy);
  const answering = busy.commitProposal;
  let release = null;
  busy.commitProposal = async (body) => {
    const answer = await answering(body);
    await new Promise((resolve) => { release = resolve; });
    return answer;
  };
  const tapped = busy.el("post-confirm-send").click();
  await watchTurns(busy);
  assert.ok(release, "the commit never reached the fixture");
  await busy.el("hang-up").click();
  await watchTurns(busy);
  assert.deepEqual(notSentLines(busy), [], "a draft whose send was on its way was marked not sent");
  release();
  await tapped;
  await watchTurns(busy, 6);
  assert.deepEqual(fateShapes(busy).slice(-2), ["CARD", "seam[ends conv_busy]"]);
  assert.deepEqual(keptMarkers(busy), []);
  assert.equal(busy.el("post-confirm").hidden, true);
});

test("`#269 approval-card-inline`: a call that drops keeps its card, tap only, and Send still posts it, filed in the call that dropped", async () => {
  for (const [how, drop] of [
    ["closed by the service", (socket) => socket.close()],
    ["lost", (socket) => { socket.readyState = 3; socket.onclose({ code: 1006, reason: "" }); }],
  ]) {
    const page = newPage();
    const socket = await recordingCall(page, "conv_dropped");
    const pending = await draftUp(page);
    drop(socket);
    await watchTurns(page);
    assert.equal(page.el("post-confirm").hidden, false, `${how}: the drop took the draft away`);
    assert.match(page.el("post-confirm-expiry").textContent, /^Call dropped · Send still posts · 120 s$/, how);
    assert.equal(page.el("post-confirm-send").disabled, false, `${how}: the kept card cannot be sent`);
    assert.deepEqual(notSentLines(page), [], `${how}: a kept draft was marked not sent`);
    assert.equal(fateShapes(page).at(-1), "seam[ends conv_dropped]", how);
    // The countdown keeps going, still saying the call dropped.
    page.setClock(page.clock() + 1000);
    page.expireTimers(1000);
    assert.match(page.el("post-confirm-expiry").textContent, /^Call dropped · Send still posts · /, how);

    await page.el("post-confirm-send").click();
    await watchTurns(page, 6);
    assert.deepEqual(page.proposalCommits.map((c) => [c.handle, c.confirmed_by]), [[pending.handle, "ui"]], how);
    assert.equal(page.el("post-confirm").hidden, true, how);
    assert.deepEqual(fateShapes(page).slice(-2), ["CARD", "seam[ends conv_dropped]"], how);
    assert.deepEqual(page.claims.map((claim) => claim.id), ["conv_dropped"], how);
  }

  // THE CONTROL: the card taken down at every end of a call, as it was before. The drop then loses
  // the draft.
  const control = newPage(new Map(), brokenScript(
    "  if (postWatch.controller) postWatch.controller.abort();\n  postWatch.controller = null;\n",
    "  if (postWatch.controller) postWatch.controller.abort();\n  postWatch.controller = null;\n  showPostProposal(null);\n"
  ));
  const socket = await recordingCall(control, "conv_dropped");
  await draftUp(control);
  socket.close();
  await watchTurns(control);
  assert.equal(control.el("post-confirm").hidden, true, "the control kept the card: this test cannot see the defect");
});

test("`#269 approval-card-inline`: a card kept through a drop goes at its lapse, and says it lapsed only on the server's word", async () => {
  for (const expired of [true, false]) {
    const page = newPage();
    const socket = await recordingCall(page, "conv_kept");
    const pending = await draftUp(page, { expires_in_ms: 3000 });
    socket.close();
    await watchTurns(page);
    assert.equal(page.el("post-confirm").hidden, false);
    if (expired) page.lapseProposal();
    page.setClock(page.clock() + 3000);
    page.expireTimers(1000);
    await page.settle();
    assert.equal(page.el("post-confirm").hidden, true, `expired ${expired}: a lapsed card stayed up with no call to take it down`);
    assert.deepEqual(notSentLines(page), [], "the lapse was drawn before the server was asked");
    const reads = proposalWatches(page).length;
    page.expireTimers(1000);
    await watchTurns(page);
    assert.equal(proposalWatches(page).length, reads + 1, "the server was not asked what became of the draft");
    if (!expired) {
      assert.deepEqual(notSentLines(page), [], "a draft the server did not say expired was drawn as lapsed");
      continue;
    }
    assert.deepEqual(fateShapes(page).slice(-2), ["NOT SENT lapsed", "seam[ends conv_kept]"]);
    assert.deepEqual(keptMarkers(page), [{ id: "conv_kept", marker: { kind: "not_sent", why: "lapsed", serial: pending.serial } }]);
  }
});

test("`#269 approval-card-inline`: a card kept through a drop and sent in the next call is filed in the call it was drafted in", async () => {
  const page = newPage();
  const socket = await recordingCall(page, "conv_one");
  const pending = await draftUp(page);
  socket.close();
  await watchTurns(page);
  await page.el("talk").click();
  const next = page.sockets.at(-1);
  next.onopen();
  next.onmessage({ data: JSON.stringify({
    type: "conversation_initiation_metadata",
    conversation_initiation_metadata_event: { conversation_id: "conv_two", agent_output_audio_format: "pcm_16000" },
  }) });
  await watchTurns(page);
  assert.equal(page.el("post-confirm").hidden, false, "the next call did not show the draft the drop kept");
  assert.equal(page.el("post-confirm-text").textContent, pending.text);
  assert.match(page.el("post-confirm-expiry").textContent, /^Waiting for you/, "a call that is up still says the last one dropped");
  await page.el("post-confirm-send").click();
  await watchTurns(page, 6);
  assert.deepEqual(fateShapes(page).slice(-2), ["CARD", "seam[ends conv_one]"]);
  assert.deepEqual(page.claims.map((claim) => claim.id), ["conv_one"]);
});

test("`#270 voice-transcript-markers`: a spoken no that withdrew the draft is its kept 'Not sent' line, after the no", async () => {
  const page = newPage();
  const socket = await recordingCall(page, "conv_no");
  hostedHears(socket, "tell lead team I'm on it, rebasing now");
  const pending = spokenDraft();
  page.propose(pending);
  await page.settle();
  hostedSays(socket, `To lead team: ${pending.text}. Send it?`);
  hostedHears(socket, "No.");
  await watchTurns(page);
  assert.deepEqual(page.proposalCancels, [{ handle: pending.handle }]);
  const lines = notSentLines(page);
  assert.equal(lines.length, 1, fateShapes(page).join(" | "));
  assert.equal(lines[0].getAttribute("data-not-sent"), "declined");
  assert.match(seamDetail(lines[0]), /^You said no to the read-back, so the draft was withdrawn and nothing was posted\./);
  const rows = page.el("transcript").children;
  assert.equal(rows.at(-1), lines[0]);
  assert.match(wordsOf(rows.at(-2)), /No\./, "the line is not right after the no it answers");
  // Kept after the no, in the order the screen shows them.
  assert.deepEqual(page.storedTurns.get("conv_no").slice(-2).map((turn) => turn.speaker === "marker" ? turn.detail : turn.text),
    ["No.", { kind: "not_sent", why: "declined", serial: pending.serial }]);

  // A no whose withdrawal never reached the server withdrew nothing, and draws no line.
  const lost = newPage();
  const lostSocket = await recordingCall(lost, "conv_lost_no");
  hostedHears(lostSocket, "tell lead team I'm on it, rebasing now");
  lost.propose(spokenDraft());
  await lost.settle();
  hostedSays(lostSocket, `To lead team: ${spokenDraft().text}. Send it?`);
  const serving = lost.postProposals;
  lost.postProposals = async (path, options) => {
    if (path === "/api/v1/post-proposals/cancel") throw new TypeError("Failed to fetch");
    return serving(path, options);
  };
  hostedHears(lostSocket, "No.");
  await watchTurns(lost);
  assert.deepEqual(notSentLines(lost), [], "a no the server never heard was drawn as a withdrawal");
  assert.deepEqual(keptMarkers(lost), []);
});

test("`#270 voice-transcript-markers`: every sentence a 'Not sent' line can show is inside the seam band", () => {
  const table = sourceStringTable("NOT_SENT_SEAMS");
  assert.deepEqual(Object.keys(table).sort(), ["ended", "lapsed", "withdrawn"]);
  for (const [why, text] of Object.entries(table)) {
    const spent = words(text);
    assert.ok(spent >= SEAM_DETAIL_MIN_WORDS && spent <= SEAM_DETAIL_MAX_WORDS, `${why}: ${spent} words: "${text}"`);
  }
});

/** Call again, into the hosted conversation `conversationId`, without hanging up first. */
async function callInto(page, conversationId) {
  await page.el("talk").click();
  const socket = page.sockets.at(-1);
  socket.onopen();
  socket.onmessage({ data: JSON.stringify({
    type: "conversation_initiation_metadata",
    conversation_initiation_metadata_event: { conversation_id: conversationId, agent_output_audio_format: "pcm_16000" },
  }) });
  await page.settle();
  return socket;
}

test("`#270 voice-transcript-markers`: a call whose words a broken store would not keep is still the one the next call resumes from", async () => {
  // A call that recorded nothing because it said nothing is never resumed from. One that said
  // something the store would not take is: resuming an older call instead would send that call's
  // recap under a line saying it is "the conversation above".
  const page = newPage();
  page.transcriptRecordsSupported = true;
  await withEarlierConversation(page, { resume: true });
  const first = await startTalkingNamed(page, "conv_a");
  await page.settle();
  first.onmessage({ data: JSON.stringify({ type: "user_transcript", user_transcription_event: { user_transcript: "what happened overnight" } }) });
  await watchTurns(page);
  // The store goes down within the call: recording stops for the rest of the page's life.
  page.storeStatus = 503;
  first.onmessage({ data: JSON.stringify({ type: "agent_response", agent_response_event: { agent_response: "the runner stalled" } }) });
  await watchTurns(page);
  assert.match(page.el("storage-state").textContent, /Not recording/);
  await page.el("hang-up").click();
  await page.settle();

  const second = await callInto(page, "conv_b");
  second.onmessage({ data: JSON.stringify({ type: "user_transcript", user_transcription_event: { user_transcript: "and the arm64 job" } }) });
  await watchTurns(page);
  await page.el("hang-up").click();
  await page.settle();

  await callInto(page, "conv_c");
  assert.match(page.replayCalls.at(-1), /\/conversations\/conv_b\/replay$/,
    "the next call was sent an older call's recap, not the one above it");
});

test("`#270 voice-transcript-markers`: the line above a call says what recap it was sent, beside how the call before ended", async () => {
  const resume = sourceStringTable("RESUME_SEAM_DETAILS");
  const page = newPage();
  page.transcriptRecordsSupported = true;
  page.replayWords = 412;
  await withEarlierConversation(page, { resume: true });
  const socket = await startTalkingNamed(page, "conv_resumed");
  await page.settle();
  // The first call after a load: the restored record's own line, which says no cause.
  const first = boundaryLines(page).at(-1);
  assert.deepEqual(boundaryWords(first), {
    label: "new conversation",
    chip: "412-word recap sent",
    detail: resume.ended,
    recap: "This call was sent 412 words as a recap: the newest of what was said in your calls of the last 12 hours, word for word.",
    title: `${resume.ended} This call was sent 412 words as a recap: the newest of what was said in your calls of the last 12 hours, word for word.`,
  });
  // A turn, a Hang up, and the next call: the line the Hang up drew keeps #268's sentence for it,
  // in the resuming wording because a recap went, and gains the chip and its own sentence.
  socket.onmessage({ data: JSON.stringify({ type: "user_transcript", user_transcription_event: { user_transcript: "what happened overnight" } }) });
  await watchTurns(page);
  await page.el("hang-up").click();
  await page.settle();
  await callInto(page, "conv_next");
  const second = boundaryLines(page).at(-1);
  assert.equal(second.getAttribute("data-ends"), "conv_resumed");
  assert.equal(seamChip(second), "412-word recap sent");
  assert.equal(seamDetail(second), resume.ended, "the line lost #268's sentence for a Hang up");
  assert.match(seamRecapDetail(second), /^This call was sent 412 words/);

  // Carried on the start request: the caveat that the agent's own settings may ignore it.
  const carried = newPage();
  carried.replayTransport = "client_data";
  carried.replayWords = 1234;
  await withEarlierConversation(carried, { resume: true });
  await startTalkingNamed(carried, "conv_carried");
  await carried.settle();
  const line = boundaryLines(carried).at(-1);
  assert.equal(seamChip(line), "1,234-word recap sent");
  assert.equal(seamRecapDetail(line),
    "This call was sent 1,234 words above as a recap with its start request; the agent's own settings may ignore a recap sent that way.");
});

test("`#270 voice-transcript-markers`: a backend that cannot take a recap says 'recap unsupported', and resuming off says nothing", async () => {
  const stored = SEAM_DETAILS_STORED();
  for (const [what, arrange, chip, recap] of [
    // `#331 resume-on-websocket`: a bridge that answered the server's hello without `replay`.
    ["replay on, vibe-talk-v1", (page) => { page.replayUnavailable = "bridge_declined"; page.replayEnabled = false; }, "recap unsupported",
      "Nothing was recapped: this server's voice service has no way to take a recap, so the agent below remembers nothing above."],
    ["resuming off", () => {}, null, null],
  ]) {
    const page = newPage();
    arrange(page);
    page.storedTurns.set("conv_earlier", [{ speaker: "you", text: "what happened overnight", at_ms: 1_700_000_000_000 }]);
    await startNeutralCall(page);
    await page.settle();
    const line = boundaryLines(page).at(-1);
    assert.equal(seamLabel(line), "new conversation", what);
    assert.equal(seamChip(line), chip, what);
    assert.equal(seamRecapDetail(line), recap, what);
    assert.equal(seamDetail(line), stored, what);
  }
});

/** The stored record's own boundary sentence, read out of the page. */
function SEAM_DETAILS_STORED() {
  const found = /const STORED_SEAM_DETAIL =\s*((?:"(?:[^"\\]|\\.)*"\s*\+?\s*)+);/.exec(PAGE_CODE);
  assert.ok(found, "the page's code (web/voice.js, web/src/) no longer states a STORED_SEAM_DETAIL");
  return [...found[1].matchAll(/"((?:[^"\\]|\\.)*)"/g)].map((m) => m[1]).join("");
}

test("`#270 voice-transcript-markers`: a conversation's first record is its start marker, a call that records nothing files nothing and is not resumed from", async () => {
  const page = newPage();
  page.transcriptRecordsSupported = true;
  await withEarlierConversation(page, { resume: true });
  const socket = await startTalkingNamed(page, "conv_a");
  await page.settle();
  assert.equal(page.storedTurns.has("conv_a"), false, "a conversation was filed before anything was said in it");
  socket.onmessage({ data: JSON.stringify({ type: "user_transcript", user_transcription_event: { user_transcript: "what happened overnight" } }) });
  await watchTurns(page);
  const a = page.storedTurns.get("conv_a");
  assert.deepEqual(a.map((turn) => turn.speaker), ["marker", "you"]);
  assert.deepEqual(a[0].detail, {
    kind: "conversation_start",
    recap: { state: "sent", words: 11, lines: 2, dropped: 0, transport: "contextual_update" },
  });
  await page.el("hang-up").click();
  await page.settle();

  // A call the service closed at once: nothing said, nothing filed, and not the one resumed from.
  const dropped = await callInto(page, "conv_b");
  dropped.close();
  await page.settle();
  assert.equal(page.storedTurns.has("conv_b"), false, "a call that recorded nothing left a conversation");

  const third = await callInto(page, "conv_c");
  assert.match(page.replayCalls.at(-1), /\/conversations\/conv_a\/replay$/, "the next call resumed from a call that recorded nothing");
  third.onmessage({ data: JSON.stringify({ type: "agent_response", agent_response_event: { agent_response: "hello again" } }) });
  await watchTurns(page);
  assert.deepEqual(page.storedTurns.get("conv_c")[0].detail, {
    kind: "conversation_start",
    recap: { state: "sent", words: 3, lines: 1, dropped: 0, transport: "contextual_update" },
    previous_end: "closed",
  });

  // A typed message held until the conversation is named goes after the marker.
  const typed = newPage();
  typed.transcriptRecordsSupported = true;
  await signIn(typed);
  typed.setFetch(MINTED);
  await compose(typed, "what landed overnight");
  await typed.el("send-text").click();
  await typed.settle();
  typed.sockets[0].onopen();
  vendorAccepts(typed.sockets[0], "conv_typed_0002");
  await watchTurns(typed);
  const held = typed.storedTurns.get("conv_typed_0002");
  assert.deepEqual(held.map((turn) => turn.speaker === "marker" ? turn.detail.kind : turn.text),
    ["conversation_start", "what landed overnight"]);
  assert.deepEqual(held[0].detail.recap, { state: "none", why: "off", dropped: 0 });
});

test("`#270 voice-transcript-markers`: a restored conversation start says what the live line said, and an older record keeps 'earlier conversation'", async () => {
  // Live: a call that recorded a turn, a service close, and a resumed call that recorded one.
  const live = newPage();
  live.transcriptRecordsSupported = true;
  live.replayWords = 77;
  await withEarlierConversation(live, { resume: true });
  const first = await startTalkingNamed(live, "conv_one");
  await live.settle();
  first.onmessage({ data: JSON.stringify({ type: "user_transcript", user_transcription_event: { user_transcript: "check the runner" } }) });
  await watchTurns(live);
  first.close();
  await live.settle();
  const second = await callInto(live, "conv_two");
  second.onmessage({ data: JSON.stringify({ type: "user_transcript", user_transcription_event: { user_transcript: "and the nightly" } }) });
  await watchTurns(live);
  const said = boundaryWords(boundaryLines(live).find((li) => li.getAttribute("data-ends") === "conv_one"));
  const saidFirst = boundaryWords(boundaryLines(live).find((li) => li.getAttribute("data-ends") === "conv_earlier"));
  assert.equal(said.chip, "77-word recap sent");
  assert.equal(said.detail, sourceStringTable("RESUME_SEAM_DETAILS").closed, "#268's sentence for a service close was lost");

  // Restored from what was filed: the same words.
  const reloaded = newPage();
  reloaded.transcriptRecordsSupported = true;
  for (const [id, held] of live.storedTurns) reloaded.storedTurns.set(id, held.map((turn) => ({ ...turn })));
  await signIn(reloaded);
  await reloaded.settle();
  const restored = boundaryLines(reloaded).find((li) => li.getAttribute("data-ends") === "conv_one");
  assert.ok(restored, `no line ends conv_one: ${fateShapes(reloaded).join(" | ")}`);
  assert.deepEqual(boundaryWords(restored), said, "a reload says something else on the line");
  // The first call after the load started below the restored record's own line, which said no cause.
  const restoredFirst = boundaryLines(reloaded).find((li) => li.getAttribute("data-ends") === "conv_earlier");
  assert.deepEqual(boundaryWords(restoredFirst), saidFirst, "a reload says something else on the first call's line");
  // Below the newest conversation, before any call has opened, the record's own legacy line.
  const legacy = boundaryLines(reloaded).at(-1);
  assert.equal(legacy.getAttribute("data-ends"), "conv_two");
  assert.equal(seamLabel(legacy), "earlier conversation");
  assert.equal(seamChip(legacy), null);
});

/**
 * Where each row of the transcript says it belongs: a late row, a line ending a conversation, or
 * something said, by the words it holds. `#269`/`#270`, for rows filed late into an older call.
 */
const spanShapes = (page) => page.el("transcript").children.map((li) => {
  if (li.getAttribute("data-not-sent")) return `NOT SENT ${li.getAttribute("data-not-sent")}`;
  if (li.hasClass("sent-card")) return "CARD";
  if (li.getAttribute("data-ends")) return `ends ${li.getAttribute("data-ends")}`;
  const said = ["check the runner", "and the nightly?", "it passed", "thanks"].find((words) => wordsOf(li).includes(words));
  return said || null;
}).filter((shape) => shape !== null);

test("`#269 approval-card-inline` `#270 voice-transcript-markers`: a card kept through a drop and answered in the next call is restored where it was drawn, and the next call still resumes the call it was answered in", async () => {
  // The page files Send, Don't send and a lapse on such a card in the call it was DRAFTED in. The
  // server stamps each row as it arrives, so the fixture does too: a row stamped later than the
  // newer call's rows, and read back in time order, would be drawn in the middle of that call,
  // under a line saying its later half had ended — and would make the older call the one a new
  // call resumes from.
  const ways = [["send", "CARD"], ["withdraw", "NOT SENT withdrawn"], ["lapse", "NOT SENT lapsed"], ["late answer", "CARD"]];
  for (const [how, late] of ways) {
    const live = newPage();
    live.transcriptRecordsSupported = true;
    const first = await startTalkingNamed(live, "conv_one");
    await live.settle();
    hostedHears(first, "check the runner");
    await watchTurns(live);
    await draftUp(live);
    // A Send tapped in the first call whose commit reaches the server only once the next call
    // is under way: its answer, and the send memory's word, arrive in the next call.
    let release = null;
    if (how === "late answer") {
      const answering = live.commitProposal;
      live.commitProposal = async (body) => {
        await new Promise((resolve) => { release = resolve; });
        return answering(body);
      };
      void live.el("post-confirm-send").click();
      await watchTurns(live);
    }
    first.close();
    await watchTurns(live);
    const next = await callInto(live, "conv_two");
    hostedHears(next, "and the nightly?");
    hostedSays(next, "it passed");
    await watchTurns(live);
    if (how === "late answer") {
      assert.ok(release, "the commit never reached the fixture");
      release();
    } else {
      assert.equal(live.el("post-confirm").hidden, false, `${how}: the next call did not show the kept draft`);
    }
    if (how === "send") await live.el("post-confirm-send").click();
    else if (how === "withdraw") await live.el("post-confirm-cancel").click();
    else if (how === "lapse") live.lapseProposal();
    await watchTurns(live, 6);
    assert.deepEqual(live.claims.map((claim) => claim.id), late === "CARD" ? ["conv_one"] : [], `${how}: filed elsewhere`);
    const drawn = ["check the runner", late, "ends conv_one", "and the nightly?", "it passed"];
    assert.deepEqual(spanShapes(live), drawn, `${how}: drawn live somewhere else`);
    const said = (id) => live.storedTurns.get(id).filter((turn) => turn.speaker === "you" || turn.speaker === "agent")
      .map((turn) => turn.at_ms);
    assert.ok(Math.min(...said("conv_two")) > Math.max(...said("conv_one")),
      `${how}: the fixture stamped the two calls at one instant, which hides where a late row is read back`);

    // The late row was the last thing written: a reload draws it where it was, and the next call
    // still resumes from the call the owner was just in.
    const reloaded = await reloadOf(live);
    assert.deepEqual(spanShapes(reloaded), [...drawn, "ends conv_two"], `${how}: restored somewhere else`);
    await reloaded.el("resume-toggle").setChecked(true);
    await startTalking(reloaded);
    await reloaded.settle();
    assert.match(reloaded.replayCalls.at(-1) || "", /\/conversations\/conv_two\/replay$/,
      `${how}: the next call resumed from the call the draft was made in, not the one it was answered in`);

    // And the call it was answered in carried on: still one conversation after a reload.
    hostedHears(next, "thanks");
    await watchTurns(live);
    const again = await reloadOf(live);
    assert.deepEqual(spanShapes(again), [...drawn, "thanks", "ends conv_two"], `${how}: the newer call was split in two`);
  }
});

test("`#270 voice-transcript-markers`: a step back adds no second line above a page that starts with a conversation's start marker", async () => {
  const page = newPage();
  page.transcriptRecordsSupported = true;
  page.storedTurns.set("conv_old", [
    { speaker: "you", text: "the oldest question", at_ms: 1_699_000_000_000 },
    { speaker: "agent", text: "the oldest answer", at_ms: 1_699_000_000_001 },
  ]);
  const start = { kind: "conversation_start", recap: { state: "none", why: "empty", dropped: 0 }, previous_end: "error" };
  page.storedTurns.set("conv_new", [
    { speaker: "marker", text: "", at_ms: 1_700_000_000_000, detail: start },
    ...Array.from({ length: 39 }, (_, at) => ({ speaker: at % 2 ? "agent" : "you", text: `line ${at}`, at_ms: 1_700_000_000_001 + at })),
  ]);
  await signIn(page);
  await page.settle();
  const head = page.el("transcript").children[0];
  assert.ok(head.hasAttribute("data-marker"), `the page does not open on the conversation's start: ${fateShapes(page).slice(0, 2).join(" | ")}`);
  assert.equal(seamChip(head), "no recap sent");
  await page.el("load-older-turns").click();
  await page.settle();
  const rows = page.el("transcript").children;
  const lines = rows.filter((li) => li.className === "seam" && rows.indexOf(li) < rows.indexOf(head));
  assert.deepEqual(lines, [], "a step back drew a second line above the conversation's own");
  assert.equal(head.getAttribute("data-ends"), "conv_old");
  assert.equal(seamDetail(head), sourceStringTable("SEAM_DETAILS").error);
});

test("`#270 voice-transcript-markers`: a replacement voice session is a new conversation of its own, sent no recap, and says so", async () => {
  const page = newPage();
  page.transcriptRecordsSupported = true;
  const socket = await startTalkingNamed(page, "conv_first_session");
  socket.onmessage({ data: JSON.stringify({ type: "user_transcript", user_transcription_event: { user_transcript: "before the swap" } }) });
  await watchTurns(page);
  socket.onmessage({ data: JSON.stringify({
    type: "conversation_initiation_metadata",
    conversation_initiation_metadata_event: { conversation_id: "conv_second_session", agent_output_audio_format: "pcm_16000" },
  }) });
  socket.onmessage({ data: JSON.stringify({ type: "user_transcript", user_transcription_event: { user_transcript: "after the swap" } }) });
  await watchTurns(page);
  const line = boundaryLines(page).at(-1);
  assert.equal(line.getAttribute("data-ends"), "conv_first_session");
  assert.equal(seamChip(line), "no recap sent");
  assert.match(seamDetail(line), /^The voice service started a new session inside this call\./);
  assert.match(seamRecapDetail(line), /^The voice service replaced this call's session/);
  assert.deepEqual(page.storedTurns.get("conv_second_session")[0].detail,
    { kind: "conversation_start", recap: { state: "none", why: "replaced_session", dropped: 0 } });

  // A reload draws the same line, in the same words: its marker names no call that ended, and its
  // recap says the session was a replacement.
  const reloaded = newPage();
  reloaded.transcriptRecordsSupported = true;
  for (const [id, turns] of page.storedTurns) reloaded.storedTurns.set(id, turns.map((turn) => ({ ...turn })));
  await signIn(reloaded);
  await reloaded.settle();
  const restored = boundaryLines(reloaded).find((li) => li.getAttribute("data-ends") === "conv_first_session");
  assert.ok(restored, "the reload drew no line between the two sessions");
  assert.equal(seamLabel(restored), "new conversation");
  assert.equal(seamChip(restored), seamChip(line));
  assert.equal(seamDetail(restored), seamDetail(line));
  assert.equal(seamRecapDetail(restored), seamRecapDetail(line));
});

test("`#270 voice-transcript-markers`: a replay that does not say how many words it held is a 'recap sent', never a '0-word recap'", async () => {
  // Found by the screenshot walk, whose canned replay answer is an older server's: the line read
  // "0-word recap sent" above a call that had been sent three lines.
  for (const [maxTurns, sentence] of [
    [40, "This call was sent the lines as a recap: the newest of what was said in your calls of the last 12 hours, word for word."],
    [1, "This call was sent the last lines of your calls of the last 12 hours; 1 older line did not fit, so it cannot see those."],
  ]) {
    const page = newPage();
    page.replayWords = false;
    page.replayMaxTurns = maxTurns;
    await withEarlierConversation(page, { resume: true });
    await startTalking(page);
    await page.settle();
    const line = boundaryLines(page).at(-1);
    assert.equal(seamChip(line), "recap sent", `max_turns ${maxTurns}`);
    assert.equal(seamRecapDetail(line), sentence);
  }
  // A marker filed with no count says so after a reload, too.
  const restored = newPage();
  restored.transcriptRecordsSupported = true;
  restored.storedTurns.set("conv_a", [{ speaker: "you", text: "check the runner", at_ms: 1_700_000_000_000 }]);
  restored.storedTurns.set("conv_b", [
    { speaker: "marker", text: "", at_ms: 1_700_000_000_010, detail: {
      kind: "conversation_start", recap: { state: "sent", words: 0, lines: 1, dropped: 0, transport: "contextual_update" },
    } },
    { speaker: "you", text: "and the nightly", at_ms: 1_700_000_000_011 },
  ]);
  await signIn(restored);
  await restored.settle();
  const marked = boundaryLines(restored).find((li) => li.getAttribute("data-ends") === "conv_a");
  assert.equal(seamChip(marked), "recap sent");
});

test("`#270 voice-transcript-markers`: the recap chip is at most three words and 24 characters, and every recap sentence is inside the seam band", async () => {
  const chip = newPage().pure("recapChip");
  for (const recap of [
    { state: "sent", words: 99999, lines: 1, dropped: 0, transport: "contextual_update" },
    { state: "none", why: "unsupported", dropped: 0 },
    { state: "none", why: "too_long", dropped: 3 },
    // `#331 resume-on-websocket`.
    { state: "none", why: "refused", dropped: 0 },
    // `#512 voice-recap-private-stack`.
    { state: "none", why: "not_taken", dropped: 0 },
  ]) {
    const said = chip(recap);
    assert.ok(said.length <= 24 && said.split(/\s+/).length <= 3, `"${said}"`);
  }
  assert.equal(chip({ state: "sent", words: 99999, lines: 1, dropped: 0, transport: "contextual_update" }), "99,999-word recap sent");
  assert.equal(chip({ state: "none", why: "off", dropped: 0 }), null);
  const table = sourceStringTable("RECAP_DETAILS");
  assert.deepEqual(Object.keys(table).sort(),
    ["client_data", "dropped", "empty", "failed", "not_taken", "refused", "replaced_session", "sent", "too_long", "unsupported"]);
  for (const [state, text] of Object.entries(table)) {
    const spent = words(text);
    assert.ok(spent >= SEAM_DETAIL_MIN_WORDS && spent <= SEAM_DETAIL_MAX_WORDS, `${state}: ${spent} words: "${text}"`);
  }
});

test("a read-scope call never asks about proposals", async () => {
  const page = newPage();
  page.tokenScope = "read";
  await startTalking(page);
  await page.settle();
  page.propose(proposal());
  await page.settle();
  assert.deepEqual(proposalWatches(page), []);
  assert.equal(page.el("post-confirm").hidden, true);
});

test("no proposal is watched for outside a call, and hanging up takes the card away", async () => {
  const page = newPage();
  await signIn(page);
  await page.settle();
  assert.deepEqual(proposalWatches(page), [], "an idle page watched for proposals");

  await proposedDuringACall(page);
  await page.el("hang-up").click();
  await page.settle();
  assert.equal(page.el("post-confirm").hidden, true, "a Send button outlived the call");
  const asked = proposalWatches(page).length;
  page.propose(proposal({ serial: 9, handle: "lAtEr-0123456789abcde" }));
  await page.settle();
  assert.equal(page.el("post-confirm").hidden, true, "a proposal reached the card after hang-up");
  assert.equal(proposalWatches(page).length, asked, "the watch kept asking after hang-up");
});

test("a draft left pending by the last call is not offered in the next one", async () => {
  const page = newPage();
  const stale = await proposedDuringACall(page);
  await page.el("hang-up").click();
  await page.settle();
  assert.equal(page.proposal, stale, "hanging up is not a decision about the proposal");

  await page.el("talk").click();
  assert.equal(page.sockets.length, 2, "the second call did not open a new socket");
  page.sockets[1].onopen();
  await page.settle();
  assert.equal(state(page), "live");
  assert.ok(proposalWatches(page).length > 2, "the second call never looked for proposals");
  assert.equal(page.el("post-confirm").hidden, true, "the last call's draft came back with a live Send");

  const fresh = proposal({ serial: 8, handle: "fReSh-0123456789abcde", text: "a new call's draft" });
  page.propose(fresh);
  await page.settle();
  assert.equal(page.el("post-confirm").hidden, false, "the new call's own proposal was hidden");
  assert.equal(page.el("post-confirm-text").textContent, fresh.text);
  assert.equal(page.el("post-confirm-send").disabled, true, "a first card in a call was not held");
  assert.equal(page.expireTimers(2000), 1, "no hold was armed");
  assert.equal(page.el("post-confirm-send").disabled, false, "Send stayed held");
});

const VOICE_SEND_ON = "The agent reads the message back and asks 'Send it?'; say yes or tap Send.";
const VOICE_SEND_OFF =
  "The agent sends as soon as you ask. A message it reads could also talk it into sending.";

test("`#264 voice-send-approval`: the Voice checkbox shows the server's setting with one honest line for each state", async () => {
  for (const [ask, line] of [[true, VOICE_SEND_ON], [false, VOICE_SEND_OFF]]) {
    const page = newPage();
    page.voiceSettings = { ask_before_send: ask, changeable: true };
    await signIn(page);
    assert.equal(page.el("voice-send-editor").hidden, false);
    assert.equal(page.el("voice-ask-before-send").checked, ask);
    assert.equal(page.el("voice-ask-before-send").disabled, false);
    assert.equal(page.el("voice-ask-before-send-state").textContent, line);
  }
  const group = settingsGroup("Sending");
  assert.match(group, /Ask me before the voice agent sends/);
  assert.match(group, /data-help="voice-send"/, "the group offers no way to reach the detail");
  assert.match(helpEntry("voice-send"), /cannot tell your voice from anyone else/);
});

test("`#264 voice-send-approval`: unticking the box saves it on the server and redraws from what the server stored", async () => {
  const page = newPage();
  await signIn(page);
  await page.el("voice-ask-before-send").setChecked(false);
  await page.settle();
  assert.deepEqual(page.voiceSettingsCalls, [{ ask_before_send: false }]);
  assert.equal(page.el("voice-ask-before-send").checked, false);
  assert.equal(page.el("voice-ask-before-send").disabled, false, "the box stayed locked after the save");
  assert.equal(page.el("voice-ask-before-send-state").textContent, VOICE_SEND_OFF);
  // The server changes at once, and the line says so. The fixture's agent is hosted and has not
  // been called, so whether its instructions follow is conditional, and the line says that too.
  assert.match(page.el("status").textContent,
    /server now posts as soon as the voice agent sends; its instructions follow at its next call only if its provider lets this page replace its prompt; otherwise paste the new one into its console by hand\.$/);

  // The server's answer is the truth, not the click: one that kept asking redraws the box ticked.
  page.voiceSettings = { ask_before_send: true, changeable: true };
  const realFetch = page.voiceSettingsStatus;
  assert.equal(realFetch, null);
  await page.el("voice-ask-before-send").setChecked(true);
  await page.settle();
  assert.equal(page.el("voice-ask-before-send").checked, true);
  assert.equal(page.el("voice-ask-before-send-state").textContent, VOICE_SEND_ON);
});

test("`#264 voice-send-approval`: a save the server refuses puts the box back and says the setting did not change", async () => {
  const page = newPage();
  await signIn(page);
  page.voiceSettingsStatus = 503;
  await page.el("voice-ask-before-send").setChecked(false);
  await page.settle();
  assert.equal(page.el("voice-ask-before-send").checked, true, "the box kept a position the server refused");
  assert.equal(page.el("voice-ask-before-send-state").textContent, VOICE_SEND_ON);
  assert.equal(page.el("error").hidden, false);
  assert.match(page.el("error").textContent, /voice send setting was not changed/);
});

test("`#264 voice-send-approval`: the box cannot be changed by a read token or on a server with no store, and is hidden by an older server", async () => {
  const read = newPage();
  read.tokenScope = "read";
  await signIn(read);
  assert.equal(read.el("voice-ask-before-send").disabled, true);
  assert.match(read.el("voice-ask-before-send-state").textContent, /Only the write token can change this\.$/);

  const storeless = newPage();
  storeless.voiceSettings = { ask_before_send: true, changeable: false };
  await signIn(storeless);
  assert.equal(storeless.el("voice-ask-before-send").disabled, true);
  assert.match(storeless.el("voice-ask-before-send-state").textContent, /keeps no settings/);

  const older = newPage();
  older.voiceSettings = undefined;
  await signIn(older);
  assert.equal(older.el("voice-send-editor").hidden, true);
  assert.match(older.el("voice-ask-before-send-state").textContent, /always asks/);
});

test("`#264 voice-send-approval`: what counts as yes, as no, and as neither", () => {
  const classify = newPage().pure("classifySpokenAnswer");
  for (const said of [
    "yes", "Yes.", "YES!", "yeah", "yep", "send it", "Send.", "do it", "go ahead", "sure",
    "please do", "yes please", "yes, send it", "send it please", "ok send it", "yeah go ahead",
    "please send it",
  ]) {
    assert.equal(classify(said), "affirmative", said);
  }
  for (const said of [
    "no", "No.", "nope", "don't send it", "Don’t send it", "do not send", "cancel", "stop",
    "wait", "wait, change it to Friday", "never mind",
  ]) {
    assert.equal(classify(said), "negative", said);
  }
  for (const said of [
    "yes and tell him I'm late", "send it to Bob", "Send it?", "yes?", "sure thing", "okay",
    "yes please now", "yes no",
  ]) {
    assert.notEqual(classify(said), "affirmative", said);
    assert.notEqual(classify(said), "negative", said);
  }
  assert.equal(classify("what was the last message"), "other");
  assert.equal(classify(""), "other");
});

/** The notices this page sent the agent, in order: hosted `user_message`s that start "Voice page:". */
const pageNotices = (socket) =>
  socket.sent
    .filter((frame) => typeof frame === "string")
    .map((frame) => JSON.parse(frame))
    .filter((event) => event.type === "user_message" && String(event.text).startsWith("Voice page:"))
    .map((event) => event.text);

test("`#264 voice-send-approval`: a plain yes after an uninterrupted read-back sends exactly the card, once, as the speaker's turn, and says so", async () => {
  const page = newPage();
  const pending = spokenDraft();
  const socket = await readBackDuringACall(page, { pending });
  const sources = page.audio.sources.length;
  hostedHears(socket, "Yes.");
  assert.deepEqual(page.proposalCommits, [], "a yes sent before its settle window passed");
  assert.match(page.el("status").textContent, /Heard yes/);
  assert.equal(page.expireTimers(SPOKEN_SETTLE_MS) >= 1, true, "no settle window was armed");
  await page.settle();
  await page.settle();
  assert.deepEqual(page.proposalCommits, [{
    handle: pending.handle,
    channel_id: pending.channel_id,
    text: pending.text,
    reply_to: null,
    confirmed_by: "speaker_turn",
  }]);
  assert.equal(page.el("post-confirm").hidden, true, "the card outlived the post");
  assert.equal(page.el("status").textContent, "Sent to lead team.");
  const notices = pageNotices(socket);
  assert.equal(notices.length, 1, notices.join("\n"));
  assert.match(notices[0], /^Voice page: the message you read back was SENT to lead team\./);
  assert.ok(
    !session_lines(page).some((li) => li.text().includes("Voice page")),
    "the page's notice was shown as the owner's words"
  );
  // `#270 voice-transcript-markers`. The send is a frozen card that says how it was approved, and no
  // seam says it a second time.
  assert.ok(!page.el("transcript").children.some((li) => li.className === "seam"), "a seam said it again");
  const card = page.el("transcript").children.find((li) => li.hasClass("sent-card"));
  assert.ok(card, "the send left no frozen card");
  const words = (node) => [node.textContent, ...node.children.map(words)].join(" ");
  assert.match(words(card), /Sent to lead team/);
  assert.match(words(card), /approved by voice/);
  assert.deepEqual(tonesSince(page, sources), ["sent"], "no rising pair said it was sent");

  // Saying it again is not a second send: there is nothing on the card to send.
  hostedHears(socket, "yes");
  await settleSpokenYes(page);
  assert.equal(page.proposalCommits.length, 1);
});

test("`#264 voice-send-approval`: on the neutral protocol a yes counts after a completed read-back turn, and the agent is told in a queued prompt", async () => {
  const page = newPage();
  const pending = spokenDraft();
  const socket = await startNeutralCall(page);
  await page.settle();
  speech(socket, "user", 1, "tell lead team I'm on it, rebasing now", true);
  page.propose(pending);
  await page.settle();
  speech(socket, "assistant", 1, "To lead team: on it,", false);
  speech(socket, "assistant", 1, `To lead team: ${pending.text}. Send it?`, true);
  turnDone(socket, 1);
  speech(socket, "user", 2, "yes", false);
  speech(socket, "user", 2, "yes please", true);
  await settleSpokenYes(page);
  assert.deepEqual(page.proposalCommits.map((c) => [c.handle, c.confirmed_by]), [[pending.handle, "speaker_turn"]]);
  // The notice waits for the spoken turn to close, as every typed turn does on this protocol — and
  // `#268 voice-call-drops`: it does not even close the microphone's segment while the owner's turn
  // is still open, because that turn's own turn_complete is still to come.
  assert.deepEqual(promptsSent(socket), [], "the notice raced the open microphone turn");
  const stringFrames = () => socket.sent.filter((f) => typeof f === "string").map((f) => JSON.parse(f).type);
  assert.ok(!stringFrames().includes("audio_end"), "the segment was closed inside the owner's open turn");
  turnDone(socket, 2);
  // ...nor inside the backend's hand-off after that turn's turn_complete, which the microphone waits
  // out as well. `#268 voice-call-drops`.
  assert.ok(!stringFrames().includes("audio_end"), "the segment was closed inside the hand-off");
  page.expireTimers(sourceConstant("VOICE_CAPTURE_RESUME_MS"));
  assert.deepEqual(JSON.parse(socket.sent.at(-1)), { type: "audio_end" });
  assert.deepEqual(promptsSent(socket), [], "the notice went before the segment's own boundary");
  turnDone(socket, 3);
  const prompts = promptsSent(socket);
  assert.equal(prompts.length, 1, prompts.join("\n"));
  assert.match(prompts[0], /^Voice page: the message you read back was SENT to lead team\./);
  assert.ok(!session_lines(page).some((li) => li.text().includes("Voice page")));
});

test("`#264 voice-send-approval`: a no to the read-back withdraws the draft and sends nothing", async () => {
  for (const said of ["No.", "don't send it", "Cancel", "wait, change it to Friday"]) {
    const page = newPage();
    const pending = spokenDraft();
    const socket = await readBackDuringACall(page, { pending });
    hostedHears(socket, said);
    await page.settle();
    await page.settle();
    assert.deepEqual(page.proposalCancels, [{ handle: pending.handle }], said);
    assert.deepEqual(page.proposalCommits, [], said);
    assert.equal(page.el("post-confirm").hidden, true, said);
    assert.deepEqual(pageNotices(socket), [], `${said}: the agent heard the no itself`);
    assert.equal(page.el("status").textContent, "Not sent.", said);
  }
});

test("`#264 voice-send-approval`: a yes inside a sentence that adds anything sends nothing, leaves the card, and is answered", async () => {
  for (const said of ["yes and tell him I'm late", "send it to Bob", "Send it?", "sure thing", "yep that's right, send it"]) {
    const page = newPage();
    const socket = await readBackDuringACall(page);
    const before = page.audio.played;
    hostedHears(socket, said);
    await settleSpokenYes(page);
    // Held a moment in case it was a new request, whose draft would replace the card. `#579`.
    assert.deepEqual(pageNotices(socket), [], `${said}: told before the card had a chance to change`);
    await settleRedraftWait(page);
    assert.deepEqual(page.proposalCommits, [], said);
    assert.equal(page.el("post-confirm").hidden, false, said);
    assert.match(page.el("status").textContent, /^Not sent — only a plain answer such as yes or send it counts/, said);
    // The agent was told to say only "One moment." on a yes, so anything like one gets the truth.
    const notices = pageNotices(socket);
    assert.equal(notices.length, 1, `${said}: ${notices.join("\n")}`);
    assert.match(notices[0], /^Voice page: the message was NOT sent: only a plain answer/, said);
    assert.match(notices[0], /Otherwise carry on\.$/, said);
    assert.equal(page.audio.played, before + 1, `${said}: no tone said it was not sent`);
  }
});

test("`#264 voice-send-approval`: a yes after an interrupted read-back sends nothing, and the agent is told why", async () => {
  const page = newPage();
  const socket = await readBackDuringACall(page);
  hosted(socket, { type: "interruption", interruption_event: {} });
  hostedHears(socket, "yes");
  await settleSpokenYes(page);
  assert.deepEqual(page.proposalCommits, []);
  assert.equal(page.el("post-confirm").hidden, false);
  const notices = pageNotices(socket);
  assert.equal(notices.length, 1, notices.join("\n"));
  assert.match(notices[0], /^Voice page: the message was NOT sent: the read-back was interrupted\./);

  const neutral = newPage();
  const v1 = await startNeutralCall(neutral);
  await neutral.settle();
  speech(v1, "user", 1, "tell lead team I'm on it, rebasing now", true);
  neutral.propose(spokenDraft());
  await neutral.settle();
  speech(v1, "assistant", 1, "To lead team: on it, rebasing now. Send it?", true);
  v1.onmessage({ data: JSON.stringify({ type: "turn_complete", turn: 1, interrupted: true }) });
  speech(v1, "user", 2, "yes", true);
  await settleSpokenYes(neutral);
  assert.deepEqual(neutral.proposalCommits, []);
  assert.match(neutral.el("status").textContent, /^Not sent — the read-back was interrupted\./);
});

test("`#264 voice-send-approval`: a read-back that left words out, or never asked, cannot be confirmed by voice", async () => {
  for (const [said, why] of [
    ["To lead team: on it. Send it?", /not the card's text word for word/],
    ["To lead team: on it, rebasing now. Shall I post that?", /did not end with the question Send it/],
  ]) {
    const page = newPage();
    const socket = await readBackDuringACall(page, { said });
    hostedHears(socket, "yes");
    await settleSpokenYes(page);
    assert.deepEqual(page.proposalCommits, [], said);
    const notices = pageNotices(socket);
    assert.equal(notices.length, 1, `${said}: ${notices.join("\n")}`);
    assert.match(notices[0], why, said);
  }
  // A statement asks nothing, so a yes after it is not one the agent waits on: no send, and the
  // status line says why.
  const statement = newPage();
  const socket = await readBackDuringACall(statement, { said: "I will send this to lead team: on it, rebasing now." });
  hostedHears(socket, "yes");
  await settleSpokenYes(statement);
  assert.deepEqual(statement.proposalCommits, []);
  assert.deepEqual(pageNotices(socket), []);
  assert.match(statement.el("status").textContent, /a yes counts only as the answer to the read-back/);
});

test("`#264 voice-send-approval`: a draft replaced after its read-back needs a read-back of its own — a shorter one is not confirmed by the longer one's", async () => {
  const page = newPage();
  const socket = await readBackDuringACall(page, { pending: spokenDraft({ text: "don't merge the PR" }) });
  const shorter = spokenDraft({ serial: 8, handle: "sHoRtEr-0123456789abcd", text: "merge the PR" });
  page.propose(shorter);
  await page.settle();
  assert.equal(page.el("post-confirm-text").textContent, "merge the PR");
  hostedHears(socket, "yes");
  await settleSpokenYes(page);
  assert.deepEqual(page.proposalCommits, [], "a read-back of one draft confirmed another");
  assert.match(page.el("status").textContent, /not the card's text word for word/);
  assert.equal(pageNotices(socket).length, 1, "the refused yes was not answered");

  // Read back as it now stands, it may be confirmed — once the refusal's tone has played.
  page.setClock(page.clock() + 1000);
  hostedSays(socket, "To lead team: merge the PR. Send it?");
  hostedHears(socket, "yes");
  await settleSpokenYes(page);
  assert.deepEqual(page.proposalCommits.map((c) => [c.handle, c.text]), [[shorter.handle, "merge the PR"]]);
});

test("`#264 voice-send-approval`: a yes before the read-back finished playing, or a bare 'send it' in its echo, sends nothing — a plain yes then does", async () => {
  const early = newPage();
  const earlySocket = await readBackDuringACall(early);
  hostedAudio(earlySocket);
  hostedHears(earlySocket, "yes");
  await settleSpokenYes(early);
  assert.deepEqual(early.proposalCommits, []);
  assert.match(pageNotices(earlySocket)[0], /before the read-back finished playing/);

  const playedMs = (0.25 + 1) * 1000; // the lead banked before an answer starts, then one second.
  const echo = newPage();
  const echoSocket = await readBackDuringACall(echo);
  hostedAudio(echoSocket);
  echo.setClock(echo.clock() + playedMs + 500);
  hostedHears(echoSocket, "send it");
  await settleSpokenYes(echo);
  assert.deepEqual(echo.proposalCommits, [], "the agent's own 'send it' was taken as the answer");
  assert.match(pageNotices(echoSocket)[0], /assistant's own voice, so answer the next read-back with a plain yes/);

  const plain = newPage();
  const plainSocket = await readBackDuringACall(plain);
  hostedAudio(plainSocket);
  plain.setClock(plain.clock() + playedMs + 500);
  hostedHears(plainSocket, "yes");
  await settleSpokenYes(plain);
  assert.deepEqual(plain.proposalCommits.map((c) => c.confirmed_by), ["speaker_turn"]);
});

test("`#264 voice-send-approval`: a tap during the settle window commits once, as the tap, and the agent hears that result", async () => {
  const page = newPage();
  const socket = await readBackDuringACall(page);
  page.expireTimers(2000); // the card's own hold on Send
  hostedHears(socket, "yes");
  await page.el("post-confirm-send").click();
  await page.settle();
  await settleSpokenYes(page);
  assert.deepEqual(page.proposalCommits.map((c) => c.confirmed_by), ["ui"]);
  const notices = pageNotices(socket);
  assert.equal(notices.length, 1, notices.join("\n"));
  assert.match(notices[0], /SENT to lead team/);
});

test("`#264 voice-send-approval`: Don't send tapped during the settle window withdraws the draft, and the agent is told once", async () => {
  const page = newPage();
  const socket = await readBackDuringACall(page);
  page.expireTimers(2000); // the card's own hold on both buttons, `#269 approval-card-inline`
  hostedHears(socket, "yes");
  await page.el("post-confirm-cancel").click();
  await page.settle();
  await settleSpokenYes(page);
  assert.deepEqual(page.proposalCommits, []);
  assert.equal(page.proposalCancels.length, 1);
  const notices = pageNotices(socket);
  assert.equal(notices.length, 1, notices.join("\n"));
  assert.match(notices[0], /^Voice page: the user tapped Don't send, so the draft was withdrawn and NOT sent\./);
});

// `#269 approval-card-inline`. Both buttons are held for a moment after a card appears, so a tap aimed
// at the row that was there lands on neither. A spoken yes can settle inside that moment — the card
// reached the page after its read-back, on a slow or reconnecting long poll, and the owner answered
// at once — and then the card says "tap Don't send to stop". Held, that tap was eaten and the message
// posted. Never held while a yes to the card settles; held again once it stops, until the hold is over.
const CARD_HOLD_MS = sourceConstant("POST_CHANGE_HOLD_MS");
/** Whether the card's hold after it appeared or changed is still running. */
const cardHeld = (page) => [...page.timers.values()].some((timer) => timer.ms === CARD_HOLD_MS);

/** Tap Don't send while the yes settles inside the hold, and check that it stopped the send. */
async function dontSendStopsTheYes(page, socket, pending) {
  assert.ok(cardHeld(page), "the card's hold was over: this tests nothing");
  assert.equal(page.el("post-confirm-expiry").textContent, "Heard yes — tap Don't send to stop");
  assert.equal(page.el("post-confirm-cancel").disabled, false, "Don't send was held while the card said to tap it");
  assert.equal(page.el("post-confirm-send").disabled, true, "Send was let through the hold too");
  await page.el("post-confirm-cancel").click();
  await page.settle();
  await settleSpokenYes(page);
  assert.deepEqual(page.proposalCommits, [], "the tap meant to stop the send was eaten, and the yes posted");
  assert.deepEqual(page.proposalCancels, [{ handle: pending.handle }]);
  assert.equal(page.el("post-confirm").hidden, true, "the withdrawn draft stayed on the card");
  assert.ok(
    page.el("transcript").children.some((li) => li.className === "seam" && li.text().includes("Not sent")),
    "the withdrawal left no 'Not sent' line",
  );
  const notices = pageNotices(socket);
  assert.equal(notices.length, 1, notices.join("\n"));
  assert.match(notices[0], /^Voice page: the user tapped Don't send, so the draft was withdrawn and NOT sent\./);
}

test("`#269 approval-card-inline`: Don't send stops a spoken yes that settles inside the card's hold", async () => {
  const page = newPage();
  const pending = spokenDraft();
  const socket = await readBackDuringACall(page, { pending });
  // A card that has just appeared, with no yes settling, holds both buttons.
  assert.equal(page.el("post-confirm-cancel").disabled, true, "Don't send was live on a card that had just appeared");
  hostedHears(socket, "yes");
  await dontSendStopsTheYes(page, socket, pending);
});

test("`#269 approval-card-inline`: Don't send stops a yes to a card that reached the page after its read-back, inside the hold", async () => {
  const page = newPage();
  const pending = spokenDraft();
  const socket = await startTalking(page);
  await page.settle();
  hostedHears(socket, "tell lead team I'm on it, rebasing now");
  hostedSays(socket, `To lead team: ${pending.text}. Send it?`);
  // The long poll brings the card a moment after the read-back's transcript: a read-back that beat
  // its card to the page still counts.
  page.propose(pending);
  await page.settle();
  assert.equal(page.el("post-confirm").hidden, false, "the proposal never reached the card");
  assert.equal(page.el("post-confirm-cancel").disabled, true, "Don't send was live on a card that had just appeared");
  hostedHears(socket, "yes");
  await dontSendStopsTheYes(page, socket, pending);
});

test("`#269 approval-card-inline`: a yes that stops settling inside the hold holds Don't send again until the hold is over", async () => {
  const page = newPage();
  const pending = spokenDraft();
  const socket = await readBackDuringACall(page, { pending });
  hostedHears(socket, "yes");
  assert.equal(page.el("post-confirm-cancel").disabled, false, "Don't send was held while a yes settled");
  hostedHears(socket, "wait");
  await page.settle();
  assert.ok(cardHeld(page), "the card's hold was over: this tests nothing");
  assert.equal(page.el("post-confirm-cancel").disabled, true, "Don't send stayed live inside the hold with no yes settling");
  await page.el("post-confirm-cancel").click();
  await page.settle();
  assert.deepEqual(page.proposalCancels, [], "a tap inside the hold, with no yes settling, withdrew the draft");
  assert.equal(page.expireTimers(CARD_HOLD_MS), 1, "no hold was running");
  assert.equal(page.el("post-confirm-cancel").disabled, false, "Don't send stayed held after the hold");
  await page.el("post-confirm-cancel").click();
  await page.settle();
  assert.deepEqual(page.proposalCancels, [{ handle: pending.handle }]);
  assert.deepEqual(page.proposalCommits, []);
});

test("`#264 voice-send-approval`: a tap while the spoken commit is on its way is ignored, and the post goes once, as the speaker's turn", async () => {
  const page = newPage();
  const socket = await readBackDuringACall(page);
  page.expireTimers(2000);
  let release = () => {};
  const held = new Promise((resolve) => {
    release = resolve;
  });
  const gate = page.commitProposal;
  page.commitProposal = async (body) => {
    await held;
    return gate(body);
  };
  hostedHears(socket, "yes");
  page.expireTimers(SPOKEN_SETTLE_MS);
  await page.settle();
  await page.el("post-confirm-send").click();
  await page.settle();
  release();
  await page.settle();
  await page.settle();
  assert.deepEqual(page.proposalCommits.map((c) => c.confirmed_by), ["speaker_turn"]);
  assert.equal(pageNotices(socket).length, 1);
});

test("`#264 voice-send-approval`: 'yes... wait' inside the settle window sends nothing, and the agent is told", async () => {
  const page = newPage();
  const socket = await readBackDuringACall(page);
  hostedHears(socket, "yes");
  hostedHears(socket, "wait");
  await settleSpokenYes(page);
  assert.deepEqual(page.proposalCommits, []);
  assert.equal(page.el("post-confirm").hidden, false, "the draft should stay up for a tap");
  const notices = pageNotices(socket);
  assert.equal(notices.length, 1, notices.join("\n"));
  assert.match(notices[0], /NOT sent: more than a yes was heard/);
});

test("`#264 voice-send-approval`: no spoken yes reaches a draft from another call, one pending when the call opened, or one from before a replacement session", async () => {
  // Hung up, called again: the old draft is not on the card, and a yes in the new call sends nothing.
  const page = newPage();
  await readBackDuringACall(page);
  await page.el("hang-up").click();
  await page.settle();
  await page.el("talk").click();
  const second = page.sockets[1];
  second.onopen();
  await page.settle();
  hostedSays(second, "To lead team: on it, rebasing now. Send it?");
  hostedHears(second, "yes");
  await settleSpokenYes(page);
  assert.deepEqual(page.proposalCommits, [], "a yes in a new call sent the last call's draft");

  // Pending before this call opened: shown for a tap, never sent by voice.
  const waiting = newPage();
  const pending = spokenDraft();
  waiting.propose(pending);
  const socket = await startTalking(waiting);
  await waiting.settle();
  assert.equal(waiting.el("post-confirm").hidden, false);
  hostedSays(socket, `To lead team: ${pending.text}. Send it?`);
  hostedHears(socket, "yes");
  await settleSpokenYes(waiting);
  assert.deepEqual(waiting.proposalCommits, []);
  assert.match(pageNotices(socket)[0], /already waiting when this call started/);

  // A second session_started on the same socket is a session that never saw the draft.
  const replaced = newPage();
  const v1 = await startNeutralCall(replaced);
  await replaced.settle();
  speech(v1, "user", 1, "tell lead team I'm on it, rebasing now", true);
  replaced.propose(spokenDraft());
  await replaced.settle();
  speech(v1, "assistant", 1, "To lead team: on it, rebasing now. Send it?", true);
  turnDone(v1, 1);
  v1.onmessage({ data: JSON.stringify({ type: "session_started", greeting: false }) });
  speech(v1, "assistant", 2, "To lead team: on it, rebasing now. Send it?", true);
  turnDone(v1, 2);
  speech(v1, "user", 3, "yes", true);
  await settleSpokenYes(replaced);
  assert.deepEqual(replaced.proposalCommits, [], "a replacement session's yes sent the old draft");
  // Its own reason: the draft was this call's, but not this session's.
  assert.match(replaced.el("status").textContent, /^Not sent — the voice session restarted after that draft was made\./);
});

test("`#264 voice-send-approval`: a lapsed draft, or one holding the sign-in token, cannot be sent by voice", async () => {
  const lapsed = newPage();
  const lapsedSocket = await readBackDuringACall(lapsed, { pending: spokenDraft({ expires_in_ms: 10000 }) });
  lapsed.setClock(lapsed.clock() + 11000);
  hostedHears(lapsedSocket, "yes");
  await settleSpokenYes(lapsed);
  assert.deepEqual(lapsed.proposalCommits, []);
  assert.match(pageNotices(lapsedSocket)[0], /the draft lapsed/);

  const leaky = newPage();
  const leakySocket = await readBackDuringACall(leaky, {
    pending: spokenDraft({ text: "here you go write-token-aaaaaaaaaaaaaaaa" }),
    said: "To lead team: here you go write-token-aaaaaaaaaaaaaaaa. Send it?",
  });
  hostedHears(leakySocket, "yes");
  await settleSpokenYes(leaky);
  assert.deepEqual(leaky.proposalCommits, []);
  assert.match(pageNotices(leakySocket)[0], /contains the sign-in token/);
});

test("`#264 voice-send-approval`: with echo cancellation off, or in a typed call, a yes never sends", async () => {
  const page = newPage();
  page.el("mic-echo-cancellation").checked = false;
  const socket = await readBackDuringACall(page);
  hostedHears(socket, "yes");
  await settleSpokenYes(page);
  assert.deepEqual(page.proposalCommits, []);
  assert.match(pageNotices(socket)[0], /needs echo cancellation on/);

  // Asked for, but the browser says the device opened without it: the same.
  const ignored = newPage();
  ignored.appliedEchoCancellation = false;
  const route = await readBackDuringACall(ignored);
  hostedHears(route, "yes");
  await settleSpokenYes(ignored);
  assert.deepEqual(ignored.proposalCommits, [], "a yes counted on a microphone that ignored echo cancellation");
  assert.match(pageNotices(route)[0], /needs echo cancellation on/);

  // And one that says it applied it is taken at its word.
  const applied = newPage();
  applied.appliedEchoCancellation = true;
  const fine = await readBackDuringACall(applied);
  hostedHears(fine, "yes");
  await settleSpokenYes(applied);
  assert.deepEqual(applied.proposalCommits.map((c) => c.confirmed_by), ["speaker_turn"]);

  const typed = newPage();
  const chat = await startChatting(typed, "tell lead team I'm on it, rebasing now");
  typed.propose(spokenDraft());
  await typed.settle();
  hostedSays(chat, "To lead team: on it, rebasing now. Send it?");
  await compose(typed, "yes");
  await typed.el("send-text").click();
  await settleSpokenYes(typed);
  assert.deepEqual(typed.proposalCommits, [], "a typed yes was taken as a spoken one");
  // ...but it is answered, after the yes itself, and the composer's "Sent." does not stand.
  const notices = pageNotices(chat);
  assert.equal(notices.length, 1, notices.join("\n"));
  assert.match(notices[0], /^Voice page: the message was NOT sent: this is a typed call, so the draft needs a tap on Send\./);
  assert.match(typed.el("status").textContent, /^Not sent — this is a typed call/);
});

// The read-back is held to one exact sentence, and only the assistant's words since the owner's last
// turn count. Each of these was a commit before that rule: a yes to some other question, a read-back
// that changed the meaning, a sentence that merely contained the card's words, and a chat message
// read aloud. None may send.

test("`#264 voice-send-approval`: the read-back must be exactly 'To {channel}: {text}. Send it?' — a lead-in is allowed, nothing added is", () => {
  const page = newPage();
  const exact = page.pure("isExactReadBack");
  const words = page.pure("answerWords");
  const card = { channel_name: "lead team", text: "merge the PR" };
  for (const said of [
    "To lead team: merge the PR. Send it?",
    "to LEAD TEAM — merge the PR… send it",
    "Okay. To lead team: merge the PR. Send it?",
    "Got it. To the lead team channel: merge the PR. Send it?",
  ]) {
    assert.equal(exact(words(said), card), true, said);
  }
  for (const said of [
    "To lead team: do not merge the PR yet. Send it?",
    "To lead team: I will not merge the PR. Send it?",
    "Updated: don't merge the PR. Send it?",
    "To lead team: merge the PR, as you asked. Send it?",
    "To lead team: merge the PR. Send it? Or shall I wait?",
    "To lead team: merge the PR. Shall I post that?",
    "To build noise: merge the PR. Send it?",
    "Okay, sure. To lead team: merge the PR. Send it?",
    "merge the PR. Send it?",
    "",
  ]) {
    assert.equal(exact(words(said), card), false, said);
  }
  assert.equal(exact(words("To lead team: . Send it?"), { channel_name: "lead team", text: "" }), false);
});

test("`#264 voice-send-approval`: a yes to a different question, after a digression, does not send the card it never read back", async () => {
  const page = newPage();
  const pending = spokenDraft({ text: "on it" });
  const socket = await startTalking(page);
  await page.settle();
  hostedHears(socket, "tell lead team I'm on it");
  page.propose(pending);
  await page.settle();
  hostedSays(socket, "To lead team: on it. Send it?");
  hostedHears(socket, "what did Bob say before that");
  await page.settle();
  hostedSays(socket, "Bob said he is on it and will send the fix tonight. Want to hear the rest?");
  hostedHears(socket, "yes");
  await settleSpokenYes(page);
  assert.deepEqual(page.proposalCommits, [], "a yes to 'want to hear the rest' posted the card");
  assert.equal(page.el("post-confirm").hidden, false);
  // Not a question about sending, so the agent is not waiting on the page for it: no notice to
  // break into the rest it is reading, only the status line.
  assert.deepEqual(pageNotices(socket), []);
  assert.match(page.el("status").textContent, /a yes counts only as the answer to the read-back/);
});

test("`#264 voice-send-approval`: what makes an answer one about sending", () => {
  const page = newPage();
  const asked = page.pure("askedAboutSending");
  const turns = (...texts) => texts.map((text) => ({ text }));
  for (const [said, answer] of [
    [["To lead team: on it. Send it?"], "yes"],
    [["To lead team: on it. Shall I post that?"], "yes"],
    [["To lead team: on it. Send it? I can add a note too."], "yes"],
    [["to lead team on it send it"], "yes"],
    [["It is ten past four."], "ok send it"],
    [[], "post it"],
    // A read-back whose transcript ended "Send it." is still the question.
    [["To lead team: on it. Send it."], "yes"],
    // Every question counts, not only the last: a relayed message read out after "Send it?" may ask
    // one of its own, and so may the assistant after asking.
    [["To lead team: on it. Send it?", "In lead team, Bob asked: are you around?"], "yes"],
    [["To lead team: on it. Send it? Or shall I wait?"], "yes"],
  ]) {
    assert.equal(asked(turns(...said), answer), true, `${said} / ${answer}`);
  }
  for (const [said, answer] of [
    [["Bob said he will send the fix tonight. Want to hear the rest?"], "yes"],
    [["It was not sent. You can tap Send or ask me to read it back again."], "okay"],
    [["It is ten past four."], "yes"],
    [[], "yes"],
  ]) {
    assert.equal(asked(turns(...said), answer), false, `${said} / ${answer}`);
  }
});

test("`#264 voice-send-approval`: an acknowledgement of a not-sent report is not answered again, so the agent does not repeat itself", async () => {
  const page = newPage();
  const socket = await readBackDuringACall(page);
  hosted(socket, { type: "interruption", interruption_event: {} });
  hostedHears(socket, "yes");
  await settleSpokenYes(page);
  assert.equal(pageNotices(socket).length, 1, "the interrupted yes was not answered");
  hostedSays(socket, "It was not sent. You can tap Send or ask me to read it back again.");
  hostedHears(socket, "okay");
  await settleSpokenYes(page);
  hostedHears(socket, "ok thanks");
  await settleSpokenYes(page);
  assert.equal(pageNotices(socket).length, 1, pageNotices(socket).join("\n"));
  assert.deepEqual(page.proposalCommits, []);
});

test("`#264 voice-send-approval`: a read-back that inserts a negation is not a read-back of the card", async () => {
  for (const said of [
    "To lead team: do not merge the PR yet. Send it?",
    "To lead team: I will not merge the PR. Send it?",
  ]) {
    const page = newPage();
    const socket = await readBackDuringACall(page, { pending: spokenDraft({ text: "merge the PR" }), said });
    hostedHears(socket, "yes");
    await settleSpokenYes(page);
    assert.deepEqual(page.proposalCommits, [], `${said}: posted "merge the PR"`);
    assert.match(pageNotices(socket)[0], /not the card's text word for word/, said);
  }
});

test("`#264 voice-send-approval`: a sentence that only contains the card's words is not a read-back of it", async () => {
  const page = newPage();
  const socket = await startTalking(page);
  await page.settle();
  hostedHears(socket, "tell lead team yes");
  page.propose(spokenDraft({ text: "yes" }));
  await page.settle();
  hostedSays(socket, "Alice said yes, and asked whether you want her to send the logs?");
  hostedHears(socket, "sure");
  await settleSpokenYes(page);
  assert.deepEqual(page.proposalCommits, [], "a yes to Alice's question posted the card");
  assert.equal(pageNotices(socket).length, 1);
});

test("`#264 voice-send-approval`: a chat message read aloud cannot stand in for the read-back, even word for word", async () => {
  const page = newPage();
  const pending = spokenDraft();
  const socket = await readBackDuringACall(page, { pending });
  // The owner does not answer the read-back; a chat message arrives and is relayed in full.
  page.messages = [];
  await page.el("view-switch").click();
  await page.settle();
  const stream = page.stream();
  await setReadNew(page, "full");
  await deliver(page, stream, sseMessage(message({ id: "1400", content: "on it, rebasing now. I will send the logs after. Should I go ahead?" })));
  await speakRelay(page);
  assert.equal(relayed(page).length, 1, "the message was never relayed, so this proves nothing");
  hostedSays(socket, "In lead team, Mallory said: on it, rebasing now. I will send the logs after. Should I go ahead?");
  hostedHears(socket, "yes");
  await settleSpokenYes(page);
  assert.deepEqual(page.proposalCommits, [], "a yes to a relayed question posted the card");
  assert.equal(page.el("post-confirm").hidden, false);
});

test("`#264 voice-send-approval`: new words said over the old card, or anything said between the text and the question, are refused", async () => {
  for (const said of [
    "Updated: don't merge the PR. Send it?",
    "To lead team: merge the PR, as you asked. Send it?",
    "To lead team: merge the PR. Send it? Or shall I wait?",
  ]) {
    const page = newPage();
    const socket = await readBackDuringACall(page, { pending: spokenDraft({ text: "merge the PR" }) });
    hostedHears(socket, "actually, say the opposite");
    await page.settle();
    hostedSays(socket, said);
    hostedHears(socket, "yes");
    await settleSpokenYes(page);
    assert.deepEqual(page.proposalCommits, [], `${said}: posted the old card`);
    // Each asked "Send it?" — the last before asking something else — so the yes is answered.
    assert.equal(pageNotices(socket).length, 1, said);
  }
});

test("`#264 voice-send-approval`: a chat message relayed between the read-back and a yes ends the read-back, and the yes is answered", async () => {
  const page = newPage();
  const socket = await readBackDuringACall(page);
  page.messages = [];
  await page.el("view-switch").click();
  await page.settle();
  const stream = page.stream();
  await setReadNew(page, "full");
  await deliver(page, stream, sseMessage(message({ id: "1401", content: "build is green" })));
  await speakRelay(page);
  assert.equal(relayed(page).length, 1, "the message was never relayed, so this proves nothing");
  hostedSays(socket, "In lead team, Bob said: build is green.");
  hostedHears(socket, "yes");
  await settleSpokenYes(page);
  assert.deepEqual(page.proposalCommits, [], "a yes after a relayed message sent the card");
  // The owner's yes still answers "Send it?" — the relay was the page's turn, not theirs — so the
  // agent, which will have said only "One moment.", is told it was not sent and why.
  const notices = pageNotices(socket);
  assert.equal(notices.length, 1, notices.join("\n"));
  assert.match(notices[0], /NOT sent: a message came between the read-back and the yes/);

  // A wrong read-back after the notice is refused for being wrong, not for the relay before it.
  page.setClock(page.clock() + 1000);
  hostedSays(socket, "To lead team: on it. Send it?");
  hostedHears(socket, "yes");
  await settleSpokenYes(page);
  assert.match(pageNotices(socket).at(-1), /not the card's text word for word/);

  // Read back again, exactly, it may be confirmed.
  page.setClock(page.clock() + 1000);
  hostedSays(socket, "To lead team: on it, rebasing now. Send it?");
  hostedHears(socket, "yes");
  await settleSpokenYes(page);
  assert.deepEqual(page.proposalCommits.map((c) => c.confirmed_by), ["speaker_turn"]);
});

test("`#264 voice-send-approval`: a text turn of the page's own ends the read-back even when no speech follows it", async () => {
  const page = newPage();
  const socket = await readBackDuringACall(page);
  page.messages = [];
  await page.el("view-switch").click();
  await page.settle();
  const stream = page.stream();
  await setReadNew(page, "full");
  await deliver(page, stream, sseMessage(message({ id: "1403", content: "build is green" })));
  await speakRelay(page);
  assert.equal(relayed(page).length, 1, "the message was never relayed, so this proves nothing");
  // The owner answers before the agent says a word about the relayed message.
  hostedHears(socket, "yes");
  await settleSpokenYes(page);
  assert.deepEqual(page.proposalCommits, [], "a read-back from before a relayed message still armed a yes");
  assert.match(pageNotices(socket).at(-1) || "", /NOT sent: a message came between the read-back and the yes/);
});

test("`#264 voice-send-approval`: a replacement draft read back in full is confirmed by its own read-back, whatever was said of the old one before it", async () => {
  const page = newPage();
  const socket = await readBackDuringACall(page, { pending: spokenDraft({ text: "merge the PR" }) });
  // The agent corrects itself before the owner says anything: a new draft, read back in full.
  const replacement = spokenDraft({ serial: 8, handle: "rEpLaCeD-0123456789abc", text: "merge the PR after the build" });
  page.propose(replacement);
  await page.settle();
  page.setClock(page.clock() + 1000);
  hostedSays(socket, "To lead team: merge the PR after the build. Send it?");
  hostedHears(socket, "yes");
  await settleSpokenYes(page);
  assert.deepEqual(page.proposalCommits.map((c) => [c.handle, c.text]), [[replacement.handle, replacement.text]]);
});

test("`#264 voice-send-approval`: after a digression a fresh read-back arms a yes again, and one with a lead-in counts", async () => {
  const page = newPage();
  const pending = spokenDraft();
  const socket = await readBackDuringACall(page, { pending });
  hostedHears(socket, "sorry, read it again");
  await page.settle();
  hostedSays(socket, `Sure. To lead team: ${pending.text}. Send it?`);
  hostedHears(socket, "yes");
  await settleSpokenYes(page);
  assert.deepEqual(page.proposalCommits.map((c) => [c.handle, c.confirmed_by]), [[pending.handle, "speaker_turn"]]);
});

test("`#264 voice-send-approval`: a read-back whose transcript beat the card to the page still counts", async () => {
  const page = newPage();
  const pending = spokenDraft();
  const socket = await startTalking(page);
  await page.settle();
  hostedHears(socket, "tell lead team I'm on it, rebasing now");
  hostedSays(socket, `To lead team: ${pending.text}. Send it?`);
  page.propose(pending);
  await page.settle();
  assert.equal(page.el("post-confirm").hidden, false);
  hostedHears(socket, "yes");
  await settleSpokenYes(page);
  assert.deepEqual(page.proposalCommits.map((c) => c.confirmed_by), ["speaker_turn"]);
  assert.match(pageNotices(socket)[0], /SENT to lead team/);
});

test("`#264 voice-send-approval`: a draft with a mention is never sent by voice, and the agent is told why", async () => {
  const page = newPage();
  const socket = await readBackDuringACall(page, {
    pending: spokenDraft({ text: "<@123456789> on it" }),
    said: "To lead team: Bob, on it. Send it?",
  });
  hostedHears(socket, "yes");
  await settleSpokenYes(page);
  assert.deepEqual(page.proposalCommits, []);
  assert.match(pageNotices(socket)[0], /mentions someone, which a read-back cannot say exactly/);
});

// A read-back is held to the card by its words, and a voice says only words. Whatever else a draft
// holds — markup, a link's dots and slashes, a sign, an emoji — is invisible to that comparison and
// is exactly what text-to-speech drops, so it could change what is posted while every word matched.

test("`#264 voice-send-approval`: only plain words and sentence punctuation can be confirmed by voice", () => {
  const unsayable = newPage().pure("unsayableDraft");
  for (const text of [
    "on it, rebasing now",
    "Don\u2019t merge the PR yet. I'll re-run the tests first!",
    "Wait \u2014 is the build green? Yes; ship it: today\u2026",
    "café at 5pm",
    "line one\nline two.",
  ]) {
    assert.equal(unsayable(text), null, text);
  }
  for (const [text, why] of [
    ["<@123456789> on it", "mention"],
    ["~~don't~~ merge the PR", "symbols"],
    ["~don't~ merge the PR", "symbols"],
    ["||not|| ready", "symbols"],
    ["> merge it", "symbols"],
    ["*do not* merge", "symbols"],
    ["`rm -rf` it", "symbols"],
    ["+1", "symbols"],
    ["-1 from me", "symbols"],
    ["\u{1F44E}", "symbols"],
    ["log in at https://github.com.evil.io/login", "symbols"],
    ["log in at github.com.evil.io", "symbols"],
    ["see [the fix](https://evil.example/x)", "symbols"],
    ["@everyone merge now", "symbols"],
    ["PR #123", "symbols"],
    ["meet at 10:30", "symbols"],
    ["version 3.5", "symbols"],
    ["d\u0336o\u0336n\u0336't merge", "symbols"],
    ["on it\u200b", "symbols"],
  ]) {
    assert.equal(unsayable(text), why, text);
  }
});

test("`#264 voice-send-approval`: a struck-through word or a changed link is never confirmed by voice, even when every word matches", async () => {
  for (const [text, said] of [
    // What text-to-speech makes of the server's own read-back: the strike is silent.
    ["~~don't~~ merge the PR", "To lead team: don't merge the PR. Send it?"],
    // The read-back word for word, markup and all: still not something a voice can say.
    ["~~don't~~ merge the PR", "To lead team: ~~don't~~ merge the PR. Send it?"],
    // One link read back as another: the same words once the dots and slashes are gone.
    ["log in at https://github.com.evil.io/login", "To lead team: log in at https://github.com/evil.io/login. Send it?"],
    ["see [the fix](https://evil.example/x)", "To lead team: see the fix. Send it?"],
  ]) {
    const page = newPage();
    const socket = await readBackDuringACall(page, { pending: spokenDraft({ text }), said });
    const before = page.audio.played;
    hostedHears(socket, "yes");
    await settleSpokenYes(page);
    assert.deepEqual(page.proposalCommits, [], `${text} / ${said}: committed`);
    assert.equal(page.el("post-confirm").hidden, false, `${text}: the card went`);
    const notices = pageNotices(socket);
    assert.equal(notices.length, 1, `${text}: ${notices.join("\n")}`);
    assert.match(notices[0], /^Voice page: the message was NOT sent: the draft has a link, a symbol or formatting in it, which a read-back cannot say exactly\./, text);
    assert.match(page.el("status").textContent, /^Not sent — the draft has a link, a symbol or formatting in it/, text);
    assert.equal(page.audio.played, before + 1, `${text}: no tone`);

    // A tap still sends it: the card shows the exact text.
    page.expireTimers(2000);
    await page.el("post-confirm-send").click();
    await page.settle();
    assert.deepEqual(page.proposalCommits.map((c) => [c.text, c.confirmed_by]), [[text, "ui"]]);
  }
});

// Every yes is answered. The agent says only "One moment." on a yes and waits for the page, so a
// yes the page does not act on must still get a "Voice page:" notice and the low tone — never only
// a status line the hands-free owner cannot see.

test("`#264 voice-send-approval`: a yes after a paraphrased question, or 'ok send it' after a digression, is answered not sent, aloud", async () => {
  const paraphrase = newPage();
  const pending = spokenDraft();
  const socket = await readBackDuringACall(paraphrase, { pending, said: `To lead team: ${pending.text}. Shall I post that?` });
  const before = paraphrase.audio.played;
  hostedHears(socket, "yes");
  await settleSpokenYes(paraphrase);
  assert.deepEqual(paraphrase.proposalCommits, []);
  assert.match(pageNotices(socket)[0] || "", /^Voice page: the message was NOT sent: the read-back did not end with the question Send it/);
  assert.equal(paraphrase.audio.played, before + 1, "no tone");

  const digression = newPage();
  const other = await readBackDuringACall(digression);
  hostedHears(other, "what time is it");
  await digression.settle();
  hostedSays(other, "It is ten past four.");
  const ahead = digression.audio.played;
  hostedHears(other, "ok send it");
  await settleSpokenYes(digression);
  assert.deepEqual(digression.proposalCommits, []);
  assert.equal(pageNotices(other).length, 1, "a yes after a digression went unanswered");
  assert.equal(digression.audio.played, ahead + 1, "no tone");
});

test("`#264 voice-send-approval`: a yes typed into a spoken call through the composer is answered not sent", async () => {
  const page = newPage();
  const socket = await readBackDuringACall(page);
  await compose(page, "yes");
  await page.el("send-text").click();
  await settleSpokenYes(page);
  assert.deepEqual(page.proposalCommits, []);
  const notices = pageNotices(socket);
  assert.equal(notices.length, 1, notices.join("\n"));
  assert.match(notices[0], /a typed yes does not send it, so it needs a tap on Send/);
  assert.match(page.el("status").textContent, /^Not sent — a typed yes/, "the composer's 'Sent.' was left standing over the card");
  // The yes went first, then the notice, so the agent reads them in that order.
  const typed = socket.sent.filter((f) => typeof f === "string").map((f) => JSON.parse(f)).filter((e) => e.type === "user_message").map((e) => e.text);
  assert.deepEqual(typed.slice(-2).map((t) => t.startsWith("Voice page:")), [false, true]);
});

test("`#264 voice-send-approval`: on the neutral protocol a yes after a paraphrase is answered in a queued prompt", async () => {
  const page = newPage();
  const socket = await startNeutralCall(page);
  await page.settle();
  speech(socket, "user", 1, "tell lead team I'm on it, rebasing now", true);
  page.propose(spokenDraft());
  await page.settle();
  speech(socket, "assistant", 1, "To lead team: on it, rebasing now. Shall I post that?", true);
  turnDone(socket, 1);
  speech(socket, "user", 2, "yes", true);
  await settleSpokenYes(page);
  turnDone(socket, 2);
  // The owner's turn has completed, so the page closes its segment once the hand-off after it is
  // over; the server's boundary for that lets the notice go (`#268 voice-call-drops`).
  page.expireTimers(sourceConstant("VOICE_CAPTURE_RESUME_MS"));
  turnDone(socket, 3);
  assert.deepEqual(page.proposalCommits, []);
  const prompts = promptsSent(socket);
  assert.equal(prompts.length, 1, prompts.join("\n"));
  assert.match(prompts[0], /^Voice page: the message was NOT sent/);
});

test("`#264 voice-send-approval`: the yeses an agent takes that the page does not act on are classified as like a yes", () => {
  const classify = newPage().pure("classifySpokenAnswer");
  for (const said of [
    "sounds good", "Go for it.", "uh-huh", "Mm-hmm.", "mhm", "Perfect!", "alright", "That's right",
    "Affirmative.", "of course", "Ship it", "right", "great", "fine", "yup", "Absolutely.",
  ]) {
    assert.equal(classify(said), "hint", said);
  }
  for (const said of ["make it so", "fire away", "change it to Friday", "what time is it"]) {
    assert.equal(classify(said), "other", said);
  }
});

/** The tone `source` played: "sent" for the rising pair, "not sent" for the single low note. */
function toneOf(source) {
  const samples = source.buffer.getChannelData(0);
  const window = Math.round(0.08 * source.buffer.sampleRate);
  let crossings = 0;
  for (let i = 1; i < window; i += 1) {
    if ((samples[i - 1] < 0) !== (samples[i] < 0)) crossings += 1;
  }
  const hertz = crossings / 2 / 0.08;
  if (Math.abs(hertz - 660) < 60) return "sent";
  if (Math.abs(hertz - 330) < 40) return "not sent";
  return `unknown (${hertz} Hz)`;
}

/** The tones the page played since `from` sources, in order. */
const tonesSince = (page, from) =>
  page.audio.sources.slice(from).filter((source) => source.buffer && source.buffer.duration < 0.5).map(toneOf);

test("`#264 voice-send-approval`: every answer an agent would take for a yes is answered aloud — sounds good, go for it, uh-huh, that's right, and the rest", async () => {
  for (const said of [
    "Sounds good.", "Go for it.", "Uh-huh.", "Mm-hmm.", "Perfect.", "Alright.", "That's right.",
    "Affirmative.", "Of course.", "Ship it.", "Right.", "make it so", "fire away",
    // Longer than a short answer, so only a word like a yes gets these answered.
    "that sounds good to me", "of course, go for it",
  ]) {
    const page = newPage();
    const socket = await readBackDuringACall(page);
    const sources = page.audio.sources.length;
    hostedHears(socket, said);
    await settleSpokenYes(page);
    await settleRedraftWait(page);
    assert.deepEqual(page.proposalCommits, [], `${said}: committed`);
    assert.equal(page.el("post-confirm").hidden, false, `${said}: the card went`);
    const notices = pageNotices(socket);
    assert.equal(notices.length, 1, `${said}: ${notices.join("\n")}`);
    assert.match(notices[0], /^Voice page: the message was NOT sent: only a plain answer such as yes or send it counts/, said);
    assert.match(notices[0], /Otherwise carry on\.$/, said);
    assert.match(page.el("status").textContent, /^Not sent — only a plain answer/, said);
    const seams = page.el("transcript").children.filter((li) => li.className === "seam");
    assert.equal(seamLabel(seams.at(-1)), "Not sent", said);
    assert.deepEqual(tonesSince(page, sources), ["not sent"], said);
  }
});

test("`#264 voice-send-approval`: a longer answer with nothing like a yes in it is the owner's own request, and is not answered over", async () => {
  const page = newPage();
  const socket = await readBackDuringACall(page);
  hostedHears(socket, "change it to Friday");
  await settleSpokenYes(page);
  assert.deepEqual(page.proposalCommits, []);
  assert.deepEqual(page.proposalCancels, []);
  assert.deepEqual(pageNotices(socket), [], "a notice talked over the owner's correction");
});

test("`#264 voice-send-approval`: a yes after this call's draft has lapsed and left the card is told no draft is waiting", async () => {
  const page = newPage();
  const socket = await readBackDuringACall(page);
  // The draft lapses on the server, and the watch takes it down before the owner answers.
  page.propose(null);
  await page.settle();
  assert.equal(page.el("post-confirm").hidden, true, "the card stayed up");
  const sources = page.audio.sources.length;
  hostedHears(socket, "yes");
  await settleSpokenYes(page);
  assert.deepEqual(page.proposalCommits, []);
  const notices = pageNotices(socket);
  assert.equal(notices.length, 1, notices.join("\n"));
  assert.match(notices[0], /^Voice page: no draft is waiting to be sent; the last one lapsed or was already settled, so that answer sent nothing\./);
  assert.match(notices[0], /Otherwise carry on\.$/);
  assert.equal(page.el("status").textContent, "Nothing sent — no draft is waiting; it lapsed or was already settled.");
  const seams = page.el("transcript").children.filter((li) => li.className === "seam");
  assert.equal(seamLabel(seams.at(-1)), "Not sent");
  assert.deepEqual(tonesSince(page, sources), ["not sent"]);

  // Typed, the same.
  const typed = newPage();
  const other = await readBackDuringACall(typed);
  typed.propose(null);
  await typed.settle();
  await compose(typed, "yes");
  await typed.el("send-text").click();
  await settleSpokenYes(typed);
  assert.match(pageNotices(other).at(-1) || "", /^Voice page: no draft is waiting to be sent/);

  // A call that never had a draft has nothing to say about one: "send it to Bob" is a request.
  const fresh = newPage();
  const quiet = await startTalking(fresh);
  await fresh.settle();
  hostedSays(quiet, "Who should I send it to?");
  hostedHears(quiet, "send it to Bob");
  await settleSpokenYes(fresh);
  assert.deepEqual(pageNotices(quiet), []);
});

test("`#264 voice-send-approval`: a relayed chat message that asks its own question after 'Send it?' does not leave the yes unanswered", async () => {
  const page = newPage();
  const socket = await readBackDuringACall(page);
  page.messages = [];
  await page.el("view-switch").click();
  await page.settle();
  const stream = page.stream();
  await setReadNew(page, "full");
  await deliver(page, stream, sseMessage(message({ id: "1402", content: "are you around?" })));
  await speakRelay(page);
  assert.equal(relayed(page).length, 1, "the message was never relayed, so this proves nothing");
  hostedSays(socket, "In lead team, Bob asked: are you around?");
  const sources = page.audio.sources.length;
  hostedHears(socket, "yes");
  await settleSpokenYes(page);
  assert.deepEqual(page.proposalCommits, [], "a yes after a relayed question sent the card");
  const notices = pageNotices(socket);
  assert.equal(notices.length, 1, notices.join("\n"));
  assert.match(notices[0], /NOT sent: a message came between the read-back and the yes/);
  assert.deepEqual(tonesSince(page, sources), ["not sent"]);
});

test("`#264 voice-send-approval`: an exact read-back whose transcript ended 'Send it.' with a full stop is still answered, and a yes sends it", async () => {
  const page = newPage();
  const pending = spokenDraft();
  const socket = await readBackDuringACall(page, { pending, said: `To lead team: ${pending.text}. Send it.` });
  hostedHears(socket, "yes");
  await settleSpokenYes(page);
  assert.deepEqual(page.proposalCommits.map((c) => [c.handle, c.confirmed_by]), [[pending.handle, "speaker_turn"]]);
  assert.match(pageNotices(socket)[0] || "", /^Voice page: the message you read back was SENT to lead team\./);
});

test("`#264 voice-send-approval`: with the agent told to post at once when the call began, no answer is judged against a card left from before", async () => {
  const page = newPage();
  page.voiceSettings = { ask_before_send: false, changeable: true };
  const socket = await readBackDuringACall(page);
  for (const said of ["yes", "send it to Bob", "no"]) {
    hostedHears(socket, said);
    await settleSpokenYes(page);
  }
  assert.deepEqual(page.proposalCommits, []);
  assert.deepEqual(page.proposalCancels, []);
  assert.deepEqual(pageNotices(socket), [], "the page answered for an agent that posts at once");
  assert.equal(page.el("post-confirm").hidden, false, "the card is still there for a tap");
});

test("`#264 voice-send-approval`: a tap on the card during a call is told to the agent, sent or withdrawn; a spoken no is not", async () => {
  const sent = newPage();
  const socket = await readBackDuringACall(sent);
  sent.expireTimers(2000);
  await sent.el("post-confirm-send").click();
  await sent.settle();
  assert.deepEqual(sent.proposalCommits.map((c) => c.confirmed_by), ["ui"]);
  const told = pageNotices(socket);
  assert.equal(told.length, 1, told.join("\n"));
  assert.match(told[0], /^Voice page: the user tapped Send, and the message was SENT to lead team\./);
  assert.equal(sent.el("status").textContent, "Posted to lead team.", "the tap's own status was replaced");

  const withdrawn = newPage();
  const other = await readBackDuringACall(withdrawn);
  withdrawn.expireTimers(2000); // the card's own hold on both buttons, `#269 approval-card-inline`
  await withdrawn.el("post-confirm-cancel").click();
  await withdrawn.settle();
  assert.match(pageNotices(other)[0] || "", /^Voice page: the user tapped Don't send, so the draft was withdrawn and NOT sent\./);
});

// --- #579 voice-send-confirm-timing ---------------------------------------------------------------
//
// The owner: "I will say send X on channel Y. The card will pop up but the agent will just
// immediately say that it wasn't sent because the confirmation timed out ... it was false because I
// could still press send on the card and it actually went through." His request says send, so the
// page judged it as an answer to the card and told the agent, while it drafted X, that the last
// draft had lapsed or was NOT sent.

/** A second draft, as the agent proposes it for the owner's next request. */
const secondDraft = () =>
  spokenDraft({ serial: 8, handle: "hAnDlE-second-draft-0001", text: "hello world" });

test("`#579 voice-send-confirm-timing`: a new send request after an earlier send is not told that no draft is waiting", async () => {
  for (const said of ["send hello world on the lead team channel", "Send hello world to lead team.", "post hello world in lead team"]) {
    const page = newPage();
    const socket = await readBackDuringACall(page);
    page.expireTimers(2000);
    await page.el("post-confirm-send").click();
    await page.settle();
    const before = pageNotices(socket).length;
    const sources = page.audio.sources.length;
    hostedSays(socket, "Sent.");
    hostedHears(socket, said);
    await settleSpokenYes(page);
    await settleRedraftWait(page);
    page.propose(secondDraft());
    await page.settle();
    assert.equal(page.el("post-confirm").hidden, false, `${said}: the new draft never reached the card`);
    assert.deepEqual(pageNotices(socket).slice(before), [], `${said}: the agent was told a draft lapsed as it made one`);
    assert.deepEqual(tonesSince(page, sources), [], `${said}: a not-sent tone played over the new card`);
    assert.doesNotMatch(page.el("status").textContent, /no draft is waiting/, said);
  }

  // Typed, the same.
  const typed = newPage();
  const socket = await readBackDuringACall(typed);
  typed.expireTimers(2000);
  await typed.el("post-confirm-send").click();
  await typed.settle();
  const before = pageNotices(socket).length;
  await compose(typed, "send hello world on the lead team channel");
  await typed.el("send-text").click();
  await settleSpokenYes(typed);
  await settleRedraftWait(typed);
  assert.deepEqual(pageNotices(socket).slice(before), []);
});

test("`#579 voice-send-confirm-timing`: a request that names no place is held, and dropped when its draft reaches the card", async () => {
  // "send hello" could be a yes to a draft that lapsed, or the owner's next request: the page waits
  // to see which, and a new draft on the card says it was a request.
  const page = newPage();
  const socket = await readBackDuringACall(page);
  page.expireTimers(2000);
  await page.el("post-confirm-send").click();
  await page.settle();
  const before = pageNotices(socket).length;
  hostedHears(socket, "send hello");
  await settleSpokenYes(page);
  page.propose(secondDraft());
  await page.settle();
  await settleRedraftWait(page);
  assert.deepEqual(pageNotices(socket).slice(before), []);

  // With no new draft, the word the page held is said: no draft is waiting.
  const quiet = newPage();
  const other = await readBackDuringACall(quiet);
  quiet.expireTimers(2000);
  await quiet.el("post-confirm-send").click();
  await quiet.settle();
  const earlier = pageNotices(other).length;
  hostedHears(other, "send hello");
  await settleSpokenYes(quiet);
  assert.deepEqual(pageNotices(other).slice(earlier), [], "said before the agent could draft");
  await settleRedraftWait(quiet);
  assert.match(pageNotices(other).slice(earlier)[0] || "", /^Voice page: no draft is waiting to be sent/);
});

test("`#579 voice-send-confirm-timing`: a new request while a card is up is not told NOT sent when its draft replaces the card", async () => {
  const page = newPage();
  const socket = await readBackDuringACall(page);
  const sources = page.audio.sources.length;
  hostedHears(socket, "actually send hello world to the lead team channel");
  await settleSpokenYes(page);
  page.propose(secondDraft());
  await page.settle();
  await settleRedraftWait(page);
  assert.deepEqual(pageNotices(socket), []);
  assert.deepEqual(tonesSince(page, sources), []);
  assert.deepEqual(page.proposalCommits, []);
  const seams = page.el("transcript").children.filter((li) => li.className === "seam");
  assert.equal(seams.length, 0, "a Not sent seam was drawn for a draft a newer one replaced");

  // A tap on Send while the word is held is told as the tap, and the held word is never said.
  const tapped = newPage();
  const other = await readBackDuringACall(tapped);
  hostedHears(other, "sounds good to me");
  await settleSpokenYes(tapped);
  tapped.expireTimers(2000);
  await tapped.el("post-confirm-send").click();
  await tapped.settle();
  await settleRedraftWait(tapped);
  const told = pageNotices(other);
  assert.equal(told.length, 1, told.join("\n"));
  assert.match(told[0], /^Voice page: the user tapped Send, and the message was SENT to lead team\./);
});

test("`#579 voice-send-confirm-timing`: on the bridge, a new request after an earlier send is not told that no draft is waiting", async () => {
  const page = newPage();
  const pending = spokenDraft();
  const socket = await neutralReadBack(page, { pending });
  page.expireTimers(2000);
  await page.el("post-confirm-send").click();
  await page.settle();
  speech(socket, "user", 2, "send hello world on the lead team channel", false);
  speech(socket, "user", 2, "Send hello world on the lead team channel.", true);
  await settleSpokenYes(page);
  await settleRedraftWait(page);
  page.propose(secondDraft());
  await page.settle();
  turnDone(socket, 2);
  page.expireTimers(CAPTURE_RESUME_MS);
  turnDone(socket, 3);
  // Sent, or still queued for the next boundary: either is the agent being told.
  const prompts = [...promptsSent(socket), ...page.pure("session.pendingPrompts").map((queued) => queued.text)];
  assert.equal(prompts.filter((text) => /no draft is waiting|NOT sent/.test(text)).length, 0, prompts.join("\n"));
});

test("`#579 voice-send-confirm-timing`: the page tells the agent when this call's draft lapses, and not about one from before the call", async () => {
  const page = newPage();
  const socket = await readBackDuringACall(page);
  page.lapseProposal();
  await page.settle();
  assert.equal(page.el("post-confirm").hidden, true, "the lapsed card stayed up");
  const notices = pageNotices(socket);
  assert.equal(notices.length, 1, notices.join("\n"));
  assert.match(notices[0], /^Voice page: the draft to lead team lapsed: nobody tapped Send or said yes in time, so it was NOT sent/);
  assert.match(page.el("status").textContent, /lapsed; nothing was posted/);

  // A draft already waiting when the call began is not this call's agent's to report.
  const earlier = newPage();
  earlier.propose(spokenDraft());
  const other = await startTalking(earlier);
  await earlier.settle();
  assert.equal(earlier.el("post-confirm").hidden, false);
  earlier.lapseProposal();
  await earlier.settle();
  assert.deepEqual(pageNotices(other), []);

  // Superseded is not lapsed: the newer draft is the agent's own news.
  const replaced = newPage();
  const third = await readBackDuringACall(replaced);
  replaced.propose(secondDraft());
  await replaced.settle();
  assert.deepEqual(pageNotices(third), []);
});

test("`#579 voice-send-confirm-timing`: on the bridge, a tap on Send or Don't send and a lapse are told to the agent as prompts", async () => {
  for (const [act, expected] of [
    ["send", /^Voice page: the user tapped Send, and the message was SENT to lead team\./],
    ["cancel", /^Voice page: the user tapped Don't send, so the draft was withdrawn and NOT sent\./],
    ["lapse", /^Voice page: the draft to lead team lapsed/],
  ]) {
    const page = newPage();
    const socket = await neutralReadBack(page);
    page.expireTimers(2000);
    if (act === "lapse") page.lapseProposal();
    else await page.el(act === "send" ? "post-confirm-send" : "post-confirm-cancel").click();
    await page.settle();
    page.expireTimers(CAPTURE_RESUME_MS);
    turnDone(socket, 2);
    await page.settle();
    const prompts = promptsSent(socket).filter((text) => text.startsWith("Voice page:"));
    assert.equal(prompts.length, 1, `${act}: ${prompts.join("\n")}`);
    assert.match(prompts[0], expected, act);
  }
});

/** The approvals the page sent on `socket`. */
const vendorApprovals = (socket) =>
  socket.sent.filter((frame) => typeof frame === "string").map((frame) => JSON.parse(frame))
    .filter((event) => event.type === "mcp_tool_approval_result");

/** The voice service reports a tool call in `state`, as it does for one marked "requires approval". */
const vendorToolCall = (socket, tool, id, state = "awaiting_approval") => hosted(socket, {
  type: "mcp_tool_call",
  mcp_tool_call: {
    service_id: "svc", tool_call_id: id, tool_name: tool, parameters: {}, state,
    timestamp: "2026-10-27T13:00:00Z", approval_timeout_secs: 10,
  },
});

test("`#579 voice-send-confirm-timing`: the voice service's request to approve post_reply is approved while the server asks first, read fresh, and nothing else is", async () => {
  const page = newPage();
  const socket = await startTalking(page);
  await page.settle();
  const reads = page.voiceSettingsReads;
  vendorToolCall(socket, "post_reply", "call-1");
  vendorToolCall(socket, "post_reply", "call-2", "loading");
  vendorToolCall(socket, "recent_activity", "call-3");
  await page.settle();
  assert.deepEqual(vendorApprovals(socket), [{ type: "mcp_tool_approval_result", tool_call_id: "call-1", is_approved: true }]);
  assert.equal(page.voiceSettingsReads, reads + 1, "approved without asking the server");
  assert.match(page.el("connection-detail").textContent, /waiting for someone to approve recent_activity/);

  // Turned off on another device during the call: the server now posts at once, so the vendor's
  // gate is the one left, and the page leaves it shut.
  page.voiceSettings = { ask_before_send: false, changeable: true };
  vendorToolCall(socket, "post_reply", "call-4");
  await page.settle();
  assert.equal(vendorApprovals(socket).length, 1, "approved a send that posts at once");
  assert.match(page.el("connection-detail").textContent, /not approved, since this server now posts at once/);

  // A read that fails approves nothing.
  page.voiceSettings = { ask_before_send: true, changeable: true };
  page.voiceSettingsReadStatus = 503;
  vendorToolCall(socket, "post_reply", "call-5");
  await page.settle();
  assert.equal(vendorApprovals(socket).length, 1, "approved on a failed read");

  // An agent told to post at once when the call began keeps whatever gate the owner set at the vendor.
  const direct = newPage();
  direct.voiceSettings = { ask_before_send: false, changeable: true };
  const other = await startTalking(direct);
  await direct.settle();
  vendorToolCall(other, "post_reply", "call-6");
  await direct.settle();
  assert.deepEqual(vendorApprovals(other), []);
});

test("`#579 voice-send-confirm-timing`: a held word is said at once when the owner speaks again, and waits longer while the agent is calling post_reply", async () => {
  // "yeah go ahead and send it" is not a plain yes; the agent said "One moment." and waits. A "did
  // it go?" before the hold ran out must not meet an agent the page told nothing.
  const page = newPage();
  const socket = await readBackDuringACall(page);
  hostedHears(socket, "yeah go ahead and send it");
  await settleSpokenYes(page);
  assert.deepEqual(pageNotices(socket), []);
  hostedSays(socket, "One moment.");
  hostedHears(socket, "did it go?");
  await settleSpokenYes(page);
  const notices = pageNotices(socket);
  assert.equal(notices.length, 1, notices.join("\n"));
  assert.match(notices[0], /^Voice page: the message was NOT sent: only a plain answer/);
  await settleRedraftWait(page);
  assert.equal(pageNotices(socket).length, 1, "said twice");

  // The voice service reports the agent calling post_reply: the hold starts again, so a slow draft
  // still lands before the word, and the word is dropped.
  const slow = newPage();
  const other = await readBackDuringACall(slow);
  slow.expireTimers(2000);
  await slow.el("post-confirm-send").click();
  await slow.settle();
  const before = pageNotices(other).length;
  hostedHears(other, "yes send Bob a note saying I'm late");
  await settleSpokenYes(slow);
  const holds = () => [...slow.timers].filter(([, timer]) => timer.ms === REDRAFT_WAIT_MS).map(([id]) => id);
  const first = holds();
  assert.equal(first.length, 1, "no word was held");
  vendorToolCall(other, "post_reply", "call-7", "loading");
  const again = holds();
  assert.equal(again.length, 1);
  assert.notEqual(again[0], first[0], "the hold was not started again");
  slow.propose(secondDraft());
  await slow.settle();
  await settleRedraftWait(slow);
  assert.deepEqual(pageNotices(other).slice(before), []);
});

test("`#579 voice-send-confirm-timing`: a draft that lapses while a yes settles on it gets one word, not a lapse notice that cuts the yes short", async () => {
  const page = newPage();
  const socket = await readBackDuringACall(page);
  hostedHears(socket, "yes");
  assert.match(page.el("status").textContent, /Heard yes/);
  page.lapseProposal();
  await page.settle();
  await settleSpokenYes(page);
  const notices = pageNotices(socket);
  assert.equal(notices.length, 1, notices.join("\n"));
  assert.doesNotMatch(notices[0], /typed message/);
  assert.match(notices[0], /NOT sent/);
});

test("`#264 voice-send-approval`: a commit the page cannot confirm tells the agent to check the channel, not that the card is up", async () => {
  const page = newPage();
  const socket = await readBackDuringACall(page);
  page.commitProposal = async () => json(200, { surprise: true });
  hostedHears(socket, "yes");
  await settleSpokenYes(page);
  assert.equal(page.el("post-confirm").hidden, true, "the server spent the proposal, so the card goes");
  const notices = pageNotices(socket);
  assert.equal(notices.length, 1, notices.join("\n"));
  assert.match(notices[0], /could not confirm whether the message was sent\. Tell the user to check the channel/);
  assert.doesNotMatch(notices[0], /card is still up/);
});

// What the agent is told, and what the owner hears, when a commit does not simply succeed. Each of
// these was once a mutation that still passed: a refusal, a partial post or a lost answer reported
// as SENT is exactly the claim this feature exists to prevent.

/** What the commit route answers, by case: a refusal, a partial post, or a request that never came back. */
const COMMIT_FAILURES = [
  {
    name: "a 409 refusal",
    answer: (page) => { page.settleProposal(); return json(409, { error: "proposal_superseded", detail: "server sentence" }); },
    notice: /^Voice page: the message was NOT sent \(a newer draft replaced it\)\. Tell the user in one short sentence that it was not sent\. Then stop\.$/,
    tapNotice: /^Voice page: the message was NOT sent \(a newer draft replaced it\)\./,
    status: "Not sent — a newer draft replaced it.",
    seam: "Not sent",
    screen: /replaced that proposal/,
  },
  {
    name: "a 410 refusal",
    answer: (page) => { page.settleProposal(); return json(410, { error: "proposal_expired", detail: "server sentence" }); },
    notice: /^Voice page: the message was NOT sent \(the draft lapsed first\)\./,
    tapNotice: /^Voice page: the message was NOT sent \(the draft lapsed first\)\./,
    status: "Not sent — the draft lapsed first.",
    seam: "Not sent",
    screen: /lapsed before Send reached the server/,
  },
  {
    name: "a 207 that posted nothing",
    answer: (page) => {
      page.settleProposal();
      return json(207, { error: "partially_posted", detail: "chat request failed", posted: 0, unsent: "on it, rebasing now" });
    },
    notice: /^Voice page: the message was NOT sent \(the chat service did not accept it\)\./,
    tapNotice: /^Voice page: the message was NOT sent \(the chat service did not accept it\)\./,
    status: "Not sent — the chat service did not accept it.",
    seam: "Not sent",
    screen: /^Nothing was posted to lead team: /,
  },
  {
    name: "a 207 that posted part",
    answer: (page) => {
      page.settleProposal();
      return json(207, { error: "partially_posted", detail: "chat request failed", posted: 1, unsent: "rebasing now" });
    },
    notice: /^Voice page: only part of the message reached lead team; the rest was NOT sent\./,
    tapNotice: /^Voice page: only part of the message reached lead team; the rest was NOT sent\./,
    status: "Only part reached lead team — the rest was not sent.",
    // `#270 voice-transcript-markers`: what went is a frozen card, never a seam.
    seam: null,
    card: /Partly sent to lead team/,
    screen: /^Only part of the message reached lead team: /,
  },
  {
    name: "a 207 whose answer for the first part was lost",
    answer: (page) => {
      page.settleProposal();
      return json(207, {
        error: "partially_posted", detail: "timed out", posted: 0, unsent: "on it, rebasing now", uncertain: true,
      });
    },
    notice: /^Voice page: I could not confirm whether the message was sent\. Tell the user to check the channel/,
    tapNotice: /^Voice page: I could not confirm whether the message was sent\./,
    status: "Could not confirm the send — check the channel.",
    seam: null,
    card: /Not confirmed: lead team/,
    screen: /could not be confirmed: timed out\. Check the channel/,
  },
  {
    name: "a request that never came back",
    answer: () => { throw new TypeError("Failed to fetch"); },
    notice: /^Voice page: I could not confirm whether the message was sent\. Tell the user to check the channel/,
    tapNotice: /^Voice page: I could not confirm whether the message was sent\./,
    status: "Could not confirm the send — check the channel.",
    seam: "Not confirmed",
    screen: /Could not reach vibe-talk/,
  },
];

test("`#264 voice-send-approval`: a spoken commit that is refused, partly posted or not confirmed is never told or played as sent", async () => {
  for (const failure of COMMIT_FAILURES) {
    const page = newPage();
    const socket = await readBackDuringACall(page);
    page.commitProposal = async () => failure.answer(page);
    const sources = page.audio.sources.length;
    hostedHears(socket, "yes");
    await settleSpokenYes(page);
    assert.deepEqual(page.proposalCommits.map((c) => c.confirmed_by), ["speaker_turn"], failure.name);
    const notices = pageNotices(socket);
    assert.equal(notices.length, 1, `${failure.name}: ${notices.join("\n")}`);
    assert.match(notices[0], failure.notice, failure.name);
    assert.doesNotMatch(notices[0], /was SENT/, failure.name);
    assert.equal(page.el("status").textContent, failure.status, failure.name);
    const seams = page.el("transcript").children.filter((li) => li.className === "seam");
    if (failure.seam === null) {
      assert.equal(seams.length, 0, `${failure.name}: a seam beside the frozen card`);
      const card = page.el("transcript").children.find((li) => li.hasClass("sent-card"));
      const words = (node) => [node.textContent, ...node.children.map(words)].join(" ");
      assert.ok(card && failure.card.test(words(card)), `${failure.name}: no frozen card said so`);
    } else {
      assert.equal(seamLabel(seams.at(-1)), failure.seam, failure.name);
    }
    assert.ok(!seams.some((li) => seamLabel(li) === "Sent by voice"), `${failure.name}: a seam said sent`);
    assert.deepEqual(tonesSince(page, sources), ["not sent"], failure.name);
  }
});

test("`#264 voice-send-approval`: a tap on Send during a call that is refused, partly posted or not confirmed is never told to the agent as sent", async () => {
  for (const failure of COMMIT_FAILURES) {
    const page = newPage();
    const socket = await readBackDuringACall(page);
    page.commitProposal = async () => failure.answer(page);
    page.expireTimers(2000); // the card's own hold on Send
    const sources = page.audio.sources.length;
    await page.el("post-confirm-send").click();
    await page.settle();
    assert.deepEqual(page.proposalCommits.map((c) => c.confirmed_by), ["ui"], failure.name);
    const notices = pageNotices(socket);
    assert.equal(notices.length, 1, `${failure.name}: ${notices.join("\n")}`);
    assert.match(notices[0], failure.tapNotice, failure.name);
    assert.doesNotMatch(notices[0], /SENT to/, failure.name);
    // The tap's own result is on the screen already, as every tap's is: the error says what happened,
    // and no seam or tone claims a send the owner can see did not happen.
    assert.equal(page.el("error").hidden, false, `${failure.name} was not reported on screen`);
    assert.match(page.el("error").textContent, failure.screen, failure.name);
    assert.doesNotMatch(page.el("status").textContent, /^(Posted|Sent) to/, failure.name);
    const seams = page.el("transcript").children.filter((li) => li.className === "seam");
    assert.ok(!seams.some((li) => seamLabel(li) === "Sent by voice"), `${failure.name}: a seam said sent`);
    assert.deepEqual(tonesSince(page, sources), [], `${failure.name}: a tone played for a tap`);
  }
});

test("`#264 voice-send-approval`: a tap during a yes's settle window that is then refused is reported as the yes's would be: not sent, aloud", async () => {
  const page = newPage();
  const socket = await readBackDuringACall(page);
  page.expireTimers(2000);
  page.commitProposal = async () => {
    page.settleProposal();
    return json(409, { error: "proposal_superseded", detail: "server sentence" });
  };
  const sources = page.audio.sources.length;
  hostedHears(socket, "yes");
  await page.el("post-confirm-send").click();
  await page.settle();
  await settleSpokenYes(page);
  assert.deepEqual(page.proposalCommits.map((c) => c.confirmed_by), ["ui"]);
  const notices = pageNotices(socket);
  assert.equal(notices.length, 1, notices.join("\n"));
  assert.match(notices[0], /^Voice page: the message was NOT sent \(a newer draft replaced it\)\./);
  assert.equal(page.el("status").textContent, "Not sent — a newer draft replaced it.");
  assert.deepEqual(tonesSince(page, sources), ["not sent"]);
});

test("`#264 voice-send-approval`: the outcome's tone is queued after the speech already playing, and what comes next waits for it", async () => {
  const page = newPage();
  const socket = await readBackDuringACall(page);
  hostedAudio(socket);
  const queuedUntil = page.audio.sources.at(-1).startedAt + 1;
  page.setClock(page.clock() + 3000);
  page.audio.currentTime = 0.5;
  hostedHears(socket, "yes please");
  await settleSpokenYes(page);
  assert.deepEqual(page.proposalCommits.map((c) => c.confirmed_by), ["speaker_turn"]);
  const tone = page.audio.sources.at(-1);
  assert.ok(Math.abs(tone.startedAt - queuedUntil) < 1e-6, `the tone started at ${tone.startedAt}, not after the speech at ${queuedUntil}`);
  hostedAudio(socket);
  const next = page.audio.sources.at(-1);
  assert.ok(next.startedAt >= tone.startedAt + tone.buffer.duration - 1e-6, "the agent's next words were scheduled over the tone");
});

test("`#264 voice-send-approval`: every seam a spoken yes can draw is inside the seam band", () => {
  const table = sourceStringTable("SPOKEN_YES_SEAMS");
  assert.ok(Object.keys(table).length >= 5, "the spoken-yes seams are not being read");
  for (const [kind, text] of Object.entries(table)) {
    const spent = words(text);
    assert.ok(spent >= SEAM_DETAIL_MIN_WORDS && spent <= SEAM_DETAIL_MAX_WORDS, `${kind}: ${spent} words: "${text}"`);
  }
});

// --- `#269 approval-card-inline`: a spoken yes the page acts on shows no turn -----------------------

/**
 * A hosted call on a server that keeps records, in `conversationId`, in which this call's agent
 * proposed `pending` and read it back. Returns the socket.
 */
async function recordedReadBack(page, conversationId, pending = spokenDraft()) {
  const socket = await recordingCall(page, conversationId);
  hostedHears(socket, "tell lead team I'm on it, rebasing now");
  page.propose(pending);
  await page.settle();
  hostedSays(socket, `To lead team: ${pending.text}. Send it?`);
  return socket;
}

/** The newest row of the transcript whose words are `said`, absorbed or not. */
const rowSaying = (page, said) =>
  [...page.el("transcript").children].reverse().find((li) => !li.hasClass("sent-card") && li.className !== "seam" &&
    li.children.some((kid) => kid.className === "body" && kid.textContent === said));

/** Whether `li` is out of sight: drawn into the card it sent. */
const absorbed = (li) => Boolean(li) && li.hasAttribute("data-absorbed");

/** A stored conversation's rows, as `speaker: text` with an answer's serial or a record's kind. */
const storedShapes = (page, id) => (page.storedTurns.get(id) || []).map((turn) =>
  turn.speaker === "sent" ? `sent ${turn.detail.how}`
    : turn.speaker === "marker" ? `marker ${turn.detail.kind}`
      : `${turn.speaker}: ${turn.text}${turn.detail && turn.detail.kind === "answer" ? ` [answer ${turn.detail.serial}]` : ""}`);

test("`#269 approval-card-inline`: a spoken yes that sends is never on screen; the card says approved by voice, and the record keeps the yes, tagged, in screen order", async () => {
  const page = newPage();
  const pending = spokenDraft({ serial: 7 });
  const socket = await recordedReadBack(page, "conv_yes", pending);
  const hiddenNow = (when) => assert.ok(absorbed(rowSaying(page, "yes please")), `the yes was on screen ${when}`);
  hostedHears(socket, "yes please");
  hiddenNow("as it arrived");
  await page.settle();
  hiddenNow("while it settled");
  hostedSays(socket, "One moment.");
  hiddenNow("beside the agent's reply");
  await settleSpokenYes(page);
  await watchTurns(page);
  hiddenNow("once it had sent");
  assert.deepEqual(page.proposalCommits.map((c) => c.confirmed_by), ["speaker_turn"]);
  const card = sentCards(page)[0];
  assert.match(wordsOf(card), /approved by voice/);
  assert.equal(rowSaying(page, "yes please").getAttribute("data-answer"), "7");
  // Kept, and in the order the screen holds them, the hidden yes included.
  const order = ["agent: To lead team: on it, rebasing now. Send it?", "you: yes please [answer 7]", "agent: One moment.", "sent voice"];
  assert.deepEqual(storedShapes(page, "conv_yes").slice(-4), order);
  const rows = page.el("transcript").children;
  const at = [rowSaying(page, `To lead team: ${pending.text}. Send it?`), rowSaying(page, "yes please"), rowSaying(page, "One moment."), card]
    .map((li) => rows.indexOf(li));
  assert.deepEqual([...at].sort((a, b) => a - b), at, `the screen's order is not the record's: ${at.join(", ")}`);
  // Only the yes is out of sight.
  assert.equal(page.el("transcript").children.filter((li) => absorbed(li)).length, 1);
});

test("`#269 approval-card-inline`: on vibe-talk-v1 a yes is out of sight from its first hypothesis, and filed tagged before the reply, with or without a final", async () => {
  for (const finalFirst of [false, true]) {
    const label = finalFirst ? "with a final" : "hypotheses only";
    const page = newPage();
    page.transcriptRecordsSupported = true;
    const pending = spokenDraft({ serial: 9 });
    const socket = await neutralReadBack(page, { pending });
    const yes = () => [...page.el("transcript").children].reverse().find((li) =>
      li.children.some((kid) => kid.className === "body" && /^ye/i.test(kid.textContent)));
    const hiddenNow = (when) => assert.ok(absorbed(yes()), `${label}: the yes was on screen ${when}`);
    speech(socket, "user", 2, "ye", false);
    hiddenNow("at its first hypothesis");
    speech(socket, "user", 2, "yes", false);
    hiddenNow("as its hypothesis grew");
    if (finalFirst) {
      speech(socket, "user", 2, "Yes.", true);
      hiddenNow("at its final");
    }
    speech(socket, "assistant", 2, "One moment.", true);
    hiddenNow("beside the reply");
    turnDone(socket, 2);
    hiddenNow("at its turn's end");
    await settleSpokenYes(page);
    await watchTurns(page);
    hiddenNow("once it had sent");
    assert.deepEqual(page.proposalCommits.map((c) => c.confirmed_by), ["speaker_turn"], label);
    assert.match(wordsOf(sentCards(page)[0]), /approved by voice/, label);
    const [id] = [...page.storedTurns.keys()];
    const said = finalFirst ? "Yes." : "yes";
    assert.deepEqual(storedShapes(page, id).slice(-3), [`you: ${said} [answer 9]`, "agent: One moment.", "sent voice"], label);
  }
});

test("`#269 approval-card-inline`: a step back that reaches a yes whose card approved by voice is on the newer page draws it into that card", async () => {
  // A page of the record can end between a yes and the card it sent: the newer page holds the card,
  // the step back brings the yes. Approved by tap, the same yes is a turn of its own.
  for (const how of ["voice", "tap"]) {
    const page = newPage();
    page.transcriptRecordsSupported = true;
    const at = (i) => RECORD_EPOCH + i * 60_000;
    const filler = (from, count) => Array.from({ length: count }, (_unused, i) => ({
      speaker: (from + i) % 2 === 0 ? "you" : "agent", text: `paged turn ${from + i}`, at_ms: at(from + i),
    }));
    const sent = {
      speaker: "sent", text: "on it, rebasing now", at_ms: at(8),
      detail: {
        kind: "sent", serial: 7, send_seq: 1, channel_id: "1110000000000000001", channel_name: "lead team",
        parts_posted: 1, parts_total: 1, unsent_chars: 0, outcome: "sent", how, origin: "commit",
        text_truncated: false, text_chars: 19, begun_at_ms: at(8), sent_at_ms: at(8),
      },
    };
    page.storedTurns.set("conv_paged", [
      ...filler(0, 5),
      { speaker: "agent", text: "To lead team: on it, rebasing now. Send it?", at_ms: at(6) },
      { speaker: "you", text: "yes please", at_ms: at(7), detail: { kind: "answer", serial: 7 } },
      sent,
      ...filler(9, TRANSCRIPT_PAGE_LIMIT - 1),
    ]);
    await signIn(page);
    await page.settle();
    assert.equal(sentCards(page).length, 1, `${how}: the newer page did not draw the card`);
    assert.equal(rowSaying(page, "yes please"), undefined, `${how}: the yes was on the newer page: this tests nothing`);
    await page.el("load-older-turns").click();
    await page.settle();
    const yes = rowSaying(page, "yes please");
    assert.ok(yes, `${how}: the step back did not bring the yes`);
    assert.equal(absorbed(yes), how === "voice",
      how === "voice" ? "the step back drew the yes beside the card approved by voice" : "a yes was hidden beside a card approved by tap");
  }
});

test("`#269 approval-card-inline`: a yes stopped in its settle window, or overtaken by a tap, is shown again in its own place", async () => {
  const page = newPage();
  const socket = await recordedReadBack(page, "conv_wait");
  hostedHears(socket, "yes");
  assert.ok(absorbed(rowSaying(page, "yes")));
  hostedHears(socket, "wait");
  const yes = rowSaying(page, "yes");
  assert.ok(!absorbed(yes), "a yes stopped by 'wait' stayed out of sight");
  const rows = page.el("transcript").children;
  assert.equal(rows.indexOf(rowSaying(page, "wait")), rows.indexOf(yes) + 1, "the yes came back somewhere other than its place");
  await settleSpokenYes(page);
  await watchTurns(page);
  assert.deepEqual(page.proposalCommits, []);
  const kept = storedShapes(page, "conv_wait");
  assert.ok(kept.findIndex((row) => row.startsWith("you: yes")) === kept.indexOf("you: wait") - 1, kept.join(" | "));
  // Nothing was sent, so a reload draws the yes as the turn it was, whatever its tag.
  const reloaded = newPage();
  reloaded.transcriptRecordsSupported = true;
  reloaded.storedTurns.set("conv_wait", page.storedTurns.get("conv_wait").map((turn) => ({ ...turn })));
  await signIn(reloaded);
  await reloaded.settle();
  assert.ok(rowSaying(reloaded, "yes") && !absorbed(rowSaying(reloaded, "yes")), "a reload hid a yes that sent nothing");

  // A tap during the settle window is what approved it: the yes is a turn again, and the card says so.
  const tapped = newPage();
  const tappedSocket = await recordedReadBack(tapped, "conv_tapped");
  tapped.expireTimers(2000);
  hostedHears(tappedSocket, "yes");
  assert.ok(absorbed(rowSaying(tapped, "yes")));
  await tapped.el("post-confirm-send").click();
  await watchTurns(tapped);
  assert.deepEqual(tapped.proposalCommits.map((c) => c.confirmed_by), ["ui"]);
  assert.ok(!absorbed(rowSaying(tapped, "yes")), "a tapped send hid the yes");
  assert.match(wordsOf(sentCards(tapped)[0]), /approved by tap/);
});

test("`#269 approval-card-inline`: a yes whose commit is refused, or hangs, comes back; one that grows past a yes is shown and never hidden again", async () => {
  const refused = newPage();
  const socket = await recordedReadBack(refused, "conv_refused");
  refused.commitProposal = async () => {
    refused.settleProposal("superseded");
    return json(409, { error: "proposal_superseded", detail: "newer" });
  };
  hostedHears(socket, "yes");
  await settleSpokenYes(refused);
  await watchTurns(refused);
  assert.ok(!absorbed(rowSaying(refused, "yes")), "a yes whose send was refused stayed out of sight");
  const seam = refused.el("transcript").children.find((li) => li.className === "seam" && !li.getAttribute("data-not-sent"));
  assert.ok(seam && seamLabel(seam) === "Not sent", "the refused send drew no line");
  assert.deepEqual(sentCards(refused), []);

  // A commit that never answers: the row comes back after YES_HOLD_MAX_MS, and the turns after it
  // were filed without waiting for it.
  const hung = newPage();
  const hungSocket = await recordedReadBack(hung, "conv_hung_commit");
  hung.commitProposal = () => new Promise(() => {});
  hostedHears(hungSocket, "yes");
  hung.expireTimers(SPOKEN_SETTLE_MS);
  await hung.settle();
  hostedSays(hungSocket, "One moment.");
  await watchTurns(hung);
  assert.ok(absorbed(rowSaying(hung, "yes")), "the yes came back while its send was still on its way");
  assert.ok(storedShapes(hung, "conv_hung_commit").includes("agent: One moment."), "a turn after the yes waited for the send");
  const max = optionalSourceConstant("YES_HOLD_MAX_MS");
  assert.ok(max !== undefined, "web/voice.js states no YES_HOLD_MAX_MS");
  assert.ok(hung.expireTimers(max) >= 1, "nothing bounds how long a yes stays out of sight");
  assert.ok(!absorbed(rowSaying(hung, "yes")), "a yes whose send hung stayed out of sight");

  // vibe-talk-v1: "ye" grows into something that is no yes.
  const grows = newPage();
  const v1 = await neutralReadBack(grows);
  const row = () => [...grows.el("transcript").children].reverse().find((li) =>
    li.children.some((kid) => kid.className === "body" && /^ye/i.test(kid.textContent)));
  speech(v1, "user", 2, "ye", false);
  assert.ok(absorbed(row()), "a hypothesis that may be a yes was on screen");
  speech(v1, "user", 2, "yesterday's build failed", false);
  assert.ok(!absorbed(row()), "a turn that is no yes stayed out of sight");
  speech(v1, "user", 2, "yes", false);
  assert.ok(!absorbed(row()), "a row once shown was hidden again");
});

test("`#269 approval-card-inline`: a yes the page did not act on — an echo, one said early, one after an interruption — is shown, and filed untagged", async () => {
  const playedMs = (0.25 + 1) * 1000;
  for (const [what, arrange, said] of [
    ["an echo", (page, socket) => { hostedAudio(socket); page.setClock(page.clock() + playedMs + 500); }, "send it"],
    ["said early", (_page, socket) => hostedAudio(socket), "yes"],
    ["after an interruption", (_page, socket) => hosted(socket, { type: "interruption", interruption_event: {} }), "yes"],
  ]) {
    const page = newPage();
    const socket = await recordedReadBack(page, "conv_refused_yes");
    arrange(page, socket);
    hostedHears(socket, said);
    assert.ok(rowSaying(page, said) && !absorbed(rowSaying(page, said)), `${what}: the yes stayed out of sight`);
    await settleSpokenYes(page);
    await watchTurns(page);
    assert.deepEqual(page.proposalCommits, [], what);
    assert.ok(storedShapes(page, "conv_refused_yes").includes(`you: ${said}`), `${what}: ${storedShapes(page, "conv_refused_yes").join(" | ")}`);
  }

  // THE CONTROL: a judged turn the page did not take is left out of sight. It must stay hidden.
  const control = newPage(new Map(), brokenScript(
    '  if (hold && hold.state === "tentative" && spokenYes.answered === key) releaseYesHold(hold);',
    "  void hold;"
  ));
  const controlSocket = await recordedReadBack(control, "conv_control");
  hosted(controlSocket, { type: "interruption", interruption_event: {} });
  hostedHears(controlSocket, "yes");
  assert.ok(absorbed(rowSaying(control, "yes")), "the control showed the yes: this test cannot see the defect");
});

test("`#269 approval-card-inline`: restored, a tagged yes is drawn into the voice-approved card after it, and only the newest yes for that card", async () => {
  const send = (serial, how) => ({
    speaker: "sent", text: "on it, rebasing now", at_ms: 1_700_000_000_010,
    detail: {
      kind: "sent", serial, send_seq: serial, channel_id: CHANNEL.id, channel_name: "lead team", parts_posted: 1,
      parts_total: 1, unsent_chars: 0, outcome: "sent", how, origin: "commit", text_truncated: false, text_chars: 19,
      begun_at_ms: 1, sent_at_ms: 1,
    },
  });
  const yes = (text, serial) => ({ speaker: "you", text, at_ms: 1_700_000_000_001, detail: { kind: "answer", serial } });
  const page = newPage();
  page.transcriptRecordsSupported = true;
  page.storedTurns.set("conv_restored", [
    { speaker: "agent", text: "To lead team: on it, rebasing now. Send it?", at_ms: 1_700_000_000_000 },
    yes("yes", 4),
    { speaker: "you", text: "wait", at_ms: 1_700_000_000_002 },
    yes("yes please", 4),
    { speaker: "agent", text: "One moment.", at_ms: 1_700_000_000_003 },
    send(4, "voice"),
    // A tagged yes whose send was tapped, and one whose send never came: both are turns.
    yes("sure", 5),
    send(5, "tap"),
    yes("yeah", 6),
  ]);
  await signIn(page);
  await page.settle();
  assert.ok(absorbed(rowSaying(page, "yes please")), "the yes that sent the card was drawn as a turn");
  assert.equal(rowSaying(page, "yes please").getAttribute("data-answer"), "4");
  for (const said of ["yes", "wait", "sure", "yeah", "One moment."]) {
    assert.ok(rowSaying(page, said) && !absorbed(rowSaying(page, said)), `"${said}" was drawn out of sight`);
  }
  assert.equal(sentCards(page).length, 2);
});

// --- `#272 voice-agent-instructions` --------------------------------------------------------------

/** Sign in with the owner's words as the server reports them, and open Settings. */
async function instructionsPage(fields = {}, arrange = (_page) => {}) {
  const page = newPage();
  page.voicePrompt = voicePromptView(fields);
  arrange(page);
  await signIn(page);
  await page.el("open-settings").click();
  return page;
}

test("`#272 voice-agent-instructions`: the Instructions group is a box, a Save, and a folded system prompt", () => {
  const group = settingsGroup("Instructions");
  assert.match(group, /<label for="voice-instructions">Additional instructions<\/label>/);
  assert.equal(PAGE_ELEMENTS.get("voice-instructions").tag, "textarea");
  assert.equal(PAGE_ELEMENTS.get("voice-system-prompt").tag, "textarea");
  const fold = /<details id="voice-system-prompt-fold" class="settings-fold">\s*<summary>Edit the system prompt<\/summary>([\s\S]*?)<\/details>/.exec(group);
  assert.ok(fold, "the system prompt is not in a folded disclosure headed Edit the system prompt");
  assert.doesNotMatch(group, /<details[^>]*\bopen\b/, "the system prompt is served unfolded");
  for (const id of ["voice-system-prompt", "save-voice-system-prompt", "restore-voice-system-prompt"]) {
    assert.ok(fold[1].includes(`id="${id}"`), `#${id} is not inside the fold`);
  }
  assert.match(fold[1], /added after it from your token and the switch above/,
    "the fold does not say that what the agent may do is not in the box");
  assert.match(group, /data-help="voice-instructions"/, "the group offers no way to reach the detail");
  const help = helpEntry("voice-instructions");
  assert.match(help, /Neither box can change what the agent is able to do/);
  assert.match(help, /only the additional instructions, as a\s+note at the start of the call/);
  assert.match(help, /given this server's prompt on\s+every call, even with nothing of yours saved/);
  assert.match(help, /give such an agent the write token/);
  assert.match(help, /starts exactly as it did\s+before/);
  assert.equal(PAGE_ELEMENTS.get("voice-prompt-editor").hidden, true, "the editor is served before the server says it exists");
});

test("`#272 voice-agent-instructions`: the boxes show what the server holds, with a line for each", async () => {
  const page = await instructionsPage({ instructions: "Call me Sam." });
  assert.equal(page.el("voice-prompt-editor").hidden, false);
  assert.equal(page.el("voice-instructions").value, "Call me Sam.");
  assert.equal(page.el("voice-system-prompt").value, SHIPPED_SYSTEM_PROMPT);
  assert.equal(page.el("voice-instructions-state").textContent, "Saved. Added to every call.");
  assert.equal(page.el("voice-system-prompt-state").textContent, "The shipped prompt, version 9.");
  assert.equal(page.el("save-voice-instructions").disabled, true, "Save is live with nothing to save");
  assert.equal(page.el("save-voice-system-prompt").disabled, true);
  assert.equal(page.el("restore-voice-system-prompt").disabled, true, "Restore is live on the shipped prompt");
  assert.match(page.el("voice-prompt-state").textContent, /hands them to the agent as each call starts; the prompt only if its provider lets it be replaced/);
  // `#272`: the replaced prompt is always the write token's, and Settings says what that asks of
  // the agent's own credential.
  assert.match(page.el("voice-prompt-state").textContent, /A replaced prompt tells the agent it may send, so that agent needs the write token\./);

  const bridge = await instructionsPage({ delivery: "profile" });
  assert.equal(bridge.el("voice-prompt-state").textContent, "Both reach the agent at its next call.");
  assert.equal(bridge.el("voice-instructions-state").textContent, "None.");
});

test("`#272 voice-agent-instructions`: typing says it is not saved, and Save sends that box alone and draws what was stored", async () => {
  const page = await instructionsPage();
  await page.el("voice-instructions").setValue("  Call me Sam.\nKeep it short.  ");
  assert.equal(page.el("voice-instructions-state").textContent, "Not saved yet.");
  assert.equal(page.el("save-voice-instructions").disabled, false);
  assert.equal(page.el("save-voice-system-prompt").disabled, true, "the other box's Save woke up");
  await page.el("save-voice-instructions").click();
  await page.settle();
  assert.deepStrictEqual(page.voicePromptCalls, [{ instructions: "  Call me Sam.\nKeep it short.  " }]);
  assert.equal(page.el("voice-instructions").value, "Call me Sam.\nKeep it short.", "the box kept the click, not what was stored");
  assert.equal(page.el("voice-instructions-state").textContent, "Saved. Added to every call.");
  assert.equal(page.el("save-voice-instructions").disabled, true);
  assert.match(page.el("status").textContent, /gets these instructions from its next call/);

  await page.el("voice-instructions").setValue("");
  await page.el("save-voice-instructions").click();
  await page.settle();
  assert.deepStrictEqual(page.voicePromptCalls[1], { instructions: "" });
  assert.match(page.el("status").textContent, /no additional instructions from its next call/);
});

test("`#272 voice-agent-instructions`: saving the system prompt keeps an unsaved instruction, and says whose prompt it is", async () => {
  const page = await instructionsPage({ instructions: "Call me Sam." });
  await page.el("voice-instructions").setValue("Call me Samantha.");
  await page.el("voice-system-prompt").setValue("You read me the chat while I drive.");
  assert.equal(page.el("voice-system-prompt-state").textContent,
    "Not saved yet. Saving an empty box restores the shipped prompt.");
  assert.equal(page.el("restore-voice-system-prompt").disabled, false);
  await page.el("save-voice-system-prompt").click();
  await page.settle();
  assert.deepStrictEqual(page.voicePromptCalls, [{ system_prompt: "You read me the chat while I drive." }]);
  assert.equal(page.el("voice-instructions").value, "Call me Samantha.", "saving the prompt threw away the instruction being typed");
  assert.equal(page.el("save-voice-instructions").disabled, false, "the unsaved instruction can no longer be saved");
  assert.equal(page.el("voice-system-prompt-state").textContent, "Your copy, in place of the shipped prompt.");
  assert.match(page.el("status").textContent, /gets your prompt from its next call/);

  // Restore, tapped twice, saves an empty box, and the server's shipped prompt comes back.
  await page.el("restore-voice-system-prompt").click();
  await page.el("restore-voice-system-prompt").click();
  await page.settle();
  assert.deepStrictEqual(page.voicePromptCalls[1], { system_prompt: "" });
  assert.equal(page.el("voice-system-prompt").value, SHIPPED_SYSTEM_PROMPT);
  assert.equal(page.el("voice-system-prompt-state").textContent, "The shipped prompt, version 9.");
  assert.match(page.el("status").textContent, /gets the shipped prompt from its next call/);
});

test("`#272 voice-agent-instructions`: Restore with nothing saved puts the box back and writes nothing", async () => {
  const page = await instructionsPage();
  await page.el("voice-system-prompt").setValue("half an edit");
  await page.el("restore-voice-system-prompt").click();
  assert.equal(page.el("voice-system-prompt").value, "half an edit", "one tap threw the edit away");
  await page.el("restore-voice-system-prompt").click();
  await page.settle();
  assert.deepStrictEqual(page.voicePromptCalls, [], "putting back an unsaved edit wrote to the server");
  assert.equal(page.el("voice-system-prompt").value, SHIPPED_SYSTEM_PROMPT);
  assert.equal(page.el("restore-voice-system-prompt").disabled, true);
});

test("`#272 voice-agent-instructions`: an edit saved against an older shipped prompt says so", async () => {
  const page = await instructionsPage({
    system_prompt: "mine", system_prompt_edited: true, edited_from_version: 8,
  });
  assert.equal(page.el("voice-system-prompt-state").textContent,
    "Your copy, saved when the shipped prompt was version 8. It is now version 9; restore it to see what changed.");
  assert.equal(page.el("restore-voice-system-prompt").disabled, false);
});

test("`#272 voice-agent-instructions`: a text over the limit cannot be saved, and says how long it is", async () => {
  const page = await instructionsPage({ max_instructions_chars: 10 });
  await page.el("voice-instructions").setValue("x".repeat(11));
  assert.equal(page.el("save-voice-instructions").disabled, true);
  assert.equal(page.el("voice-instructions-state").textContent, "11 characters; at most 10 are kept. Shorten them to save.");
  // Counted as the server counts, by character: one emoji is one, not two.
  await page.el("voice-instructions").setValue("🙂".repeat(10));
  assert.equal(page.el("save-voice-instructions").disabled, false);
});

test("`#272 voice-agent-instructions`: a refused save keeps what was typed and says it was not saved", async () => {
  const page = await instructionsPage({}, (p) => { p.voicePromptStatus = 503; });
  await page.el("voice-instructions").setValue("Call me Sam.");
  await page.el("save-voice-instructions").click();
  await page.settle();
  assert.equal(page.el("voice-instructions").value, "Call me Sam.", "a refused save lost the text");
  assert.equal(page.el("voice-instructions-state").textContent, "Not saved yet.");
  assert.equal(page.el("save-voice-instructions").disabled, false, "the text cannot be tried again");
  assert.equal(page.el("error").hidden, false);
  assert.match(page.el("error").textContent, /The instructions were not saved/);
});

test("`#272 voice-agent-instructions`: a read token and a store-less server can read the words but not change them; an older server hides them", async () => {
  const read = await instructionsPage({ instructions: "Call me Sam." }, (p) => { p.tokenScope = "read"; });
  assert.equal(read.el("voice-instructions").value, "Call me Sam.");
  assert.equal(read.el("voice-instructions").disabled, true);
  assert.equal(read.el("voice-system-prompt").disabled, true);
  assert.equal(read.el("save-voice-instructions").disabled, true);
  assert.equal(read.el("restore-voice-system-prompt").disabled, true);
  assert.match(read.el("voice-prompt-state").textContent, /^Only the write token can change these\./);

  const storeless = await instructionsPage({ changeable: false });
  assert.equal(storeless.el("voice-instructions").disabled, true);
  assert.match(storeless.el("voice-prompt-state").textContent, /keeps no settings/);

  const older = newPage();
  older.voicePrompt = undefined;
  await signIn(older);
  assert.equal(older.el("voice-prompt-editor").hidden, true);
  assert.equal(older.el("voice-prompt-state").textContent, "This server does not take instructions for its voice agent.");
});

/** A hosted agent's session carrying the owner's words, as `/api/v1/voice-session` serializes one. */
const mintWithPrompt = (agentPrompt) => async () => {
  const minted = JSON.parse(await (await MINTED()).text());
  return json(200, agentPrompt === undefined ? minted : { ...minted, agent_prompt: agentPrompt });
};

/** Start a spoken call on a session handed `agentPrompt`, the way `startTalking` starts one. */
async function talkWithPrompt(page, agentPrompt) {
  page.el("api-token").value = "write-token-aaaaaaaaaaaaaaaa";
  await page.el("save-token").click();
  await page.settle();
  page.setFetch(mintWithPrompt(agentPrompt));
  await page.el("talk").click();
  assert.equal(page.el("error").hidden, true, `start() failed: ${page.el("error").textContent}`);
  assert.equal(page.sockets.length, 1, "start() opened no websocket");
  page.sockets[0].onopen();
  assert.equal(state(page), "live");
  return page.sockets[0];
}

const OVERRIDE = {
  transport: "prompt_override",
  text: "You read me the chat.\n\nThe user has added the standing instructions below.\n\nCall me Sam.",
  system_prompt_applied: true,
  instructions_applied: true,
  override_allowed: true,
};
const NOTE = {
  transport: "contextual_update",
  text: "The user has added the standing instructions below.\nCall me Sam.",
  system_prompt_applied: false,
  instructions_applied: true,
  override_allowed: false,
};

test("`#272 voice-agent-instructions`: an agent that allows it is given the whole prompt on the initiation, and nothing more", async () => {
  const page = newPage();
  page.voicePrompt = voicePromptView({
    instructions: "Call me Sam.", system_prompt: "You read me the chat.", system_prompt_edited: true, edited_from_version: 9,
  });
  const socket = await talkWithPrompt(page, OVERRIDE);
  assert.deepStrictEqual(initiationFrame(socket), {
    type: "conversation_initiation_client_data",
    conversation_config_override: { agent: { prompt: { prompt: OVERRIDE.text } } },
  });
  assert.equal(framesOfType(socket, "contextual_update").length, 0, "the words went twice");
  assert.equal(framesSent(socket)[0].type, "conversation_initiation_client_data");
  assert.match(page.el("voice-prompt-state").textContent, /The last call was given your prompt and instructions\.$/);
});

test("`#272 voice-agent-instructions`: an agent that does not is sent the instructions as a note straight after an unchanged initiation", async () => {
  const page = newPage();
  page.voicePrompt = voicePromptView({
    instructions: "Call me Sam.", system_prompt: "You read me the chat.", system_prompt_edited: true, edited_from_version: 9,
  });
  const socket = await talkWithPrompt(page, NOTE);
  assert.deepStrictEqual(initiationFrame(socket), { type: "conversation_initiation_client_data" },
    "an override went to an agent that does not allow one");
  const frames = framesSent(socket);
  assert.equal(frames[0].type, "conversation_initiation_client_data");
  assert.deepStrictEqual(frames[1], { type: "contextual_update", text: NOTE.text },
    "the note is not the frame straight after the initiation");
  assert.match(page.el("voice-prompt-state").textContent,
    /The last call's agent does not let its prompt be replaced, so only your instructions went, as a note at the start\.$/);

  const unknown = newPage();
  const { override_allowed: _said, ...unsaid } = NOTE;
  await talkWithPrompt(unknown, unsaid);
  assert.match(unknown.el("voice-prompt-state").textContent, /could not be checked, so only your instructions went/);
});

test("`#272 voice-agent-instructions`: an edited prompt an agent cannot take sends nothing, and Settings says it did not reach it", async () => {
  const page = newPage();
  page.voicePrompt = voicePromptView({ system_prompt: "mine", system_prompt_edited: true, edited_from_version: 9 });
  const socket = await talkWithPrompt(page, {
    transport: "not_sent", system_prompt_applied: false, instructions_applied: false, override_allowed: false,
  });
  assert.deepStrictEqual(framesSent(socket).filter((f) => f.type).map((f) => f.type),
    ["conversation_initiation_client_data"]);
  assert.deepStrictEqual(initiationFrame(socket), { type: "conversation_initiation_client_data" });
  assert.match(page.el("voice-prompt-state").textContent, /so your edited prompt did not reach it\.$/);
});

test("`#272 voice-agent-instructions`: an agent that keeps its console prompt, with nothing saved, is said to keep it, beside the switch too", async () => {
  // The vendor's default, and nothing of the owner's: the server now says `not_sent` rather than
  // nothing, so the page knows this agent keeps its console prompt. Nothing more goes on the wire,
  // and Settings says what happened without claiming an edited prompt that does not exist.
  const page = newPage();
  const socket = await talkWithPrompt(page, {
    transport: "not_sent", system_prompt_applied: false, instructions_applied: false, override_allowed: false,
  });
  assert.deepStrictEqual(framesSent(socket).filter((f) => f.type).map((f) => f.type),
    ["conversation_initiation_client_data"]);
  assert.deepStrictEqual(initiationFrame(socket), { type: "conversation_initiation_client_data" });
  assert.match(page.el("voice-prompt-state").textContent,
    /The last call's agent does not let its prompt be replaced, so it kept the prompt in its provider's console\.$/);
  assert.doesNotMatch(page.el("voice-prompt-state").textContent, /edited prompt/, "an edit nobody made was reported");
  assert.equal(page.el("voice-send-hosted").textContent, HOSTED_KEPT, "the warning beside the switch stayed conditional");
});

test("`#272 voice-agent-instructions`: a session with no agent_prompt, a bridge's or an older server's, opens exactly as before, and says nothing about it", async () => {
  const page = newPage();
  const socket = await talkWithPrompt(page, undefined);
  assert.deepStrictEqual(initiationFrame(socket), { type: "conversation_initiation_client_data" });
  assert.equal(framesOfType(socket, "contextual_update").length, 0);
  assert.doesNotMatch(page.el("voice-prompt-state").textContent, /last call/);
});

test("`#272 voice-agent-instructions`: a typed call asks for text-only AND the owner's prompt, on one initiation", async () => {
  const page = newPage();
  page.el("api-token").value = "write-token-aaaaaaaaaaaaaaaa";
  await page.el("save-token").click();
  await page.settle();
  page.setFetch(mintWithPrompt(OVERRIDE));
  await compose(page, "hello");
  await page.el("send-text").click();
  await page.settle();
  assert.equal(page.sockets.length, 1, "the typed conversation did not open");
  const socket = page.sockets[0];
  socket.onopen();
  vendorAccepts(socket);
  await page.settle();
  assert.deepStrictEqual(initiationFrame(socket).conversation_config_override, {
    conversation: { text_only: true },
    agent: { prompt: { prompt: OVERRIDE.text } },
  });
});

test("`#272 voice-agent-instructions`: with resuming on, the standing instructions go before the record of the earlier call", async () => {
  const page = newPage();
  page.storedTurns.set("conv_earlier", [
    { speaker: "you", text: "what happened to the arm64 job", at_ms: 1 },
    { speaker: "agent", text: "the mac runner stalled mid-deploy", at_ms: 2 },
  ]);
  await signIn(page);
  await page.settle();
  await page.el("resume-toggle").setChecked(true);
  page.setFetch(mintWithPrompt(NOTE));
  await page.el("talk").click();
  assert.equal(page.sockets.length, 1, "the call did not open");
  page.sockets[0].onopen();
  const updates = framesSent(page.sockets[0]).filter((frame) => frame.type === "contextual_update");
  assert.equal(updates.length, 2, "the instructions or the record did not go");
  assert.equal(updates[0].text, NOTE.text, "the record went before the standing instructions");
  assert.ok(String(updates[1].text).includes(REPLAY_PREAMBLE), "the record of the earlier call did not follow");
});

// --- `#272 voice-agent-instructions`: what the send switch reaches, and the review's other findings ---

const HOSTED_UNKNOWN =
  "The server changes at once. The agent's own instructions follow at its next call only if its provider lets this page replace its prompt; if not, paste the new prompt into the provider's console by hand. Change post_reply's approval there to match, either way.";
const HOSTED_KEPT =
  "The server changes at once, but the last call's agent does not let this page replace its prompt: paste the new prompt into its provider's console by hand, and change post_reply's approval there to match.";
const HOSTED_REPLACED =
  "The server changes at once, and the last call's agent was given this server's prompt, so its instructions follow from its next call. Change post_reply's approval in its provider's console to match.";

test("`#272 voice-agent-instructions`: beside the send switch, a hosted agent's warning that a prompt pasted by hand must be pasted again", async () => {
  // In the markup, inside the switch's editor, served with the wording for not knowing.
  const sending = settingsGroup("Sending");
  const opens = sending.indexOf('<div id="voice-send-editor"');
  assert.ok(opens > 0, "the switch has no editor");
  const editor = sending.slice(opens, sending.indexOf("</div>", opens));
  assert.ok(editor.includes(`<p id="voice-send-hosted" class="hint" hidden>${HOSTED_UNKNOWN}</p>`),
    "the warning is not beside the switch");

  // A hosted agent nobody has called yet: shown, conditional.
  const page = newPage();
  await signIn(page);
  assert.equal(page.el("voice-send-hosted").hidden, false, "a hosted agent's switch says nothing about its console prompt");
  assert.equal(page.el("voice-send-hosted").textContent, HOSTED_UNKNOWN);

  // An older server that does not say how its prompt is delivered: shown, as the page always did.
  const older = newPage();
  older.voicePrompt = undefined;
  await signIn(older);
  assert.equal(older.el("voice-send-hosted").hidden, false);
  assert.equal(older.el("voice-send-hosted").textContent, HOSTED_UNKNOWN);

  // A bridge fetches the prompt as each call opens: nothing to paste, and no warning.
  const bridge = newPage();
  bridge.voicePrompt = voicePromptView({ delivery: "profile" });
  await signIn(bridge);
  assert.equal(bridge.el("voice-send-hosted").hidden, true, "a bridge was told to paste a prompt");
  await bridge.el("voice-ask-before-send").setChecked(false);
  await bridge.settle();
  assert.match(bridge.el("status").textContent, /server now posts as soon as the voice agent sends; its instructions change from its next call\.$/);
});

test("`#272 voice-agent-instructions`: after a call, the warning and the saved line say whether that agent's prompt follows the switch", async () => {
  // An agent that does not let its prompt be replaced keeps the console's: the owner must paste.
  const kept = newPage();
  kept.voicePrompt = voicePromptView({ instructions: "Call me Sam." });
  await talkWithPrompt(kept, NOTE);
  assert.equal(kept.el("voice-send-hosted").textContent, HOSTED_KEPT);
  await kept.el("hang-up").click();
  await kept.settle();
  await kept.el("voice-ask-before-send").setChecked(false);
  await kept.settle();
  assert.match(kept.el("status").textContent,
    /server now posts as soon as the voice agent sends; this agent keeps the prompt pasted into its provider's console, so paste the new one there by hand\.$/,
    "the save claimed the agent's instructions change when nothing hands them over");

  // An agent that took this server's prompt follows the switch; only the vendor's approval does not.
  const replaced = newPage();
  await talkWithPrompt(replaced, { ...OVERRIDE, system_prompt_applied: false, instructions_applied: false });
  assert.equal(replaced.el("voice-send-hosted").textContent, HOSTED_REPLACED);
  await replaced.el("hang-up").click();
  await replaced.settle();
  await replaced.el("voice-ask-before-send").setChecked(false);
  await replaced.settle();
  assert.match(replaced.el("status").textContent,
    /its instructions follow from its next call; change post_reply's approval in its provider's console to match\.$/);
});

test("`#272 voice-agent-instructions`: the hint about unticking shows only while the box is ticked", async () => {
  const ticked = newPage();
  await signIn(ticked);
  assert.equal(ticked.el("voice-send-untick").hidden, false);
  await ticked.el("voice-ask-before-send").setChecked(false);
  await ticked.settle();
  assert.equal(ticked.el("voice-send-untick").hidden, true, "an unticked box still says to untick it");

  const unticked = newPage();
  unticked.voiceSettings = { ask_before_send: false, changeable: true };
  await signIn(unticked);
  assert.equal(unticked.el("voice-send-untick").hidden, true);
});

test("`#272 voice-agent-instructions`: a call given the shipped prompt in place of the console's says so under Instructions", async () => {
  // Only instructions saved, and an agent that allows the override: the shipped base replaced the
  // prompt pasted into its console, which the owner did not write and may not expect.
  const instructions = newPage();
  instructions.voicePrompt = voicePromptView({ instructions: "Call me Sam." });
  await talkWithPrompt(instructions, { ...OVERRIDE, system_prompt_applied: false, instructions_applied: true });
  assert.match(instructions.el("voice-prompt-state").textContent,
    /The last call was given the shipped prompt with your instructions, in place of the prompt in its provider's console\.$/);

  // Nothing saved: the shipped prompt went all the same, carrying the send rules for the switch.
  const nothing = newPage();
  await talkWithPrompt(nothing, { ...OVERRIDE, system_prompt_applied: false, instructions_applied: false });
  assert.match(nothing.el("voice-prompt-state").textContent,
    /The last call was given the shipped prompt, in place of the prompt in its provider's console\.$/);
});

test("`#272 voice-agent-instructions`: after Restore, the status says whether the shipped prompt reaches the agent, as a saved copy's does", async () => {
  // Restore is a save of the system prompt like any other. A hosted agent that keeps its console
  // prompt does not get the shipped one from its next call any more than it got the owner's copy,
  // and the status used to say it did.
  const restoreAfter = async (agentPrompt) => {
    const page = newPage();
    page.voicePrompt = voicePromptView({ system_prompt: "mine", system_prompt_edited: true, edited_from_version: 9 });
    if (agentPrompt !== null) {
      await talkWithPrompt(page, agentPrompt);
      await page.el("hang-up").click();
      await page.settle();
    } else {
      await signIn(page);
    }
    await page.el("open-settings").click();
    await page.el("restore-voice-system-prompt").click();
    await page.el("restore-voice-system-prompt").click();
    await page.settle();
    assert.deepStrictEqual(page.voicePromptCalls, [{ system_prompt: "" }]);
    return page.el("status").textContent;
  };

  const kept = await restoreAfter({
    transport: "not_sent", system_prompt_applied: false, instructions_applied: false, override_allowed: false,
  });
  assert.equal(kept,
    "Saved, but the last call's agent does not let this page replace its prompt, so the shipped prompt will not reach it; it keeps the one in its provider's console.",
    "Restore claimed the shipped prompt reaches an agent that keeps its console prompt");

  assert.equal(await restoreAfter(null),
    "Saved. The voice agent gets the shipped prompt from its next call if its provider lets this page replace its prompt.",
    "with no call yet, Restore spoke as if the prompt always follows");

  assert.equal(await restoreAfter({ ...OVERRIDE, system_prompt_applied: true, instructions_applied: false }),
    "Saved. The voice agent gets the shipped prompt from its next call.");
});

test("`#272 voice-agent-instructions`: a call that sent nothing is described from what was saved when it opened, not from a save after it", async () => {
  // The line under Instructions says why the last call's agent got nothing: an edited prompt it
  // would not take, or nothing of the owner's at all. It used to read that from the settings as
  // they are now, so a save after the call rewrote what the call had been given.
  const NOT_SENT = {
    transport: "not_sent", system_prompt_applied: false, instructions_applied: false, override_allowed: false,
  };
  const afterCall = async (fields) => {
    const page = newPage();
    page.voicePrompt = voicePromptView(fields);
    await talkWithPrompt(page, NOT_SENT);
    await page.el("hang-up").click();
    await page.settle();
    await page.el("open-settings").click();
    return page;
  };

  // His copy was saved when the call opened, and Restore came after: it was his edit that did not
  // reach the agent.
  const edited = await afterCall({ system_prompt: "mine", system_prompt_edited: true, edited_from_version: 9 });
  await edited.el("restore-voice-system-prompt").click();
  await edited.el("restore-voice-system-prompt").click();
  await edited.settle();
  assert.deepStrictEqual(edited.voicePromptCalls, [{ system_prompt: "" }]);
  assert.match(edited.el("voice-prompt-state").textContent, /so your edited prompt did not reach it\.$/,
    "a Restore after the call said the call had nothing of his");

  // Nothing was saved when the call opened, and his copy came after: no edit was refused.
  const shipped = await afterCall({});
  await shipped.el("voice-system-prompt").setValue("You read me the chat while I drive.");
  await shipped.el("save-voice-system-prompt").click();
  await shipped.settle();
  assert.deepStrictEqual(shipped.voicePromptCalls, [{ system_prompt: "You read me the chat while I drive." }]);
  assert.match(shipped.el("voice-prompt-state").textContent,
    /The last call's agent does not let its prompt be replaced, so it kept the prompt in its provider's console\.$/);
  assert.doesNotMatch(shipped.el("voice-prompt-state").textContent, /edited prompt/,
    "a copy saved after the call was said to have been refused by it");
});

test("`#272 voice-agent-instructions`: Restore asks twice before it deletes the owner's copy, and a lapse disarms it", async () => {
  const page = await instructionsPage({ system_prompt: "mine, and long", system_prompt_edited: true, edited_from_version: 9 });
  const restore = page.el("restore-voice-system-prompt");
  await restore.click();
  await page.settle();
  assert.deepStrictEqual(page.voicePromptCalls, [], "one tap deleted the owner's prompt");
  assert.equal(restore.textContent, "Delete your copy?");
  assert.equal(restore.className, "secondary armed");
  assert.match(page.el("status").textContent, /Tap again to delete your copy/);
  assert.match(cssBlock("#restore-voice-system-prompt.armed"), /color:\s*var\(--warn\)/, "armed looks like disarmed");

  // Left alone, it disarms, and the next tap only arms it again.
  assert.equal(page.expireTimers(CLEAR_ARMED_MS), 1, "nothing was scheduled to disarm it");
  assert.equal(restore.textContent, "Restore the shipped prompt");
  assert.equal(restore.className, "secondary");
  await restore.click();
  await page.settle();
  assert.deepStrictEqual(page.voicePromptCalls, [], "a tap after the lapse acted at once");

  await restore.click();
  await page.settle();
  assert.deepStrictEqual(page.voicePromptCalls, [{ system_prompt: "" }]);
  assert.equal(page.el("voice-system-prompt").value, SHIPPED_SYSTEM_PROMPT);
  assert.equal(restore.textContent, "Restore the shipped prompt");
  assert.equal(page.expireTimers(CLEAR_ARMED_MS), 0, "the arming outlived the restore");
});

test("`#272 voice-agent-instructions`: Settings is drawn only after a call's opening frames are sent, and cannot stop the call", async () => {
  const page = newPage();
  page.voicePrompt = voicePromptView({ instructions: "Call me Sam." });
  page.el("api-token").value = "write-token-aaaaaaaaaaaaaaaa";
  await page.el("save-token").click();
  await page.settle();
  page.setFetch(mintWithPrompt(NOTE));
  await page.el("talk").click();
  assert.equal(page.sockets.length, 1, "start() opened no websocket");
  const socket = page.sockets[0];
  // Settings' line breaks the moment it is drawn, and records how much had been sent by then.
  let sentWhenDrawn = null;
  Object.defineProperty(page.el("voice-prompt-state"), "textContent", {
    configurable: true,
    get: () => "",
    set: () => {
      sentWhenDrawn = socket.sent.length;
      throw new Error("a settings screen that cannot draw");
    },
  });
  socket.onopen();
  assert.equal(state(page), "live", "a Settings line that could not draw stopped the call");
  assert.equal(page.processors.length, 1, "capture never started");
  assert.deepStrictEqual(framesSent(socket).slice(0, 2).map((f) => f.type),
    ["conversation_initiation_client_data", "contextual_update"]);
  assert.equal(sentWhenDrawn, 2, "Settings was drawn before the opening frames were sent");
});

test("`#272 voice-agent-instructions`: a resumed call whose prompt is replaced still gets the record, when resuming rides on the initiation", async () => {
  const page = newPage();
  page.replayTransport = "client_data";
  await withEarlierConversation(page, { resume: true });
  page.setFetch(mintWithPrompt(OVERRIDE));
  await page.el("talk").click();
  assert.equal(page.sockets.length, 1, "the call did not open");
  const socket = page.sockets[0];
  socket.onopen();
  const initiation = initiationFrame(socket);
  assert.equal(initiation.conversation_config_override.agent.prompt.prompt, OVERRIDE.text);
  // Kept on the initiation, so nothing configured at the vendor goes missing...
  assert.ok(String(initiation.dynamic_variables.vibe_talk_resume).includes("the mac runner stalled"));
  // ...but the console prompt that had the placeholder was replaced, so the record goes as a note.
  const record = resumeFrame(socket);
  assert.ok(record, "the record of the earlier call reached nothing: its placeholder went with the replaced prompt");
  assert.match(record.text, /the mac runner stalled mid-deploy/);

  // An agent that keeps its prompt keeps its placeholder: the record rides on the initiation alone.
  const kept = newPage();
  kept.replayTransport = "client_data";
  await withEarlierConversation(kept, { resume: true });
  kept.setFetch(mintWithPrompt(NOTE));
  await kept.el("talk").click();
  kept.sockets[0].onopen();
  assert.equal(resumeFrame(kept.sockets[0]), undefined, "the record went twice to an agent that keeps its prompt");
});

// --- `#303 voice-post-prefix` ---------------------------------------------------------------------
//
// Every message the voice agent posts starts with the owner's tag, which the SERVER adds. The page's
// part is the Settings field that keeps it, and showing and confirming exactly what will be posted:
// the card and the read-back carry the tag, and the tag — his words, not the model's — does not make
// a draft tap-only.

/**
 * Sign in with the tag as the server reports it, and open Settings, letting the read that opening it
 * sends answer.
 */
async function postPrefixPage(fields = {}, arrange = (_page) => {}) {
  const page = newPage();
  page.voicePostPrefix = voicePostPrefixView(fields);
  arrange(page);
  await signIn(page);
  await page.el("open-settings").click();
  await page.settle();
  return page;
}

/** What the page wrote to the prefix route, leaving out its reads. */
const postPrefixWrites = (page) => page.voicePostPrefixCalls.filter((call) => call.method !== "GET");

test("`#303 voice-post-prefix`: the Sending group has a labelled box, a help line, Save and Reset to default", () => {
  const sending = settingsGroup("Sending");
  const opens = sending.indexOf('<div id="voice-post-prefix-editor" hidden>');
  assert.ok(opens > 0, "the prefix field is not in the Sending group, hidden until the server reports it");
  const editor = sending.slice(opens);
  assert.match(editor, /<label for="voice-post-prefix">Prefix on messages the agent posts<\/label>/);
  assert.equal(PAGE_ELEMENTS.get("voice-post-prefix").tag, "input");
  assert.match(editor, /<p id="voice-post-prefix-help" class="hint">Put in front of every message the voice agent posts, and in its read-back, but never on what you type\. Empty turns it off\.<\/p>/);
  assert.match(editor, /<button id="save-voice-post-prefix" class="secondary" type="button" disabled>Save<\/button>/);
  assert.match(editor, /<button id="reset-voice-post-prefix" class="secondary" type="button">Reset to default<\/button>/);
  assert.ok(sending.indexOf('id="voice-ask-before-send"') < opens, "the prefix comes before the switch about asking");
  const help = helpEntry("voice-send");
  assert.match(help, /Prefix on messages the agent posts/);
  assert.match(help, /Messages you type on this\s+page never get it/);
  assert.match(help, /save\s+it empty to turn it off/);
});

test("`#303 voice-post-prefix`: the box shows the tag the server holds, and says it is the default", async () => {
  const page = await postPrefixPage();
  assert.equal(page.el("voice-post-prefix-editor").hidden, false);
  assert.equal(page.el("voice-post-prefix").value, DEFAULT_POST_PREFIX, "the trailing space was lost");
  assert.equal(page.el("voice-post-prefix-state").textContent, "The default.");
  assert.equal(page.el("save-voice-post-prefix").disabled, true, "Save is live with nothing to save");
  assert.equal(page.el("reset-voice-post-prefix").disabled, true, "Reset is live on the default");

  const own = await postPrefixPage({ prefix: "[voice] ", custom: true });
  assert.equal(own.el("voice-post-prefix").value, "[voice] ");
  assert.equal(own.el("voice-post-prefix-state").textContent, "Saved. Every message the agent posts starts with it.");
  assert.equal(own.el("reset-voice-post-prefix").disabled, false);

  const off = await postPrefixPage({ prefix: "", custom: true });
  assert.equal(off.el("voice-post-prefix").value, "");
  assert.equal(off.el("voice-post-prefix-state").textContent, "Off: the agent's messages carry no prefix.");
});

test("`#303 voice-post-prefix`: Save sends the box as typed and draws what the server stored", async () => {
  const page = await postPrefixPage();
  await page.el("voice-post-prefix").setValue("  [voice] ");
  assert.equal(page.el("voice-post-prefix-state").textContent, "Not saved yet.");
  assert.equal(page.el("save-voice-post-prefix").disabled, false);
  assert.equal(page.el("reset-voice-post-prefix").disabled, false, "an edit cannot be put back");
  await page.el("save-voice-post-prefix").click();
  await page.settle();
  assert.deepStrictEqual(postPrefixWrites(page), [{ method: "PUT", body: { prefix: "  [voice] " } }]);
  assert.equal(page.el("voice-post-prefix").value, "[voice] ", "the box kept the click, not what was stored");
  assert.equal(page.el("voice-post-prefix-state").textContent, "Saved. Every message the agent posts starts with it.");
  assert.equal(page.el("save-voice-post-prefix").disabled, true);
  assert.match(page.el("status").textContent, /Every message the voice agent posts starts with this prefix/);
});

test("`#303 voice-post-prefix`: an empty box is saved as off, and Reset to default brings the default back", async () => {
  const page = await postPrefixPage();
  await page.el("voice-post-prefix").setValue("");
  await page.el("save-voice-post-prefix").click();
  await page.settle();
  assert.deepStrictEqual(postPrefixWrites(page), [{ method: "PUT", body: { prefix: "" } }]);
  assert.equal(page.el("voice-post-prefix-state").textContent, "Off: the agent's messages carry no prefix.");
  assert.match(page.el("status").textContent, /carry no prefix/);
  // Off is a choice, so Reset has something to forget.
  assert.equal(page.el("reset-voice-post-prefix").disabled, false);
  await page.el("reset-voice-post-prefix").click();
  await page.settle();
  assert.deepStrictEqual(postPrefixWrites(page)[1], { method: "DELETE", body: null });
  assert.equal(page.el("voice-post-prefix").value, DEFAULT_POST_PREFIX);
  assert.equal(page.el("voice-post-prefix-state").textContent, "The default.");
  assert.equal(page.el("reset-voice-post-prefix").disabled, true);
  assert.match(page.el("status").textContent, /default prefix again/);
});

test("`#303 voice-post-prefix`: Reset with nothing saved puts the default back in the box and writes nothing", async () => {
  const page = await postPrefixPage();
  await page.el("voice-post-prefix").setValue("half a tag");
  await page.el("reset-voice-post-prefix").click();
  await page.settle();
  assert.deepStrictEqual(postPrefixWrites(page), [], "putting back an unsaved edit wrote to the server");
  assert.equal(page.el("voice-post-prefix").value, DEFAULT_POST_PREFIX);
  assert.equal(page.el("voice-post-prefix-state").textContent, "The default.");
});

test("`#303 voice-post-prefix`: a tag over the limit cannot be saved, and a refused save keeps what was typed", async () => {
  const page = await postPrefixPage({ max_chars: 10 });
  await page.el("voice-post-prefix").setValue("x".repeat(11));
  assert.equal(page.el("save-voice-post-prefix").disabled, true);
  assert.equal(page.el("voice-post-prefix-state").textContent, "11 characters; at most 10 are kept. Shorten it to save.");
  // Counted as the server counts, by character, after the space in front is gone.
  await page.el("voice-post-prefix").setValue(`  ${"🎙".repeat(10)}`);
  assert.equal(page.el("save-voice-post-prefix").disabled, false);

  const refused = await postPrefixPage({}, (p) => { p.voicePostPrefixStatus = 503; });
  await refused.el("voice-post-prefix").setValue("[voice] ");
  await refused.el("save-voice-post-prefix").click();
  await refused.settle();
  assert.equal(refused.el("voice-post-prefix").value, "[voice] ", "a refused save lost the text");
  assert.equal(refused.el("voice-post-prefix-state").textContent, "Not saved yet.");
  assert.equal(refused.el("save-voice-post-prefix").disabled, false, "the text cannot be tried again");
  assert.match(refused.el("error").textContent, /The prefix was not saved/);
});

test("`#303 voice-post-prefix`: a read token and a store-less server show the tag but cannot change it; an older server hides it", async () => {
  const read = await postPrefixPage({ prefix: "[voice] ", custom: true }, (p) => { p.tokenScope = "read"; });
  assert.equal(read.el("voice-post-prefix").value, "[voice] ");
  assert.equal(read.el("voice-post-prefix").disabled, true);
  assert.equal(read.el("save-voice-post-prefix").disabled, true);
  assert.equal(read.el("reset-voice-post-prefix").disabled, true);
  assert.match(read.el("voice-post-prefix-state").textContent, /^Only the write token can change this\. Saved\./);

  const storeless = await postPrefixPage({ changeable: false });
  assert.equal(storeless.el("voice-post-prefix").disabled, true);
  assert.match(storeless.el("voice-post-prefix-state").textContent, /keeps no settings/);

  const older = await postPrefixPage({}, (p) => { p.voicePostPrefix = undefined; });
  assert.equal(older.el("voice-post-prefix-editor").hidden, true);
  assert.deepStrictEqual(older.voicePostPrefixCalls, [], "a server that reports no tag was asked for one");
});

test("`#303 voice-post-prefix`: opening Settings reads the tag again, so a second device shows what another saved and Reset forgets it", async () => {
  // Signed in while the default was in force; another device then saves its own tag.
  const page = newPage();
  await signIn(page);
  assert.equal(page.el("voice-post-prefix").value, DEFAULT_POST_PREFIX, "fixture: sign-in read the default");
  page.voicePostPrefix = voicePostPrefixView({ prefix: "[phone] ", custom: true });

  await page.el("open-settings").click();
  await page.settle();
  assert.deepStrictEqual(page.voicePostPrefixCalls, [{ method: "GET", body: null }], "opening Settings did not read the tag again");
  assert.equal(page.el("voice-post-prefix").value, "[phone] ", "the box shows the tag read at sign-in");
  assert.equal(page.el("voice-post-prefix-state").textContent, "Saved. Every message the agent posts starts with it.");
  assert.equal(page.el("reset-voice-post-prefix").disabled, false, "Reset is held as though the default were in force");

  await page.el("reset-voice-post-prefix").click();
  await page.settle();
  assert.deepStrictEqual(postPrefixWrites(page), [{ method: "DELETE", body: null }], "Reset took the default's path and forgot nothing at the server");
  assert.equal(page.voicePostPrefix.custom, false, "the other device's tag is still in force");
  assert.equal(page.el("voice-post-prefix").value, DEFAULT_POST_PREFIX);
  assert.equal(page.el("voice-post-prefix-state").textContent, "The default.");
});

test("`#303 voice-post-prefix`: negative control: Settings left open reads nothing more, and shows the change only when opened again", async () => {
  const page = await postPrefixPage();
  const reads = page.voicePostPrefixCalls.length;
  assert.equal(reads, 1, "fixture: opening Settings read the tag once");
  page.voicePostPrefix = voicePostPrefixView({ prefix: "[phone] ", custom: true });
  await page.settle();
  // Nothing but opening Settings reads it, so without that read this page would stay stale.
  assert.equal(page.voicePostPrefixCalls.length, reads, "something other than opening Settings read the tag");
  assert.equal(page.el("voice-post-prefix").value, DEFAULT_POST_PREFIX);
  assert.equal(page.el("reset-voice-post-prefix").disabled, true);

  await page.el("close-settings").click();
  await page.el("open-settings").click();
  await page.settle();
  assert.equal(page.voicePostPrefixCalls.length, reads + 1, "opening Settings again did not read the tag");
  assert.equal(page.el("voice-post-prefix").value, "[phone] ");
  assert.equal(page.el("reset-voice-post-prefix").disabled, false);
});

test("`#303 voice-post-prefix`: a tag typed and not saved stays in the box when Settings reads the tag again", async () => {
  const page = await postPrefixPage();
  await page.el("voice-post-prefix").setValue("half a tag");
  await page.el("close-settings").click();
  page.voicePostPrefix = voicePostPrefixView({ prefix: "[phone] ", custom: true });
  await page.el("open-settings").click();
  await page.settle();
  assert.equal(page.el("voice-post-prefix").value, "half a tag", "the read threw away what was typed");
  assert.equal(page.el("voice-post-prefix-state").textContent, "Not saved yet.");
  // Measured against what the server now holds: Reset forgets the other device's tag.
  await page.el("reset-voice-post-prefix").click();
  await page.settle();
  assert.deepStrictEqual(postPrefixWrites(page), [{ method: "DELETE", body: null }]);
  assert.equal(page.el("voice-post-prefix").value, DEFAULT_POST_PREFIX);
});

/** A draft as the server holds one the voice agent proposed, with the owner's tag in front. */
const taggedDraft = (words = "on it, rebasing now", overrides = {}) =>
  spokenDraft({ text: `${DEFAULT_POST_PREFIX}${words}`, prefix: DEFAULT_POST_PREFIX, ...overrides });

test("`#303 voice-post-prefix`: the card shows the tagged text exactly as it will be posted, and Send restates it", async () => {
  const page = newPage();
  const pending = await proposedDuringACall(page, proposal({
    text: `${DEFAULT_POST_PREFIX}on my way`, prefix: DEFAULT_POST_PREFIX,
  }));
  assert.equal(page.el("post-confirm-text").textContent, `${DEFAULT_POST_PREFIX}on my way`);
  await page.el("post-confirm-send").click();
  await page.settle();
  assert.deepEqual(page.proposalCommits, [{
    handle: pending.handle,
    channel_id: pending.channel_id,
    text: `${DEFAULT_POST_PREFIX}on my way`,
    reply_to: pending.reply_to,
    confirmed_by: "ui",
  }], "Send did not restate the text as posted, tag and all");
});

test("`#303 voice-post-prefix`: the agent's words are judged for a spoken yes; the owner's tag in front of them is not", () => {
  const draft = newPage().pure("agentDraft");
  assert.equal(draft({ text: `${DEFAULT_POST_PREFIX}on it`, prefix: DEFAULT_POST_PREFIX }), "on it");
  // Tagging off, or an older server: the whole text is the agent's.
  assert.equal(draft({ text: "(aside) on it" }), "(aside) on it");
  assert.equal(draft({ text: "(aside) on it", prefix: "" }), "(aside) on it");
  // A prefix the text does not start with names nothing.
  assert.equal(draft({ text: "on it", prefix: DEFAULT_POST_PREFIX }), "on it");
  const unsayable = newPage().pure("unsayableDraft");
  assert.equal(unsayable(DEFAULT_POST_PREFIX), "symbols", "fixture: the default tag is not plain words");
});

test("`#303 voice-post-prefix`: a yes after the exact read-back, tag included, sends the tagged draft once", async () => {
  const page = newPage();
  const pending = taggedDraft();
  // As a voice says it: the parentheses are silent, and the words match the card's.
  const socket = await readBackDuringACall(page, {
    pending, said: "To lead team: Voice-agent relayed, from user: on it, rebasing now. Send it?",
  });
  hostedHears(socket, "Yes.");
  await settleSpokenYes(page);
  assert.deepEqual(page.proposalCommits, [{
    handle: pending.handle,
    channel_id: pending.channel_id,
    text: pending.text,
    reply_to: null,
    confirmed_by: "speaker_turn",
  }]);
  assert.match(pageNotices(socket)[0], /^Voice page: the message you read back was SENT/);
});

test("`#303 voice-post-prefix`: a read-back that leaves the tag out, or a draft whose own words a voice cannot say, sends nothing", async () => {
  // The tag is posted, so it must be read back: what is heard is what goes.
  const dropped = newPage();
  const socket = await readBackDuringACall(dropped, {
    pending: taggedDraft(), said: "To lead team: on it, rebasing now. Send it?",
  });
  hostedHears(socket, "yes");
  await settleSpokenYes(dropped);
  assert.deepEqual(dropped.proposalCommits, []);
  assert.match(pageNotices(socket)[0], /not the card's text word for word/);

  // The agent's words after the tag are judged as ever.
  const struck = newPage();
  const strikeSocket = await readBackDuringACall(struck, {
    pending: taggedDraft("~~don't~~ merge the PR"),
    said: `To lead team: ${DEFAULT_POST_PREFIX}don't merge the PR. Send it?`,
  });
  hostedHears(strikeSocket, "yes");
  await settleSpokenYes(struck);
  assert.deepEqual(struck.proposalCommits, []);
  assert.match(pageNotices(strikeSocket)[0], /a link, a symbol or formatting/);

  // The same parentheses written by the agent, with no tag named by the server, are the agent's.
  const unnamed = newPage();
  const unnamedSocket = await readBackDuringACall(unnamed, {
    pending: spokenDraft({ text: `${DEFAULT_POST_PREFIX}on it, rebasing now` }),
  });
  hostedHears(unnamedSocket, "yes");
  await settleSpokenYes(unnamed);
  assert.deepEqual(unnamed.proposalCommits, []);
  assert.match(pageNotices(unnamedSocket)[0], /a link, a symbol or formatting/);
});

// --- `#303 voice-post-prefix` with `#269 approval-card-inline` and `#270 voice-transcript-markers` --
//
// The server puts the owner's tag in front of the voice agent's words before it proposes or posts
// them, so the tagged text is the one text there is: the card shows it, the read-back says it, Send
// and a spoken yes restate it, and the frozen card the send leaves shows it, drawn from the server's
// own copy of what went. Nothing on the page takes the tag off: `agentDraft` is for judging the
// agent's words for a spoken yes, and never for what is put on screen.

test("`#303 voice-post-prefix`: a tagged draft freezes into a card showing what was posted, tag and all, by tap, by voice and sent without asking", async () => {
  for (const how of ["tap", "voice"]) {
    const page = newPage();
    const pending = taggedDraft("on it, rebasing now", { serial: 7 });
    const socket = await recordedReadBack(page, `conv_tagged_${how}`, pending);
    assert.equal(page.el("post-confirm-text").textContent, pending.text, `${how}: the card`);
    if (how === "tap") {
      page.expireTimers(2000);
      await page.el("post-confirm-send").click();
    } else {
      hostedHears(socket, "yes please");
      await settleSpokenYes(page);
    }
    await watchTurns(page);
    assert.deepEqual(page.proposalCommits.map((commit) => [commit.text, commit.confirmed_by]),
      [[pending.text, how === "tap" ? "ui" : "speaker_turn"]], `${how}: the commit`);
    const cards = sentCards(page);
    assert.equal(cards.length, 1, `${how}: the send left no frozen card, or two`);
    assert.match(wordsOf(cards[0]), how === "tap" ? /approved by tap/ : /approved by voice/);
    const body = cards[0].children.find((kid) => kid.className === "body");
    assert.equal(body && body.textContent, pending.text, `${how}: the frozen card does not show what was posted`);
    assert.ok(body.textContent.startsWith(DEFAULT_POST_PREFIX), `${how}: the frozen card lost the tag`);
  }

  // A direct send is never proposed: the frozen card is the server's copy alone, tag and all.
  const page = newPage();
  page.setClock(1_700_000_100_000);
  await recordingCall(page);
  await watchTurns(page);
  const direct = taggedDraft("rebase onto main", { serial: 21, handle: "dIrEcT" });
  page.publishSend(direct, "direct");
  await watchTurns(page);
  const card = sentCards(page)[0];
  assert.ok(card, "the direct send left no frozen card");
  assert.match(wordsOf(card), /sent without asking/);
  assert.equal(card.children.find((kid) => kid.className === "body").textContent, `${DEFAULT_POST_PREFIX}rebase onto main`);
});
