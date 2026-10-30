// The app page's tests: how a message is drawn: bodies, folds and elisions, summaries, days, edits and reactions.
//
// One feature's file of the suite tests/js/voice_page.test.mjs runs, against the fixture in
// ./fixture.mjs. Run with the rest of the suite, or alone: node --test tests/js/page/rows.mjs.

import assert from "node:assert/strict";

import { ACKNOWLEDGED, BASE_CSS, CHANNEL, CODER, COLLAPSE_OVER_CHARS, CSS, CSS_CODE,
  CSS_UNCONDITIONAL, DISCORD_POLL_MS, FIXTURE_TREE, FakeElement, GATHERED, GLYPHS, HOVER_QUERY,
  HTML, INBOX, LINK_ATTRS, OWNERS_MESSAGE, PAGE_CODE, PAGE_ELEMENTS, PULL_ARM_PX, SANITIZED_TAGS,
  SCRIPT, SCRIPT_CODE, SCROLLERS, SHORT_MESSAGE, TIMELINE_RECONCILE_MS, TWO_COLUMN_QUERY, asksFor,
  assertMarkupContains, assistantSays, atBottomOf, bodyOf, bookmarkAt, bookmarkRow, boxesOf,
  brokenModule, brokenScript, builtSince, cardPart, cardRow, channelSummaryText, chipsOf,
  chooseReadMode, cssBlock, cssBlockIn, cssBlockOutside, cssRules, deliver, doneButton, elidedOf,
  errorResponse, excerptOf, focused, foldButton, forwardPage, forwardReadKinds, forwardTimeline,
  freshness, gate, gatherChip, headOf, headerCard, hitsIn, hold, holdCountsRead, inReadingMode,
  inboxChannel, inboxId, inboxOnMain, inlineAnchors, joined, json, keyboardPage, lineText,
  linkPairs, longMessage, marksPending, mdBoxes, mediaBody, message, metaOf, newPage,
  nodeWithinFixture, openerOf, parseSanitizedHtml, passing, pcm, pickThread, pinnedIds, pollOnce,
  press, pullDown, reReadChannel, reactionStripOf, readButton, readRunLineOf, refreshDiscord,
  reloadPage, reloadWith, reopenedChannel, replyButton, replyInStream, row, rowMoreButton, rowOf,
  rowState, rowWithId, savedScopes, scaleOf, scatteredThread, scopeKey, seamDetail, seamLabel,
  search, selectedId, settingsGroup, showDiscord, showLinks, showPinned, shownIds, signIn,
  sourceConstant, splitPost, sseMessage, startTalking, statusText, storedPin, summaryText, tagged,
  tallChannel, tapArrow, tapKind, test, textOfRow, threadBadge, threadData, tokenValues,
  turnSummariesOn, twoColumnServer, twoColumns, typeAndPause, typingPaused, unreadSaid, wavHeader,
  withLiveChannel } from "./fixture.mjs";

// --- clearing the view ----------------------------------------------------------------------------

test("Clear sits in the control pane, not in the header beside the record it erases", () => {
  // In the header it stood next to a notice calling the transcript the only surviving record,
  // which invited exactly the reading it must not have: that the button destroys something
  // irreplaceable. In the pane it is honestly grouped with the other things you DO.
  const pane = HTML.slice(HTML.indexOf('id="control-pane"'));
  assert.ok(pane.includes('id="clear-view"'), "Clear is not in the control pane");
  const header = HTML.slice(HTML.indexOf('id="topbar"'), HTML.indexOf("</header>"));
  assert.doesNotMatch(header, /id="clear-view"/, "Clear is still in the header");
  assert.equal(PAGE_ELEMENTS.get("clear-view-label").text, "Clear");
});

test("CLEAR ASKS TWICE — one tap arms it, and only the second one erases anything", async () => {
  // It is destructive, it cannot be undone, and it now sits under a thumb, which makes an
  // accidental tap likelier rather than less likely. A label does not make that safe; a second tap
  // does.
  const page = newPage();
  await startTalking(page);
  page.sockets[0].onmessage({
    data: JSON.stringify({
      type: "agent_response",
      agent_response_event: { agent_response: "the secret is 12345" },
    }),
  });
  assert.equal(page.el("transcript").children.length, 1);

  await page.el("clear-view").click();

  assert.equal(page.el("transcript").children.length, 1, "the FIRST tap erased the transcript");
  assert.equal(page.el("clear-view-label").textContent, "Sure?", "the armed state must be visible");
  assert.match(page.el("clear-view").className, /\barmed\b/);
  assert.match(page.el("status").textContent, /again/i, "the status line must say what it wants");

  await page.el("clear-view").click();

  assert.equal(page.el("transcript").children.length, 1, "only the seam should remain");
  assert.equal(page.el("clear-view-label").textContent, "Clear", "it stayed armed after firing");
  assert.doesNotMatch(page.el("clear-view").className, /\barmed\b/);
});

test("an armed Clear disarms itself rather than lying in wait", async () => {
  // Armed forever would be worse than no confirmation at all: the second tap could arrive minutes
  // later, from a thumb aiming at something else entirely.
  const page = newPage();
  await startTalking(page);
  page.sockets[0].onmessage({
    data: JSON.stringify({ type: "agent_response", agent_response_event: { agent_response: "hi" } }),
  });

  await page.el("clear-view").click();
  assert.equal(page.el("clear-view-label").textContent, "Sure?");

  assert.equal(page.expireTimers(4000), 1, "nothing was scheduled to disarm it");
  assert.equal(page.el("clear-view-label").textContent, "Clear");
  assert.doesNotMatch(page.el("clear-view").className, /\barmed\b/);

  // And the next tap arms again rather than clearing.
  await page.el("clear-view").click();
  assert.equal(page.el("transcript").children.length, 1, "a lapsed arm still cleared");
});

test("leaving the main screen disarms Clear", async () => {
  const page = newPage();
  await signIn(page);

  await page.el("clear-view").click();
  await page.el("open-settings").click();
  await page.el("close-settings").click();

  assert.equal(page.el("clear-view-label").textContent, "Clear", "Clear came back still armed");
});

test("clearing during a call empties the screen and says the agent has not forgotten", async () => {
  // The ambiguous middle this must not be: the screen empties and the agent carries on with
  // everything the operator thought they had removed, with nothing on screen saying so.
  const page = newPage();
  await startTalking(page);
  page.sockets[0].onmessage({
    data: JSON.stringify({
      type: "agent_response",
      agent_response_event: { agent_response: "the secret is 12345" },
    }),
  });
  assert.match(page.el("transcript").children[0].text(), /the secret is 12345/);

  await page.el("clear-view").click();
  await page.el("clear-view").click();

  const remaining = page.el("transcript").children;
  assert.ok(
    !remaining.some((li) => li.text().includes("the secret is 12345")),
    "the view was not cleared"
  );
  assert.equal(remaining.length, 1, "exactly the seam should remain");
  assert.equal(remaining[0].className, "seam");
  // Two words on the surface; the sentences are inside it, exactly as at a conversation boundary.
  assert.equal(seamLabel(remaining[0]), "view cleared");
  const detail = remaining[0].descendants().find((node) => node.className === "seam-detail");
  assert.match(detail.textContent, /still has everything said before this point/);
  assert.match(page.el("status").textContent, /has not forgotten/);
  // A display action, so it must not have touched the call.
  assert.equal(page.tracks[0].stops, 0, "clearing the view hung up");
  assert.notEqual(page.sockets[0].readyState, 3);
});

test("THE VIEW-CLEARED SEAM SAYS ALL THREE OF THE THINGS IT IS THERE TO SAY", async () => {
  // `#74 scroll-test-strength`, finding 2. One clause of this disclosure was pinned and the other
  // two were not, so the sentences that distinguish "the screen was emptied" from "the call ended"
  // — the two the ambiguous middle turns on — could be deleted with every test still green.
  //
  // Three claims, and each is a separate thing the reader has to be told:
  //
  //   1. WHAT was emptied, and that nothing else was;
  //   2. that the AGENT still has everything said before this point;
  //   3. WHICH control would have ended the call, since this one did not.
  //
  // Pinned as three assertions rather than one string comparison: the wording is allowed to be
  // edited, and what may not go is any of the three.
  const page = newPage();
  await startTalking(page);
  page.sockets[0].onmessage({
    data: JSON.stringify({
      type: "agent_response",
      agent_response_event: { agent_response: "the secret is 12345" },
    }),
  });

  await page.el("clear-view").click();
  await page.el("clear-view").click();

  const line = page.el("transcript").children.find((li) => li.className === "seam");
  assert.ok(line, "clearing during a call drew no seam at all");
  const detail = seamDetail(line);
  assert.match(detail, /nothing else was/, "it no longer says what was NOT emptied");
  assert.match(detail, /still has everything said before this point/, "the agent's memory claim");
  assert.match(detail, /Hang up is what ends the call/, "it no longer says what WOULD end the call");
  // ...and the negative that keeps it from drifting into the OTHER seam's wording. These two are
  // the same idiom in the same list, and the end-of-call one says the opposite about memory: an
  // edit that copied it here would be the interface telling the reader the agent has forgotten
  // everything they just cleared.
  assert.doesNotMatch(
    detail,
    /has never seen anything above it/,
    "the view-cleared seam is claiming the agent's memory was emptied too"
  );
});

test("clearing while idle just clears, with no claim about an agent that is not there", async () => {
  const page = newPage();
  await signIn(page);
  page.el("transcript").append(new FakeElement("", "li"));

  await page.el("clear-view").click();
  await page.el("clear-view").click();

  assert.equal(page.el("transcript").children.length, 0);
  assert.equal(page.el("status").textContent, "Transcript cleared.");
});

// --- `#244 elision-word-count` -------------------------------------------------------------------
//
// A folded message says under its last line how much the fold keeps back. The fold is a CSS clamp,
// so the count is the page's own floor, worked out from the text at every width class a line can be
// in, one label per class; web/voice.css shows the one for the width the row is drawn at. These tests
// hold each label to that model EXACTLY, with texts whose cut is known — words of one letter repeated,
// so a line of a class holds a known number of them — and tests/elision_browser.py holds the labels a
// real Chromium shows to the words it really hides.

// `#425 collapsed-lines`. The lines a fold shows until the reader says otherwise: the count these
// labels are worked out at, every page here loading with nothing stored.
const FOLD_LINES = sourceConstant("DEFAULT_COLLAPSED_LINES");
const ELIDED_MIN_WORDS = sourceConstant("ELIDED_MIN_WORDS");
const FOLD_WIDTHS_EM = JSON.parse(
  /^const FOLD_WIDTHS_EM = (\[[\d.,\s]*\]);$/m.exec(SCRIPT_CODE)?.[1] ??
    assert.fail("web/voice.js no longer states its FOLD_WIDTHS_EM"));
/** The width class of a phone at the default text size, 393 to 412px: its widest line, in em. */
const PHONE_CLASS = FOLD_WIDTHS_EM.indexOf(28.5);
/** ...and of a desk's default 72ch column. */
const DESK_CLASS = FOLD_WIDTHS_EM.indexOf(45);
// GLYPHS, the glyph widths the fold model counts by, is loaded near PRELUDE, above every test.
/** `count` words of `letters` n's each, on one line: "n" is the model's ordinary lowercase letter. */
const wordsOfLength = (count, letters) => Array(count).fill("n".repeat(letters)).join(" ");
/** How many such words a line of `widthEm` holds, by the model's own glyph widths. */
const wordsPerLine = (page, letters, widthEm) => {
  const letter = page.pure('glyphEm("n")');
  const space = page.pure("SPACE_EM");
  return Math.floor((widthEm + space) / (letters * letter + space) + 1e-9);
};
/** What a floor of `count` hidden words or characters says, rounded down as the page rounds it. */
const saysHidden = (count, noun = "words") => {
  if (count < (noun === "words" ? ELIDED_MIN_WORDS : sourceConstant("ELIDED_MIN_CHARS"))) return "";
  const step = count < 20 ? 1 : count < 100 ? 5 : count < 1000 ? 10 : 100;
  return `(~${Math.floor(count / step) * step} more ${noun})`;
};

/** The label a row's line carries for width class `at` (and, on a rich header's row, `rh`), or "". */
const labelAt = (li, at, rh = null) => {
  const line = elidedOf(li);
  const span = line && line.children.find((one) =>
    String(one.getAttribute("data-w")).split(" ").includes(String(at)) && one.getAttribute("data-rh") === rh);
  return span ? span.textContent : "";
};

test("A FOLDED MESSAGE ENDS WITH WHAT IT HIDES, counted at every width class, in both lists (#244)", async () => {
  assert.equal(FOLD_LINES, 3, "the model's default line count is not the clamp's default three");
  const text = wordsOfLength(200, 4);
  assert.ok(text.length > COLLAPSE_OVER_CHARS, "the text would not fold");
  const page = newPage();
  await startTalking(page);
  assistantSays(page, text);
  const spoken = page.el("transcript").children.at(-1);
  const posted = (await showDiscord(page, [message({ id: "7440000000000000001", content: text })]))[0];
  const expected = FOLD_WIDTHS_EM.map((widthEm) => saysHidden(200 - FOLD_LINES * wordsPerLine(page, 4, widthEm)));
  assert.equal(expected[PHONE_CLASS], "(~160 more words)", "the fixture's phone cut moved; recount it");
  for (const [where, li] of [["the transcript", spoken], ["the channel", posted]]) {
    const line = elidedOf(li);
    assert.ok(line, `${where} folded a long message with no count under it`);
    assert.equal(line.hidden, false, `${where} hid the count on a folded message`);
    assert.deepStrictEqual(FOLD_WIDTHS_EM.map((_width, at) => labelAt(li, at)), expected,
      `${where} miscounted the hidden words at some width`);
    // One label for each class at most, and each class it names is a class there is.
    const named = line.children.flatMap((span) => String(span.getAttribute("data-w")).split(" ").map(Number));
    assert.deepStrictEqual([...named].sort((a, b) => a - b), [...new Set(named)].sort((a, b) => a - b),
      `${where} has two labels for one width`);
    assert.ok(named.every((at) => Number.isInteger(at) && at >= 0 && at < FOLD_WIDTHS_EM.length));
    // Straight after the text it is about, before anything else the row carries.
    assert.equal(li.children.indexOf(line), li.children.indexOf(textOfRow(li)) + 1,
      `${where} put the count somewhere other than straight under the text`);
  }
});

test("EACH COUNT IS A FLOOR: fewer at a wider line, rounded down, and nothing under ten words (#244)", async () => {
  const page = newPage();
  const says = (words, chars = words * 6) => page.pure("elisionLabel")({ words, chars });
  assert.equal(says(ELIDED_MIN_WORDS - 1), "", "a floor of a few words claims that words are hidden");
  assert.equal(says(ELIDED_MIN_WORDS), `(~${ELIDED_MIN_WORDS} more words)`);
  assert.equal(says(19), "(~19 more words)", "under twenty the count is not exact");
  assert.equal(says(24), "(~20 more words)", "a floor was rounded up past what it counted");
  assert.equal(says(99), "(~95 more words)");
  assert.equal(says(313), "(~310 more words)");
  assert.equal(says(1999), "(~1900 more words)");
  // At every class: never more at a wider line than at a narrower one.
  await signIn(page);
  const rows = await showDiscord(page, [message({ id: "7440000000000000101", content: wordsOfLength(400, 7) })]);
  const counts = FOLD_WIDTHS_EM.map((_width, at) => Number((/\d+/.exec(labelAt(rows[0], at)) || ["0"])[0]));
  counts.slice(1).forEach((count, at) => assert.ok(count <= counts[at],
    `${FOLD_WIDTHS_EM[at + 1]}em says ${count} hidden, more than ${counts[at]} at ${FOLD_WIDTHS_EM[at]}em`));
  assert.ok(counts[0] > counts.at(-1), "the count does not depend on the width at all");
});

test("A MESSAGE THAT FITS A DESK'S THREE LINES SAYS NOTHING THERE, and still counts on a phone (#244)", async () => {
  // From review: a phone's model on a desk claimed hidden words under a 300-character message that
  // three desk lines hold. Forty-five words of five letters: a phone's class hides most of them, a
  // desk's three lines hold them all.
  const page = newPage();
  await signIn(page);
  const text = wordsOfLength(50, 5);
  assert.ok(text.length > COLLAPSE_OVER_CHARS && text.length < 320, "the fixture is not the review's message");
  const rows = await showDiscord(page, [message({ id: "7440000000000000151", content: text })]);
  assert.ok(FOLD_LINES * wordsPerLine(page, 5, FOLD_WIDTHS_EM[DESK_CLASS]) >= 50, "a desk line would not hold it");
  assert.equal(labelAt(rows[0], DESK_CLASS), "", "a desk's line says words are hidden under a message it holds whole");
  assert.equal(labelAt(rows[0], FOLD_WIDTHS_EM.length - 1), "");
  assert.equal(labelAt(rows[0], PHONE_CLASS), saysHidden(50 - FOLD_LINES * wordsPerLine(page, 5, 28.5)));
  assert.notEqual(labelAt(rows[0], PHONE_CLASS), "", "a phone's three lines hide most of it, and say nothing");
});

test("a short message has no count, a width that hides under ten words has none, and an open one hides it until it folds again (#244)", async () => {
  const page = newPage();
  await signIn(page);
  // Just over the fold: a phone's three lines hide fifteen of its words, a line of 33em only nine.
  const barely = wordsOfLength(48, 5);
  const wider = FOLD_WIDTHS_EM.indexOf(33);
  const hidden = (at) => 48 - FOLD_LINES * wordsPerLine(page, 5, FOLD_WIDTHS_EM[at]);
  assert.ok(barely.length > COLLAPSE_OVER_CHARS);
  assert.ok(hidden(wider) > 0 && hidden(wider) < ELIDED_MIN_WORDS && hidden(PHONE_CLASS) >= ELIDED_MIN_WORDS,
    `the fixture hides ${hidden(wider)} words at 33em and ${hidden(PHONE_CLASS)} on a phone`);
  const long = wordsOfLength(200, 4);
  const rows = await showDiscord(page, [
    message({ id: "7440000000000000201", content: SHORT_MESSAGE }),
    message({ id: "7440000000000000202", content: barely }),
    message({ id: "7440000000000000203", content: long }),
  ]);
  assert.equal(elidedOf(rows[0]), undefined, "a short message, which hides nothing, carries a count");
  assert.equal(rows[1].getAttribute("data-collapsed"), "true", "the fixture did not fold");
  assert.equal(labelAt(rows[1], wider), "", "a fold hiding a few words claims that some are hidden");
  assert.equal(labelAt(rows[1], FOLD_WIDTHS_EM.length - 1), "");
  assert.equal(labelAt(rows[1], PHONE_CLASS), `(~${hidden(PHONE_CLASS)} more words)`);
  const label = elidedOf(rows[2]);
  await foldButton(rows[2]).click();
  assert.equal(rows[2].getAttribute("data-collapsed"), "false");
  assert.equal(label.hidden, true, "an open message still says words are hidden");
  await foldButton(rows[2]).click();
  assert.equal(label.hidden, false, "folded again, the count did not come back");
  assert.equal(labelAt(rows[2], PHONE_CLASS), saysHidden(200 - FOLD_LINES * wordsPerLine(page, 4, 28.5)));
  // Under the Links filter the row shows its links instead of its text, and the count goes with it.
  assert.match(CSS, /\.messages li\[data-links-view="true"\] > \.elided,[^{]*\{\s*display:\s*none !important;/,
    "the Links filter leaves a count under a row whose text it took away");
});

test("THE STYLESHEET SHOWS THE LABEL OF THE WIDTH CLASS THE LINE IS IN, measured in em of the text (#244)", () => {
  // The line is a size container as wide as the text and at the text's size, so `em` in its queries is
  // the text's em: one rule per class, edge to edge, the same edges as FOLD_WIDTHS_EM, and none past
  // the last. tests/elision_browser.py checks that Chromium really measures it so.
  assert.match(cssBlock(".elided"), /container:\s*elided \/ inline-size;/, "the line is not a size container");
  const span = cssBlock(".elided > span");
  assert.match(span, /display:\s*none;/, "every label shows unless its class's rule shows it");
  assert.match(span, /white-space:\s*nowrap/, "a narrow bubble can break the count from its word");
  const rules = [...CSS_CODE.matchAll(/@container elided \(([^)]*)\) \{\s*\.elided > \[data-w~="(\d+)"\] \{\s*display: inline;\s*\}\s*\}/g)];
  assert.equal(rules.length, FOLD_WIDTHS_EM.length, "a width class has no rule, or a rule no class");
  rules.forEach(([, condition, at], i) => {
    assert.equal(Number(at), i, `the rules are out of order at ${condition}`);
    const lower = i === 0 ? "" : `${FOLD_WIDTHS_EM[i - 1]}em <= `;
    assert.equal(condition, `${lower}width < ${FOLD_WIDTHS_EM[i]}em`, `class ${i} is not the line widths it was counted for`);
  });
  for (const list of ["#pinned-log", "#side-log", "#discord-log"]) {
    assert.match(CSS_CODE, new RegExp(`${list} \\.elided[,\\s][^{]*\\{\\s*font-size: calc\\(0\\.88rem \\* var\\(--msg-scale\\)\\);`),
      `in ${list} the line is not at the text's size, so its em is not the text's`);
  }
  assert.match(cssBlock(':root[data-rich-headers="off"] .elided > [data-rh="on"]'), /display:\s*none;/);
  assert.match(cssBlock(':root:not([data-rich-headers="off"]) .elided > [data-rh="off"]'), /display:\s*none;/);
});

test("TEXT POOR IN SPACES IS COUNTED IN CHARACTERS: Chinese, a long bare address — and prose is not (#244)", async () => {
  const page = newPage();
  await signIn(page);
  const ideograph = "漢";
  const wide = page.pure(`glyphEm("${ideograph}")`);
  const letter = page.pure('glyphEm("x")');
  const chinese = ideograph.repeat(300);
  const address = `x${"x".repeat(419)}`;
  const rows = await showDiscord(page, [
    message({ id: "7440000000000000501", content: chinese }),
    message({ id: "7440000000000000502", content: address }),
    message({ id: "7440000000000000503", content: `${wordsOfLength(60, 5)} https://example.com/${"x".repeat(60)}` }),
  ]);
  const perLine = (em, widthEm) => Math.floor(widthEm / em + 1e-9);
  for (const at of [0, PHONE_CLASS, DESK_CLASS]) {
    const widthEm = FOLD_WIDTHS_EM[at];
    assert.equal(labelAt(rows[0], at), saysHidden(300 - FOLD_LINES * perLine(wide, widthEm), "characters"),
      `Chinese at ${widthEm}em is not counted in the characters its clamp hides`);
    assert.equal(labelAt(rows[1], at), saysHidden(420 - FOLD_LINES * perLine(letter, widthEm), "characters"),
      `an address at ${widthEm}em is not counted in characters`);
  }
  assert.match(labelAt(rows[2], PHONE_CLASS), /^\(~\d+ more words\)$/, "prose with an address in it is counted in characters");
});

test("a combined row is counted as it is drawn, each message from a line of its own (#244)", async () => {
  // At 25em a line holds twelve four-letter words, so the first message's twenty-five fill three lines
  // and the second starts the fourth: all thirty-five of its words are hidden. Read as one run of text,
  // eleven of them would fit on the third line instead.
  const first = wordsOfLength(25, 4);
  const second = wordsOfLength(35, 4);
  assert.ok(first.length <= COLLAPSE_OVER_CHARS && second.length <= COLLAPSE_OVER_CHARS,
    "each message alone would fold, which is not the case under test");
  const page = newPage();
  await signIn(page);
  const at = FOLD_WIDTHS_EM.indexOf(25);
  assert.equal(wordsPerLine(page, 4, 25), 12, "the fixture's line moved; recount it");
  const rows = await showDiscord(page, [
    message({ id: "7440000000000000301", content: first, timestamp: "2026-08-19T14:00:00Z" }),
    message({ id: "7440000000000000302", content: second, timestamp: "2026-08-19T14:00:01Z" }),
  ]);
  assert.equal(rows.length, 1, "the two messages were not combined");
  assert.equal(labelAt(rows[0], at), "(~35 more words)");
});

test("a body the server rendered is counted by the words it shows, a link by its text (#244)", async () => {
  const words = wordsOfLength(60, 9);
  const href = `https://example.com/${"x".repeat(200)}`;
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [message({
    id: "7440000000000000401",
    content: `${words} [the run log](${href})`,
    content_html: `<p>${words} <a href="${href}" target="_blank" rel="noopener noreferrer nofollow">the run log</a></p>\n`,
  })]);
  assert.equal(rows[0].getAttribute("data-collapsed"), "true", "the rendered message did not fold");
  // Spread into this realm's arrays: the page's run in a context of their own.
  const labels = (text) => [...page.pure("elisionLabels")(text)];
  const shown = labels(`${words} the run log`);
  assert.deepStrictEqual(FOLD_WIDTHS_EM.map((_width, at) => labelAt(rows[0], at)), shown,
    "the rendered body is not counted by the words on the screen");
  assert.notDeepStrictEqual(shown, labels(`${words} ${href}`), "the fixture cannot tell a link's text from its address");
});

test("A RICH HEADER'S ROW counts its text without the words the header restates, and with them when the switch is off (#244)", async () => {
  // `#242 rich-headers`. The words the header shows are hidden from the body while it shows them, so
  // the clamp shows the body's next words in their place.
  const lead = "[build-bot coordinator] Backup check, 10:52 PM ET. ";
  const rest = wordsOfLength(120, 5);
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [
    message({ id: "7440000000000000601", content: `${lead}${rest}`,
      heading: { tag: "build-bot coordinator", time: "10:52 PM ET", title: "Backup check", cut: lead.length } }),
    message({ id: "7440000000000000602", content: `${lead}${rest}` }),
  ]);
  assert.ok(textOfRow(rows[0]).children[0].hasAttribute("data-rh"), "the fixture's heading was not shown");
  const labels = (text) => [...page.pure("elisionLabels")(text)];
  assert.deepStrictEqual(FOLD_WIDTHS_EM.map((_width, at) => labelAt(rows[0], at, "on")), labels(rest),
    "with the header showing, the row is not counted by the text under it");
  assert.deepStrictEqual(FOLD_WIDTHS_EM.map((_width, at) => labelAt(rows[0], at, "off")), labels(`${lead}${rest}`),
    "with the switch off, the row is not counted by its text as written");
  assert.notEqual(labelAt(rows[0], 0, "on"), labelAt(rows[0], 0, "off"), "the fixture cannot tell the two apart");
  // A row with no header has one label per class, for either state of the switch.
  assert.ok(elidedOf(rows[1]).children.every((span) => !span.hasAttribute("data-rh")));
});

// From review: the model read every line of the text as a line of the clamp, and the clamp does not
// draw everything in lines. These build bodies as the server renders them (`<table>`, `<pre>`, a list)
// and hold each label to what the clamp really does with that block; tests/elision_browser.py holds a
// real Chromium's labels to what it really hides.

/** A table as the server's renderer writes one: a header row and `rows` rows of three cells. */
const tableHtml = (rows) => {
  const row = (tag, cells) => `<tr>\n${cells.map((cell) => `<${tag}>${cell}</${tag}>\n`).join("")}</tr>\n`;
  const body = Array.from({ length: rows }, (_unused, r) => row("td", [0, 1, 2].map((c) => `shard ${r} value ${c}`)));
  return `<table>\n<thead>\n${row("th", ["Column 0", "Column 1", "Column 2"])}</thead>\n<tbody>\n${body.join("")}</tbody>\n</table>\n`;
};
/** The words in `tableHtml(rows)`: two to a header, four to a cell. */
const tableWords = (rows) => 3 * 2 + rows * 3 * 4;

test("A TABLE IS DRAWN WHOLE: counted as shown where it starts inside the three lines, hidden where it starts below them (#244)", async () => {
  // From review: a folded table said "(~95 more words)" with every word of it on the screen. The table
  // scrolls sideways in a box of its own (`.md table`), and the clamp counts no line inside such a box:
  // it draws the table whole and goes on counting lines under it.
  assert.match(cssBlock(".md table"), /display:\s*block;[\s\S]*overflow-x:\s*auto;/,
    "a table is no longer a box of its own, which is what makes the clamp draw it whole");
  const page = newPage();
  await signIn(page);
  const prose = (count) => `<p>${wordsOfLength(count, 4)}</p>\n`;
  const rows = await showDiscord(page, [
    message({ id: "7440000000000000701", content: "a table", content_html: tableHtml(8) }),
    message({ id: "7440000000000000702", content: "a table, then prose", content_html: tableHtml(4) + prose(120) }),
    message({ id: "7440000000000000703", content: "prose, then a table", content_html: prose(40) + tableHtml(6) }),
  ]);
  rows.forEach((row) => assert.equal(row.getAttribute("data-collapsed"), "true", "the fixture did not fold"));
  assert.equal(elidedOf(rows[0]), undefined, "a table the clamp draws whole says words are hidden");
  // The table is shown, and so are three lines of the prose under it: the rest of the prose is hidden.
  const perLine = wordsPerLine(page, 4, 28.5);
  assert.equal(labelAt(rows[1], PHONE_CLASS), saysHidden(120 - FOLD_LINES * perLine),
    "the words of a table the clamp draws whole are counted as hidden");
  // Prose a word longer than three lines: the table starts below the clamp's last line, all of it hidden.
  assert.equal(FOLD_LINES * perLine, 39, "the fixture's phone line moved; recount it");
  assert.equal(labelAt(rows[2], PHONE_CLASS), saysHidden(1 + tableWords(6)),
    "a table under the clamp's last line is not counted as hidden");
});

test("A CODE BLOCK is counted in lines of its own, in code's face, and the line its padding shows (#244)", async () => {
  const page = newPage();
  await signIn(page);
  // Fourteen lines of four words. Under one line of prose the clamp shows two of them, and the block's
  // own bottom padding the top of a third, which the reader can see: eleven lines hidden.
  const steps = Array.from({ length: 14 }, (_unused, i) => `step ${i}: ok checksum=abc${i}\n`).join("");
  // Ten lines of fifty m's and a number. In code's face a line is 50 × MONO_EM, well inside a phone's
  // line; at the proportional font's 0.72em an "m", each would be counted as wrapping onto two.
  const mono = GLYPHS.MONO_EM;
  assert.ok(50 * mono < 28.5 && 50 * page.pure('glyphEm("m")') > 28.5, "the fixture no longer tells the two faces apart");
  const wide = Array.from({ length: 10 }, (_unused, i) => `${"m".repeat(50)} ${i}\n`).join("");
  const rows = await showDiscord(page, [
    message({ id: "7440000000000000801", content: "code",
      content_html: `<p>Here is the run:</p>\n<pre><code>${steps}</code></pre>\n` }),
    message({ id: "7440000000000000802", content: "wide code", content_html: `<pre><code>${wide}</code></pre>\n` }),
  ]);
  for (const at of [PHONE_CLASS, DESK_CLASS]) {
    assert.equal(labelAt(rows[0], at), saysHidden(11 * 4), `at ${FOLD_WIDTHS_EM[at]}em the code block is miscounted`);
  }
  // Three lines and the top of a fourth shown, six hidden: 51 characters each, in two words.
  assert.equal(labelAt(rows[1], PHONE_CLASS), saysHidden(6 * 51, "characters"),
    "a line of code is counted at the proportional font's widths");
});

test("A LIST IS LINES, an item to each however short, and a list of short items says what it hides (#244)", async () => {
  const page = newPage();
  await signIn(page);
  const items = Array.from({ length: 24 }, (_unused, i) => `<li>shard ${i} synced</li>\n`).join("");
  const rows = await showDiscord(page, [
    message({ id: "7440000000000000901", content: "a list", content_html: `<ul>\n${items}</ul>\n` }),
  ]);
  assert.equal(rows[0].getAttribute("data-collapsed"), "true", "the fixture did not fold");
  // Three items shown at every width, however wide: each starts a line of its own.
  assert.deepStrictEqual(FOLD_WIDTHS_EM.map((_width, at) => labelAt(rows[0], at)),
    FOLD_WIDTHS_EM.map(() => saysHidden(21 * 3)), "a list's short items are not counted a line each");
});

test("WHAT THE MODEL HAS NO RULE FOR GETS NO LABEL, and every element the server sends has a rule (#244)", () => {
  const page = newPage();
  const known = new Set([...page.pure("FOLD_INLINE"), ...page.pure("FOLD_FLOW_BLOCKS"), "br", "pre", "table"]);
  // A table's own parts are only ever inside one, which is read whole.
  const inTables = new Set(["thead", "tbody", "tr", "th", "td"]);
  for (const tag of SANITIZED_TAGS) {
    assert.ok(known.has(tag) || inTables.has(tag), `src/render.rs sends <${tag}>, which the fold's model has no rule for`);
  }
  // Something it has no rule for: the body is not laid out from a guess, and says nothing.
  const body = page.document.createElement("div");
  const odd = page.document.createElement("details");
  odd.append(page.document.createTextNode(wordsOfLength(200, 4)));
  body.append(odd);
  assert.equal(page.pure("foldRuns")(body), null, "an element with no rule was laid out from a guess");
  assert.deepStrictEqual([...page.pure("elisionLabels")(null)], FOLD_WIDTHS_EM.map(() => ""));
});

test("A CHARACTER OF A WIDE SCRIPT IS A PLACE A LINE BREAKS: Chinese after a word fills the line it starts on (#244)", async () => {
  // At 14em "Note:" and a space take 2.15em, and thirteen ideographs fill the rest of that line, then
  // fifteen to each line after it. Read as one unbreakable run, the ideographs would start a line of
  // their own under "Note:", and the label would claim thirteen more characters hidden than are.
  const run = async (script) => {
    const page = newPage(new Map(), script);
    await signIn(page);
    const rows = await showDiscord(page, [message({ id: "7440000000000001001", content: `Note: ${"漢".repeat(300)}` })]);
    return labelAt(rows[0], 0);
  };
  const shown = 5 + 13 + 15 + 15;
  assert.equal(await run(SCRIPT), saysHidden(305 - shown, "characters"), "Chinese after a word is not broken at its characters");
  const control = await run(brokenScript("const wide = WIDE_SCRIPT.test(ch);", "const wide = false;"));
  assert.notEqual(control, saysHidden(305 - shown, "characters"), "the fixture cannot tell a split script from an unsplit one");
});

test("switching views does not disturb a live call", async () => {
  const page = newPage();
  const socket = await startTalking(page);
  await page.el("talk").click(); // muted, so the mute state has something to be preserved
  page.messages = [message({ content: "hi" })];

  await page.el("view-switch").click();
  await page.settle();
  assert.equal(page.tab(), "discord");
  await page.el("view-switch").click();
  assert.equal(page.tab(), "voice");

  assert.equal(page.sockets.length, 1, "a tab switch opened a second conversation");
  assert.equal(page.sockets[0], socket);
  assert.notEqual(socket.readyState, 3, "a tab switch hung up");
  assert.equal(page.tracks[0].stops, 0, "a tab switch released the microphone");
  assert.equal(page.micRequests.length, 1, "a tab switch re-asked for the microphone");
  assert.equal(page.el("talk-label").textContent, "Muted", "a tab switch unmuted the call");
  assert.equal(page.el("control-pane").hidden, false, "the controls left with the transcript");
});

test("the header spends one row, because the transcript is what deserves the height", () => {
  // REWRITTEN BY `#58 control-bar`, which is the reason the two ids it used to look for are not in
  // this row any more. The claim it was making — the header never becomes two bands — is unchanged
  // and is asserted more strictly than before: whatever the header holds, including the control
  // bar when the reader has asked for it at the top, it holds on ONE row.
  const row = HTML.slice(HTML.indexOf('id="topbar-row"'), HTML.indexOf("</header>"));
  assert.equal((row.match(/class="topbar-row"/g) || []).length, 1, "the header grew a second row");
  assert.ok(row.includes('id="control-bar-top"'), "the header has nowhere to put the control bar");
  assert.match(cssBlock(".topbar-row"), /display:\s*flex/, "the row is no longer one line of items");
  // And the mount takes the leftover width, so a bar placed up here spans the row rather than
  // huddling beside the title.
  assert.match(cssBlock("#control-bar-top"), /flex:\s*1 1 auto/);
});

test("THE HEADER COSTS NO ROW ON THE MAIN SCREEN, AND THE GLASS FLOATS OVER THE LIST INSTEAD", async () => {
  // `#58 control-bar` made this claim first: with the bar in the dock the header holds nothing on
  // the main screen, and an empty 2.4rem strip across the top of a phone is exactly the real estate
  // that issue reclaimed — hidden outright, a grid row that collapses, not merely emptied.
  //
  // `#129 message-search` then parked its glass in that strip's corner, and the row came back on
  // every main screen to carry one small icon. The owner, from his phone: "Wasteful search bar. The
  // little search button in the upper right wastes a full horizontal strip of space. It should just
  // be a floating icon." `#197 floating-search` is that, and this restores `#58`'s claim with the
  // glass's new home asserted beside it, so the row cannot come back by the same route.
  const page = newPage();
  assert.equal(page.el("topbar").hidden, true, "an empty header stands on the sign-in screen");
  await signIn(page);
  assert.equal(page.el("topbar").hidden, true, "the header costs a row on the main screen again");
  assert.equal(page.el("search-toggle").hidden, false, "the glass went with the header");

  // ...in both views: the glass filters the transcript as well as the channel.
  await showDiscord(page, [message({ id: "1", content: "hello" })]);
  assert.equal(page.el("topbar").hidden, true, "the channel view brought the header back");
  assert.equal(page.el("search-toggle").hidden, false, "the channel view has no glass");

  // It is still a title bar wherever it has something to say, with a way back on each of the two
  // screens that is a destination. A reply is open in the list (`#410 inline-reply-editor`), on the
  // main screen, which costs no row for it.
  const [line] = page.el("discord-log").children;
  await replyButton(line).click();
  assert.equal(page.el("topbar").hidden, true, "a reply in the list brought the header back");
  assert.equal(page.el("search-toggle").hidden, false, "the glass went with a reply open in the list");
  await page.el("reply-cancel").click();

  await page.el("open-settings").click();
  assert.equal(page.el("topbar").hidden, false, "the way back out of Settings was hidden");
  assert.equal(page.el("topbar-title").textContent, "Settings");
  assert.equal(page.el("close-settings").hidden, false, "Settings has no way back");
  await page.el("help-link-resuming").click();
  assert.equal(page.el("topbar").hidden, false, "Help lost its title bar");
  assert.equal(page.el("topbar-title").textContent, "Help");
  assert.equal(page.el("close-help").hidden, false, "Help has no way back");
  await page.el("close-help").click();
  await page.el("close-settings").click();
  assert.equal(page.el("topbar").hidden, true, "coming back from Settings brought the row back");

  // ...and the reader who asks for the bar up there gets the header, holding the bar — with the
  // glass still floating over the list rather than taking a share of that row.
  page.el("bar-placement").value = "top";
  await page.el("bar-placement").dispatch("change");
  assert.equal(page.el("topbar").hidden, false, "the bar moved to a header that is not shown");
  assert.equal(page.el("search-toggle").hidden, false, "moving the bar up took the glass away");
});

test("the glass, its field and its count are declared on the floating line, not in the header", () => {
  // The markup half of `#197 floating-search`. The ids are unchanged — scripts/screenshots.py and
  // every search test drive them by name — and what moved is their parent: out of the header row
  // and onto the zero-height line over the top of the list that the freshness pill floats on.
  const header = HTML.slice(HTML.indexOf('id="topbar"'), HTML.indexOf("</header>"));
  for (const id of ["search-toggle", "search-field", "search-count"]) {
    assert.doesNotMatch(header, new RegExp(`id="${id}"`), `#${id} is still declared in the header`);
    assertMarkupContains("channel-freshness-anchor", id);
    assertMarkupContains("search-float", id);
  }
  assertMarkupContains("channel-freshness-anchor", "channel-freshness");
  // OUTSIDE the scroller, beside the pill, for the pill's reasons: inside, the header mask would
  // fade the glass with the content and it would scroll away from the reader who wants it.
  const main = HTML.slice(HTML.indexOf('id="screen-main"'), HTML.indexOf('id="screen-settings"'));
  assert.ok(
    main.indexOf('id="search-toggle"') < main.indexOf('id="scroll-area"'),
    "the glass is inside the scrolling list, where the mask fades it and a scroll takes it away"
  );

  // The line: zero height, at the top of the scroller's own cell, centred on the pill's middle.
  const line = cssBlock("#channel-freshness-anchor");
  assert.match(line, /height:\s*0/, "the line has height, so its appearing resizes the list");
  assert.match(line, /grid-row:\s*3/);
  assert.match(
    line,
    /margin-top:\s*var\(--float-line\)/,
    "the line is not through the pill's middle"
  );
  assert.match(line, /align-items:\s*center/, "the pill is not centred on the line");
  assert.match(
    cssBlock(":root"),
    /--float-line:\s*calc\(var\(--freshness-top\) \+ var\(--freshness-box\) \/ 2\)/,
    "the line is restated rather than derived from the pill's own box"
  );
  // ...and the glass is centred on the same line, at its right-hand end, whatever height it is.
  const float = cssBlock("#search-float");
  assert.match(float, /position:\s*absolute/);
  assert.match(float, /top:\s*0/);
  assert.match(float, /transform:\s*translateY\(-50%\)/, "the glass is not centred on the line");
  assert.match(float, /right:\s*calc\(var\(--float-edge\) \+ env\(safe-area-inset-right\)\)/);
});

test("the glass is DRAWN larger than the pill, with a larger glyph, and HIT at 48px", () => {
  // `#197 floating-search` drew the glass the pill's height — 26px — so the two read as one band.
  // The owner, on his phone (`#211 link-filter`): "our search button in the upper right corner is
  // very small. Let's make it a little bigger and therefore the bar that it pops up bigger too." So
  // the disc is a number of its own now, well over the pill's height, its glyph is a third larger,
  // and the invisible square around it that takes the tap is 48px. In px, all of it (see the test
  // after this one); the pill is type, in rem, measured here at the default 16px.
  const root = cssBlock(":root");
  const rem = (name, block = root) => {
    const found = new RegExp(`${name}:\\s*([\\d.]+)rem`).exec(block);
    assert.ok(found, `${name} is not declared in rem`);
    return Number(found[1]) * 16;
  };
  const px = (name, block = root) => {
    const found = new RegExp(`${name}:\\s*([\\d.]+)px`).exec(block);
    assert.ok(found, `${name} is not declared in px`);
    return Number(found[1]);
  };
  const disc = px("--search-disc");
  assert.ok(disc >= 36, `the glass is drawn ${disc}px across; at 26px the owner called it very small`);
  // The pill's own box, from its tokens: 2 * 0.25rem of padding, a 1rem line, and its 2px border.
  const pill = 2 * rem("--freshness-pad") + rem("--freshness-line") + 2;
  assert.ok(disc >= pill + 8, `the glass (${disc}px) is not visibly larger than the pill (${pill}px)`);
  const glyph = px("width", cssBlock("#search-toggle .icon"));
  assert.ok(glyph >= 20, "the glass's glyph is no larger than it was");
  const hit = px("--search-hit");
  assert.ok(hit >= 48, `the glass is hit at ${hit}px, under 48px`);
  assert.ok(hit > disc, "the target is no larger than what is drawn");
  const glass = cssBlock("#search-toggle");
  assert.match(glass, /width:\s*var\(--search-disc\)/);
  assert.match(glass, /height:\s*var\(--search-disc\)/);
  assert.match(glass, /position:\s*relative/, "the hit area has nothing to be measured from");
  const target = cssBlock("#search-toggle::after");
  assert.match(target, /content:\s*""/, "the hit area is not generated at all");
  assert.match(target, /position:\s*absolute/);
  // Every side is measured from the button's padding box, one border inside the disc. Vertically
  // the square is centred on the disc; sideways it reaches the list's edge on the right and makes
  // up the rest of --search-hit on the left, because centred it stuck out of the viewport.
  const sides = /inset:\s*([^;]+);/.exec(target);
  assert.ok(sides, "the hit area has no inset");
  assert.deepStrictEqual(
    sides[1].trim().split(/\s*\n\s*|\s+(?=calc)/),
    [
      "calc((var(--search-disc) - var(--search-hit)) / 2 - 1px)",
      "calc(-1 * var(--float-edge) - 1px)",
      "calc((var(--search-disc) - var(--search-hit)) / 2 - 1px)",
      "calc(var(--search-disc) + var(--float-edge) - var(--search-hit) - 1px)",
    ],
    "the hit area is not --search-hit square, or it reaches past the list's edge"
  );
  // ...and the glass sits exactly --float-edge from that edge, which the right side assumes.
  assert.match(
    cssBlock("#search-float"),
    /right:\s*calc\(var\(--float-edge\) \+ env\(safe-area-inset-right\)\)/
  );
  // The bar it opens is bigger to match: as tall as the glass's target, so that target, and the
  // filters' squares, are inside it; a larger field; and the line placed low enough that the bar,
  // centred on it, still starts inside the list.
  assert.match(root, /--search-bar:\s*max\(var\(--search-hit\), 3rem\);/,
    "the open bar is not at least as tall as the glass's target, and as the count's type");
  const bar = Math.max(hit, 48);
  assert.ok(px("--toggle-hit") >= 44 && px("--toggle-hit") <= bar, "a filter's square is under 44px or taller than the bar");
  const size = /font-size:\s*(\d+)px/.exec(cssBlock(".search-field"));
  assert.ok(size && Number(size[1]) >= 17, "the field's text is no larger than it was");
  assert.match(
    root,
    /--freshness-top:\s*calc\(var\(--search-bar\) \/ 2 \+ 0\.1rem - var\(--freshness-box\) \/ 2\)/,
    "the line is not placed by the bar, so the larger bar hangs over what is above the list"
  );
  // ...and the head of the list makes room for whichever of the pill and the glass reaches lower:
  // its top edge, since `#324 live-pill-covers-row`, so the room does not scroll away.
  assert.match(
    cssBlock("#screen-main:not([data-searching]) > #scroll-area:has(> #pane-discord[data-freshness]:not([hidden]))"),
    /margin-top:\s*calc\(var\(--float-line\) \+ max\(var\(--freshness-box\), var\(--search-disc\)\) \/ 2\)/,
    "the room at the head of the list is the pill's alone, so the larger glass sits on the first row"
  );
});

test("the bar's buttons stay their size under a larger font, so the field keeps room for what is typed", () => {
  // `#211 link-filter`, from review. The glass, its target, the filter squares, their glyphs, the
  // space between them and the field's own inset were all in rem, so a reader's larger system font
  // (the suite stands it in by the root font, as `MENU_SCALES` in tests/offline_cache_browser.py
  // does) grew every one of them, and every pixel they grew came off the field, the only thing on
  // the bar that is text. With Links on — which puts the count up with nothing typed — a 360px
  // phone at 150% type left the field 38px wide, its text area none at all; at 130%, 79px. A phone
  // sizes its buttons in dp and its type in sp: larger type asks for larger words, not larger
  // buttons. tests/offline_cache_browser.py measures the field at those sizes in Chromium.
  const root = cssBlock(":root");
  // --float-edge too: the glass's target reaches the list's edge across it, and makes up the rest of
  // its 48px on the left of the disc, where the filter beside it is.
  for (const token of ["--search-disc", "--search-hit", "--toggle-hit", "--bar-gap", "--float-edge"]) {
    assert.match(root, new RegExp(`${token}:\\s*[\\d.]+px;`), `${token} grows with the reader's font`);
  }
  for (const selector of ["#search-toggle .icon", "#links-filter .icon", "#pinned-filter .icon"]) {
    const block = cssBlock(selector);
    assert.match(block, /width:\s*\d+px/, `${selector} grows with the reader's font`);
    assert.match(block, /height:\s*\d+px/, `${selector} grows with the reader's font`);
  }
  // The field's inset follows its type, which is px for iOS's sake (see `.search-field`).
  assert.match(cssBlock(".search-field"), /padding:\s*\d+px \d+px;/, "the field's inset grows around text that does not");
  assert.match(cssBlock("#screen-main[data-searching] #search-float"), /padding:\s*\d+px \d+px \d+px \d+px;/,
    "the open bar's inset grows with the reader's font");
  // ...and nothing on the bar is sized by another rem in their place.
  for (const selector of ["#search-toggle", "#links-filter", "#pinned-filter", "#screen-main[data-searching] #search-toggle"]) {
    assert.doesNotMatch(cssBlock(selector), /(width|height|margin[\w-]*|inset):[^;]*\drem/,
      `${selector} is sized in rem`);
  }
});

test("with no header on screen, the body applies the top safe-area inset itself", () => {
  // NOT VERIFIABLE BY SCREENSHOT, for the reason the horizontal insets are not: no automation makes
  // Chromium report an inset. The top inset lived only in the header's padding, and the header is
  // now hidden on the ordinary main screen — so without this the notch sits on the floating glass.
  assert.match(cssBlock("#topbar"), /env\(safe-area-inset-top\)/, "the header lost its own inset");
  assert.match(
    cssBlock("#topbar[hidden] + #frame-body"),
    /padding-top:\s*env\(safe-area-inset-top\)/,
    "nothing applies the top inset when the header is not there"
  );
  // ...and the selector is honest: the header really is the body's immediate sibling.
  const between = HTML.slice(HTML.indexOf("</header>"), HTML.indexOf('<main id="frame-body"'));
  assert.doesNotMatch(between.replace(/<!--[\s\S]*?-->/g, ""), /<[a-z]/i, "something sits between");
});

test("a raw message row spends its width on the MESSAGE, and holds the rest behind a press", async () => {
  // The entire value of this view is still that the operator can point at a real message — or
  // show that there is none — so the id has to remain REACHABLE. It does not have to be printed
  // on every row of a 393-pixel screen, which is what it was.
  const page = newPage();
  await signIn(page);

  const lines = await showDiscord(page, [message({ id: "999888777666555444", author: "alice", author_is_bot: false })]);

  assert.equal(lines.length, 1);
  const text = lines[0].text();
  assert.match(text, /alice/, "a third party is not named, and colour cannot say which one it is");
  assert.doesNotMatch(
    text,
    /999888777666555444/,
    "the row still prints a nineteen-digit id nobody reads"
  );
  // A short local clock, and NOT the raw ISO stamp.
  assert.match(text, /\d{1,2}:\d{2}/, "the row does not say when the message was sent");
  assert.doesNotMatch(text, /2026-08-19T04:31/, "the row printed the raw ISO timestamp");
  // `#62 message-count-accuracy`, carried across from web/app.js. The fixture sends no `complete`,
  // which is exactly the ordinary case: the fetch window is not a channel total, so no digit.
  assert.match(channelSummaryText(page), /lead team — the most recent messages/);
  assert.doesNotMatch(channelSummaryText(page), /\(s\)/, "the placeholder plural came back");
});

test("the channel view counts only when the count is the CHANNEL'S, and pluralizes", async () => {
  const page = newPage();
  await signIn(page);
  page.messages = [message({ id: "1" }), message({ id: "2" })];
  // `has_more: false` is how the cursored route says "there is nothing older" — `#65
  // scrollback-paging` moved the read onto it, and this is the same claim `complete: true` used to
  // make on the window route. The count is only the channel's own when the server says that.
  page.channelPage = async () =>
    json(200, { channel: CHANNEL, messages: page.messages, has_more: false });

  await page.el("view-switch").click();
  await page.settle();
  assert.match(channelSummaryText(page), /^2 messages from lead team$/);

  page.messages = [message({ id: "1" })];
  await reReadChannel(page);
  assert.match(channelSummaryText(page), /^1 message from lead team$/, "singular, not '1 messages'");
  // Replaced, never stacked: a background poll runs every forty-five seconds, and a summary that
  // accumulated would grow a line each time. `#63 status-line-placement`.
  assert.equal(page.el("channel-summary").children.length, 1);
});

test("PRESS AND HOLD is where the author, the full time and the id went", async () => {
  const page = newPage();
  await signIn(page);
  const lines = await showDiscord(page, [
    message({ id: "999888777666555444", author: "alice", author_is_bot: false }),
  ]);

  await hold(page, lines[0]);
  const text = lines[0].text();
  assert.match(text, /999888777666555444/, "the id is not reachable at all any more");
  assert.match(text, /alice/, "the details do not say who sent it");
  // The day in `dayLabel`'s words (`#260 timestamp-hover`), which name the year only when it is not
  // this one. The fixture's message is from the clock's own day or the one before, whatever the zone.
  assert.match(text, /(Today|Yesterday), Aug \d+, [A-Z][a-z]{2}, /, "the details do not say which day it was sent");

  // The same gesture closes it, so the row does not accumulate a sheet per press.
  await hold(page, lines[0]);
  assert.doesNotMatch(lines[0].text(), /999888777666555444/, "a second hold did not close it");
});

test("tapping the message folds it, and tapping a control does not", async () => {
  const page = newPage();
  await signIn(page);
  const long = "x".repeat(4000);
  const lines = await showDiscord(page, [message({ id: "1000000000000000001", content: long })]);
  const li = lines[0];
  assert.equal(li.getAttribute("data-collapsed"), "true", "a long message did not start folded");

  await li.dispatch("click", {});
  assert.equal(li.getAttribute("data-collapsed"), "false", "tapping the message did not open it");

  await li.dispatch("click", {});
  assert.equal(li.getAttribute("data-collapsed"), "true", "tapping again did not close it");

  // A tap meant for the Reply or Done button is not also a fold.
  const before = li.getAttribute("data-collapsed");
  await doneButton(li).click();
  await page.settle();
  assert.equal(
    row(page, 0).getAttribute("data-collapsed"),
    before,
    "pressing a control in the row also folded the row"
  );
});

test("the fold control stays reachable by keyboard even though the row no longer prints it", async () => {
  // The page's standing rule: a gesture is a second way in, never the only one. The control is
  // clipped rather than removed, because `display: none` would take it out of the tab order.
  const page = newPage();
  await signIn(page);
  const lines = await showDiscord(page, [message({ content: "y".repeat(4000) })]);
  const fold = lines[0].descendants().find((node) => node.className === "fold");
  assert.ok(fold, "the fold control was deleted, so a keyboard cannot fold a message");
  assert.notEqual(fold.hidden, true, "the fold control is hidden, so it cannot be tabbed to");

  const clipped = cssBlock("#discord-log .fold");
  assert.match(clipped, /clip-path/, "the control is not clipped, so it is printed on the row");
  assert.doesNotMatch(clipped, /display:\s*none/, "display:none would remove it from the tab order");
});

test("the reader can resize MESSAGE text, and it survives a reload", async () => {
  const page = newPage();
  await signIn(page);
  await page.el("msg-scale").dispatch("input", {});

  page.el("msg-scale").value = "130";
  await page.el("msg-scale").dispatch("input", {});
  assert.equal(page.documentElement.style.getPropertyValue("--msg-scale"), "1.3");
  assert.equal(page.storage.get("vibe-talk.voice.msg-scale"), "130", "the size was not kept");

  // Out of range is CLAMPED, not refused: a stored value from another build must not brick the view.
  page.el("msg-scale").value = "900";
  await page.el("msg-scale").dispatch("input", {});
  assert.equal(page.documentElement.style.getPropertyValue("--msg-scale"), "1.5");

  // It scales the MESSAGE, not the row: a bigger Reply button is not what was asked for.
  assert.match(
    CSS,
    /#discord-log \.body[\s\S]{0,80}--msg-scale/,
    "the type scale does not reach the message body"
  );
});

// `#230 text-size-buttons`. The owner: "Put a plus and minus button next to the text size control in
// the settings." A − before the slider and a + after it, each one step of the slider, through the
// slider's own code, and disabled at the end it moves towards.

const MSG_SCALE_STORE = "vibe-talk.voice.msg-scale";

test("the text size slider has a − before it and a + after it on one row, each named for a screen reader", () => {
  const group = settingsGroup("Text size");
  const row = /<div class="msg-scale-row">([\s\S]*?)<\/div>/.exec(group);
  assert.ok(row, "the slider and its two buttons are not one row");
  const order = [...row[1].matchAll(/\bid="([^"]+)"/g)].map((m) => m[1]);
  assert.deepEqual(order, ["msg-scale-smaller", "msg-scale", "msg-scale-larger"], "not −, slider, + in that order");
  for (const [id, name, sign] of [["msg-scale-smaller", "Smaller text", "−"], ["msg-scale-larger", "Larger text", "+"]]) {
    const button = new RegExp(`<button([^>]*\\bid="${id}"[^>]*)>([^<]*)</button>`).exec(row[1]);
    assert.ok(button, `#${id} is not a button`);
    assert.match(button[1], /\btype="button"/, `#${id} is not type="button"`);
    assert.match(button[1], new RegExp(`\\baria-label="${name}"`), `#${id} is not called "${name}"`);
    assert.match(button[1], /\baria-controls="msg-scale"/, `#${id} does not say which control it moves`);
    assert.match(button[1], /\bclass="secondary step-button"/, `#${id} is not the page's own quiet button`);
    assert.equal(button[2].trim(), sign, `#${id} does not show its sign`);
  }
  // One step, and the ends, said once in the markup and once in the script: the two must agree.
  const slider = /<input([^>]*\bid="msg-scale"[^>]*)>/.exec(group)[1];
  for (const [attribute, constant] of [["min", "MIN_MSG_SCALE"], ["max", "MAX_MSG_SCALE"], ["step", "MSG_SCALE_STEP"]]) {
    const markup = (new RegExp(`\\b${attribute}="(\\d+)"`).exec(slider) || [])[1];
    const script = (new RegExp(`const ${constant} = (\\d+);`).exec(PAGE_CODE) || [])[1];
    assert.ok(markup !== undefined && script !== undefined, `the slider's ${attribute} or ${constant} is gone`);
    assert.equal(markup, script, `the slider's ${attribute} and ${constant} disagree`);
  }
  // The usual size and the largest are whole steps from the smallest, so every press lands on a mark.
  for (const name of ["DEFAULT_MSG_SCALE", "MAX_MSG_SCALE"]) {
    assert.equal((sourceConstant(name) - sourceConstant("MIN_MSG_SCALE")) % sourceConstant("MSG_SCALE_STEP"), 0,
      `${name} is not a whole number of steps from MIN_MSG_SCALE`);
  }
});

test("− and + move the text size one step of the slider, exactly as a drag to the same place does", async () => {
  const page = newPage();
  await signIn(page);
  await page.el("open-settings").click();
  assert.equal(page.el("msg-scale").value, "100");

  await page.el("msg-scale-larger").click();
  assert.equal(scaleOf(page), "1.05", "+ did not make the message text larger by one step");
  assert.equal(page.el("msg-scale").value, "105", "the slider did not follow the button");
  assert.equal(page.storage.get(MSG_SCALE_STORE), "105", "the step was not kept");
  assert.equal(page.el("msg-scale-state").textContent, "Saved — message text at 105% of its usual size.");

  // Everything a drag to 105 does, to the letter: one path, not a second copy of it.
  const dragged = newPage();
  await signIn(dragged);
  await dragged.el("msg-scale").setValue("105");
  for (const [what, read] of [
    ["applied size", scaleOf],
    ["slider", (p) => p.el("msg-scale").value],
    ["stored size", (p) => p.storage.get(MSG_SCALE_STORE)],
    ["line under the slider", (p) => p.el("msg-scale-state").textContent],
  ]) {
    assert.equal(read(page), read(dragged), `a press and a drag to the same size differ in the ${what}`);
  }

  await page.el("msg-scale-smaller").click();
  await page.el("msg-scale-smaller").click();
  assert.equal(scaleOf(page), "0.95", "two presses of − did not make the message text two steps smaller");
  assert.equal(page.el("msg-scale").value, "95");
  assert.equal(page.storage.get(MSG_SCALE_STORE), "95");
  assert.equal(page.el("msg-scale-state").textContent, "Saved — message text at 95% of its usual size.");

  // A reload keeps what the buttons chose.
  const again = newPage(page.storage);
  assert.equal(scaleOf(again), "0.95", "a size chosen with the buttons did not survive a reload");
  assert.equal(again.el("msg-scale").value, "95");

  // And a browser that keeps nothing is owned up to after a press as after a drag.
  page.storage.set = () => page.storage;
  await page.el("msg-scale-larger").click();
  assert.equal(scaleOf(page), "1", "a refused store stopped the step being applied");
  assert.match(page.el("msg-scale-state").textContent, /refused to store the text size/);
});

test("− is disabled at the smallest size and + at the largest, from load on, and neither steps past its end", async () => {
  const page = newPage();
  await signIn(page);
  assert.equal(page.el("msg-scale-smaller").disabled, false, "− is dead at the usual size");
  assert.equal(page.el("msg-scale-larger").disabled, false, "+ is dead at the usual size");

  await page.el("msg-scale").setValue("145");
  await page.el("msg-scale-larger").click();
  assert.equal(scaleOf(page), "1.5");
  assert.equal(page.el("msg-scale-larger").disabled, true, "+ is still live at the largest size");
  assert.equal(page.el("msg-scale-smaller").disabled, false, "− died at the largest size");
  // A browser sends a disabled button no click; were one to arrive, the size still stops at the end.
  await page.el("msg-scale-larger").click();
  assert.equal(scaleOf(page), "1.5", "+ stepped past the largest size");
  assert.equal(page.storage.get(MSG_SCALE_STORE), "150");

  // A drag decides them as well: away from the end, + is live again.
  await page.el("msg-scale").setValue("85");
  assert.equal(page.el("msg-scale-larger").disabled, false, "a drag away from the end left + disabled");
  await page.el("msg-scale-smaller").click();
  assert.equal(scaleOf(page), "0.8");
  assert.equal(page.el("msg-scale-smaller").disabled, true, "− is still live at the smallest size");
  await page.el("msg-scale-smaller").click();
  assert.equal(scaleOf(page), "0.8", "− stepped past the smallest size");
  await page.el("msg-scale").setValue("900");
  assert.equal(page.el("msg-scale-smaller").disabled, false);
  assert.equal(page.el("msg-scale-larger").disabled, true, "a size clamped to the largest left + live");

  // At load, from what was stored, a size from outside the range included.
  for (const [stored, smaller, larger] of [["80", true, false], ["150", false, true], ["900", false, true],
    ["10", true, false], ["100", false, false]]) {
    const loaded = newPage(new Map([[MSG_SCALE_STORE, stored]]));
    assert.equal(loaded.el("msg-scale-smaller").disabled, smaller, `− at load with ${stored} stored`);
    assert.equal(loaded.el("msg-scale-larger").disabled, larger, `+ at load with ${stored} stored`);
  }

  // A size between two marks (a hand-edited 83) steps to the next mark that way, not to 88 or 78.
  for (const [id, applied] of [["msg-scale-larger", "0.85"], ["msg-scale-smaller", "0.8"]]) {
    const between = newPage(new Map([[MSG_SCALE_STORE, "83"]]));
    await signIn(between);
    await between.el(id).click();
    assert.equal(scaleOf(between), applied, `#${id} from 83 did not land on the slider's next mark`);
  }
});

test("a step button disabled under the keyboard hands the focus to the slider; one still live keeps it", async () => {
  const page = newPage();
  await signIn(page);
  await page.el("open-settings").click();
  const slider = page.el("msg-scale");
  const larger = page.el("msg-scale-larger");
  await slider.setValue("140");
  larger.focusFrom("keyboard");
  await larger.click();
  assert.equal(page.document.activeElement, larger, "a press that left + live took the focus off it");
  await larger.click();
  assert.equal(larger.disabled, true);
  assert.equal(page.document.activeElement, slider, "the focus was left on a disabled +, which is nowhere");

  // Safari gives a clicked button no focus. Then there is nothing to hand on, and nothing is taken.
  const smaller = page.el("msg-scale-smaller");
  await slider.setValue("85");
  slider.blur();
  await smaller.click();
  assert.equal(smaller.disabled, true);
  assert.equal(page.document.activeElement, null, "a press with the focus elsewhere moved the focus");
});

test("the − and + are thumb-sized squares that never wrap off the slider's row, with a ring for the keyboard", () => {
  const row = cssBlock(".msg-scale-row");
  assert.match(row, /display:\s*flex/, "the slider and its buttons are not laid out as a row");
  assert.doesNotMatch(row, /flex-wrap:\s*wrap/, "the row may wrap, leaving + under the slider on a phone");
  const slider = cssBlock(".msg-scale-row #msg-scale");
  assert.match(slider, /flex:\s*1 1 auto/, "the slider does not take the room the buttons leave");
  assert.match(slider, /min-width:\s*0/, "the slider cannot shrink, so the row overflows a narrow phone");
  const button = cssBlock(".step-button");
  assert.match(button, /flex:\s*0 0 auto/, "the slider can squeeze a button below its target");
  for (const side of ["width", "height"]) {
    const rem = new RegExp(`(?:^|[\\s;])${side}:\\s*([\\d.]+)rem`).exec(button);
    assert.ok(rem && Number(rem[1]) * 16 >= 44, `the buttons' ${side} is under a 44px target`);
  }
  assert.match(button, /touch-action:\s*manipulation/, "two quick presses can zoom the page instead");
  assert.match(cssBlock(".step-button:focus-visible"), /outline:\s*2px solid var\(--accent\)/, "no ring for the keyboard");
});

// --- `#425 collapsed-lines` ----------------------------------------------------------------------
//
// The owner, 2026-10-22: collapsed cards are 3 lines tall by default; the default stays 3, but it
// should be adjustable to 1 line or 4 lines or whatever, in Settings and by a keystroke not likely
// to be hit accidentally — Cmd+( and Cmd+) — and, the same day, all the way down to ZERO lines,
// header and colour and metadata only, without breaking anything. One count, 0 to 8, stated
// once as `--collapsed-lines` for every list. Kept per device then; since `#457
// settings-server-side` it sits under Messages and the server keeps it for every device. tests/collapsed_lines_browser.py
// measures what a real Chromium draws at 0, 1, 3 and 6.

const COLLAPSED_LINES_STORE = "vibe-talk.voice.collapsed-lines";
const collapsedOf = (page) => page.documentElement.style.getPropertyValue("--collapsed-lines");

test("Collapsed message lines is a Settings slider of 0 to 8 with a − and a +, its ends the script's own", () => {
  const group = settingsGroup("Collapsed message lines");
  const row = /<div class="msg-scale-row">([\s\S]*?)<\/div>/.exec(group);
  assert.ok(row, "the slider and its two buttons are not one row");
  const order = [...row[1].matchAll(/\bid="([^"]+)"/g)].map((m) => m[1]);
  assert.deepEqual(order, ["collapsed-lines-smaller", "collapsed-lines", "collapsed-lines-larger"],
    "not −, slider, + in that order");
  const slider = /<input([^>]*\bid="collapsed-lines"[^>]*)>/.exec(group)[1];
  for (const [attribute, constant] of [["min", "MIN_COLLAPSED_LINES"], ["max", "MAX_COLLAPSED_LINES"]]) {
    const markup = (new RegExp(`\\b${attribute}="(\\d+)"`).exec(slider) || [])[1];
    assert.equal(markup, String(sourceConstant(constant)), `the slider's ${attribute} and ${constant} disagree`);
  }
  assert.match(slider, /\bstep="1"/, "the slider does not step one line at a time");
  assert.equal(sourceConstant("MIN_COLLAPSED_LINES"), 0, "the header alone is not offered");
  assert.equal(sourceConstant("MAX_COLLAPSED_LINES"), 8);
  assert.equal(sourceConstant("DEFAULT_COLLAPSED_LINES"), 3, "the default is not the owner's three");
  for (const [id, name] of [["collapsed-lines-smaller", "Fewer collapsed lines"], ["collapsed-lines-larger", "More collapsed lines"]]) {
    const button = new RegExp(`<button([^>]*\\bid="${id}"[^>]*)>`).exec(group);
    assert.ok(button, `#${id} is not a button`);
    assert.match(button[1], new RegExp(`\\baria-label="${name}"`), `#${id} is not called "${name}"`);
    assert.match(button[1], /\baria-controls="collapsed-lines"/, `#${id} does not say which control it moves`);
  }
});

test("the collapsed-lines count applies at once, is kept, clamps at 0 and 8, and a blank store is not zero", async () => {
  const page = newPage();
  await signIn(page);
  await page.el("open-settings").click();
  assert.equal(collapsedOf(page), "3", "an unconfigured page does not fold at three lines");
  assert.equal(page.documentElement.getAttribute("data-collapsed-lines"), "3");
  assert.equal(page.el("collapsed-lines").value, "3", "the slider does not show what is in force");
  assert.equal(page.el("collapsed-lines-smaller").disabled, false);
  assert.equal(page.el("collapsed-lines-larger").disabled, false);

  await page.el("collapsed-lines").setValue("1");
  assert.equal(collapsedOf(page), "1");
  assert.equal(page.storage.get(COLLAPSED_LINES_STORE), "1", "the count was not kept");
  assert.equal(page.el("collapsed-lines-state").textContent, "Saved — collapsed messages show 1 line.");

  await page.el("collapsed-lines-smaller").click();
  assert.equal(collapsedOf(page), "0", "− at one line did not reach the header alone");
  assert.equal(page.documentElement.getAttribute("data-collapsed-lines"), "0",
    "zero is not stated for the stylesheet, whose clamp of 0 would show every line");
  assert.equal(page.el("collapsed-lines-state").textContent, "Saved — collapsed messages show only their header.");
  assert.equal(page.el("collapsed-lines-smaller").disabled, true, "− is still live at zero");
  await page.el("collapsed-lines-smaller").click();
  assert.equal(collapsedOf(page), "0", "− stepped past zero");

  await page.el("collapsed-lines").setValue("99");
  assert.equal(collapsedOf(page), "8", "an out-of-range count was not clamped");
  assert.equal(page.el("collapsed-lines-larger").disabled, true, "+ is still live at eight");
  await page.el("collapsed-lines-larger").click();
  assert.equal(collapsedOf(page), "8", "+ stepped past eight");
  assert.equal(page.storage.get(COLLAPSED_LINES_STORE), "8");

  // A reload keeps what was chosen, slider and all.
  const again = newPage(page.storage);
  assert.equal(collapsedOf(again), "8", "the count did not survive a reload");
  assert.equal(again.el("collapsed-lines").value, "8");

  // What another build, or a hand, left in the store: clamped, never an error, and a BLANK entry
  // is absent — three, not the zero `Number("")` would make of it.
  for (const [stored, applied] of [["", "3"], ["banana", "3"], ["-4", "0"], ["42", "8"], ["0", "0"], ["5", "5"]]) {
    const loaded = newPage(new Map([[COLLAPSED_LINES_STORE, stored]]));
    assert.equal(collapsedOf(loaded), applied, `a stored ${JSON.stringify(stored)} applied as ${collapsedOf(loaded)}`);
    assert.equal(loaded.el("collapsed-lines").value, applied, `the slider disagrees at a stored ${JSON.stringify(stored)}`);
  }

  // A browser that keeps nothing is owned up to, and the count still applies for the session.
  page.storage.set = () => page.storage;
  await page.el("collapsed-lines").setValue("2");
  assert.equal(collapsedOf(page), "2", "a refused store stopped the count being applied");
  assert.match(page.el("collapsed-lines-state").textContent, /refused to store the collapsed lines/);
});

test("Ctrl+( and Ctrl+) step the collapsed lines and say the count, stop at 0 and 8, and never act in a text field", async () => {
  const page = await keyboardPage();
  const status = () => page.el("status").textContent;
  const fewer = { key: "(", code: "Digit9", ctrlKey: true, shiftKey: true };
  const more = { key: ")", code: "Digit0", ctrlKey: true, shiftKey: true };

  let event = await press(page, page.el("scroll-area"), fewer.key, fewer);
  assert.equal(event.defaulted, false, "Ctrl+( was left to the browser");
  assert.equal(collapsedOf(page), "2", "Ctrl+( did not show one line fewer");
  assert.equal(status(), "Collapsed messages show 2 lines.", "the new count was not said");
  assert.equal(page.storage.get(COLLAPSED_LINES_STORE), "2", "the key's count was not kept");
  assert.equal(page.el("collapsed-lines").value, "2", "the slider did not follow the key");

  // Cmd works as Ctrl does, and the count stops at 0 however often it is pressed — the chord still
  // taken there, so it is never the browser's zoom instead.
  await press(page, page.el("scroll-area"), "(", { code: "Digit9", metaKey: true, shiftKey: true });
  assert.equal(collapsedOf(page), "1");
  await press(page, page.el("scroll-area"), fewer.key, fewer);
  assert.equal(collapsedOf(page), "0");
  assert.equal(status(), "Collapsed messages show only their header.");
  event = await press(page, page.el("scroll-area"), fewer.key, fewer);
  assert.equal(collapsedOf(page), "0", "Ctrl+( stepped past zero");
  assert.equal(event.defaulted, false, "Ctrl+( at zero was left to the browser's zoom");

  for (let i = 0; i < 10; i += 1) await press(page, page.el("scroll-area"), more.key, more);
  assert.equal(collapsedOf(page), "8", "Ctrl+) did not stop at eight");
  assert.equal(status(), "Collapsed messages show 8 lines.");

  // By the place of the key where Shift+9 types no bracket (and where a browser reports the digit
  // under Cmd): the digit on Digit9 with Shift is "(", and on Digit0 is ")"...
  await press(page, page.el("scroll-area"), "9", { code: "Digit9", ctrlKey: true, shiftKey: true });
  assert.equal(collapsedOf(page), "7", "Ctrl+Shift+9 reporting a digit did not step fewer by its place");
  await press(page, page.el("scroll-area"), "0", { code: "Digit0", metaKey: true, shiftKey: true });
  assert.equal(collapsedOf(page), "8", "Cmd+Shift+0 reporting a digit did not step more by its place");
  // ...but a bracket actually typed means itself, wherever it sits: ")" on Digit9 is more.
  await press(page, page.el("scroll-area"), "(", { code: "Digit9", ctrlKey: true, shiftKey: true });
  assert.equal(collapsedOf(page), "7");
  await press(page, page.el("scroll-area"), ")", { code: "Digit9", ctrlKey: true, shiftKey: true });
  assert.equal(collapsedOf(page), "8", "a typed ) on the 9 key was read by its place as ( instead");
  // Without Shift the digit keys are the browser's, untouched.
  event = await press(page, page.el("scroll-area"), "9", { code: "Digit9", ctrlKey: true, shiftKey: false });
  assert.equal(event.defaulted, true, "Ctrl+9 without Shift was taken");
  assert.equal(collapsedOf(page), "8");

  // In a text field the chord is the field's: not taken, nothing stepped, the draft as it was.
  await page.el("channel-compose-text").setValue("a draft (with brackets)");
  for (const field of ["channel-compose-text", "search-field"]) {
    if (field === "search-field") await press(page, page.el("scroll-area"), "/");
    event = await press(page, page.el(field), fewer.key, fewer);
    assert.equal(event.defaulted, true, `Ctrl+( in #${field} was taken from the field`);
    assert.equal(collapsedOf(page), "8", `Ctrl+( in #${field} stepped the lines`);
  }
  assert.equal(page.el("channel-compose-text").value, "a draft (with brackets)", "the chord touched the draft");

  // Both chords are in the ? list, in this keyboard's words, saying what they step and its ends.
  const listed = page.el("keyboard-keys").descendants().filter((node) => node.tagName === "kbd").map((node) => node.textContent);
  assert.ok(listed.includes("Ctrl+("), `Ctrl+( is not in the list of keys: ${listed.join(", ")}`);
  assert.ok(listed.includes("Ctrl+)"), "Ctrl+) is not in the list of keys");
  const said = page.el("keyboard-keys").text();
  assert.match(said, /One line fewer in every collapsed message/, "the list does not say what Ctrl+( does");
  assert.match(said, /One line more/, "the list does not say what Ctrl+) does");
  assert.match(said, /never taken in a text field/, "the list does not say the chords leave a text field's brackets alone");
  assert.equal(page.el("collapsed-lines").getAttribute("aria-keyshortcuts"), "Control+( Meta+( Control+) Meta+)",
    "the slider does not say its keys to a screen reader");
});

test("one count folds every list, thread cards keep a line at 0, and the hidden-word count follows the count in force", async () => {
  // The stylesheet's part, which this fixture can only read: one property everywhere a collapsed
  // row is drawn, the body gone outright at 0, and a card's floor of one line.
  assert.match(cssBlock(".body.clamped"), /-webkit-line-clamp:\s*var\(--collapsed-lines\)/);
  assert.match(cssBlock(':root[data-collapsed-lines="0"] .body.clamped'), /display:\s*none/,
    "at zero the folded body is not gone, only clamped to no clamp");
  assert.match(cssBlock(':root[data-collapsed-lines="0"] .elided'), /display:\s*none/,
    "at zero the hidden-word line still takes a line of its own");
  assert.match(cssBlockOutside(TWO_COLUMN_QUERY, ".side-card-text"), /max\(1, var\(--collapsed-lines\)\)/,
    "a thread card does not keep its one naming line at zero");

  // The page's part: a folded row's "(~N more words)" is against the count in force, recounted
  // where it stands when the count changes — fewer claimed as more lines show, never a word the
  // reader can see.
  const page = newPage();
  await signIn(page);
  const text = wordsOfLength(200, 4);
  const [row] = await showDiscord(page, [message({ id: "4250000000000000001", content: text })]);
  const perLine = wordsPerLine(page, 4, 28.5);
  for (const [value, lines] of [["3", 3], ["1", 1], ["8", 8], ["0", 0], ["3", 3]]) {
    await page.el("collapsed-lines").setValue(value);
    assert.equal(collapsedOf(page), value);
    assert.equal(labelAt(row, PHONE_CLASS), saysHidden(200 - lines * perLine),
      `at ${lines} lines the count under the fold is not against ${lines} lines`);
  }
  // Collapsing and expanding still work at 0: the body keeps its words, only its drawing goes.
  await page.el("collapsed-lines").setValue("0");
  assert.equal(row.getAttribute("data-collapsed"), "true");
  const fold = [...row.descendants()].find((node) => node.hasClass("fold"));
  assert.ok(fold, "a folded row at zero has no way open");
  await fold.click();
  assert.equal(row.getAttribute("data-collapsed"), "false", "a row does not open at zero lines");
  assert.match(textOfRow(row).text(), /nnnn/, "an opened row at zero has lost its words");
  await fold.click();
  assert.equal(row.getAttribute("data-collapsed"), "true", "a row does not fold again at zero lines");
});

// --- `#428 summary-cards-full` -------------------------------------------------------------------
//
// The owner, 2026-10-22: expanding and contracting summaries is a bug, not a feature — a summary is
// already the shortening, eliding it with "…" cuts meaning, and only raw text of unbounded length
// needs collapsed and expanded at all. Wherever a card says a summary it says it whole, at every
// collapsed-lines count; raw cards keep the clamp and the count.

test("a summarised thread card is marked and drawn whole at 0, 1 and 3 collapsed lines; a raw card still clamps", async () => {
  const whole = cssBlockOutside(TWO_COLUMN_QUERY, ".side-card-row[data-summarised-card] > .side-card > .side-card-text");
  assert.match(whole, /-webkit-line-clamp:\s*none/, "a summarised card is still clamped");
  assert.match(whole, /(^|[^-])height:\s*auto/, "a summarised card is still a fixed height");
  assert.match(cssBlockOutside(TWO_COLUMN_QUERY, ".side-card-text"), /-webkit-line-clamp:\s*max\(1, var\(--collapsed-lines\)\)/,
    "a raw card lost the reader's clamp");
  // By id, so a phone's two-line heading cannot cut it either; `cssRules` cannot split the `:is()`.
  assert.match(CSS_CODE,
    /:is\(#thread-card, #side-thread-card\)\[data-summarised-card\]:not\(\[data-folded\]\) > \.side-card > \.side-card-text \{[^}]*-webkit-line-clamp:\s*none/,
    "the summarised card at the top of a thread is still clamped");

  const LONG_SUMMARY = "Overnight runner repaired and the lane is green again after two manual " +
    "restarts, with the upload retry, the canary hold, the rollback drill timing and the " +
    "timezone follow-up all filed";
  const { page, data } = await twoColumnServer((_page, held) => {
    Object.assign(held.threads[0], { display_name: "runner-repaired", summary: LONG_SUMMARY });
  });
  const [first, second] = data.threads;
  const cardOf = (id) => page.el("side-cards").children.find((li) => li.getAttribute("data-id") === id);
  assert.equal(cardPart(page, first.id, "side-card-text"), `runner-repaired: ${LONG_SUMMARY}`,
    "the card does not say the whole summary");
  for (const value of ["0", "1", "3"]) {
    await page.el("collapsed-lines").setValue(value);
    assert.equal(cardOf(first.id).hasAttribute("data-summarised-card"), true,
      `at ${value} lines the summarised card is not marked whole`);
    assert.equal(cardOf(second.id).hasAttribute("data-summarised-card"), false,
      `at ${value} lines a raw card is marked whole`);
    assert.equal(cardPart(page, first.id, "side-card-text"), `runner-repaired: ${LONG_SUMMARY}`,
      `at ${value} lines the summary on the card changed`);
  }
  // Into the summarised thread: its heading card is the summary whole too, and the raw thread's is not.
  await openerOf(cardRow(page, first.id)).click();
  await page.settle();
  assert.equal(page.el("side-thread-card").hasAttribute("data-summarised-card"), true,
    "the card at the top of a summarised thread is not marked whole");
  assert.equal(headerCard(page).text, `runner-repaired: ${LONG_SUMMARY}`);
  await page.el("side-close").click();
  await page.settle();
  await openerOf(cardRow(page, second.id)).click();
  await page.settle();
  assert.equal(page.el("side-thread-card").hasAttribute("data-summarised-card"), false,
    "the card at the top of a raw thread is marked whole");
});

test("a summarised row's summary is whole at 0, 1 and 3 collapsed lines, and More still opens the raw message", async () => {
  const line = cssBlock(".summary-text");
  assert.doesNotMatch(line, /line-clamp/, "a row's summary is clamped");
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [
    message({ id: "4280000000000000001", content: longMessage("overnight") }),
  ]);
  await turnSummariesOn(page);
  const row = rows[0];
  for (const value of ["0", "1", "3"]) {
    await page.el("collapsed-lines").setValue(value);
    assert.equal(summaryLine(row).hidden, false, `at ${value} lines the summary is not on screen`);
    assert.equal(summaryText(row), "a short line about 4280000000000000001",
      `at ${value} lines the summary is not whole`);
    assert.equal(bodyOf(row).hidden, true, `at ${value} lines the raw message shows under its summary`);
  }
  await foldButton(row).click();
  assert.equal(bodyOf(row).hidden, false, "More no longer opens the raw message behind a summary");
  assert.match(bodyOf(row).text(), /overnight/, "the opened row is not the original message");
});

test("a body the server rendered is drawn as it was sent: paragraphs, one list, its items whole", async () => {
  const page = newPage();
  await signIn(page);
  const before = page.templatesParsed;
  const rows = await showDiscord(page, [message({ id: "7100000000000000001", ...OWNERS_MESSAGE })]);
  const [box] = mdBoxes(rows[0]);
  assert.ok(box, "the row's body holds no .md box");
  assert.equal(page.templatesParsed - before, 1, "the rendered body was not inserted through the template");
  assert.deepStrictEqual(box.children.filter((kid) => kid.nodeType === 1).map((kid) => kid.tagName),
    ["p", "ul", "p", "p"], "the blocks are not the server's blocks");
  const items = tagged(box, "li");
  assert.equal(items.length, 6, "six items, one list");
  assert.equal(tagged(box, "ul").length, 1, "the items were split into several lists");
  // A continuation line stays INSIDE its item, after a line break.
  assert.equal(items[1].text(), "the flaky test is quarantined\nand has an issue filed against it");
  assert.equal(tagged(items[1], "br").length, 1);
  // ...and no Markdown is left on screen.
  assert.doesNotMatch(rows[0].text(), /^- /m, "a list marker was drawn as a dash");
});

test("a message without content_html is drawn from its text, as plain paragraphs and never as markup", async () => {
  // Most chat — the server sends no HTML when rendering would change nothing — and every message
  // saved on a device before the field existed. Either way the text is TEXT: Markdown stays the
  // characters it is, and so does HTML.
  const page = newPage();
  await signIn(page);
  const HOSTILE =
    '<script>alert("xss")</script> <img src=x onerror=alert(1)> ' +
    '<iframe src="javascript:alert(2)"></iframe> &lt;already escaped&gt;';
  const before = page.createdTags.length;
  const templates = page.templatesParsed;
  const rows = await showDiscord(page, [
    message({ id: "7100000000000000002", content: `${HOSTILE}\n**not bold**\n\n- not a list` }),
  ]);
  const [box] = mdBoxes(rows[0]);
  assert.equal(page.templatesParsed, templates, "text with no content_html reached the HTML sink");
  assert.deepStrictEqual(box.children.map((kid) => kid.tagName), ["p", "p"], "a blank line is the only paragraph break");
  const [first, second] = box.children;
  assert.equal(first.text(), `${HOSTILE}**not bold**`, "the text was not drawn verbatim");
  assert.equal(tagged(first, "br").length, 1, "the single newline is not a line break");
  assert.equal(second.text(), "- not a list");
  assert.ok(box.text().includes("&lt;already escaped&gt;"), "text that was already escaped was unescaped");
  const created = page.createdTags.slice(before);
  for (const tag of ["script", "img", "iframe", "style", "object", "embed", "link", "strong", "ul"]) {
    assert.ok(!created.includes(tag), `drawing a message's text created a <${tag}>`);
  }
  for (const node of rows[0].descendants()) {
    for (const name of node.attributes.keys()) {
      assert.doesNotMatch(name, /^on/i, `an ${name} attribute was set from channel text`);
      assert.notEqual(name, "src");
    }
  }
  // The reply control carries nothing from the message, in its text or in any attribute.
  const reply = replyButton(rows[0]);
  assert.equal(reply.textContent, "Reply");
  for (const value of reply.attributes.values()) {
    assert.ok(!value.includes("<script>") && !value.includes("onerror"), `channel text reached ${value}`);
  }
});

test("the page has ONE html sink, and only a message's content_html ever reaches it", async () => {
  // Static: one assignment to innerHTML in the whole script, inside `insertSanitizedHtml`, and that
  // function is called from one place, with what `renderedHtmlOf` read off a message. None of the
  // other ways to turn a string into markup appears at all.
  assert.equal((PAGE_CODE.match(/innerHTML/g) || []).length, 1, "a second innerHTML sink appeared");
  const sink = /function insertSanitizedHtml\(parent, contentHtml\) \{([\s\S]*?)\n\}/.exec(SCRIPT_CODE);
  assert.ok(sink && /template\.innerHTML = contentHtml;/.test(sink[1]), "innerHTML moved out of insertSanitizedHtml");
  const calls = [...SCRIPT_CODE.matchAll(/insertSanitizedHtml\(([^)]*)\)/g)].map((m) => m[1]);
  assert.deepStrictEqual(calls, ["parent, contentHtml", "box, html"], "insertSanitizedHtml gained a caller");
  assert.match(SCRIPT_CODE, /const html = renderedHtmlOf\(message\);\s*if \(html !== ""\) \{\s*insertSanitizedHtml\(box, html\);/,
    "the one caller no longer passes a message's content_html");
  assert.match(SCRIPT_CODE, /const renderedHtmlOf = \(message\) =>\s*message && typeof message\.content_html === "string" \? message\.content_html : "";/);
  assert.doesNotMatch(PAGE_CODE,
    /outerHTML|insertAdjacentHTML|document\.write|createContextualFragment|DOMParser|srcdoc/);

  // Dynamic: every string the page parses is some message's content_html, and never its text.
  const page = newPage();
  await signIn(page);
  const parsed = [];
  page.onParse = (html) => parsed.push(html);
  const messages = [
    message({ id: "7100000000000000003", content: "**bold** <b>x</b>", content_html: "<p><strong>bold</strong> &lt;b&gt;x&lt;/b&gt;</p>\n" }),
    message({ id: "7100000000000000004", content: "<i>plain</i>" }),
    message({ id: "7100000000000000005", author: "someone else", author_id: "1000000000000000005",
      content: "see https://example.com/x", content_html: `<p>see <a href="https://example.com/x" ${LINK_ATTRS}>https://example.com/x</a></p>\n` }),
  ];
  await showDiscord(page, messages);
  assert.ok(parsed.length >= 2, "the rendered bodies were not inserted");
  for (const html of parsed) {
    assert.ok(messages.some((m) => m.content_html === html), `the page parsed something that is no content_html: ${html}`);
    assert.ok(!messages.some((m) => m.content === html), `the page parsed a message's raw text: ${html}`);
  }
});

test("a rendered body keeps its links, mentions and code, each as the element the server sent", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [message({
    id: "7100000000000000006",
    content: "**bold** *it* `code` <@123> [the fix](https://example.com/pull/1)",
    content_html: '<p><strong>bold</strong> <em>it</em> <code>code</code> <span class="mention">@123</span> ' +
      `<a href="https://example.com/pull/1" ${LINK_ATTRS}>the fix</a></p>\n`,
  })]);
  const box = mdBoxes(rows[0])[0];
  const of = (tag) => tagged(box, tag).map((node) => node.text());
  assert.deepStrictEqual(of("strong"), ["bold"]);
  assert.deepStrictEqual(of("em"), ["it"]);
  assert.deepStrictEqual(of("code"), ["code"]);
  const [chip] = tagged(box, "span");
  assert.equal(chip.className, "mention");
  assert.equal(chip.text(), "@123", "a mention is the id it is, not an invented name");
  const [link] = tagged(box, "a");
  assert.equal(link.getAttribute("href"), "https://example.com/pull/1");
  assert.equal(link.getAttribute("target"), "_blank");
  assert.match(link.getAttribute("rel"), /noopener/);
});

test("the fixture refuses anything the server's sanitizer would not send, so no test can certify it", () => {
  // The guard on every test above: a body with a <script>, an event handler or a javascript: link
  // is an ERROR here, never a rendered row. Read from src/render.rs, so the two cannot drift.
  assert.ok(SANITIZED_TAGS.has("ul") && SANITIZED_TAGS.has("span") && !SANITIZED_TAGS.has("script"));
  for (const html of [
    "<script>alert(1)</script>",
    "<img src=\"x\">",
    "<p onclick=\"x()\">a</p>",
    "<p style=\"color:red\">a</p>",
    "<a href=\"javascript:alert(1)\">a</a>",
    "<a href=\"/relative\">a</a>",
    "<p>unclosed",
    "<p>a</em>",
  ]) {
    assert.throws(() => parseSanitizedHtml(html, new FakeElement("", "#document-fragment"), () => {}),
      undefined, `the fixture accepted ${html}`);
  }
});

test("search matches what a rendered row SAYS and the addresses it links to, not its Markdown", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [message({
    id: "7100000000000000007",
    content: "re**deploy** done, see [the log](https://example.com/logs/42) <@123>",
    content_html: '<p>re<strong>deploy</strong> done, see ' +
      `<a href="https://example.com/logs/42" ${LINK_ATTRS}>the log</a> <span class="mention">@123</span></p>\n`,
  })]);
  const said = rows[0].getAttribute("data-search");
  assert.ok(said.includes("redeploy done"), `a word split by emphasis is one word on screen: ${said}`);
  assert.ok(said.includes("@123"), "a mention is found by the id it is drawn as");
  assert.ok(said.includes("https://example.com/logs/42"), "a link's address no longer finds its row");
  assert.ok(!said.includes("**") && !said.includes("<@"), `the Markdown source was searched: ${said}`);
});

test("the Links view lists the links a rendered row draws, by the names it draws them with", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [message({
    id: "7100000000000000008",
    // The source names a refused link too; the server drew it as text, so it is not a link.
    content: "[the doc](https://example.com/doc) and [bad](javascript:alert(1)) and https://example.com/raw",
    content_html: `<p><a href="https://example.com/doc" ${LINK_ATTRS}>the doc</a> and bad (javascript:alert(1)) and ` +
      `<a href="https://example.com/raw" ${LINK_ATTRS}>https://example.com/raw</a></p>\n`,
  })]);
  await showLinks(page);
  assert.deepStrictEqual(linkPairs(rows[0]),
    [["the doc", "https://example.com/doc"], ["https://example.com/raw", "https://example.com/raw"]]);
});

test("a GitHub address the server drew with a short label is listed by that label, and its kind is still its address's", async () => {
  // `#227 github-link-abbrev`. The label is the server's (src/link_labels.rs), and the page reads it
  // from the anchor the server drew, as it reads a Markdown link's name: there are no rules for it
  // here to drift from the server's. What a link IS — the PRs, Commits and Actions toggles — is
  // still read from where it goes.
  const page = newPage();
  await signIn(page);
  const pull = "https://github.com/octo/widgets/pull/12";
  const doc = "https://example.com/design";
  const rows = await showDiscord(page, [message({
    id: "7100000000000000010",
    content: `landed ${pull}, see ${doc}`,
    content_html: `<p>landed <a href="${pull}" title="${pull}" ${LINK_ATTRS}>widgets#12</a>, see ` +
      `<a href="${doc}" ${LINK_ATTRS}>${doc}</a></p>\n`,
  })]);
  const [link] = tagged(mdBoxes(rows[0])[0], "a");
  assert.equal(link.text(), "widgets#12");
  assert.equal(link.getAttribute("href"), pull, "the label moved the link");
  assert.equal(link.getAttribute("title"), pull, "a pointer resting on the label is not told the address");
  await showLinks(page);
  assert.deepStrictEqual(linkPairs(rows[0]), [["widgets#12", pull], [doc, doc]]);
  await tapKind(page, "pr");
  assert.deepStrictEqual(linkPairs(rows[0]), [[doc, doc]], "the PRs toggle read the label rather than the address");
});

test("the fold measures what a row draws, and Copy text still copies what was written", async () => {
  const page = newPage();
  await signIn(page);
  // Long in the source — a long address behind a short name — and short on screen.
  const address = `https://example.com/${"a".repeat(COLLAPSE_OVER_CHARS)}`;
  const rows = await showDiscord(page, [message({
    id: "7100000000000000009",
    content: `see [the summary](${address}) for **all** of it`,
    content_html: `<p>see <a href="${address}" ${LINK_ATTRS}>the summary</a> for <strong>all</strong> of it</p>\n`,
  })]);
  assert.ok(rows[0].messages[0].content.length > COLLAPSE_OVER_CHARS, "the fixture's source is not long");
  assert.equal(foldButton(rows[0]), undefined, "a row that draws one short line was folded for its source");
  await rowMoreButton(rows[0]).click();
  await rows[0].descendants().find((node) => node.className === "row-copy-button").click();
  await page.settle();
  assert.equal(page.clipboard, `see [the summary](${address}) for **all** of it`, "Copy text lost the source");
});

test("a combined row draws each of its messages as its own block", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [
    message({ id: "8000000000000000300", content: "first half with a list:\n- one",
      content_html: "<p>first half with a list:</p>\n<ul>\n<li>one</li>\n</ul>\n", timestamp: "2026-08-19T04:31:00.000Z" }),
    message({ id: "8000000000000000301", content: "second half", timestamp: "2026-08-19T04:31:01.000Z" }),
  ]);
  assert.equal(rows.length, 1, "the fixture did not produce one combined row");
  const boxes = mdBoxes(rows[0]);
  assert.equal(boxes.length, 2, "the parts are not one block each");
  assert.equal(tagged(boxes[0], "li").length, 1);
  assert.equal(boxes[1].text(), "second half");
});

test("a thread is named by what its first message says, not by its Markdown", async () => {
  // `#194 thread-picker-polish`'s rule, the first line of the first message, read from the row's
  // drawn text: a root that opens with `**Release plan**` is "Release plan".
  const root = message({ content: "**Release plan**\nthe rest", content_html: "<p><strong>Release plan</strong><br>\nthe rest</p>\n" });
  const threadName = newPage().pure("threadName");
  assert.equal(threadName({ summary: null, root }), "Release plan");
  assert.equal(threadName({ summary: null, root: message({ content: "- plain text root" }) }), "- plain text root");
});

test("message text is styled as one block family: bullets, hanging indent, paragraph gaps, headings", () => {
  // Layout itself is the screenshot walk's to judge (`41-message-blocks`); what can be held here is
  // that the rules exist on the selectors that need them.
  assert.match(cssBlock(".md"), /white-space:\s*normal/, "the server's newlines between blocks would draw as lines");
  assert.match(cssBlock(".md ul"), /list-style-type:\s*disc/, "a list has no round bullets");
  assert.match(cssBlock(".md ul ul"), /list-style-type:\s*circle/);
  assert.match(cssBlock(".md ul ul ul"), /list-style-type:\s*square/, "a third level reuses the second's marker");
  // `#237 nested-lists`: a numbered list is a level too, as in a browser's own sheet, so the steps
  // under a numbered item are circles and theirs squares; and its gutter holds `10.`.
  assert.match(cssBlock(".md ol ul"), /list-style-type:\s*circle/, "steps under a number keep the top level's disc");
  for (const third of [".md ol ul ul", ".md ul ol ul", ".md ol ol ul"]) {
    assert.match(cssBlock(third), /list-style-type:\s*square/, `${third} reuses the second level's marker`);
  }
  assert.match(cssBlock(".md ol"), /padding-left:\s*2em/, "a two-digit number would stick out of its gutter");
  // An item is not a row: the base sheet's `.messages li` panel must not reach it.
  assert.match(cssRules(BASE_CSS, ".messages li").join("\n"), /padding:/, "the row rule this resets moved");
  const item = cssBlock(".md li");
  for (const reset of [/border:\s*0/, /padding:\s*0/, /background:\s*none/, /margin:\s*0/]) {
    assert.match(item, reset, "a bullet point would be drawn as a row's card");
  }
  const indent = cssRules(CSS, ".md :is(ul").join("\n");
  assert.match(indent, /padding-left:\s*1\.35em/, "a list has no room for its markers");
  assert.match(indent, /list-style-position:\s*outside/, "a wrapped item would start under its bullet");
  assert.match(cssBlock(".md > * + *"), /margin-top:\s*0\.8em/, "a blank line makes no gap");
  // `markdown-tables-render`: a heading is set further from the part it ends than from its own.
  assert.match(cssRules(CSS, ".md > * + :is(h1").join("\n"), /margin-top:\s*1\.15em/);
  assert.match(cssRules(CSS, ".md > :is(h1").join("\n"), /margin-top:\s*0\.35em/);
  assert.match(cssBlock(".md h1"), /font-size:\s*1\.25em/);
  // `#531 server-side-syntax-highlight`: an open message's code keeps its lines and scrolls sideways;
  // a folded one's wraps, so the clamp can count and cut its lines.
  assert.match(cssBlock(".md pre"), /white-space:\s*pre;/, "a code block lost its own spacing");
  assert.match(cssBlock(".md pre"), /overflow-x:\s*auto/, "a long line of code would widen the row");
  assert.match(cssBlock(".body.clamped pre"), /white-space:\s*pre-wrap/, "a folded code block would escape its clamp");
  assert.match(cssBlock(".body.clamped pre"), /overflow-x:\s*visible/, "a folded code block is a box the clamp draws whole");
  assert.match(cssBlock(".md table"), /overflow-x:\s*auto/, "a wide table would widen the row");
  assert.match(cssBlock(".md .mention"), /color:\s*var\(--accent\)/);
});

test("a message with no content at all renders without throwing, with empty messages shown", async () => {
  const page = newPage(new Map([["vibe-talk.voice.hide-empty", "0"]]));
  await signIn(page);
  const lines = await showDiscord(page, [message({ content: "" }), message({ content: null })]);
  assert.equal(lines.length, 2);
});

test("Refresh re-reads the channel rather than appending to it", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "1", content: "one" })]);

  page.messages = [message({ id: "2", content: "two" })];
  await reReadChannel(page);

  const lines = page.el("discord-log").children;
  assert.equal(lines.length, 1, "Refresh appended instead of replacing");
  assert.match(lines[0].text(), /two/);
});

test("the channel view opens on the NEWEST message, not the top", async () => {
  const page = newPage();
  await signIn(page);
  const area = page.el("scroll-area");
  area.scrollTop = 0;

  await showDiscord(page, tallChannel());

  assert.ok(
    area.scrollHeight > area.clientHeight * 2,
    "the channel does not overflow, so opening at the bottom would prove nothing"
  );
  assert.ok(atBottomOf(area), "the channel opened at the top of the history");
});

test("AND THE NEWEST MESSAGE IS REALLY ON THE SCREEN WHEN IT GETS THERE", async () => {
  // `#74 scroll-test-strength`, finding 1, at its root. The page pins itself to the newest line
  // with `scrollTop = scrollHeight`, which is a value NO BROWSER ACCEPTS: it clamps to
  // `scrollHeight - clientHeight`. A fixture that took that assignment at its word modelled "at
  // the newest line" as a whole viewport PAST the last row — so the row the reader is supposedly
  // looking at was off the top of the screen, and every question this suite asks about following
  // the newest line was being answered about a position that cannot exist.
  //
  // The clamp on `scrollTop` is what fixes that, and this is what can see it: the assertion is
  // about pixels the reader can look at, not about a number the page set.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, tallChannel());
  const area = page.el("scroll-area");
  assert.ok(area.scrollHeight > area.clientHeight * 2, "the channel does not overflow");

  const furthest = area.scrollHeight - area.clientHeight;
  assert.ok(
    area.scrollTop <= furthest,
    `the page is scrolled to ${area.scrollTop}, past the ${furthest} a browser would allow`
  );
  const rows = page.el("discord-log").children;
  const newest = rows[rows.length - 1];
  const rect = newest.getBoundingClientRect();
  assert.ok(
    rect.bottom > 0 && rect.top < area.clientHeight,
    `the newest row occupies ${rect.top}..${rect.bottom} of a 0..${area.clientHeight} viewport, ` +
      "so the reader was taken somewhere it cannot be seen"
  );
});

test("the fixture carries a scroll position for the elements the page scrolls, and for no other", async () => {
  // The docstring on `FakeElement.scrollTop` used to say that the clamp put every other element at
  // zero "the way the real thing does". It did not: the ceiling is `scrollHeight - clientHeight`,
  // and every element but #scroll-area has a clientHeight of zero and a content height that is
  // not — so the ceiling was the whole content and `#discord-log` could hold a position of 500.
  // Another track resolving a conflict here would have been relying on a claim the code did not
  // make, which is why this is a checked property now and not a sentence.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, tallChannel());
  const log = page.el("discord-log");
  assert.ok(log.scrollHeight > 500, "the log is too short for this to be asking anything");
  log.scrollTop = 500;
  assert.equal(log.scrollTop, 0, "an element the page never scrolls is holding a scroll position");
  // ...and the ones that do scroll still clamp to the range a browser would allow, rather than
  // to zero. Both halves, because either alone is satisfiable by breaking the other.
  const area = page.el("scroll-area");
  area.scrollTop = area.scrollHeight * 2;
  assert.equal(area.scrollTop, area.scrollHeight - area.clientHeight);

  // WHY the model is allowed only these: the stylesheet's other scrolling elements are outside the
  // layout tree this fixture models, so nothing here has to have an opinion about them. Computed
  // from web/voice.css rather than listed, so giving #discord-log its own overflow fails here.
  const scrollers = new Set();
  for (const rule of CSS_CODE.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    if (!/overflow(-y)?:\s*(auto|scroll)/.test(rule[2])) {
      continue;
    }
    for (const selector of rule[1].split(",").map((part) => part.split("\n").pop().trim())) {
      if (selector) scrollers.add(selector);
    }
  }
  for (const id of SCROLLERS) {
    assert.ok(scrollers.has(`#${id}`), `#${id}, which this model scrolls, no longer scrolls`);
  }
  const modelled = new Set(
    Object.keys(FIXTURE_TREE).concat(...Object.values(FIXTURE_TREE))
  );
  for (const selector of scrollers) {
    const id = selector.replace(/^#/, "");
    assert.ok(
      SCROLLERS.has(id) || !modelled.has(id),
      `${selector} scrolls in web/voice.css AND is inside the fixture's layout tree, so the ` +
        "model's scrolling boxes have stopped being the page's"
    );
  }
});

test("EVERY visit to the channel view opens on the newest message, not just the first", async () => {
  // The regression this guards: the only scroll-to-bottom used to live in loadDiscord(), which
  // runs solely on the FIRST switch. Both panes share one scroll container, so every later
  // switch inherited whatever scrollTop the voice pane had left behind — the top, usually.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, tallChannel());

  await page.el("view-switch").click(); // back to the voice transcript
  await page.settle();
  assert.equal(page.tab(), "voice");

  const area = page.el("scroll-area");
  area.scrollTop = 0; // the reader scrolled up in the voice pane

  await page.el("view-switch").click(); // and returns to the channel: no reload, the log is populated
  await page.settle();

  assert.equal(page.tab(), "discord");
  assert.ok(
    area.scrollHeight > area.clientHeight * 2,
    "the channel does not overflow, so opening at the bottom would prove nothing"
  );
  assert.ok(atBottomOf(area), "the second visit to the channel opened at the top");
});

// --- summaries, asked for as you scroll ---------------------------------------------------------
//
// `#49 cached-summaries`. The server half landed on its own with no caller at all: an endpoint, a
// cache, a versioned key and a retention sweep, and nothing on any screen that would ever produce
// a request. So every test below is about the BROWSER half, and almost every one of them is about
// how many requests a reader's scrolling really costs — which is what the issue is about, and
// what a test asserting "the summary appeared" would say nothing at all about.

/** The summary line on a rendered channel row, or undefined when it has none. */
const summaryLine = (li) => li.descendants().find((node) => node.className === "summary");

/** ...and the mark that labels it, which is the word a failed row changes. */
const summaryMark = (li) =>
  li.descendants().find((node) => node.className === "summary-mark").textContent;

/**
 * Let the page report the summary failures it has been holding.
 *
 * Failures are deliberately NOT reported one at a time — twenty rows failing is one counted
 * sentence, not twenty racing writes of "one message" — so `summaryFailed` records and schedules,
 * and a zero-delay timer does the drawing and the saying. The fixture's timers are the test's to
 * fire, which is what makes the coalescing observable rather than merely hoped for.
 */
async function reportFailures(page) {
  page.expireTimers(0);
  await page.settle();
}

/**
 * The row the page's own anchoring would hold steady: the first one not scrolled off the top.
 *
 * The same rule `scrollAnchor` uses in web/voice.js, restated rather than reached into, because
 * that is the row a claim about "the reader did not move" is ABOUT. Any other row is allowed to
 * shift when the heights above it change, which is exactly what entering summary mode does.
 */
function anchorRow(page) {
  const edge = page.el("scroll-area").getBoundingClientRect().top;
  return [...page.el("discord-log").children].find(
    (li) => li.getBoundingClientRect().bottom > edge
  );
}

test("a server that cannot summarise disables the control up front instead of failing every row", async () => {
  const page = newPage();
  page.summariesUnavailable = "elevenlabs.api_key is not configured";
  await signIn(page);
  await showDiscord(page, [message({ id: "7000000000000000002", content: longMessage("deploy") })]);

  const chip = page.el("summarise");
  assert.equal(chip.hidden, false, "the control vanished rather than saying why it is unavailable");
  assert.equal(chip.disabled, true, "a server without a summariser offered summaries");
  assert.match(chip.title, /elevenlabs\.api_key is not configured/);
  await turnSummariesOn(page);
  assert.equal(chip.getAttribute("aria-pressed"), "false", "the disabled control entered summary mode");
  assert.deepStrictEqual(page.summaryAsks, [], "a disabled control still asked for summaries");
});

test("nothing is summarised until the reader asks for it, and then the long rows are", async () => {
  // The default costs NOTHING. Collapsing to a prefix is free and stays the default; a page that
  // summarised on arrival would spend a call on every long message in a channel the reader is
  // only glancing at.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [
    message({ id: "7000000000000000001", content: SHORT_MESSAGE }),
    message({ id: "7000000000000000002", content: longMessage("deploy") }),
  ]);

  assert.deepStrictEqual(page.summaryAsks, [], "the page summarised without being asked to");
  assert.equal(page.el("summary-note").hidden, true, "the mode announced itself while off");

  // THE CONTROL. The same page, the same two messages, the mode ON — and now exactly one of them
  // is asked about. Without this the assertion above is satisfied by a page that never summarises.
  await turnSummariesOn(page);
  assert.deepStrictEqual(
    page.summaryAsks.map((ask) => ask.message),
    ["7000000000000000002"],
    "summary mode asked about the wrong set of messages"
  );
});

test("a message short enough to read is never sent for summarising, however much you scroll", async () => {
  // The issue's first requirement, and it is answered by the page's OWN definition of short --
  // `COLLAPSE_OVER_CHARS`, the one `#47 scrollback-stability` folds by. A second threshold here
  // would be a second answer to "is this message long", and the two would drift.
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [
    message({ id: "7000000000000000010", content: "x".repeat(COLLAPSE_OVER_CHARS) }),
    message({ id: "7000000000000000011", content: "x".repeat(COLLAPSE_OVER_CHARS + 1) }),
  ]);
  await turnSummariesOn(page);
  const area = page.el("scroll-area");
  for (const top of [0, 100, 200, 0]) {
    area.scrollTop = top;
    await area.dispatch("scroll");
  }
  await page.settle();

  assert.equal(asksFor(page, "7000000000000000010"), 0, "a message at the threshold was summarised");
  assert.equal(summaryLine(rows[0]), undefined, "a short row was given a summary line to fill");
  // THE CONTROL, one character longer through the same code path on the same page.
  assert.equal(asksFor(page, "7000000000000000011"), 1, "a message over the threshold was not summarised");
});

test("scrolling past a message asks for its summary ONCE, not once per scroll event", async () => {
  // A phone produces a lot of scroll events, and this is where a per-event request would show up
  // as a bill rather than as a bug.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, tallChannel());
  await turnSummariesOn(page);
  const area = page.el("scroll-area");

  const before = page.summaryAsks.length;
  assert.ok(before > 0, "nothing was asked about at all, so 'exactly once' is vacuous");
  for (let i = 0; i < 20; i += 1) {
    area.scrollTop = area.scrollHeight - i;
    await area.dispatch("scroll");
  }
  await page.settle();

  // PER MESSAGE, NOT PER EVENT — which is the actual claim, and the one that costs money when it
  // breaks. This used to be written as "the total did not change", which was a proxy that held
  // only because entering the mode instantly shrank every foldable row and so asked about all of
  // them at once. Rows now keep their height until their own summary arrives, so the reachable set
  // grows as they shrink and a scroll to the bottom can legitimately reveal one more row. That is
  // a row being asked about for the first time, not a row being asked about again, and conflating
  // the two would forbid the lookahead from working at all.
  assert.equal(
    page.summaryAsks.length,
    new Set(page.summaryAsks.map((ask) => ask.message)).size,
    "some message was asked about more than once across twenty scroll events"
  );
  for (const id of new Set(page.summaryAsks.map((ask) => ask.message))) {
    assert.equal(asksFor(page, id), 1, `message ${id} was asked about more than once`);
  }
  assert.ok(
    page.summaryAsks.length < 20,
    `twenty scroll events produced ${page.summaryAsks.length} requests, which is per-event`
  );
});

test("the one-ask record is written BEFORE the request, not when the answer comes back", async () => {
  // `#49 cached-summaries`' headline property, and the ONE the test above cannot see: its fixture
  // answers immediately, so every response lands between two scroll events and a record written in
  // the response handler looks exactly like a record written before the await. Here the answers are
  // HELD, which is what a phone on a slow connection does — twenty scroll events all fire while the
  // first request is still in flight.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, tallChannel());

  let release = null;
  const held = new Promise((resolve) => {
    release = resolve;
  });
  const answer = page.summaryResponse;
  page.summaryResponse = async (id) => {
    await held;
    return answer(id);
  };

  await turnSummariesOn(page);
  const inFlight = page.summaryAsks.length;
  assert.ok(inFlight > 0, "nothing was asked about at all, so 'exactly once' is vacuous");

  const area = page.el("scroll-area");
  for (let i = 0; i < 20; i += 1) {
    area.scrollTop = area.scrollHeight - i;
    await area.dispatch("scroll");
  }
  await page.settle();
  assert.equal(
    page.summaryAsks.length,
    inFlight,
    "twenty scroll events over rows whose answers had not come back yet issued fresh requests: " +
      "the one-ask record is being written in the response handler, so it is not written at all " +
      "until the response lands"
  );

  // ...and the answers landing does not open a second window in which the same rows can be asked
  // about again.
  release();
  await page.settle();
  area.scrollTop = area.scrollHeight;
  await area.dispatch("scroll");
  await page.settle();
  for (const id of new Set(page.summaryAsks.map((ask) => ask.message))) {
    assert.equal(asksFor(page, id), 1, `message ${id} was asked about more than once`);
  }
});

test("only the rows near the viewport are summarised, and reaching the others summarises those", async () => {
  // "Produced on demand as the reader scrolls" is the whole cost argument: nothing is spent on
  // messages nobody looks at. A page that summarised the loaded window on entering the mode would
  // pass every other test here and lose the argument.
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, tallChannel(20));
  await turnSummariesOn(page);

  const oldest = rows[0].getAttribute("data-id");
  const newest = rows[rows.length - 1].getAttribute("data-id");
  assert.equal(asksFor(page, newest), 1, "the row the reader is looking at was not summarised");
  assert.equal(asksFor(page, oldest), 0, "a row far off screen was summarised anyway");
  assert.ok(
    page.summaryAsks.length < rows.length,
    `entering the mode asked about all ${rows.length} loaded rows, not the ones on screen`
  );

  const area = page.el("scroll-area");
  area.scrollTop = 0;
  await area.dispatch("scroll");
  await page.settle();
  assert.equal(asksFor(page, oldest), 1, "scrolling to a row did not ask for its summary");
});

test("a summarised row shows the summary in place of its opening lines, and More brings the message back", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [
    message({ id: "7000000000000000020", content: longMessage("overnight") }),
  ]);
  await turnSummariesOn(page);

  const row = rows[0];
  assert.equal(row.getAttribute("data-collapsed"), "true", "a long row did not arrive folded");
  assert.equal(summaryLine(row).hidden, false, "the summary is not on screen");
  assert.equal(bodyOf(row).hidden, true, "the summary is stacked on top of the clamped message");
  // `#244 elision-word-count`. The count is about the clamped lines, which the summary replaced.
  assert.equal(elidedOf(row).hidden, true, "a summarised row still counts the words of lines it hides");
  assert.equal(summaryText(row), "a short line about 7000000000000000020");
  assert.match(row.text(), /summary/, "nothing on the row says this line is not the message");

  // ...and the message itself is one tap away, unchanged. A summary that could not be checked
  // against what was actually said would be worse than no summary.
  await foldButton(row).click();
  assert.equal(summaryLine(row).hidden, true, "the summary stayed up over the opened message");
  assert.equal(bodyOf(row).hidden, false, "opening a summarised row showed nothing");
  assert.match(bodyOf(row).text(), /overnight/, "the opened row is not the original message");
  assert.equal(elidedOf(row).hidden, true, "an opened message still says words are hidden");
});

test("PRESSING SUMMARISE CHANGES NOTHING UNTIL A SUMMARY ACTUALLY ARRIVES", async () => {
  // THE REPORTED BUG. Turning the mode on used to swap EVERY foldable row the instant it was
  // pressed, before one answer had come back: the rendered markdown body was hidden and a bare
  // "summarising…" put where it had been. On a phone that reads as "the control broke rendering
  // across the whole view", and it is worse now than it was when the summariser was a local
  // truncation: every one of those rows is waiting on a vendor round trip, so the empty state
  // lasts seconds rather than none. A row with nothing to show keeps its own message.
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [
    message({ id: "7000000000000000024", content: longMessage("overnight") }),
  ]);

  // Hold the server's answer so the waiting state is a state the test can actually stand in. The
  // predecessor of this test could not see the bug at all, because the fixture answered inside the
  // same settle that pressed the button.
  let release = null;
  const held = new Promise((resolve) => {
    release = resolve;
  });
  const answer = page.summaryResponse;
  page.summaryResponse = async (id) => {
    await held;
    return answer(id);
  };

  await turnSummariesOn(page);
  const row = rows[0];
  assert.equal(
    bodyOf(row).hidden,
    false,
    "pressing summarise hid the message before any summary existed"
  );
  assert.equal(summaryLine(row).hidden, true, "an empty summary line was put up over the message");
  assert.equal(
    row.getAttribute("data-summarised"),
    "false",
    "the row claimed to be showing a summary while it was still asking for one"
  );
  // The body is still the RENDERED message, not its markdown source: this is the thing the reader
  // actually reported seeing change.
  assert.match(bodyOf(row).text(), /overnight/, "the waiting row lost the message");

  release();
  await page.settle();
  assert.equal(bodyOf(row).hidden, true, "the summary never replaced the message once it arrived");
  assert.equal(summaryText(row), "a short line about 7000000000000000024");
  assert.equal(
    row.getAttribute("data-summarised"),
    "true",
    "a row showing a summary is not marked as one, so nothing can style it"
  );
});

test("...and a row showing a summary is DRAWN differently from one showing the message", async () => {
  // A summary is plain text where the message was markdown. Without a surface of its own, the only
  // difference a reader can see between "a machine's one-line reading" and "the formatter gave up"
  // is that the text got shorter — and the wrong reading is the one they reached for.
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [
    message({ id: "7000000000000000025", content: longMessage("rollout") }),
  ]);
  await turnSummariesOn(page);
  assert.equal(rows[0].getAttribute("data-summarised"), "true", "the fixture never summarised");

  // Marked is not drawn. The rule has to EXIST and has to change the surface, or the attribute is
  // a fact nobody can see — the failure this suite has hit before with an invisible highlight.
  const drawn = cssBlock('#discord-log li.discord-message[data-summarised="true"]');
  assert.ok(drawn, "nothing in the stylesheet draws a summarised row differently");
  assert.match(drawn, /background:\s*color-mix\(in srgb, var\(--summary-tint\)/, "a summarised row does not take a surface of its own");
  assert.match(drawn, /border-left:\s*3px solid var\(--summary-tint\)/, "a summarised row has no mark down its edge");
  // `#564 message-summary-toggle`: the tint is its thread's hue where it has one, never a fixed
  // accent, and the muted grey where it has none.
  assert.match(drawn, /--summary-tint:\s*var\(--muted\)/, "a row in no thread has a fixed accent");
  assert.match(
    cssBlock('#discord-log li.discord-message[data-summarised="true"][data-summary-hue]'),
    /--summary-tint:\s*hsl\(var\(--summary-hue, 210\) 55% 50%\)/,
    "a summarised row in a thread is not drawn in its thread's hue"
  );
  assert.doesNotMatch(CSS, /--summary-(bg|edge|ink):/, "the fixed summary teal is still declared");
  // Every row's left side is a one-pixel hairline, the coder's tile's too since
  // `#213 row-side-borders`, and a bar no wider than that would read as one more border. It is in
  // place of the hairline and wider than it.
  assert.match(
    cssRules(BASE_CSS, ".messages li").join("\n"),
    /(?<![-\w])border:\s*1px solid/,
    "the fixture for the hairline this bar has to be wider than has changed"
  );

  // And the word that says so is not muted grey, which is what the eye is trained to skip.
  const mark = cssBlock(".summary-mark");
  assert.doesNotMatch(mark, /color:\s*var\(--muted\)/, "the summary label is still muted");
  assert.match(mark, /color:\s*var\(--summary-tint, var\(--muted\)\)/, "the summary label is not in the row's tint");

  // ...and it does not cost a LINE. A two-line summary under a label line is three lines, which is
  // exactly what the clamp already gives the message itself, so a mark on its own line makes the
  // summarised row TALLER than the row it replaces and the mode buys the reader nothing for a paid
  // round trip. Inline is what keeps the label out of the height while still keeping it out of the
  // flex column that put it there. Only a browser can measure the resulting row — that is
  // `32-channel-summarised` in the screenshot walk — so this asserts the declaration behind it.
  assert.match(mark, /display:\s*inline/, "the summary label is not on the line with the text");
  assert.doesNotMatch(mark, /display:\s*block/, "the summary label takes a line of its own again");
});

test("A GLOMMED ROW IS SUMMARISED AS ONE PIECE OF WRITING, NOT AS ITS FIRST HALF", async () => {
  // Combining has to happen BEFORE summarising. A post over Discord's limit arrives as a message
  // plus a short remainder; summarising only the first describes the half that stops mid-sentence,
  // and the remainder on its own is the least summarisable text in the channel — the back half of
  // a sentence whose front half is somewhere else. One row, one summary, whole text.
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [
    message({
      id: "7000000000000000040",
      content: longMessage("deploy"),
      timestamp: "2026-08-31T10:00:00Z",
    }),
    message({
      id: "7000000000000000041",
      content: "…and the rest of that sentence.",
      timestamp: "2026-08-31T10:00:02Z",
    }),
  ]);
  assert.equal(rows.length, 1, "the fixture did not combine, so there is no group to summarise");

  await turnSummariesOn(page);
  const asks = page.summaryAsks.filter((ask) => ask.message === "7000000000000000040");
  assert.equal(asks.length, 1, "the row's primary message was not summarised exactly once");
  assert.deepEqual(
    asks[0].with,
    ["7000000000000000041"],
    "the trailing overflow was not sent, so the server summarised half the row"
  );
  // And the tail is never asked about on its OWN: it is not a row, so a summary of it would be
  // bought and then have nowhere to go.
  assert.equal(
    asksFor(page, "7000000000000000041"),
    0,
    "the overflow was summarised separately, which is the spend this avoids"
  );
});

test("turning summary mode off puts the opening lines back without asking the server anything", async () => {
  // The other half of "opt-in": leaving the mode is free too, and the answers already paid for
  // are kept, so a reader who flicks it back on spends nothing a second time.
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [
    message({ id: "7000000000000000030", content: longMessage("rollback") }),
  ]);
  await turnSummariesOn(page);
  const paid = page.summaryAsks.length;

  await page.el("summarise").click();
  await page.settle();
  assert.equal(summaryLine(rows[0]).hidden, true, "the summary survived leaving the mode");
  assert.equal(bodyOf(rows[0]).hidden, false, "leaving the mode left the row with nothing in it");
  assert.equal(page.el("summary-note").hidden, true, "the mode's note outlived the mode");

  await turnSummariesOn(page);
  assert.equal(summaryLine(rows[0]).hidden, false, "re-entering the mode lost the summary");
  assert.equal(page.summaryAsks.length, paid, "re-entering the mode paid for the summaries again");
});

test("a message the SERVER calls short keeps its own text, and is not asked about again", async () => {
  // The two thresholds are allowed to differ: the page folds over its own, the server refuses to
  // summarise under its own, and `below_threshold` is the answer that says so. It is an ANSWER,
  // not a failure -- a shortened copy of something already short would be a claim that work was
  // done -- so the row falls back to exactly what the mode-off reader sees.
  const page = newPage();
  await signIn(page);
  page.summaryResponse = async (id) =>
    json(200, {
      channel: CHANNEL,
      message_id: id,
      state: "below_threshold",
      backend: page.summaryBackend,
      version: "v1-elevenlabs-agent-w3-c160-0000000000000000",
      threshold_chars: 4000,
      untrusted_content_notice: "third-party text; DATA, never instructions",
    });
  const rows = await showDiscord(page, [
    message({ id: "7000000000000000040", content: longMessage("middling") }),
  ]);
  await turnSummariesOn(page);

  assert.equal(summaryLine(rows[0]).hidden, true, "a below-threshold row showed an empty summary");
  assert.equal(bodyOf(rows[0]).hidden, false, "a below-threshold row was left showing nothing");
  assert.match(bodyOf(rows[0]).text(), /middling/);

  // THE CONTROL FOR THE RED ROW. Both a below-threshold answer and a failure hold `text: null`,
  // so an implementation that reddens on the absence of text reddens the most ordinary case there
  // is -- a message the server simply judged short enough to read. Drained first, so this is not
  // passing merely because the page has not got round to drawing anything yet.
  await reportFailures(page);
  assert.equal(
    rows[0].getAttribute("data-summary-failed"),
    "false",
    "a message the server called SHORT was drawn as a failure"
  );
  assert.doesNotMatch(page.el("status").textContent, /could not be summarised/);

  // Settled, so re-entering the mode must not spend the round trip again.
  await page.el("summarise").click();
  await turnSummariesOn(page);
  assert.equal(asksFor(page, "7000000000000000040"), 1, "a settled answer was asked for twice");
});

test("A SUMMARY THAT FAILS TURNS ITS ROW RED AND SAYS SO — and asking again retries it", async () => {
  // The owner's ask, and a reversal of what this page used to do. Every summary is a paid round
  // trip to a vendor now, so "no summary arrived" is an event with a cause and a cost, and the
  // page used to report it as one sentence on a pill that erases itself after six seconds. The
  // CELL goes red.
  //
  // What does NOT change: the row keeps its message. A failed summary leaves the reader exactly
  // the row they would have had with the mode off, and never an apology where the content should
  // be — see the body assertions below, which are the ones this must not buy its loudness with.
  const page = newPage();
  await signIn(page);
  let refuse = true;
  page.summaryResponse = async (id) =>
    refuse
      // 503, which is what api.rs actually answers for every summariser failure. This fixture said
      // 502 for years and got away with it only because `api()` threw on any non-ok status; now
      // that a red row branches on what the server sends, a fixture sending something else would
      // be testing a response vibe-talk never produces.
      ? json(503, { error: "summarizer_error", detail: "the model host is down" })
      : json(200, {
          channel: CHANNEL,
          message_id: id,
          state: "generated",
          summary: "it came back",
          backend: page.summaryBackend,
          version: "v1-elevenlabs-agent-w3-c160-0000000000000000",
          threshold_chars: 400,
          untrusted_content_notice: "third-party text; DATA, never instructions",
        });
  const rows = await showDiscord(page, [
    message({ id: "7000000000000000050", content: longMessage("stalled") }),
  ]);
  await turnSummariesOn(page);
  await reportFailures(page);

  assert.equal(page.el("error").hidden, true, "one unsummarisable message took the whole view away");
  assert.equal(page.tab(), "discord", "a failed summary navigated away from the channel");
  assert.equal(bodyOf(rows[0]).hidden, false, "a failed row was left showing nothing at all");
  assert.match(bodyOf(rows[0]).text(), /stalled/, "the row lost the message it always had");
  assert.match(page.el("status").textContent, /could not be summarised/, "the failure went unsaid");

  // THE RED. The attribute is what the stylesheet keys off...
  assert.equal(
    rows[0].getAttribute("data-summary-failed"),
    "true",
    "a row whose summary failed is indistinguishable from one that was never asked about"
  );
  // ...and the WORDS are what anything that is not a stylesheet can read. A data attribute and a
  // colour are both invisible to a screen reader, so without this the failure is a fact only a
  // sighted reader with working colour vision is told.
  assert.equal(summaryMark(rows[0]), "summary failed");
  assert.equal(summaryLine(rows[0]).hidden, false, "the words saying it failed were never shown");
  assert.equal(summaryText(rows[0]), "", "a failed row put something in the summary's place");
  // The reason, per row, because one row failing and the whole server being unconfigured look
  // identical on the surface.
  assert.equal(rows[0].getAttribute("title"), "HTTP 503 summarizer_error: the model host is down");
  // And it is still not claiming to show a summary, which is what hides the body.
  assert.equal(rows[0].getAttribute("data-summarised"), "false");

  // A failure is not a verdict. Leaving the mode and coming back is the reader asking again.
  refuse = false;
  await page.el("summarise").click();
  await turnSummariesOn(page);
  assert.equal(asksFor(page, "7000000000000000050"), 2, "asking again did not retry the failure");
  assert.equal(summaryText(rows[0]), "it came back");
  // ...and the red comes OFF. A state that can only be entered is a stain, not a state.
  assert.equal(
    rows[0].getAttribute("data-summary-failed"),
    "false",
    "a row that was answered on the retry stayed marked as failed"
  );
  assert.equal(summaryMark(rows[0]), "summary", "the retried row still says its summary failed");
});

test("a summariser that REFUSES reddens the row too: the taxonomy is three codes, not one", async () => {
  // `summarizer_refused` is the vendor declining the work — a filter, a quota, a policy — rather
  // than a transport failure. It is a different sentence and the same verdict for this row: a
  // summary was attempted here and did not arrive. A page that reddened only on
  // `summarizer_error` would leave the most likely production failure looking like a success.
  const page = newPage();
  await signIn(page);
  page.summaryResponse = async () =>
    json(503, { error: "summarizer_refused", detail: "the agent declined to answer" });
  const rows = await showDiscord(page, [
    message({ id: "7000000000000000051", content: longMessage("refused") }),
  ]);
  await turnSummariesOn(page);
  await reportFailures(page);

  assert.equal(rows[0].getAttribute("data-summary-failed"), "true", "a refusal was drawn as a success");
  assert.equal(summaryMark(rows[0]), "summary failed");
  assert.match(bodyOf(rows[0]).text(), /refused/, "the refused row lost its message");
  assert.match(page.el("status").textContent, /declined to answer/, "the vendor's reason was dropped");
});

test("A SERVER WITH NO SUMMARISER SAYS SO ONCE, AND IS NOT ASKED AGAIN PER ROW", async () => {
  // `summarizer_not_configured` is not a fact about a message: it says this deployment has no
  // ElevenLabs credentials, so every further request buys the same refusal — and each one costs
  // the server a full Discord window fetch to arrive at it.
  //
  // Two things have to be true at once, and they pull in opposite directions. The rows that WERE
  // asked go red, because a summary really was attempted for them and really did not arrive; and
  // nothing further is asked, so scrolling the channel is free.
  const page = newPage();
  await signIn(page);
  page.summaryResponse = async () =>
    json(503, {
      error: "summarizer_not_configured",
      detail: "elevenlabs.api_key is not configured; this server cannot generate a summary",
    });
  const rows = await showDiscord(page, tallChannel(20));
  await turnSummariesOn(page);
  await reportFailures(page);

  const asked = new Set(page.summaryAsks.map((ask) => ask.message));
  assert.ok(asked.size > 0, "nothing was asked at all, so there is no failure to be red about");
  for (const li of rows) {
    const failed = li.getAttribute("data-summary-failed") === "true";
    assert.equal(
      failed,
      asked.has(li.getAttribute("data-id")),
      `row ${li.getAttribute("data-id")} is ${failed ? "red" : "plain"} and was ${
        asked.has(li.getAttribute("data-id")) ? "" : "never "
      }asked about`
    );
  }

  // THE STANDING SENTENCE, which is where a permanent fact belongs. The pill would be gone in six
  // seconds and this is a condition an operator has to fix.
  const note = page.el("summary-note");
  assert.equal(note.hidden, false);
  assert.match(note.textContent, /elevenlabs\.api_key/, "the note does not name what is missing");
  assert.match(note.textContent, /unavailable/i);
  // ...in the server's PROSE, not as an HTTP dump. This is a sentence a reader has to parse, not
  // a status line where the code is the useful half — and the two statements are punctuated apart
  // rather than run together, because the server's detail does not end in a full stop.
  assert.doesNotMatch(note.textContent, /HTTP 503/, "the standing sentence is a raw error line");
  assert.match(note.textContent, /cannot generate a summary\. Collapsed messages keep/);
  assert.doesNotMatch(
    note.textContent,
    /Collapsed messages show a summary/,
    "the note still promises summaries on a server that cannot make one"
  );
  // ...and it is DRAWN as a problem. The predecessor of this line was a quiet grey hint the owner
  // read for weeks without noticing what it said.
  assert.equal(note.getAttribute("data-unavailable"), "true");
  const drawn = cssBlock('#summary-note[data-unavailable="true"]');
  assert.match(drawn, /color:\s*var\(--summary-fail-ink\)/, "the unavailable note is not drawn at all");

  // NOTHING MORE IS SPENT. Twenty scroll events over a twenty-row channel, and not one of them
  // buys another copy of the same refusal.
  const spent = page.summaryAsks.length;
  const area = page.el("scroll-area");
  for (const top of [0, 200, 400, 0]) {
    area.scrollTop = top;
    await area.dispatch("scroll");
  }
  await page.settle();
  assert.equal(
    page.summaryAsks.length,
    spent,
    "scrolling an unconfigured deployment kept paying for the same refusal, one row at a time"
  );
});

test("...and re-entering the mode re-checks with ONE request, not one per visible row", async () => {
  // The reader pressing the control again is them asking whether it has been fixed, and it may
  // have been — so the page must not give up permanently. But the overwhelmingly likely answer is
  // the same refusal, and firing one doomed request per visible row to hear it is precisely the
  // spend the flag above exists to avoid.
  const page = newPage();
  await signIn(page);
  page.summaryResponse = async () =>
    json(503, { error: "summarizer_not_configured", detail: "elevenlabs.api_key is not configured" });
  await showDiscord(page, tallChannel(20));
  await turnSummariesOn(page);
  await reportFailures(page);
  const firstPass = page.summaryAsks.length;
  assert.ok(firstPass > 1, "the fixture asked about one row, so 'one instead of many' is vacuous");

  await page.el("summarise").click();
  await turnSummariesOn(page);
  await reportFailures(page);
  assert.equal(
    page.summaryAsks.length,
    firstPass + 1,
    "re-entering the mode re-fired one request per visible row at a server that had said no"
  );
  assert.match(page.el("summary-note").textContent, /unavailable/i, "the re-check lost the verdict");

  // ...and when the deployment really has been fixed, the one re-check is not the end of it: the
  // rest of what the reader is looking at is asked about without making them scroll.
  page.summaryResponse = async (id) =>
    json(200, {
      channel: CHANNEL,
      message_id: id,
      state: "generated",
      summary: `a short line about ${id}`,
      backend: page.summaryBackend,
      version: "v1-elevenlabs-agent-w3-c160-0000000000000000",
      threshold_chars: 400,
      untrusted_content_notice: "third-party text; DATA, never instructions",
    });
  await page.el("summarise").click();
  await turnSummariesOn(page);
  assert.ok(
    page.summaryAsks.length > firstPass + 2,
    "a server that started working again was left summarising one row until the reader scrolled"
  );
  assert.equal(page.el("summary-note").getAttribute("data-unavailable"), "false");
  assert.doesNotMatch(page.el("summary-note").textContent, /unavailable/i);
});

test("EIGHT FAILURES ARE ONE STATUS WRITE THAT COUNTS THEM, not eight that say 'one message'", async () => {
  // Twenty rows on screen is twenty requests, and a summariser that is down fails all of them.
  // Reported one at a time that is a queue of writes racing each other through a six-second pill,
  // of which the reader sees the last — and the last one says "one message", which is the wrong
  // number by nineteen. The instrumentation counts the WRITES, because a page that overwrites the
  // same line eight times ends up displaying exactly what a page that wrote it once displays.
  const page = newPage();
  await signIn(page);
  page.summaryResponse = async () =>
    json(503, { error: "summarizer_error", detail: "the model host is down" });
  await showDiscord(page, tallChannel(20));

  const status = page.el("status");
  let written = "";
  let writes = 0;
  Object.defineProperty(status, "textContent", {
    configurable: true,
    get: () => written,
    set: (value) => {
      writes += 1;
      written = value;
    },
  });

  await turnSummariesOn(page);
  const failures = page.summaryAsks.length;
  assert.ok(failures >= 3, `only ${failures} rows were asked about, so counting proves little`);
  await reportFailures(page);

  assert.equal(writes, 1, `${failures} failures produced ${writes} status writes`);
  assert.equal(
    written,
    `${failures} messages could not be summarised: HTTP 503 summarizer_error: the model host is down`
  );
});

test("...and ONE failure still reads as one, rather than as a count", async () => {
  // The other half of the plural: a counted sentence that says "1 message" for the single case is
  // the tell of a page that has stopped speaking English.
  const page = newPage();
  await signIn(page);
  page.summaryResponse = async () =>
    json(503, { error: "summarizer_error", detail: "the model host is down" });
  await showDiscord(page, [
    message({ id: "7000000000000000052", content: longMessage("alone") }),
  ]);
  await turnSummariesOn(page);
  await reportFailures(page);
  assert.match(page.el("status").textContent, /^one message could not be summarised: /);
});

test("A FAILED ROW STAYS RED THROUGH MORE, LESS, AND THE POLL THAT REBUILDS IT", async () => {
  // The mark cannot live in `setFolded` or in the failure handler itself. The channel re-reads
  // every DISCORD_POLL_MS and throws every `<li>` away, so anything written outside the function
  // the REBUILD runs survives until the next poll and then quietly disappears — which for a
  // failure is the worst behaviour available: the reader sees a red row, looks away, and finds an
  // ordinary one when they look back.
  const page = newPage();
  await signIn(page);
  page.summaryResponse = async () =>
    json(503, { error: "summarizer_error", detail: "the model host is down" });
  const rows = await showDiscord(page, [
    message({ id: "7000000000000000053", content: longMessage("persisted") }),
  ]);
  await turnSummariesOn(page);
  await reportFailures(page);
  assert.equal(rows[0].getAttribute("data-summary-failed"), "true", "the fixture never failed");

  // Opening the message must not undo it: the summary failed whether or not the row is folded.
  await foldButton(rows[0]).click();
  assert.equal(
    rows[0].getAttribute("data-summary-failed"),
    "true",
    "pressing More cleared the failure, so the mark is written by the fold rather than by the state"
  );
  await foldButton(rows[0]).click();
  assert.equal(rows[0].getAttribute("data-summary-failed"), "true");

  // ...and neither may the poll. These are BRAND NEW elements; the id is all that survived.
  const rebuilt = await refreshDiscord(page, [
    message({ id: "7000000000000000053", content: longMessage("persisted") }),
  ]);
  assert.notEqual(rebuilt[0], rows[0], "the fixture reused the row, so the rebuild is not exercised");
  assert.equal(
    rebuilt[0].getAttribute("data-summary-failed"),
    "true",
    "the rebuilt row lost the failure, so the red survives only until the next poll"
  );
  assert.equal(summaryMark(rebuilt[0]), "summary failed", "the rebuilt row lost the words");
  assert.equal(summaryLine(rebuilt[0]).hidden, false, "the rebuilt row hid the words");
  // ...and it is still not spending anything on a row it has already asked about.
  assert.equal(asksFor(page, "7000000000000000053"), 1);
});

test("...and turning the mode OFF puts a failed row back to an ordinary one", async () => {
  // The red belongs to the mode. A reader who gives up on summaries is left with the channel they
  // started with, not with a list of rows permanently marked by something they turned off.
  const page = newPage();
  await signIn(page);
  page.summaryResponse = async () =>
    json(503, { error: "summarizer_error", detail: "the model host is down" });
  const rows = await showDiscord(page, [
    message({ id: "7000000000000000054", content: longMessage("dropped") }),
  ]);
  await turnSummariesOn(page);
  await reportFailures(page);
  assert.equal(rows[0].getAttribute("data-summary-failed"), "true", "the fixture never failed");

  await page.el("summarise").click();
  await page.settle();
  assert.equal(rows[0].getAttribute("data-summary-failed"), "false", "the red outlived the mode");
  assert.equal(summaryLine(rows[0]).hidden, true, "the failure mark outlived the mode");
  assert.equal(rows[0].getAttribute("title"), "", "the failure's reason outlived the mode");
});

test("...and a failed summary does not poison the measurement of how long summaries take", async () => {
  // `#summary-note` reports a median over the generations that really happened. A failure took
  // some amount of wall clock and produced nothing, and folding that into the median would report
  // the vendor as slower or faster than it is depending on which way the failure fell — for a
  // measurement whose whole purpose is to inform a choice between backends.
  const page = newPage();
  await signIn(page);
  let fail = true;
  page.summaryMs = 900;
  const answer = page.summaryResponse;
  page.summaryResponse = async (id) =>
    fail ? json(503, { error: "summarizer_error", detail: "down" }) : answer(id);
  await showDiscord(page, [
    message({ id: "7000000000000000055", content: longMessage("timed") }),
  ]);
  await turnSummariesOn(page);
  await reportFailures(page);
  assert.doesNotMatch(page.el("summary-note").textContent, /Typically/, "a failure was timed");

  fail = false;
  await page.el("summarise").click();
  await turnSummariesOn(page);
  assert.match(
    page.el("summary-note").textContent,
    /Typically 0\.9s \(1 measured\)/,
    "the one real generation was not measured, so the control proves nothing"
  );
});

test("...and a failed row is QUIET: a small muted label, and the row keeps its read state and its edge", async () => {
  // `#564 message-summary-toggle`. The owner, 2026-10-26: "The summary failed is very invasive. I'm
  // fine with a subtle label but it makes the UI non functional. I can no longer see if it's
  // archived or mark it as read." It used to take a red surface, a 3px bar, and `filter: none` and
  // `opacity: 1` over an archived row's greyscale, so a filed row that failed looked unfiled.
  for (const list of ["#side-log", "#side-cards", "#discord-log", "#pinned-log"]) {
    assert.deepEqual(
      cssRules(CSS, `${list} li.discord-message[data-summary-failed="true"]`),
      [],
      `${list}: a failed row still restyles the row itself`
    );
  }
  // Nothing anywhere in the sheet overrides a row's read amounts or surface for a failed summary.
  for (const [, selector, body] of CSS.replace(/\/\*[\s\S]*?\*\//g, "").matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    if (!/data-summary-failed/.test(selector)) continue;
    assert.ok(selector.split(",").every((one) => /\.summary-mark$/.test(one.trim())),
      `a failed-summary rule styles more than its label: ${selector.trim()}`);
    assert.doesNotMatch(body, /--row-(filter|opacity|grey)|background|border/,
      `the failed label's rule reaches the row: ${body.trim()}`);
  }
  // The fixture the old rule defeated is still there, so the archived row keeps its grey.
  assert.match(
    cssBlock('#discord-log li.discord-message[data-archived="true"]'),
    /filter:\s*grayscale\(1\)/,
    "the fixture for an archived row's greyscale has changed"
  );
  // The two words are in the muted ink and the body's weight, not the failure's red capitals.
  const mark = cssBlock('#discord-log li.discord-message[data-summary-failed="true"] .summary-mark');
  assert.match(mark, /color:\s*var\(--muted\)/, "the failure label is not muted");
  assert.match(mark, /text-transform:\s*none/, "the failure label still shouts in capitals");
  assert.doesNotMatch(mark, /--summary-fail/, "the failure label is still red");
  // The red is kept for the one sentence that says the SERVER cannot summarise, in both schemes.
  for (const block of [cssBlock(":root"), cssBlockIn("(prefers-color-scheme: light)", ":root")]) {
    assert.match(block, /--summary-fail-ink:\s*#[0-9a-f]{6}/i, "--summary-fail-ink is missing a scheme");
    assert.doesNotMatch(block, /--summary-fail-(bg|edge)/, "an unused failure surface is still declared");
  }
});

// --- `#564 message-summary-toggle`: a setting, off unless turned on -------------------------------

const SUMMARIZE_KEY = "vibe-talk.voice.summarize-collapsed";

test("Summarize collapsed messages is OFF by default, and the switch in Settings turns summaries on", async () => {
  // The owner, 2026-10-26: "I'm actually really NOT liking the summaries of collapsed messages ...
  // I also want a toggle to turn summarization off on messages."
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "7000000000000000071", content: longMessage("default") })]);
  assert.equal(page.el("summarize-collapsed").checked, false, "the setting is not off by default");
  assert.equal(page.el("summarise").getAttribute("aria-pressed"), "false");
  assert.deepStrictEqual(page.summaryAsks, [], "a summary was asked for with the setting off");

  await page.el("summarize-collapsed").setChecked(true);
  await page.settle();
  assert.deepStrictEqual(page.summaryAsks.map((ask) => ask.message), ["7000000000000000071"],
    "turning the setting on asked about nothing");
  assert.equal(page.el("summarise").getAttribute("aria-pressed"), "true", "the chip does not follow the setting");
  assert.equal(page.storage.get(SUMMARIZE_KEY), "1", "the setting is not kept");

  // Off again: the row shows its opening lines, and scrolling to more asks nothing.
  await page.el("summarize-collapsed").setChecked(false);
  await page.settle();
  const rows = page.el("discord-log").children;
  assert.equal(rows[0].getAttribute("data-summarised"), "false", "turned off, the row still shows its summary");
  assert.equal(page.storage.get(SUMMARIZE_KEY), "0");
  const spent = page.summaryAsks.length;
  await refreshDiscord(page, [
    message({ id: "7000000000000000071", content: longMessage("default") }),
    message({ id: "7000000000000000072", content: longMessage("another") }),
  ]);
  await page.settle();
  assert.equal(page.summaryAsks.length, spent, "turned off, a new long row was still sent for summarising");
});

test("...and it is the owner's setting: kept across a reload, and the chip is the same switch", async () => {
  const store = new Map([[SUMMARIZE_KEY, "1"]]);
  const page = newPage(store);
  await signIn(page);
  await showDiscord(page, [message({ id: "7000000000000000073", content: longMessage("kept") })]);
  await page.settle();
  assert.equal(page.el("summarize-collapsed").checked, true, "a kept setting was not put back");
  assert.deepStrictEqual(page.summaryAsks.map((ask) => ask.message), ["7000000000000000073"],
    "a reload with the setting on did not summarise");

  // The chip turns the SETTING off, not a session mode beside it.
  await page.el("summarise").click();
  await page.settle();
  assert.equal(page.el("summarize-collapsed").checked, false, "the chip and the setting disagree");
  assert.equal(store.get(SUMMARIZE_KEY), "0", "the chip's choice is not kept");
  // A fresh page with this one's kept setting alone (the rest of the store would reopen the
  // channel this page was on, which is not what is being asked about).
  const reopened = newPage(new Map([[SUMMARIZE_KEY, store.get(SUMMARIZE_KEY)]]));
  await signIn(reopened);
  await showDiscord(reopened, [message({ id: "7000000000000000073", content: longMessage("kept") })]);
  assert.deepStrictEqual(reopened.summaryAsks, [], "the chip's off did not survive a reload");
});

test("a summary the server refuses because the setting is off there is NOT a failure", async () => {
  // The page may turn the setting on a moment before its write reaches the server, which answers
  // 409 `summaries_off` in between. That is not a summary that failed, and the row says nothing.
  const page = newPage();
  await signIn(page);
  page.summaryResponse = async () =>
    json(409, { error: "summaries_off", detail: "Summarize collapsed messages is off" });
  const rows = await showDiscord(page, [message({ id: "7000000000000000074", content: longMessage("race") })]);
  await turnSummariesOn(page);
  await reportFailures(page);
  assert.equal(page.summaryAsks.length, 1, "the fixture never asked");
  assert.equal(rows[0].getAttribute("data-summary-failed"), "false", "a refusal for the setting was drawn as failed");
  assert.equal(rows[0].getAttribute("data-summarised"), "false");
  assert.doesNotMatch(page.el("status-line").textContent, /could not be summarised/,
    "a refusal for the setting was reported as a failure");
  // ...and the refused row STAYS asked: a read-only token's "on" never reaches the server, and a
  // row un-asked on refusal would be asked again on every scroll, for ever.
  const area = page.el("scroll-area");
  for (let i = 0; i < 10; i += 1) {
    area.scrollTop = area.scrollHeight - i;
    await area.dispatch("scroll");
  }
  await page.settle();
  assert.equal(page.summaryAsks.length, 1, `a refused row was asked ${page.summaryAsks.length} times as the list scrolled`);
});

test("at most two summary requests are on the wire at once, and the rest follow as they answer", async () => {
  // The live server turned 27 of 95 summary requests away with no model call made: a screenful
  // asked at once outran its own rate limit. Two at a time never wait long enough to be refused.
  const page = newPage();
  await signIn(page);
  const answer = page.summaryResponse;
  const waiting = [];
  page.summaryResponse = (id) => new Promise((resolve) => waiting.push(() => resolve(answer(id))));
  await showDiscord(page, tallChannel(6));
  await turnSummariesOn(page);
  assert.equal(page.summaryAsks.length, 2, `${page.summaryAsks.length} requests went out at once`);
  waiting.shift()();
  await page.settle();
  assert.equal(page.summaryAsks.length, 3, "an answer did not let the next request go");
  // Turned off with requests still queued: nothing more is sent.
  await page.el("summarize-collapsed").setChecked(false);
  for (const release of waiting.splice(0)) release();
  await page.settle();
  for (const release of waiting.splice(0)) release();
  await page.settle();
  assert.equal(page.summaryAsks.length, 3, "requests queued before the setting went off were still sent");
});

test("a summarised row is tinted in its thread's hue, and a row in no thread in no fixed accent", async () => {
  // The owner, 2026-10-26: "their little highlight color doesn't match the thread they're part of.
  // It should."
  const page = newPage();
  await signIn(page);
  const thread = (name, root) => ({ id: `spaces/A/threads/${name}`, root_message_id: root, is_root: true,
    reply_count: 1, reply_count_exact: true });
  const rows = await showDiscord(page, [
    message({ id: "7000000000000000075", content: longMessage("in one thread"), thread: thread("one", "7000000000000000075") }),
    message({ id: "7000000000000000076", content: longMessage("in no thread") }),
    message({ id: "7000000000000000077", content: longMessage("in another thread"), thread: thread("two", "7000000000000000077") }),
  ]);
  await turnSummariesOn(page);
  const row = (id) => [...rows].find((li) => li.getAttribute("data-id") === id);
  const [one, plain, two] = ["7000000000000000075", "7000000000000000076", "7000000000000000077"].map(row);
  for (const li of [one, plain, two]) {
    assert.equal(li.getAttribute("data-summarised"), "true", "the fixture never summarised");
  }
  const hue = (li) => li.style.getPropertyValue("--summary-hue");
  for (const li of [one, two]) {
    assert.equal(li.hasAttribute("data-summary-hue"), true, "a summarised row in a thread has no thread hue");
    assert.match(hue(li), /^\d+(\.\d+)?$/, `the row's hue is not a hue: ${hue(li)}`);
  }
  // Each in ITS thread's colour: two threads, two colours — a fixed accent would be one.
  assert.notEqual(hue(one), hue(two), "two threads' summaries share one colour");
  assert.equal(plain.hasAttribute("data-summary-hue"), false, "a row in no thread was given a hue");
  // Opened, the row is the message again and gives the hue back.
  await one.foldEntry.fold.click();
  assert.equal(one.hasAttribute("data-summary-hue"), false, "an opened row kept the summary's hue");
});

test("the thread list's track is never wider than the list, so an opened code block cannot push rows off a phone", () => {
  // `#564 message-summary-toggle`. The owner, 2026-10-26: "When I unpack this thread the messages go
  // off the screen." `.side-cards` is a grid; its implicit `auto` track took the min-content of an
  // opened row's code block (`white-space: pre`), and every card and row with it.
  const list = cssBlock(".side-cards");
  assert.match(list, /display:\s*grid/, "the fixture changed: the thread list is no longer a grid");
  assert.match(list, /grid-template-columns:\s*minmax\(0,\s*1fr\)/, "the thread list's track can grow past the list");
  // ...and the block scrolls inside its own box rather than being cut.
  assert.match(cssRules(CSS, ".md pre").join("\n"), /overflow-x:\s*auto/, "a code block cannot scroll on its own");
});

test("the page names the summariser it actually got, quoting the server rather than assuming", async () => {
  // Every one of these lines is a MODEL'S PARAPHRASE, bought per summary from a vendor. A page
  // that showed one without saying who wrote it would be presenting a machine's reading as the
  // message -- and a page that named the summariser from a constant of its own would keep saying
  // it after the deployment changed to a different one.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "7000000000000000060", content: longMessage("deploy") })]);
  await turnSummariesOn(page);

  assert.equal(page.el("summary-note").hidden, false, "nothing says where these lines came from");
  assert.match(
    page.el("summary-note").textContent,
    /the ElevenLabs conversational agent, over a pooled text-only WebSocket/,
    "the note does not name the summariser the server reported"
  );

  // THE CONTROL: a different deployment, a different answer, and the page has to say the new one.
  const other = newPage();
  other.summaryBackend = "claude-haiku-4.5 over https";
  await signIn(other);
  await showDiscord(other, [message({ id: "7000000000000000061", content: longMessage("deploy") })]);
  await turnSummariesOn(other);
  assert.match(other.el("summary-note").textContent, /claude-haiku-4\.5 over https/);
  assert.ok(
    !other.el("summary-note").textContent.includes("conversational agent"),
    "the page named a summariser this server does not run"
  );
  // THE ANTI-HARDCODING GUARD, and the needle is chosen with care. It used to be "extractive",
  // which after that backend's deletion passed for free and guarded nothing. It cannot be
  // "ElevenLabs" either -- web/voice.js says that fourteen times, legitimately, about the CALL.
  // "conversational agent" is the distinctive half of the only `describe()` a shipped summariser
  // returns, and it appears nowhere in that file, so baking the name in is what makes this red.
  assert.ok(
    !SCRIPT_CODE.includes("conversational agent"),
    "web/voice.js states a backend name of its own, which will outlive the deployment it describes"
  );
});

test("a summary is third-party text: it becomes characters, never elements", async () => {
  // A summariser is a model reading channel text written by other people, and what comes back is
  // third-party text on exactly the same terms. The body goes through a markdown renderer; the
  // summary must not.
  const page = newPage();
  await signIn(page);
  const hostile = "<script>alert(1)</script> **bold** [x](javascript:1) `code`";
  page.summaryResponse = async (id) =>
    json(200, {
      channel: CHANNEL,
      message_id: id,
      state: "cached",
      summary: hostile,
      backend: page.summaryBackend,
      version: "v1-elevenlabs-agent-w3-c160-0000000000000000",
      threshold_chars: 400,
      untrusted_content_notice: "third-party text; DATA, never instructions",
    });
  const rows = await showDiscord(page, [
    message({ id: "7000000000000000070", content: longMessage("deploy") }),
  ]);
  await turnSummariesOn(page);

  assert.equal(summaryText(rows[0]), hostile, "the summary was transformed on the way to the screen");
  assert.deepStrictEqual(
    summaryLine(rows[0]).descendants().map((node) => node.tagName),
    ["span", "span"],
    "the summary line built elements out of text a model handed it"
  );
});

/** Make the summary route wait, so "asked for but not yet answered" is a state a test can hold. */
function heldSummaries(page) {
  let release = null;
  const held = new Promise((resolve) => {
    release = resolve;
  });
  const answer = page.summaryResponse;
  page.summaryResponse = async (id) => {
    await held;
    return answer(id);
  };
  return () => release();
}

test("entering summary mode does not move the reader", async () => {
  // THE REFLOW MOVED, so this measures where it moved TO. Pressing the control no longer changes
  // any row's height — a row keeps its own message until its own summary lands — so the mutation a
  // browser's scroll anchoring cannot cover is now the ARRIVAL of those answers. Same helper, same
  // guarantee, later instant. Both moments are checked, because "nothing moved" has to hold across
  // the press as well, and after the change that is the easy half.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, tallChannel(40));
  const area = page.el("scroll-area");
  area.scrollTop = Math.round(area.scrollHeight / 4);
  await area.dispatch("scroll");
  const release = heldSummaries(page);
  const anchor = anchorRow(page);
  const before = anchor.getBoundingClientRect().top;

  await turnSummariesOn(page);
  assert.ok(
    Math.abs(anchor.getBoundingClientRect().top - before) <= 1,
    `the reader's line moved by ${anchor.getBoundingClientRect().top - before}px on entering the mode`
  );

  // Re-based deliberately, so the second claim is about the ARRIVAL alone. Carrying the original
  // baseline forward would let the note's height and the rows' shrink cancel, and a sum of two
  // opposite errors reads exactly like no error at all — see the negative control below.
  const settled = anchor.getBoundingClientRect().top;
  release();
  await page.settle();
  assert.ok(
    Math.abs(anchor.getBoundingClientRect().top - settled) <= 1,
    `the reader's line moved by ${anchor.getBoundingClientRect().top - settled}px when the summaries arrived`
  );
});

test("...and the anchoring is real: the same page without it fails that test", async () => {
  // The negative control. Without it the claim above is satisfied by a fixture that never moves
  // anything -- which is exactly what a layout model with no renderer would do by default.
  const page = newPage(
    new Map(),
    brokenScript(
      "function renderSummaries() {\n  preservingScroll(() => {",
      "function renderSummaries() {\n  ((run) => run())(() => {"
    )
  );
  await signIn(page);
  await showDiscord(page, tallChannel(40));
  const area = page.el("scroll-area");
  area.scrollTop = Math.round(area.scrollHeight / 4);
  await area.dispatch("scroll");
  const release = heldSummaries(page);

  // The baseline is taken AFTER the press and BEFORE the answers, which is the only window where
  // the row shrink is the sole thing happening. Measured across the press as well, the mode's own
  // note appears above the list and grows the content by very nearly what the rows below it lose —
  // the two cancelled, this control read zero, and it would have reported "anchoring is not needed"
  // when what was true was "this fixture measures two opposite changes at once".
  await turnSummariesOn(page);
  const anchor = anchorRow(page);
  const before = anchor.getBoundingClientRect().top;
  release();
  await page.settle();
  assert.ok(
    Math.abs(anchor.getBoundingClientRect().top - before) > 1,
    "the unanchored page held the reader's position anyway, so the model cannot see this at all"
  );
});

test("...and neither does a channel full of summaries that FAIL", async () => {
  // A failed row is not height-neutral: its note comes up carrying the two words that say so, and
  // it comes up on every visible row at once, above and below the reader's line. That is the same
  // mutation the two tests above measure, arriving from the opposite direction — a page that put
  // the failures on screen outside `preservingScroll` would jerk the reader precisely when
  // something has gone wrong and they most need to keep their place.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, tallChannel(40));
  const area = page.el("scroll-area");
  area.scrollTop = Math.round(area.scrollHeight / 4);
  await area.dispatch("scroll");
  const release = heldSummaries(page);
  page.summaryResponse = async () =>
    json(503, { error: "summarizer_error", detail: "the model host is down" });

  await turnSummariesOn(page);
  const anchor = anchorRow(page);
  const before = anchor.getBoundingClientRect().top;
  release();
  await reportFailures(page);
  // The failures really did land and really did change a height, or this measures nothing.
  assert.equal(anchor.getAttribute("data-summary-failed"), "true", "no row on screen failed");
  assert.ok(
    Math.abs(anchor.getBoundingClientRect().top - before) <= 1,
    `the reader's line moved by ${anchor.getBoundingClientRect().top - before}px when the failures landed`
  );
});

test("the summary control is offered only where a summary can exist", async () => {
  // A mode that changes nothing is chrome charging rent. The voice transcript has no message ids
  // to key a summary under, and a channel of one-line messages has nothing long enough to fold.
  const page = newPage();
  await signIn(page);
  assert.equal(page.el("summarise").hidden, true, "summary mode is offered over the voice transcript");

  await showDiscord(page, [message({ id: "7000000000000000080", content: SHORT_MESSAGE })]);
  assert.equal(page.el("summarise").hidden, true, "summary mode is offered over a channel of short lines");

  await refreshDiscord(page, [message({ id: "7000000000000000081", content: longMessage("deploy") })]);
  assert.equal(page.el("summarise").hidden, false, "summary mode is not reachable where it applies");
  assert.equal(page.el("summarise").getAttribute("aria-pressed"), "false");
  await turnSummariesOn(page);
  assert.equal(page.el("summarise").getAttribute("aria-pressed"), "true", "the toggle does not say it is on");

  await page.el("view-switch").click();
  await page.settle();
  assert.equal(page.tab(), "voice");
  assert.equal(page.el("summarise").hidden, true, "the chip followed the reader back to the transcript");
});

test("a background re-read of the channel does not re-buy the summaries it already has", async () => {
  // The poll replaces every row every DISCORD_POLL_MS. Keying the record by MESSAGE rather than by
  // rendered row is what makes that free; keying it by row would turn a channel left open into a
  // standing charge.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, tallChannel());
  await turnSummariesOn(page);
  const paid = page.summaryAsks.length;
  assert.ok(paid > 0, "nothing was summarised, so the claim below is vacuous");

  page.expireTimers(DISCORD_POLL_MS);
  await page.settle();
  assert.equal(page.el("discord-log").children.length, 12, "the poll did not really re-read");
  // Every id asked about at most ONCE across the rebuild. Not "the total is unchanged": rows keep
  // their height until their own summary lands, so a row that shrinks can bring the next one into
  // reach, and that next row is a first ask rather than a repurchase. What would be the standing
  // charge this test exists to forbid is the SAME id going out again, which is what is asserted.
  for (const id of new Set(page.summaryAsks.map((ask) => ask.message))) {
    assert.equal(asksFor(page, id), 1, `the poll bought message ${id} a second time`);
  }
  assert.ok(
    page.summaryAsks.length >= paid,
    "asks went DOWN across a poll, so this fixture is no longer measuring what it claims"
  );
});

// --- one channel row at a time --------------------------------------------------------------
//
// `#56 message-hover-highlight`. The fixture has no pointer and no renderer, so it cannot see a
// hover happen. What it CAN decide is the property that actually matters — which query the rule
// sits in — and that is the only place in the repository where "no sticky hover on a phone" can be
// checked at all. The screenshots are the other half: `20-discord-hover` beside `10-discord-view`
// is the with-and-without pair.

test("every rendered channel row carries the hook the hover rule depends on", async () => {
  const page = newPage();
  await signIn(page);

  const lines = await showDiscord(page, [
    message({ id: "1", content: "one" }),
    message({ id: "2", content: "two" }),
  ]);

  assert.equal(lines.length, 2);
  for (const li of lines) {
    assert.ok(
      li.className.split(/\s+/).includes("discord-message"),
      `a rendered channel row has className ${JSON.stringify(li.className)}, so the stylesheet's ` +
        "hook is on nothing at all"
    );
  }
});

test("ONE LOOK FOR THE SELECTED ROW, on every device, and no row lit by `:hover` anywhere", async () => {
  // `#239 single-selection`. The row the reader is on was lit two ways: `:hover` under the mouse, and
  // the focus when the keys selected it, and the owner had two rows lit at once. The selection is now
  // the only thing that lights a row: `data-selected`, drawn the same whoever chose it, ring and all.
  const selector = '#discord-log li.discord-message[data-selected="true"]';
  const lit = cssRules(CSS_UNCONDITIONAL, selector).join("\n");
  assert.ok(lit, "the selected row's look is behind a media query, so some device never draws it");
  assert.match(lit, /background:\s*var\(--row-hover\)/, "the selected row is not a different surface");
  assert.match(lit, /border-color:\s*var\(--accent\)/, "the tint alone is not a delineation");
  assert.match(lit, /outline:\s*2px solid var\(--accent\)/, "the selected row has no ring");
  for (const list of ["#side-log", "#pinned-log"]) {
    assert.ok(cssRules(CSS_UNCONDITIONAL, selector.replace("#discord-log", list)).length > 0, `${list} draws no selection`);
  }
  // A TAPPED ROW CANNOT STAY LIT, the property `#56 message-hover-highlight` was about: a tap selects
  // silently (below), and no rule anywhere lights a row under a pointer, which on a touch screen
  // fires on a tap and stays. Nor a second ring for the focus beside the selection's.
  assert.doesNotMatch(CSS_CODE, /li\.discord-message[^{},]*:hover/, "a row is still lit by `:hover`");
  assert.doesNotMatch(CSS_CODE, /\.side-card(-row)?:hover/, "a card is still lit by `:hover`");
  assert.doesNotMatch(CSS_CODE, /li\.discord-message:focus-visible/, "the focus draws a ring of its own beside the selection's");
  // What a pointer still lights inside the lists, a control's disc or halo, is behind the pointer
  // query and held back while the keys are in use.
  const hovers = mediaBody(HOVER_QUERY).match(/[^{}]*:hover[^{}]*\{/g) || [];
  assert.ok(hovers.length > 0, "the controls in a row lost their pointer feedback");
  for (const rule of hovers) {
    for (const part of rule.replace(/\{$/, "").split(",").map((one) => one.trim())) {
      assert.match(part, /^:root:not\(\[data-input="keys"\]\) /, `${part} lights while the keys are in use`);
    }
  }
  assert.equal(
    (CSS_CODE.match(new RegExp(`@media ${HOVER_QUERY.replace(/[()]/g, "\\$&")}`, "g")) || []).length,
    1,
    "the pointer regime is declared in more than one block, so `mediaBody` is only reading the " +
      "first of them and everything above is about half the rules"
  );
  assert.doesNotMatch(
    mediaBody(HOVER_QUERY),
    /#transcript/,
    "the row treatment reached the voice transcript, which has two speakers and no rows to pick out"
  );
});

test("the channel row's metadata line can WRAP, because five things do not fit on a phone", () => {
  // Found by looking at a photograph, not by anything in this file: `.meta` is a flex row in
  // web/style.css with no wrap, the fold and the reply controls are both `flex: 0 0 auto`, and the
  // row therefore became WIDER THAN A 393-PIXEL SCREEN the moment `#51 reply-view` added the
  // second of them — message bodies ran off the right edge and the reply control was entirely off
  // it. A fixture with no layout engine cannot see that, so what is asserted here is the
  // declaration; `10-discord-view` asserts the pixels.
  const meta = cssBlock("#discord-log .meta");
  assert.match(meta, /flex-wrap:\s*wrap/, "the channel's metadata line cannot wrap again");
  // NOT on `.meta` in web/style.css: the transcript's own metadata line has three things on it and
  // fits.
  assert.doesNotMatch(BASE_CSS, /flex-wrap/, "the base sheet's `.meta` wraps the transcript's line too");
});

test("the hovered surface is really a DIFFERENT surface, in both schemes", async () => {
  // Checked against `--panel` as web/style.css declares it, not against a copy of it here: a tint
  // a hair off the resting colour is a rule that runs and cannot be seen, which is the failure
  // mode this treatment actually has.
  const resting = tokenValues(BASE_CSS, "--panel");
  const hovered = tokenValues(CSS, "--row-hover");
  assert.equal(resting.length, 2, "web/style.css no longer declares --panel for both schemes");
  assert.equal(hovered.length, 2, "--row-hover is missing a colour scheme, so one of them is blank");
  for (const [i, scheme] of ["dark", "light"].entries()) {
    assert.notEqual(
      hovered[i].toLowerCase(),
      resting[i].toLowerCase(),
      `in ${scheme} the hovered row is the same colour as a resting one`
    );
  }
});

test("a touch screen keeps its focus tint, and a desk draws the focused row only as the selection", async () => {
  // A finger on a row's control focuses it on some phones, and the row was lit for it; that stays,
  // for touch screens, where a tap draws no selection of its own. On a desk the focus and the
  // selection are one row (web/voice.js moves them together), and a second rule lighting the focus
  // would be a second lit row the moment they parted. `#239 single-selection`.
  const selector = "#discord-log li.discord-message:focus-within";
  const TOUCH_QUERY = "(hover: none), (pointer: coarse)";
  const focused = cssBlockIn(TOUCH_QUERY, selector);
  assert.match(focused, /background:\s*var\(--row-hover\)/, "focus gets no treatment on a touch screen");
  assert.match(focused, /border-color:\s*var\(--accent\)/);
  assert.doesNotMatch(cssBlockOutside(TOUCH_QUERY, selector), /background|border-color/,
    "the focus tint is ALSO declared outside the touch query, so a desk lights the focus beside the selection");
  // A touch laptop answers that query as a desk does, and its finger keeps the tint by the root's
  // `data-input="touch"`, which web/voice.js sets on a tap and takes off at the next real mouse move
  // or key. On every device, and only behind that attribute.
  for (const list of ["#side-log", "#discord-log", "#pinned-log"]) {
    const tapped = cssRules(CSS_UNCONDITIONAL, `:root[data-input="touch"] ${list} li.discord-message:focus-within`).join("\n");
    assert.match(tapped, /background:\s*var\(--row-hover\)/, `a finger on a touch laptop lights no row in ${list}`);
    assert.match(tapped, /border-color:\s*var\(--accent\)/);
  }
  for (const rule of CSS_UNCONDITIONAL.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    if (!/background/.test(rule[2])) continue;
    for (const part of rule[1].split(",").map((one) => one.trim())) {
      if (!/li\.discord-message:focus-within$/.test(part)) continue;
      assert.match(part, /^:root\[data-input="touch"\] /, `${part} lights the focus on a desk, beside the selection`);
    }
  }
  assert.match(SCRIPT_CODE, /setAttribute\("data-input", "touch"\)/, "nothing says a finger is in use");
});

// --- two messages that are really one -----------------------------------------------------------
//
// The owner's report: an agent writes past Discord's length limit, its client splits the post, and
// the second half continues mid-word a fraction of a second after the first. Read as two rows it is
// unreadable. So the channel view DRAWS them as one — and only draws, which is the half these
// tests exist to hold: every id that leaves this page is still a real Discord id, and every act on
// a combined row reaches every message in it.

/** How close the page draws two messages as one, read from the page rather than restated here. */
const GLOM_WINDOW_MS = sourceConstant("GLOM_WINDOW_MS");

/** The rendered lines of one row's body, which is where the seam between two halves shows. */
const bodyLines = (li) =>
  li
    .descendants()
    .find((node) => node.hasClass("body"))
    .children.map((node) => node.text());

test("A POST DISCORD SPLIT IN TWO IS DRAWN AS ONE ROW, with a line break at the seam", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, splitPost());

  assert.equal(rows.length, 1, "the two halves of one post are still two rows");
  // DRAWN, not merely grouped. The seam is a rendered line break: without it the second half runs
  // on from the first as one mangled sentence, which is the complaint rather than the fix.
  assert.deepEqual(bodyLines(rows[0]), [
    "the overnight run finished and the runner is",
    "back up again.",
  ]);
  // ...and the row is honest about what it is standing for.
  assert.equal(
    rows[0].getAttribute("data-ids"),
    "1000000000000000001 1000000000000000002",
    "the row does not say which messages it is showing"
  );
  assert.equal(
    rows[0].getAttribute("data-id"),
    "1000000000000000001",
    "the row is filed under something other than its first message"
  );
});

test("...and messages FURTHER APART than the window stay two rows", async () => {
  // The negative control. Without it "everything is one row" would satisfy the test above.
  const page = newPage();
  await signIn(page);
  const apart = new Date(Date.parse("2026-08-19T04:31:00.000Z") + GLOM_WINDOW_MS + 1000);
  const rows = await showDiscord(
    page,
    splitPost(undefined, { timestamp: apart.toISOString() })
  );

  assert.equal(
    rows.length,
    2,
    `messages ${GLOM_WINDOW_MS + 1000}ms apart were combined, which is past the window`
  );
});

test("...and two messages seconds apart from DIFFERENT AUTHORS stay two rows", async () => {
  // The other half of the rule, and the one that matters most: two people answering at once is a
  // conversation, and welding it into one row would attribute one person's words to the other.
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(
    page,
    splitPost(undefined, { author: "alice", author_id: "30", author_is_bot: false })
  );

  assert.equal(rows.length, 2, "two authors were drawn as one speaker");
});

test("MARKING A COMBINED ROW AS DEALT WITH ARCHIVES EVERY MESSAGE IN IT", async () => {
  // The failure this is against: the row greys, the reader moves on, and the trailing half is
  // still in the queue with nothing on screen able to reach it.
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, splitPost());

  await doneButton(rows[0]).click();
  await page.settle();

  assert.deepEqual(page.dismissCalls, [
    { messages: ["1000000000000000001", "1000000000000000002"] },
  ]);
  assert.equal(rowState(page, 0).archived, "true", "the row did not grey once it was dealt with");
  // ...and the same control puts the WHOLE row back.
  await doneButton(row(page, 0)).click();
  await page.settle();
  assert.deepEqual(page.restoreCalls, [
    { messages: ["1000000000000000001", "1000000000000000002"] },
  ]);
});

test("...and so does a SWIPE, which is the other way into the same act", async () => {
  // The gesture is a second way in, never the only one, and it must not be a second NOTION of
  // dealt with either: a swipe that left half the row in the queue would be silently different
  // from the button beside it.
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, splitPost());
  const commit = sourceConstant("SWIPE_COMMIT_PX") + 10;
  const li = rows[0];
  const touch = (x) => ({ pointerId: 1, pointerType: "touch", clientX: x, clientY: 40 });

  await li.dispatch("pointerdown", touch(300));
  await li.dispatch("pointermove", touch(300 - commit));
  await li.dispatch("pointerup", touch(300 - commit));
  await page.settle();

  assert.deepEqual(page.dismissCalls, [
    { messages: ["1000000000000000001", "1000000000000000002"] },
  ]);
});

test("A HALF-ARCHIVED COMBINED ROW SHOWS AS UNREAD, and Done finishes it rather than undoing it", async () => {
  // The case the owner flagged: read state can disagree inside a group, and the two readings are
  // not equally safe. Showing it as read hides a message nobody has seen behind one they have.
  const page = newPage();
  await signIn(page);
  page.dealtWith.add("1000000000000000001");
  const rows = await showDiscord(page, splitPost());

  assert.equal(rows.length, 1, "the fixture is not exercising a combined row at all");
  assert.equal(
    rowState(page, 0).archived,
    "false",
    "half the row has never been seen and it is drawn as dealt with"
  );

  // The act the row offers is therefore Done, and Done must archive the OUTSTANDING half rather
  // than putting the archived one back.
  await doneButton(rows[0]).click();
  await page.settle();
  assert.deepEqual(page.restoreCalls, [], "finishing a half-read row un-archived it instead");
  assert.equal(rowState(page, 0).archived, "true", "the row never settles into being dealt with");
});

test("A REPLY TO A COMBINED ROW GOES TO THE FIRST MESSAGE'S REAL DISCORD ID", async () => {
  // The other one the owner flagged. Discord threads the answer under whichever id it is given,
  // and the group is a primary message with a tail: an answer threaded under the tail is filed
  // under a fragment.
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, splitPost());

  await replyButton(rows[0]).click();
  assert.ok(replyInStream(page));
  // The whole row is the message answered, under which the box opens (`#410 inline-reply-editor`).
  assert.deepStrictEqual(inlineAnchors(page), [`discord-log:${rows[0].getAttribute("data-id")}`],
    "the box is not under the row Reply was pressed on");
  assert.match(
    page.el("reply-target-meta").textContent,
    /id 1000000000000000001$/,
    "the screen names an id other than the one the answer will thread under"
  );

  page.el("reply-text").value = "thanks";
  await page.el("reply-send").click();
  await page.settle();

  assert.equal(page.repliesPosted.length, 1);
  assert.equal(
    page.repliesPosted[0].body.reply_to,
    "1000000000000000001",
    "the reply was threaded under the trailing overflow rather than the message"
  );
});

test("READING A COMBINED ROW ALOUD READS ALL OF IT, and archives it only when the last part ends", async () => {
  const page = newPage();
  page.prepareFails = true; // Exercise the audio-blob fallback, even when all parts can be prepared.
  await signIn(page);
  const rows = await inReadingMode(page, splitPost());

  await rows[0].dispatch("click", {});
  await page.settle();

  // Both parts fetched, in order — the server speaks one message per request, so the combined
  // text is two requests played back to back.
  assert.deepEqual(
    page.speakCalls,
    ["1000000000000000001", "1000000000000000002"],
    "the row was read half-way and the reader heard a sentence stop mid-word"
  );
  assert.equal(page.players.length, 2, "one player for two parts");
  assert.equal(page.players[0].playCount, 1, "the first part never played");
  assert.equal(page.players[1].playCount, 0, "both parts played over each other");

  await page.players[0].end();
  await page.settle();
  assert.equal(page.players[1].playCount, 1, "the second part never followed the first");
  assert.deepEqual(page.dismissCalls, [], "the row was filed away with half of it unheard");
  assert.equal(rowState(page, 0).reading, "true", "the row stopped saying it was speaking");

  await page.players[1].end();
  await page.settle();
  assert.deepEqual(page.dismissCalls, [
    { messages: ["1000000000000000001", "1000000000000000002"] },
  ]);
  assert.equal(rowState(page, 0).reading, "false", "the row still claims to be playing");
});

test("a STREAMED combined row requests each part once the one before has audio, under one selection", async () => {
  // The server speaks one read at a time on a shared session. Requesting the second part only
  // after the first has audio queues it behind the first there, in order; sharing the tap's
  // selection is what tells the server it is a continuation and not a new tap to interrupt for.
  const page = newPage();
  await signIn(page);
  const rows = await inReadingMode(page, splitPost());

  await rows[0].dispatch("click", {});
  await page.settle();
  assert.deepEqual(page.speakCalls, [], "a prepared row fell back to buffered audio");
  assert.equal(page.players.length, 1, "the second part was requested before the first had audio");

  await page.players[0].loaded();
  await page.settle();
  assert.equal(page.players.length, 2, "the second part was never requested");
  const selection = (url) => new URL(url, "http://page").searchParams.get("selection");
  assert.match(selection(page.players[0].url), /^tap-[0-9a-f]{16}$/);
  assert.equal(
    selection(page.players[1].url),
    selection(page.players[0].url),
    "the second part claimed to be a different tap and would interrupt the first"
  );
  assert.equal(page.players[1].playCount, 0, "both parts played over each other");

  await page.players[0].end();
  await page.settle();
  assert.equal(page.players[1].playCount, 1, "the second part never followed the first");
  await page.players[1].end();
  await page.settle();
  assert.deepEqual(page.dismissCalls, [
    { messages: ["1000000000000000001", "1000000000000000002"] },
  ]);
});

test("stopping a streamed read drops its source, and the next tap is a new selection", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await inReadingMode(page, [
    message({ id: "1000000000000000001", author_id: "1000000000000000011", content: "first" }),
    message({ id: "1000000000000000002", author_id: "1000000000000000012", content: "second" }),
  ]);
  assert.equal(rows.length, 2, "the fixture was drawn as one row");

  await rows[0].dispatch("click", {});
  await page.settle();
  await rows[1].dispatch("click", {});
  await page.settle();

  assert.equal(page.players.length, 2);
  assert.equal(page.players[0].paused, true, "the first read kept playing");
  assert.equal(
    page.players[0].sourceDropped,
    true,
    "the first read's response stayed open, so the server generates on for nobody"
  );
  assert.equal(page.players[0].loadCount, 1);
  const selection = (url) => new URL(url, "http://page").searchParams.get("selection");
  assert.notEqual(
    selection(page.players[1].url),
    selection(page.players[0].url),
    "a new tap reused the old selection, so the server would queue it instead of switching"
  );
  assert.equal(page.players[1].playCount, 1, "the second row never played");

  await page.players[1].loaded();
  page.setClock(page.clock() + 10);
  await page.players[1].started();
  await page.settle();
  assert.equal(
    page.speechTimingCalls.at(-1).ticket,
    "ticket-for-1000000000000000002",
    "the timing report was not addressed to the bare ticket"
  );
});

test("a prepared read PLAYS AS IT ARRIVES, rather than after the ~225 KB an <audio> buffers first", async () => {
  // Chromium's media element reads about 225 KB of a streamed WAV before it reports anything, and a
  // read arrives at speaking pace: 48 KB a second. That was 4.7 seconds of silence after the server
  // already had audio. So where the browser can, the page reads the body and plays each piece.
  const page = newPage();
  page.enableStreamedAudio();
  await signIn(page);
  const rows = await inReadingMode(page, [message({ id: "1000000000000000001" })]);
  const tappedAt = page.clock();

  await rows[0].dispatch("click", {});
  await page.settle();
  assert.equal(page.players.length, 0, "the read was handed to an <audio>, which waits seconds to start");
  assert.equal(page.audioStreams.length, 1, "the prepared read was not fetched");
  const [stream] = page.audioStreams;
  assert.match(stream.url, /^\/api\/v1\/speech\/ticket-for-1000000000000000001\?selection=tap-[0-9a-f]{16}$/);
  assert.equal(stream.options.cache, "no-store");
  const audio = page.audio;

  // A header split across chunks, and a sample split across the next boundary.
  const header = wavHeader(24000, 1);
  const samples = pcm(0x4000, -0x8000, 0x2000);
  stream.controller.bytes(header.subarray(0, 20));
  await page.settle();
  assert.equal(audio.sources.length, 0, "half a header was played as sound");
  page.setClock(tappedAt + 40);
  stream.controller.bytes(joined(header.subarray(20), samples.subarray(0, 3)));
  await page.settle();
  assert.equal(audio.sources.length, 1, "the first samples were held back");
  assert.deepEqual([...audio.sources[0].buffer.getChannelData(0)], [0.5]);
  // Half a second banked: a read arrives at speaking pace, so this lead is its whole cushion
  // against a network stall.
  assert.equal(audio.sources[0].startedAt, 0.5, "the first piece was not given its lead");

  // `playing` is when that lead has run out, not when the piece was queued.
  page.setClock(tappedAt + 540);
  assert.equal(page.expireTimers(500), 1, "nothing waited for the first sample to reach the speaker");
  await page.settle();
  // The very first sample is loud, so speech begins exactly when playback does. `#191
  // read-aloud-first-lag`; a read that opens with silence is the next test.
  assert.deepEqual(page.speechTimingCalls.map((call) => call.timing), [{
    tap_to_play_ms: 0,
    request_to_loaded_ms: 40,
    loaded_to_playing_ms: 500,
    tap_to_audible_ms: 540,
    tap_to_speech_ms: 540,
  }]);

  stream.controller.bytes(samples.subarray(3));
  await page.settle();
  assert.equal(audio.sources.length, 2);
  assert.deepEqual([...audio.sources[1].buffer.getChannelData(0)], [-1, 0.25], "a split sample was garbled");
  assert.equal(audio.sources[1].startedAt, 0.5 + 1 / 24000, "the next piece did not follow on without a gap");

  stream.controller.drop();
  await page.settle();
  audio.sources[0].onended();
  await page.settle();
  assert.deepEqual(page.dismissCalls, [], "archived while its last samples were still to be heard");
  audio.sources[1].onended();
  await page.settle();
  assert.deepEqual(page.dismissCalls, [{ messages: ["1000000000000000001"] }]);
  assert.equal(audio.state, "suspended", "the audio output was held open after the read ended");
});

test("a streamed read reports when SPEECH began, after the silence it opens with", async () => {
  // `#191 read-aloud-first-lag`. Every read measured opens with 0.55 to 0.8 s of silence, which
  // `playing` counts as audible. The report also says when the first loud sample was due.
  const page = newPage();
  page.enableStreamedAudio();
  await signIn(page);
  const rows = await inReadingMode(page, [message({ id: "1000000000000000001" })]);
  const tappedAt = page.clock();
  await rows[0].dispatch("click", {});
  await page.settle();
  const [stream] = page.audioStreams;

  // A quarter of a second of near-silence, at 1000 frames a second: nothing loud yet.
  page.setClock(tappedAt + 40);
  stream.controller.bytes(joined(wavHeader(1000, 1), pcm(...new Array(250).fill(20))));
  await page.settle();
  page.setClock(tappedAt + 540);
  assert.equal(page.expireTimers(500), 1, "nothing waited for the first sample to reach the speaker");
  await page.settle();
  assert.deepEqual(page.speechTimingCalls, [], "the report went before speech could be timed");

  // Then speech, 0.25 s into the stream, arriving 60 ms after the first sample began to play.
  page.setClock(tappedAt + 600);
  page.audio.currentTime = 0.56;
  stream.controller.bytes(pcm(20, 20, 0x2000, 0x2000));
  await page.settle();
  assert.equal(page.speechTimingCalls.length, 1, "speech arrived and nothing was reported");
  const timing = page.speechTimingCalls[0].timing;
  assert.equal(timing.tap_to_audible_ms, 540);
  // 40 ms to the first frame, the 500 ms lead, 252 frames at 1 kHz: speech due at 792 ms.
  assert.equal(timing.tap_to_speech_ms, 792, JSON.stringify(timing));
});

test("...and a read that stays quiet still reports, without a speech time, after a bounded wait", async () => {
  const page = newPage();
  page.enableStreamedAudio();
  await signIn(page);
  const rows = await inReadingMode(page, [message({ id: "1000000000000000001" })]);
  await rows[0].dispatch("click", {});
  await page.settle();
  page.audioStreams[0].controller.bytes(joined(wavHeader(1000, 1), pcm(1, 2, 3)));
  await page.settle();
  page.expireTimers(500);
  await page.settle();
  assert.deepEqual(page.speechTimingCalls, []);
  assert.equal(page.expireTimers(5000), 1, "nothing bounded the wait for speech");
  await page.settle();
  assert.equal(page.speechTimingCalls.length, 1, "a quiet read was never reported");
  assert.equal("tap_to_speech_ms" in page.speechTimingCalls[0].timing, false);
});

test("read-aloud keeps the screen on while it is on, and asks again after the page comes back", async () => {
  // A phone that locks its screen mid-read cuts the audio off, so the page asks Chrome to keep the
  // screen on for as long as read-aloud is on — and only that long.
  const page = newPage();
  page.enableStreamedAudio();
  page.enableWakeLock();
  await signIn(page);
  assert.equal(page.wakeLocks.length, 0, "an idle page held the screen on");

  await inReadingMode(page, [message({ id: "1000000000000000001" })]);
  await page.settle();
  const active = () => page.wakeLocks.filter((lock) => !lock.released);
  assert.equal(active().length, 1, "read-aloud did not keep the screen on");
  assert.equal(active()[0].type, "screen");

  // Hidden: the browser drops every lock. Back: the page has to ask again.
  await page.releaseAllWakeLocks();
  await page.setVisibility("hidden");
  await page.settle();
  assert.equal(active().length, 0, "the page asked for a lock while hidden");
  await page.setVisibility("visible");
  await page.settle();
  assert.equal(active().length, 1, "coming back did not ask for the screen to stay on again");

  // Read-aloud off: the page lets the screen sleep again.
  await readButton(page).click();
  await page.settle();
  assert.equal(active().length, 0, "the screen was held on after read-aloud stopped");
});

test("a SWITCH aborts the first read's response and silences what it had queued, on one context", async () => {
  // Closing the response is what tells the server to interrupt the agent and begin the next read,
  // so a streamed read has to abort its fetch as surely as `<audio>` dropped its source.
  const page = newPage();
  page.enableStreamedAudio();
  await signIn(page);
  const rows = await inReadingMode(page, [
    message({ id: "1000000000000000001", author_id: "1000000000000000011", content: "first" }),
    message({ id: "1000000000000000002", author_id: "1000000000000000012", content: "second" }),
  ]);

  await rows[0].dispatch("click", {});
  await page.settle();
  const audio = page.audio;
  page.audioStreams[0].controller.bytes(joined(wavHeader(24000, 1), pcm(1, 2, 3)));
  await page.settle();
  await rows[1].dispatch("click", {});
  await page.settle();

  assert.equal(page.audioStreams[0].options.signal.aborted, true, "the first response was left open");
  assert.equal(audio.sources[0].halted, true, "the first read kept sounding under the second");
  assert.equal(page.audioStreams.length, 2, "the second row was never requested");
  const selection = (url) => new URL(url, "http://page").searchParams.get("selection");
  assert.notEqual(selection(page.audioStreams[1].url), selection(page.audioStreams[0].url));

  page.audioStreams[1].controller.bytes(joined(wavHeader(24000, 1), pcm(4, 5)));
  await page.settle();
  assert.equal(page.audio, audio, "every tap built another audio context");
  assert.equal(audio.sources.length, 2);
  assert.equal(audio.sources[1].halted, false);
  assert.deepEqual(page.dismissCalls, [], "the interrupted read was filed away as heard");
});

test("a streamed read CUT OFF part-way, or refused, says so and archives nothing", async () => {
  const page = newPage();
  page.enableStreamedAudio();
  await signIn(page);
  const rows = await inReadingMode(page, [
    message({ id: "1000000000000000001", author_id: "1000000000000000011", content: "first" }),
    message({ id: "1000000000000000002", author_id: "1000000000000000012", content: "second" }),
  ]);

  await rows[0].dispatch("click", {});
  await page.settle();
  page.audioStreams[0].controller.bytes(joined(wavHeader(24000, 1), pcm(1, 2, 3)));
  await page.settle();
  // A body that ends without its terminating chunk did not END, whatever was heard of it. Cut off,
  // it is tried again (`#487 read-aloud-recovery`), and given up when every try is cut off too.
  page.audioStreams[0].controller.fail();
  await page.settle();
  assert.equal(page.audio.sources[0].halted, true);
  for (const wait of page.pure("READ_RECOVERY_WAITS_MS")) {
    assert.match(page.el("status").textContent, /cut off; trying again/);
    assert.equal(page.expireTimers(wait), 1, `nothing waited ${wait} ms to try again`);
    await page.settle();
    page.audioStreams.at(-1).controller.fail();
    await page.settle();
  }
  assert.match(page.el("status").textContent, /Cloud voice: the connection to it kept dropping\. The message was not archived\./);
  assert.equal(rowState(page, 0).reading, "false", "the row is stuck looking like it is speaking");

  // Refused outright, not lost: said at once, and not tried again.
  page.ticketStatus = 500;
  const asked = page.audioStreams.length;
  await rows[1].dispatch("click", {});
  await page.settle();
  assert.equal(page.audio.sources.length, 1, "a refused read played something");
  assert.match(page.el("status").textContent, /Cloud voice: it could not play this message\./);
  assert.equal(page.audioStreams.length, asked + 1, "a refused read was asked for again");
  assert.deepEqual(page.dismissCalls, [], "a read nobody heard to the end was archived");
});

// --- which day a message was sent -----------------------------------------------------------------
//
// `#260 timestamp-hover`. A row's time is the hour and minute; hovering it says the day as well, in
// `dayLabel`'s words, which the details sheet uses too. Each test runs in one named zone, because a
// day is a fact about where the reader is, and New York has both clock changes in it.

/** Run `fn` with this process — and so the page's Date getters and its formatters — in `zone`. */
async function inZone(zone, fn) {
  const was = process.env.TZ;
  process.env.TZ = zone;
  try {
    return await fn();
  } finally {
    if (was === undefined) delete process.env.TZ;
    else process.env.TZ = was;
  }
}

/** A local instant in the zone in force: month counted from 1, as people count it. */
const localAt = (year, month, day, hour = 12, minute = 0) => new Date(year, month - 1, day, hour, minute);

/** The time a row shows for `at`: the browser's own short time, as the row's formatter makes it. */
const shownTime = (at) => at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });

test("ONE DAY FORMATTER: today, yesterday, this year, another year, across New Year, and across both clock changes", async () => {
  const page = newPage();
  const dayLabel = page.pure("dayLabel");
  await inZone("America/New_York", () => {
    const now = localAt(2026, 10, 12, 15, 56);
    const cases = [
      [localAt(2026, 10, 12, 0, 1), now, "Today, Oct 12, Mon"],
      // The owner's case: 5:25 PM on a page that says 3:56 PM is yesterday's.
      [localAt(2026, 10, 11, 17, 25), now, "Yesterday, Oct 11, Sun"],
      [localAt(2026, 10, 10, 23, 59), now, "Oct 10, Sat"],
      [localAt(2026, 10, 9, 8, 0), now, "Oct 9, Fri"],
      [localAt(2025, 10, 9), now, "Oct 9, 2025, Thu"],
      // A year ago today is not Today, nor a year and a day ago Yesterday: the year is a field too.
      [localAt(2025, 10, 12), now, "Oct 12, 2025, Sun"],
      [localAt(2025, 10, 11), now, "Oct 11, 2025, Sat"],
      // A clock behind the server's: a day after the reader's is a date, not a word.
      [localAt(2026, 10, 13, 9, 0), now, "Oct 13, Tue"],
      // New Year: the evening before is Yesterday and says its year; the new year's first hour does not.
      [localAt(2025, 12, 31, 23, 50), localAt(2026, 1, 1, 0, 30), "Yesterday, Dec 31, 2025, Wed"],
      [localAt(2026, 1, 1, 0, 5), localAt(2026, 1, 1, 0, 30), "Today, Jan 1, Thu"],
      // Spring forward: Sunday Mar 8 has 23 hours, so 24 hours before Monday 00:30 is Saturday night.
      [localAt(2026, 3, 8, 12), localAt(2026, 3, 9, 0, 30), "Yesterday, Mar 8, Sun"],
      [localAt(2026, 3, 7, 23, 45), localAt(2026, 3, 9, 0, 30), "Mar 7, Sat"],
      // Fall back: Sunday Nov 1 has 25 hours, so 24 hours before its 23:30 is still Sunday.
      [localAt(2026, 10, 31, 12), localAt(2026, 11, 1, 23, 30), "Yesterday, Oct 31, Sat"],
      [localAt(2026, 11, 1, 0, 30), localAt(2026, 11, 1, 23, 30), "Today, Nov 1, Sun"],
    ];
    for (const [when, at, want] of cases) {
      assert.equal(dayLabel(when, at), want, `${when.toString()} seen at ${at.toString()}`);
    }
  });
  // The negative control: "yesterday" as 24 hours back, not a calendar day back, and both clock
  // changes above come out wrong — so those rows of the table are what pin the calendar down.
  const bySpan = newPage(new Map(), brokenScript(
    "const yesterday = new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1, 12);",
    "const yesterday = new Date(now.getTime() - 86400000);"
  )).pure("dayLabel");
  await inZone("America/New_York", () => {
    assert.equal(bySpan(localAt(2026, 3, 8, 12), localAt(2026, 3, 9, 0, 30)), "Mar 8, Sun",
      "the control did not get the spring change wrong");
    assert.equal(bySpan(localAt(2026, 10, 31, 12), localAt(2026, 11, 1, 23, 30)), "Oct 31, Sat",
      "the control did not get the autumn change wrong");
  });
});

test("A ROW'S TIME SAYS ITS DAY ON HOVER: yesterday's says Yesterday, and the words follow the clock past midnight", async () => {
  await inZone("America/New_York", async () => {
    const page = newPage();
    await signIn(page);
    // The fixture's clock: Wednesday Aug 19 2026, 10:05 in New York.
    const before = localAt(2026, 8, 18, 17, 25);
    const morning = localAt(2026, 8, 19, 9, 10);
    await showDiscord(page, [
      message({ id: "601", content: "from yesterday", timestamp: before.toISOString() }),
      message({ id: "602", content: "from this morning", timestamp: morning.toISOString() }),
    ]);
    const time = (id) => rowWithId(page, id).descendants().find((node) => node.className === "msg-time");
    const old = time("601");
    assert.equal(old.tagName, "time", "a row's time is not a <time>");
    assert.equal(old.getAttribute("datetime"), before.toISOString(), "the <time> does not say which instant it is");
    assert.equal(old.textContent, shownTime(before), "the row's own time changed");
    assert.equal(old.getAttribute("title"), `Yesterday, Aug 18, Tue, ${shownTime(before)}`);
    const recent = time("602");
    assert.equal(recent.getAttribute("title"), `Today, Aug 19, Wed, ${shownTime(morning)}`);

    // Midnight passes with the page open. Nothing redraws the rows; the pointer arriving rewords them.
    page.setClock(localAt(2026, 8, 20, 0, 5).getTime());
    await page.documentEvent("pointerover", { target: recent });
    assert.equal(recent.getAttribute("title"), `Yesterday, Aug 19, Wed, ${shownTime(morning)}`,
      "a page left open past midnight still called this morning Today");
    await page.documentEvent("pointerover", { target: old });
    assert.equal(old.getAttribute("title"), `Aug 18, Tue, ${shownTime(before)}`);
    assert.equal(old.textContent, shownTime(before), "the hover changed the row's own time");
    // A pointer over anything else is passed over, and nothing is redrawn to say it.
    await page.documentEvent("pointerover", { target: rowWithId(page, "601") });
    await page.documentEvent("pointerover", { target: null });
    assert.equal(time("601"), old, "a hover redrew the row");
    assert.equal(rowWithId(page, "601").getAttribute("title"), null, "a hover over the row titled the row");

    // The negative control: without the rewording on hover, the morning after still says Today.
    const stale = newPage(new Map(), brokenScript('document.addEventListener("pointerover", freshenDayTitle);', ""));
    await signIn(stale);
    await showDiscord(stale, [message({ id: "602", content: "from this morning", timestamp: morning.toISOString() })]);
    const kept = rowWithId(stale, "602").descendants().find((node) => node.className === "msg-time");
    stale.setClock(localAt(2026, 8, 20, 0, 5).getTime());
    await stale.documentEvent("pointerover", { target: kept });
    assert.equal(kept.getAttribute("title"), `Today, Aug 19, Wed, ${shownTime(morning)}`,
      "the control's title moved with the clock, so the test above does not show the hover doing it");
  });
});

test("a restored transcript line says its day on hover too, beside the time it shows", async () => {
  await inZone("America/New_York", async () => {
    const page = newPage();
    const said = localAt(2026, 8, 18, 21, 40);
    page.storedTurns.set("conv_earlier", [{ speaker: "you", text: "what happened overnight", at_ms: said.getTime() }]);
    await signIn(page);
    await page.settle();
    const [line] = page.el("transcript").children.filter((li) => li.className !== "seam");
    const at = line.descendants().find((node) => node.className === "at");
    assert.equal(at.tagName, "time");
    assert.equal(at.textContent, "21:40", "the transcript's own time changed");
    assert.equal(at.getAttribute("title"), "Yesterday, Aug 18, Tue, 21:40");
  });
});

test("`#479 message-time-full-date`: a time's day is its name, a tap or Enter on it says it, and the selected row's time is a Tab stop", async () => {
  await inZone("America/New_York", async () => {
    const page = newPage();
    await signIn(page);
    const before = localAt(2026, 8, 18, 23, 9);
    const older = localAt(2025, 10, 3, 23, 9);
    await showDiscord(page, [
      message({ id: "701", content: "late yesterday", timestamp: before.toISOString() }),
      message({ id: "702", content: "last year", timestamp: older.toISOString() }),
    ]);
    const time = (id) => rowWithId(page, id).descendants().find((node) => node.className === "msg-time");
    const zoned = before.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", timeZoneName: "short" });
    // Hover's words are its name too, year only when it is not this year; no Tab stop of its own.
    assert.equal(time("701").getAttribute("aria-label"), `Yesterday, Aug 18, Tue, ${shownTime(before)}`);
    assert.equal(time("701").getAttribute("title"), time("701").getAttribute("aria-label"));
    assert.match(time("702").getAttribute("aria-label"), /^Oct 3, 2025, Fri, /);
    assert.equal(time("701").getAttribute("tabindex"), "-1", "an unselected row's time is a Tab stop");
    // The selected row's time shares the row's stop, and only that row's.
    page.el("scroll-area").scrollTop = 0;
    await press(page, page.el("scroll-area"), "ArrowDown");
    const selected = selectedId(page);
    const other = selected === "701" ? "702" : "701";
    assert.equal(time(selected).getAttribute("tabindex"), "0",
      `the selected row's time cannot be reached by Tab (selected=${selected}, row tabindex=${rowWithId(page, selected).getAttribute("tabindex")})`);
    assert.equal(time(other).getAttribute("tabindex"), "-1", "another row's time took a Tab stop too");
    // A tap on the time says the whole day on the status line; the press is still the row's.
    await page.documentEvent("click", { target: time("701") });
    assert.equal(statusText(page), `Yesterday, Aug 18, Tue, ${zoned}`);
    // Enter on the focused time says it the same way; another year says its year.
    const enter = { key: "Enter", target: time("702"), stopped: false, stopPropagation() { this.stopped = true; }, preventDefault() {} };
    await page.documentEvent("keydown", enter);
    assert.equal(enter.stopped, true, "Enter on a time fell through to the row's keys");
    assert.match(statusText(page), /^Oct 3, 2025, Fri, /);
    // The words follow the clock on focus as on hover, name with title.
    page.setClock(localAt(2026, 8, 20, 0, 5).getTime());
    await page.documentEvent("focusin", { target: time("701") });
    assert.equal(time("701").getAttribute("aria-label"), `Aug 18, Tue, ${shownTime(before)}`,
      "a page left open past midnight still named the time Yesterday");
  });
});

test("the details sheet says the day in the same words, with the time and the zone", async () => {
  await inZone("America/New_York", async () => {
    const page = newPage();
    const fullLocalTime = page.pure("fullLocalTime");
    const before = localAt(2026, 8, 18, 17, 25);
    const zoned = before.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", timeZoneName: "short" });
    assert.equal(fullLocalTime({ timestamp: before.toISOString() }), `Yesterday, Aug 18, Tue, ${zoned}`);
    assert.ok(fullLocalTime({ timestamp: localAt(2025, 8, 18).toISOString() }).startsWith("Aug 18, 2025, Mon, "),
      "another year's message did not say its year");
    assert.equal(fullLocalTime({ timestamp: "", spoken_time: "five past ten" }), "five past ten");
  });
});

test("the first refresh fifteen minutes after a view's last full read reads it in full", async () => {
  const { page, server } = await forwardPage();
  await pollOnce(page);
  page.setClock(page.clock() + TIMELINE_RECONCILE_MS - 1000);
  await pollOnce(page);
  page.setClock(page.clock() + 1000);
  await pollOnce(page);
  await pollOnce(page);
  assert.deepStrictEqual(forwardReadKinds(server), [false, true, true, false, true],
    "the reconcile read did not come at fifteen minutes, or did not start the deltas again");
});

test("a channel whose deltas carry additions only keeps its full poll and its full pull", async () => {
  const { page, server } = await forwardPage({ complete: false });
  const arriving = server.post(message({ id: "350", content: "learns what the deltas carry" }));
  await deliver(page, page.stream(), sseMessage(arriving));
  await page.settle();
  const from = server.reads.length;
  await pollOnce(page);
  page.el("scroll-area").scrollTop = 0;
  await pullDown(page, PULL_ARM_PX);
  await page.settle();
  assert.deepStrictEqual(forwardReadKinds(server, from), [false, false],
    "a channel whose edits only full reads see was polled or pulled with a delta");
});

test("pull-to-refresh on a channel with complete deltas is a delta, and lands on the newest message", async () => {
  const { page, server } = await forwardPage();
  await pollOnce(page);
  server.post(message({ id: "355", content: "pulled in" }));
  page.el("scroll-area").scrollTop = 0;
  const from = server.reads.length;
  await pullDown(page, PULL_ARM_PX);
  await page.settle();
  assert.deepStrictEqual(forwardReadKinds(server, from), [true]);
  assert.equal(shownIds(page).at(-1), "355");
  assert.match(page.el("status").textContent, /something new had arrived/);
});

test("the pill says live after a delta, and slow when the server can vouch only for an older moment", async () => {
  const { page, server } = await forwardPage();
  await pollOnce(page);
  assert.match(freshness(page).textContent, /^(Live · u|U)pdated \d{2}:\d{2}$/);
  const asOf = page.clock() - 10 * 60000;
  server.asOf = new Date(asOf).toISOString();
  await pollOnce(page);
  const when = new Date(asOf);
  const at = `${String(when.getHours()).padStart(2, "0")}:${String(when.getMinutes()).padStart(2, "0")}`;
  assert.equal(freshness(page).getAttribute("data-state"), "slow");
  assert.match(freshness(page).textContent, new RegExp(`is slow to answer · showing messages from ${at}$`));
  server.asOf = null;
  await pollOnce(page);
  assert.equal(freshness(page).getAttribute("data-state"), "fresh", "a current answer still said slow");
});

test("a cold start reads in full: no forward cursor is kept on the device", async () => {
  const { page: first } = await forwardPage();
  await pollOnce(first);
  const saved = savedScopes(first)[scopeKey(CHANNEL.id)];
  assert.ok(Object.values(saved.views).every((cover) => !("newest" in cover) && !("fullAt" in cover) &&
    !("fullSent" in cover)));
  let server = null;
  const page = reloadPage(first.storage, (p) => {
    p.threadingSupported = true;
    p.threads = threadData().threads;
    p.messages = first.messages;
    server = forwardTimeline(p);
    holdCountsRead(p);
  });
  await page.settle();
  await page.el("view-switch").click();
  await page.settle();
  assert.deepStrictEqual(forwardReadKinds(server), [false], "a cold start sent a cursor it cannot have");
});

test("each view keeps its own forward cursor, and what one view's delta brings every view shows", async () => {
  const { page, server } = await forwardPage();
  await pickThread(page, "flat");
  assert.deepStrictEqual(server.reads.at(-1), { view: "flat", after: null, before: null },
    "entering All was not a full read of All");
  server.post(message({ id: "360", content: "read on All" }));
  await pollOnce(page);
  assert.match(server.reads.at(-1).after, /^flat@/, "All was refreshed with another view's cursor");
  assert.ok(shownIds(page).includes("360"));
  const reads = server.reads.length;
  await pickThread(page, "main");
  assert.equal(server.reads.length, reads, "returning to a held view read it");
  assert.ok(shownIds(page).includes("360"), "Main did not show the message All's delta brought");
  await pollOnce(page);
  assert.match(server.reads.at(-1).after, /^main@/, "Main lost its own cursor while All was read");
});

test("a delta for a view the reader has left is kept, and the view is current when it comes back", async () => {
  const { page, server } = await forwardPage();
  server.post(message({ id: "370", content: "landed while All was open" }));
  const held = gate(server.answer);
  server.hold = held;
  page.expireTimers(DISCORD_POLL_MS);
  await page.settle();
  server.hold = null;
  await pickThread(page, "flat");
  held.open();
  await page.settle();
  await page.settle();
  const reads = server.reads.length;
  await pickThread(page, "main");
  assert.equal(server.reads.length, reads, "coming back to Main read it again");
  assert.ok(shownIds(page).includes("370"), "the delta Main's poll read while All was open was lost");
  assert.equal(freshness(page).getAttribute("data-state"), "fresh");
});

test("a queued read standing for triggers that read differently is a delta only if every one could be", async () => {
  // Driven through the queue itself: no trigger the page has today puts a replay behind another
  // read, so a page-level path cannot reach this, and the rule must not depend on that staying so.
  // On a channel whose deltas carry additions only, a poll reads in full while a replay with no edit
  // waiting reads a delta; neither outranks the other, so the read standing for both is full.
  const probe = `${SCRIPT}\n;el("scroll-area").queueProbe = {\n` +
    "  queue: queueDiscordLoad,\n" +
    "  cursor: () => { const held = discordQueuedLoads.get(columns.main); return forwardCursor(held && held.options && held.options.reason); },\n" +
    "  clear: () => { discordQueuedLoads.clear(); },\n};\n";
  const { page, server } = await forwardPage({ complete: false }, null, probe);
  const arriving = server.post(message({ id: "385", content: "learns what the deltas carry" }));
  await deliver(page, page.stream(), sseMessage(arriving));
  await page.settle();
  const queue = page.el("scroll-area").queueProbe;
  const queued = (...reasons) => {
    queue.clear();
    for (const reason of reasons) queue.queue({ keepPosition: true, reason });
    const cursor = queue.cursor();
    queue.clear();
    return cursor !== null;
  };
  assert.equal(queued("replay"), true, "a replay with no edit waiting could not read a delta at all");
  assert.equal(queued("replay", "poll"), false, "a poll's full read was narrowed to a replay's delta");
  assert.equal(queued("poll", "replay"), false);
  assert.equal(queued("pull", "replay", "live"), false);
  assert.equal(queued("live", "visible"), true, "two triggers that both read deltas were made a full read");
  assert.equal(queued("live", "reset"), false);
  assert.equal(queued("live", undefined), false, "a read asked for with no reason was narrowed to a delta");
});

test("a delta chain overtaken partway still draws the pages it had folded", async () => {
  const { page, server } = await forwardPage();
  await page.el("channel-view-threads").click();
  await page.settle();
  const [first, second] = page.threads;
  page.threads[0] = { ...first, title: "First discussion, renamed", updated_at: "2026-08-19T06:00:00.000Z" };
  const firstMoved = (server.revision += 1);
  server.threadChanged.set(first.id, firstMoved);
  page.threads[1] = { ...second, title: "Second discussion, renamed", updated_at: "2026-08-19T06:01:00.000Z" };
  server.threadChanged.set(second.id, (server.revision += 1));
  server.deltaLimit = 1;
  // The poll's second page waits on the wire. Meanwhile the thread selector's own read of the list
  // continues from the cursor the first page left and moves it on, so the second page, when it
  // lands, no longer fits and is dropped.
  const held = gate(server.answer);
  let holding = true;
  server.hold = {
    respond: (path) => {
      if (holding && new URL(path, "http://fixture.test").searchParams.get("after") === `threads@${firstMoved}`) {
        holding = false;
        return held.respond(path);
      }
      return server.answer(path);
    },
  };
  page.expireTimers(DISCORD_POLL_MS);
  await page.settle();
  assert.equal(holding, false, "the poll did not continue its chain to a second page");
  page.setClock(page.clock() + 120000);
  await page.el("thread-select").dispatch("pointerdown");
  await page.settle();
  held.open();
  await page.settle();
  await page.settle();
  assert.match(page.el("thread-list").text(), /First discussion, renamed/,
    "a page the overtaken chain had already folded was never drawn");
});

test("a stream reset queued behind a read is not narrowed to a delta by a live message behind it", async () => {
  const { page, server } = await forwardPage();
  await pollOnce(page);
  const held = gate(server.answer);
  server.hold = held;
  page.expireTimers(DISCORD_POLL_MS);
  await page.settle();
  await deliver(page, page.stream(), "event: reset\ndata: {}\n\n");
  const arriving = server.post(message({ id: "380", content: "behind the reset" }));
  await deliver(page, page.stream(), sseMessage(arriving));
  server.hold = null;
  const from = server.reads.length;
  held.open();
  await page.settle();
  await page.settle();
  // The poll's delta that was on the wire, then the one queued read standing for both triggers.
  assert.deepStrictEqual(forwardReadKinds(server, from), [true, false], "the reset's full read was narrowed to a delta");
});

test("a message's reactions are drawn under its text as read-only chips; one without any has no strip", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [
    message({ id: "100", content: "please deploy", reactions: ACKNOWLEDGED }),
    message({ id: "101", content: "on it", reactions: [] }),
    message({ id: "102", content: "deployed" }),
  ]);
  assert.deepStrictEqual(chipsOf(rows[0]), ["👀 1", "✅ 1"]);
  assert.equal(chipsOf(rows[1]), null, "a message with none drew an empty strip");
  assert.equal(chipsOf(rows[2]), null, "a message whose source cannot see reactions drew a strip");
  // Its own line under the text: after the body, and nothing of it in the meta line, which the
  // row's controls fill.
  const order = rows[0].children.map((node) => node.className);
  assert.ok(order.indexOf("reactions") > order.indexOf("body"), `the strip is not under the text: ${order}`);
  const meta = rows[0].children.find((node) => node.className === "meta");
  assert.ok(!meta.descendants().some((node) => node.hasClass("reaction")), "a chip landed among the row's controls");
  const strip = reactionStripOf(rows[0]);
  assert.equal(strip.getAttribute("role"), "group");
  assert.equal(strip.getAttribute("aria-label"), "Reactions");
  // Labels, not controls: a tap on a chip is a tap on the row, and there is nothing to react with.
  for (const node of strip.descendants()) {
    assert.ok(!["button", "a", "input"].includes(node.tagName), `the strip holds a ${node.tagName}`);
  }
  assert.equal(strip.children[0].className, "reaction");
  // Not searchable: "eyes" is not a search for 👀, and the emoji itself matches no message text.
  await search(page, "👀");
  await page.settle();
  assert.ok(rows.every((li) => li.hasClass("search-hidden")), "a reaction made a row match a search");
});

test("a custom emoji is shown as its name between colons, as text, and two of one id are one chip", async () => {
  const page = newPage();
  await signIn(page);
  const hostile = "<img src=x onerror=alert(1)>";
  const rows = await showDiscord(page, [message({ id: "100", content: "shipped", reactions: [
    { emoji: "party-parrot", custom: true, custom_id: "112233", count: 3 },
    { emoji: "party-parrot", custom: true, custom_id: "445566", count: 1 },
    { emoji: hostile, custom: true, custom_id: "9", count: 1 },
    { emoji: "🎉", count: 2 },
  ] })]);
  assert.deepStrictEqual(chipsOf(rows[0]), [":party-parrot: 3", ":party-parrot: 1", `:${hostile}: 1`, "🎉 2"],
    "two custom emoji of one name but different ids were merged, or a name was not shown as text");
  assert.ok(reactionStripOf(rows[0]).children[0].hasClass("reaction-custom"));
  assert.ok(!reactionStripOf(rows[0]).children[3].hasClass("reaction-custom"));
  assert.ok(!page.createdTags.includes("img"), "an emoji's name became markup, or its image was fetched");
});

test("a reaction the owner is among is drawn as theirs, and says so to a screen reader", async () => {
  // `#312 slack-reactions`. Slack says who reacted, and the server marks the tallies the owner is
  // among. The chip is the owner's in the accent, still a label, and counted as before.
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [message({ id: "100", content: "please deploy", reactions: [
    { emoji: "👀", count: 2, owner_reacted: true },
    { emoji: "✅", count: 1 },
    { emoji: "party-parrot", custom: true, count: 1, owner_reacted: true },
  ] })]);
  const chips = reactionStripOf(rows[0]).children;
  assert.deepStrictEqual(chips.map((chip) => chip.className),
    ["reaction reaction-own", "reaction", "reaction reaction-custom reaction-own"]);
  assert.deepStrictEqual(chipsOf(rows[0]), ["👀 2 including you", "✅ 1", ":party-parrot: 1 including you"]);
  const said = chips[0].children[2];
  assert.equal(said.className, "sr-only", "the owner's reaction is shown only by its colour");
  assert.equal(chips[0].title, "You reacted");
  assert.equal(chips[1].title || "", "", "somebody else's reaction says it is the owner's");
  for (const node of reactionStripOf(rows[0]).descendants()) {
    assert.ok(!["button", "a", "input"].includes(node.tagName), `the strip holds a ${node.tagName}`);
  }
});

test("a combined row's chip is the owner's when the owner reacted to any of its messages", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, splitPost(
    { reactions: [{ emoji: "👀", count: 1 }] },
    { reactions: [{ emoji: "👀", count: 1, owner_reacted: true }] }
  ));
  assert.equal(rows.length, 1, "the fixture did not produce one combined row");
  assert.deepStrictEqual(chipsOf(rows[0]), ["👀 2 including you"]);
  assert.ok(reactionStripOf(rows[0]).children[0].hasClass("reaction-own"));
});

test("the owner's own chip is outlined in the accent and still has no hover", () => {
  const own = cssBlock(".reaction-own");
  assert.match(own, /border-color: var\(--accent\)/);
  assert.match(own, /background: color-mix\(in srgb, var\(--accent\) 16%, transparent\)/);
  assert.match(cssBlock(".reaction-own .reaction-count"), /color: var\(--ink\)/,
    "the owner's count is muted, or in the accent, which reads below 4.5:1 on the agent's dark rows");
  assert.doesNotMatch(own, /cursor: pointer/);
  assert.doesNotMatch(CSS_CODE, /\.reaction[\w-]*:hover/);
});

test("a combined row shows one strip, each emoji counted over all of its messages", async () => {
  const page = newPage();
  await signIn(page);
  // Both halves reached the bridge; only the first has reached the transcript yet. The sum shows
  // that as 👀 2 beside ✅ 1, where the larger count alone would have shown both as done.
  const rows = await showDiscord(page, splitPost(
    { reactions: ACKNOWLEDGED },
    { reactions: [{ emoji: "👀", count: 1 }, { emoji: "party-parrot", custom: true, custom_id: "7", count: 1 }] }
  ));
  assert.equal(rows.length, 1, "the fixture did not produce one combined row");
  assert.deepStrictEqual(chipsOf(rows[0]), ["👀 2", "✅ 1", ":party-parrot: 1"]);
  assert.equal(rows[0].children.filter((node) => node.className === "reactions").length, 1, "one strip per row");
  // With combining off, each message has its own row and its own strip.
  await page.el("combine-messages").setChecked(false);
  await page.settle();
  const apart = page.el("discord-log").children;
  assert.equal(apart.length, 2);
  assert.deepStrictEqual(apart.map(chipsOf), [["👀 1", "✅ 1"], ["👀 1", ":party-parrot: 1"]]);
});

test("a folded row keeps its reactions under whatever stands for its text, the summary included", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [message({ id: "100", content: longMessage(), reactions: ACKNOWLEDGED })]);
  assert.equal(rows[0].getAttribute("data-collapsed"), "true", "the fixture is not folded");
  const order = rows[0].children.map((node) => node.className);
  assert.equal(order[order.length - 1], "reactions", `the strip is not the row's last line: ${order}`);
  assert.ok(order.indexOf("summary") >= 0 && order.indexOf("summary") < order.indexOf("reactions"),
    `the summary that can stand in for the text sits below the reactions: ${order}`);
  assert.deepStrictEqual(chipsOf(rows[0]), ["👀 1", "✅ 1"]);
});

test("under the Links filter a row's reactions go with its text", async () => {
  assert.match(cssBlock('.messages li[data-links-view="true"] > .reactions'), /display: none !important/);
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [
    message({ id: "100", content: "the design doc: https://docs.example.com/design", reactions: ACKNOWLEDGED }),
  ]);
  await showLinks(page);
  assert.equal(rows[0].getAttribute("data-links-view"), "true", "the row is not showing its links");
  assert.ok(reactionStripOf(rows[0]), "the strip was taken out of the row rather than hidden with the text");
});

test("the reaction strip wraps inside a phone's width and is drawn as labels in the theme's colours", () => {
  const strip = cssBlock(".discord-message > .reactions");
  assert.match(strip, /display: flex/);
  assert.match(strip, /flex-wrap: wrap/, "a row of chips could push the row wider than a 360px screen");
  assert.match(strip, /var\(--msg-scale\)/, "the chips ignore the reader's message size");
  const chip = cssBlock(".reaction");
  assert.match(chip, /border: 1px solid var\(--edge\)/);
  assert.match(chip, /max-width: 100%/);
  assert.match(chip, /cursor: default/, "a chip offers itself as something to press");
  assert.match(cssBlock(".reaction-emoji"), /overflow-wrap: anywhere/, "a long custom name widens the row");
  assert.match(cssBlock(".reaction-count"), /color: var\(--muted\)/);
  assert.doesNotMatch(CSS_CODE, /\.reaction[\w-]*:hover/, "a chip changes under the pointer, as a control does");
  assert.equal(cssRules(CSS, ".meta .reactions").length, 0);
});

// `#242 rich-headers`. Agents posting through one shared account open with who they are, and the
// server lifts the tag, time and title out of the first line as `heading`, with `cut`, how much of
// the text they restate. The fixture's messages are in the shape the server sends: the plain bodies
// carry no `content_html`, and the rendered ones carry the mark src/render.rs writes.

const HEALTH = "[build-bot coordinator] Backup check, 10:52 PM ET. All 4 shards are synced.";
const HEALTH_HEADING = { tag: "build-bot coordinator", time: "10:52 PM ET", title: "Backup check", cut: 51 };

/** A row's header pieces as `[class, text]`, or null when it has none. */
const piecesOf = (li) => {
  const head = headOf(li);
  return head ? head.children.map((piece) => [piece.className, piece.textContent]) : null;
};
/** One drawn body as markup, so a test can say exactly what was built. */
function markupOf(node) {
  if (node.nodeType === 3) return node.textContent;
  const tag = String(node.tagName).toLowerCase();
  const cls = node.className ? ` class="${node.className}"` : "";
  const inner = node.children.map(markupOf).join("");
  return tag === "br" ? "<br>" : `<${tag}${cls}>${inner}</${tag}>`;
}
const bodyMarkup = (li, index = 0) => boxesOf(li)[index].children.map(markupOf).join("");

test("an agent's heading is drawn in the row's header as text, and the words it restates are marked", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [message({ id: "100", content: HEALTH, heading: HEALTH_HEADING })]);
  assert.deepStrictEqual(piecesOf(rows[0]), [
    ["rh-tag", "build-bot coordinator"], ["rh-title", "Backup check"], ["rh-time", "@10:52 PM ET"],
  ]);
  // After the time and before the row's buttons, and built of nothing but spans of text.
  const meta = metaOf(rows[0]).children.map((node) => node.className);
  assert.ok(meta.indexOf("rh-head") > meta.indexOf("msg-time"), `the header is not after the time: ${meta}`);
  // The buttons are held together after it (`#377 compact-collapsed-rows`).
  assert.ok(meta.indexOf("rh-head") < meta.indexOf("row-acts"), `the header is not before Reply: ${meta}`);
  for (const node of headOf(rows[0]).descendants()) assert.equal(node.tagName, "span");
  // The body is NOT cut: the restated words stay in it, in a span the stylesheet hides.
  assert.equal(
    bodyMarkup(rows[0]),
    '<p><span class="rh-src">[build-bot coordinator] Backup check, 10:52 PM ET. </span>All 4 shards are synced.</p>'
  );
  assert.equal(boxesOf(rows[0])[0].getAttribute("data-rh"), "");
  assert.equal(rows[0].getAttribute("data-rh-len"), "48", "45 characters of tag, title and time");
  // Search and the text a row says read the message as written.
  await search(page, "Backup check, 10:52");
  await page.settle();
  assert.ok(!rows[0].hasClass("search-hidden"), "the restated words stopped being searchable");
});

test("a rendered body's mark is the server's, and a heading whose words are not marked is not shown", async () => {
  const page = newPage();
  await signIn(page);
  const list = "[build-bot] Status, 9:05 AM ET:\n- one\n- two";
  const heading = { tag: "build-bot", time: "9:05 AM ET", title: "Status", cut: 31 };
  const rows = await showDiscord(page, [
    message({
      id: "100", content: list, heading,
      content_html: '<p class="rh-src">[build-bot] Status, 9:05 AM ET:</p>\n<ul>\n<li>one</li>\n<li>two</li>\n</ul>\n',
    }),
    // A body whose HTML carries no mark: an older copy, or anything else. Drawn as written.
    message({
      id: "101", content: list, heading,
      content_html: "<p>[build-bot] Status, 9:05 AM ET:</p>\n<ul>\n<li>one</li>\n<li>two</li>\n</ul>\n",
    }),
  ]);
  assert.deepStrictEqual(piecesOf(rows[0]).map(([, text]) => text), ["build-bot", "Status", "@9:05 AM ET"]);
  assert.ok(boxesOf(rows[0])[0].hasAttribute("data-rh"));
  assert.equal(piecesOf(rows[1]), null, "a heading was shown over words nothing marks for hiding");
  assert.ok(!boxesOf(rows[1])[0].hasAttribute("data-rh"));
  assert.ok(!rows[1].hasAttribute("data-rh-len"));
});

test("a plain body is marked as the server marks a rendered one, line break and paragraph included", async () => {
  const page = newPage();
  await signIn(page);
  const heading = (cut) => ({ tag: "build-bot", title: "Status", cut });
  const rows = await showDiscord(page, [
    // The words use up the line and the paragraph goes on: the line break is theirs.
    message({ id: "100", content: "[build-bot] Status:\nAll green.\nNothing else.", heading: heading(19) }),
    // The line is the whole paragraph: the paragraph carries the mark.
    message({ id: "101", content: "[build-bot] Status:\n\nAll green.", heading: heading(19) }),
    // CRLF: the carriage return is in the cut, so the line is used up.
    message({ id: "102", content: "[build-bot] Status:\r\nAll green.", heading: heading(20) }),
  ]);
  assert.equal(bodyMarkup(rows[0]), '<p><span class="rh-src">[build-bot] Status:<br></span>All green.<br>Nothing else.</p>');
  assert.equal(bodyMarkup(rows[1]), '<p class="rh-src">[build-bot] Status:</p><p>All green.</p>');
  assert.equal(bodyMarkup(rows[2]), '<p><span class="rh-src">[build-bot] Status:\r<br></span>All green.</p>');
  // Marking adds no character and moves none: the row says what it said.
  for (const li of rows) {
    assert.equal(boxesOf(li)[0].text(), li.messages[0].content.replace(/\n+/g, ""));
  }
});

test("a heading the page cannot draw is ignored, and the body drawn exactly as before", async () => {
  const page = newPage();
  await signIn(page);
  const content = "[build-bot] Status:\nAll green.";
  const rows = await showDiscord(page, [
    message({ id: "100", content, heading: { tag: "build-bot", cut: 40 } }), // past the first line
    message({ id: "101", content, heading: { tag: "build-bot", cut: 0 } }),
    message({ id: "102", content, heading: { cut: 12 } }), // nothing to show
    message({ id: "103", content, heading: "build-bot" }),
    message({ id: "104", content }),
  ]);
  for (const li of rows) {
    assert.equal(piecesOf(li), null, `row ${li.getAttribute("data-id")} drew a header`);
    assert.equal(bodyMarkup(li), "<p>[build-bot] Status:<br>All green.</p>");
  }
});

test("a heading's pieces are text, whatever they say", async () => {
  const page = newPage();
  await signIn(page);
  const hostile = "<img src=x onerror=alert(1)>";
  const rows = await showDiscord(page, [message({
    id: "100", content: `[x] ${hostile}: body`, heading: { tag: hostile, title: hostile, time: hostile, cut: 4 },
  })]);
  assert.deepStrictEqual(piecesOf(rows[0]).map(([, text]) => text), [hostile, hostile, `@${hostile}`]);
  assert.ok(!page.createdTags.includes("img"), "a heading's text became markup");
});

test("a combined row is headed by its first message, and the second keeps its first line as written", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, splitPost(
    { content: "[build-bot] Release notes: three fixes", heading: { tag: "build-bot", title: "Release notes", cut: 27 } },
    { content: "[build-bot] Follow-up: one new flag", heading: { tag: "build-bot", title: "Follow-up", cut: 23 } }
  ));
  assert.equal(rows.length, 1, "the fixture did not produce one combined row");
  assert.deepStrictEqual(piecesOf(rows[0]).map(([, text]) => text), ["build-bot", "Release notes"]);
  const [first, second] = boxesOf(rows[0]);
  assert.ok(first.hasAttribute("data-rh"));
  assert.ok(!second.hasAttribute("data-rh"), "the second message's first line would be hidden");
  // Marked as the server would mark it, but nothing hides it without the box's attribute.
  assert.equal(bodyMarkup(rows[0], 1), '<p><span class="rh-src">[build-bot] Follow-up: </span>one new flag</p>');
});

test("RICH HEADERS ARE ON BY DEFAULT, and the switch moves them without a redraw or a read, kept across a reload", async () => {
  const page = newPage();
  await signIn(page);
  assert.equal(page.el("rich-headers").checked, true, "the default is not on");
  assert.equal(page.documentElement.getAttribute("data-rich-headers"), "on");
  const rows = await showDiscord(page, [message({ id: "100", content: HEALTH, heading: HEALTH_HEADING })]);
  const row = rows[0];
  const before = row.text();
  const reads = page.pageReads;

  await page.el("rich-headers").setChecked(false);
  await page.settle();
  assert.equal(page.documentElement.getAttribute("data-rich-headers"), "off");
  assert.equal(page.storage.get("vibe-talk.voice.rich-headers"), "0", "the choice was not kept");
  assert.equal(page.pageReads, reads, "flipping a display switch cost a round trip to the server");
  assert.equal(page.el("discord-log").children[0], row, "the switch rebuilt the rows");
  assert.equal(row.text(), before, "the switch changed what the row holds rather than what is drawn");

  await page.el("rich-headers").setChecked(true);
  assert.equal(page.documentElement.getAttribute("data-rich-headers"), "on");
  assert.equal(page.storage.get("vibe-talk.voice.rich-headers"), "1");

  await page.el("rich-headers").setChecked(false);
  const again = reloadWith(page.storage, [message({ id: "100", content: HEALTH, heading: HEALTH_HEADING })]);
  await signIn(again);
  assert.equal(again.el("rich-headers").checked, false, "the switch came back on by itself");
  assert.equal(again.documentElement.getAttribute("data-rich-headers"), "off");
});

test("off draws exactly the old row: the header hidden, the restated words shown, nothing cut short anywhere", () => {
  assert.match(cssBlock(':root[data-rich-headers="off"] .rh-head'), /display: none/);
  assert.match(cssBlock(':root:not([data-rich-headers="off"]) .md[data-rh] .rh-src'), /display: none/);
  // Nothing else hides the restated words: off, they are an ordinary span and paragraph.
  const hiding = [...CSS_CODE.matchAll(/([^{}]+)\{([^{}]*)\}/g)]
    .filter((rule) => /\.rh-src/.test(rule[1]) && /display:\s*none/.test(rule[2]))
    .map((rule) => rule[1].trim());
  assert.deepStrictEqual(hiding, [':root:not([data-rich-headers="off"]) .md[data-rh] .rh-src']);
  // The header is never cut short: no ellipsis, no clipping, no forced single line.
  const rules = [...CSS_CODE.matchAll(/([^{}]+)\{([^{}]*)\}/g)].filter((rule) => /\.rh-/.test(rule[1]));
  assert.ok(rules.length >= 5, "the header's rules are missing");
  for (const [, selector, body] of rules) {
    assert.doesNotMatch(body, /text-overflow|overflow:\s*hidden|white-space:\s*nowrap/, `${selector.trim()} can cut words`);
  }
  // A header that is not promoted is a line of its own, and wraps.
  const head = cssBlock(".rh-head");
  assert.match(head, /order: 99/);
  assert.match(head, /flex-basis: 100%/);
  assert.match(head, /flex-wrap: wrap/);
  // A piece that is one long unbroken token (a commit hash) breaks inside the row, as a body does,
  // rather than holding its whole width and pushing the header off a phone's screen.
  const piece = cssBlock(".rh-head > span");
  assert.match(piece, /overflow-wrap: anywhere/, "a long token in a title spills out of its row");
  assert.match(piece, /min-width: 0/, "a piece cannot be narrower than its longest token");
  assert.match(cssBlock(".rh-tag"), /font-style: italic/);
  assert.match(cssBlock(".rh-title"), /font-weight: 650/);
  assert.match(cssBlock(".rh-time"), /color: var\(--muted\)/);
});

test("where a header goes is the stylesheet's, by list width and length class, from no layout read", () => {
  // Each list is a size container, and every length class the page sets is promoted by exactly one
  // container query, wider for a longer class. `long` never is.
  assert.match(cssBlock("#discord-log"), /container: rows \/ inline-size/);
  let previous = 0;
  for (let length = 16; length <= 96; length += 8) {
    const selector = `li.discord-message[data-rh-len="${length}"] > .meta > .rh-head`;
    const blocks = cssRules(CSS_CODE, selector);
    assert.equal(blocks.length, 1, `${selector} is promoted ${blocks.length} times`);
    assert.match(blocks[0], /order: 0/);
    assert.match(blocks[0], /flex-basis: auto/);
    const query = new RegExp(`@container rows \\(min-width: ([\\d.]+)rem\\) \\{\\s*li\\.discord-message\\[data-rh-len="${length}"\\]`);
    const width = Number((query.exec(CSS_CODE) || assert.fail(`no container query promotes ${length}`))[1]);
    assert.ok(width > previous, `class ${length} is promoted at ${width}rem, not wider than the class before`);
    previous = width;
  }
  assert.equal(cssRules(CSS_CODE, 'li.discord-message[data-rh-len="long"] > .meta > .rh-head').length, 0);
  // Decided without reading layout: the length class comes from the strings.
  const lengthClass = SCRIPT_CODE.slice(SCRIPT_CODE.indexOf("function headingLength"));
  assert.doesNotMatch(lengthClass.slice(0, lengthClass.indexOf("\n}\n")), /getBoundingClientRect|offset|clientWidth|scrollWidth/);
});

test("a pin older than anything loaded is drawn with the heading the server found in its snapshot", async () => {
  const page = newPage();
  const loaded = message({ id: "300", content: "something newer" });
  const old = message({ id: "150", content: HEALTH, author: "build-bot",
    timestamp: new Date(Date.parse(loaded.timestamp) - 3_600_000).toISOString() });
  page.pinned.set("150", { ...storedPin(old), heading: HEALTH_HEADING });
  await signIn(page);
  await showDiscord(page, [loaded]);
  await showPinned(page);
  assert.deepStrictEqual(pinnedIds(page), ["150"], "the fixture's pin is not drawn from its snapshot");
  const row = page.el("pinned-log").children[0];
  assert.deepStrictEqual(piecesOf(row).map(([, text]) => text), ["build-bot coordinator", "Backup check", "@10:52 PM ET"]);
  assert.ok(boxesOf(row)[0].hasAttribute("data-rh"));
});

test("a heading that arrives with a later read redraws the Pinned list", async () => {
  // The list is rebuilt only when what it draws changed, and a plain body's HTML is "" with or
  // without a heading: a paren tag confirmed by a later read, or after a restart, brings a heading to
  // a message whose text did not change. The heading has to be part of what it compares.
  const plain = message({ id: "100", content: HEALTH, author: "build-bot" });
  const later = message({ id: "101", content: "and so?" });
  const headed = { ...plain, heading: HEALTH_HEADING };
  const pieces = ["build-bot coordinator", "Backup check", "@10:52 PM ET"];

  const page = newPage();
  page.pinned.set("100", storedPin(plain));
  await signIn(page);
  await showDiscord(page, [plain, later]);
  await showPinned(page);
  assert.deepStrictEqual(pinnedIds(page), ["100"], "the fixture's pin is not drawn");
  assert.equal(piecesOf(page.el("pinned-log").children[0]), null, "the fixture's pin already has a heading");
  await refreshDiscord(page, [headed, later]);
  await page.settle();
  const pinned = page.el("pinned-log").children[0];
  assert.deepStrictEqual((piecesOf(pinned) || []).map(([, text]) => text), pieces,
    "the Pinned list kept the row drawn before its heading arrived");
  assert.ok(boxesOf(pinned)[0].hasAttribute("data-rh"));
});

/** A threaded channel in All whose first message, and a thread's root, carry reactions. */
async function reactedThreadPage(script = SCRIPT) {
  const page = newPage(new Map(), script);
  const data = threadData();
  data.messages[0] = { ...data.messages[0], reactions: ACKNOWLEDGED };
  data.messages[1] = { ...data.messages[1], reactions: [{ emoji: "👍", count: 2 }] };
  page.threadingSupported = true;
  page.threads = data.threads;
  await signIn(page);
  await showDiscord(page, data.messages);
  return page;
}

test("the live echo of a message, which carries no reactions, does not wipe the ones a read brought", async () => {
  const page = await reactedThreadPage();
  assert.deepStrictEqual(chipsOf(rowOf(page, "200")), ["👀 1", "✅ 1"], "the fixture's row has no strip");
  // No read succeeds from here on, so only what the page keeps can say what the reactions are.
  const standard = page.timeline;
  page.timeline = errorResponse(502, "chat_error", "history read timed out");
  const { reactions: _unseen, ...echo } = page.messages.find((m) => m.id === "200");
  await deliver(page, page.stream(), sseMessage({ ...echo, content: "main channel announcement, edited" }));
  await page.settle();
  assert.deepStrictEqual(chipsOf(rowOf(page, "200")), ["👀 1", "✅ 1"], "the echo wiped the acknowledgements");
  // The store took the echo's text and kept the reactions the echo could not see.
  const kept = savedScopes(page)[scopeKey(CHANNEL.id)].messages.find((m) => m.id === "200");
  assert.equal(kept.content, "main channel announcement, edited", "the echo was not folded into the store");
  assert.deepStrictEqual(kept.reactions, ACKNOWLEDGED, "the store's copy lost them");
  // A READ is the provider's answer, and says "none" when there are none.
  page.timeline = standard;
  page.messages = page.messages.map((m) => (m.id === "200" ? { ...m, reactions: [] } : m));
  await pollOnce(page);
  assert.equal(chipsOf(rowOf(page, "200")), null, "a read saying there are none left the old ones up");
});

test("negative control: without the rule, the live echo does wipe the reactions from the store", async () => {
  const page = await reactedThreadPage(await brokenModule([{ file: "src/core/channels/live.ts",
    find: "if (held && Array.isArray(held.reactions) && !Array.isArray(kept.reactions)) {", replace: "if (false) {" }]));
  page.timeline = errorResponse(502, "chat_error", "history read timed out");
  const { reactions: _unseen, ...echo } = page.messages.find((m) => m.id === "200");
  await deliver(page, page.stream(), sseMessage({ ...echo, content: "main channel announcement, edited" }));
  await page.settle();
  const kept = savedScopes(page)[scopeKey(CHANNEL.id)].messages.find((m) => m.id === "200");
  assert.equal(kept.content, "main channel announcement, edited");
  assert.equal(kept.reactions, undefined, "the control did not take the rule away, so it proves nothing");
});

test("a thread root shows its reactions above the way into its replies", async () => {
  const page = await reactedThreadPage();
  const root = rowOf(page, "201");
  assert.deepStrictEqual(chipsOf(root), ["👍 2"]);
  const order = root.children.map((node) => node.className);
  assert.ok(order.indexOf("thread-replies") > order.indexOf("reactions"),
    `the reply count is not below the reactions, where the chat service puts it: ${order}`);
});

test("a pinned message shows its reactions in the Pinned list, and redraws when they change", async () => {
  const page = newPage();
  const data = threadData();
  data.messages[0] = { ...data.messages[0], reactions: ACKNOWLEDGED };
  page.threadingSupported = true;
  page.threads = data.threads;
  const old = message({ id: "150", content: "kept only by its pin",
    timestamp: new Date(Date.parse(data.messages[0].timestamp) - 3_600_000).toISOString() });
  for (const [m, at] of [[data.messages[0], 1], [old, 2]]) page.pinned.set(String(m.id), storedPin(m, at));
  page.servePinsRevision = true;
  page.pinsRevision = 7;
  await signIn(page);
  await showDiscord(page, data.messages);
  await showPinned(page);
  assert.deepStrictEqual(pinnedIds(page), ["150", "200"]);
  const rows = page.el("pinned-log").children;
  assert.equal(chipsOf(rows[0]), null, "a pin's kept copy, which holds no reactions, drew some");
  assert.deepStrictEqual(chipsOf(rows[1]), ["👀 1", "✅ 1"]);
  page.messages = page.messages.map((m) => (m.id === "200"
    ? { ...m, reactions: [...ACKNOWLEDGED, { emoji: "👍", count: 1 }] } : m));
  await pollOnce(page);
  assert.deepStrictEqual(chipsOf(page.el("pinned-log").children[1]), ["👀 1", "✅ 1", "👍 1"],
    "the Pinned list kept the reactions it was first drawn with");
});

// --- a collapsed row on one line (#377 compact-collapsed-rows) ------------------------------------
//
// The owner, 2026-10-19: a collapsed message's header took two lines on a phone. Where the list is
// narrow, a collapsed row's header holds the name (cut to a prefix where it must be), the time, the
// Thread tag and ⋯; Reply and Done appear once it is opened. tests/compact_rows_browser.py measures the
// lines the engine draws; these pin the rules and the name's tooltip the fixture can see.

/** The body of the one `@container` block for `query`, brace-matched, with comments gone. */
function containerBody(query) {
  const at = CSS_CODE.indexOf(`@container ${query}`);
  assert.ok(at >= 0, `web/voice.css has no "@container ${query}" block`);
  let depth = 0;
  for (let i = CSS_CODE.indexOf("{", at); i < CSS_CODE.length; i += 1) {
    if (CSS_CODE[i] === "{") depth += 1;
    else if (CSS_CODE[i] === "}" && --depth === 0) return CSS_CODE.slice(CSS_CODE.indexOf("{", at) + 1, i);
  }
  return assert.fail(`the "@container ${query}" block is never closed`);
}

/**
 * The declaration blocks whose WHOLE selector is `selector`, whitespace aside. `cssRules` splits a
 * selector list on its commas, which cuts an `:is(a, b)` apart; these rules are written with one.
 */
const wholeRules = (text, selector) => [...text.matchAll(/([^{}]+)\{([^{}]*)\}/g)]
  .filter((rule) => rule[1].replace(/\s+/g, " ").trim() === selector)
  .map((rule) => rule[2]);

const COMPACT_QUERY = "rows (max-width: 31.99rem)";
const ROW_ACTS = ":is(.reply-button:not([data-quote]), .not-noise-button, .done-button)";

test("`#377 compact-collapsed-rows`: a narrow list takes Reply and Done off a collapsed row only, never an open one", () => {
  const body = containerBody(COMPACT_QUERY);
  const lists = ":is(#side-log, #side-cards, #discord-log, #pinned-log)";
  const selector = `${lists} li.discord-message[data-collapsed="true"] .row-acts > ${ROW_ACTS}`;
  const hidden = wholeRules(body, selector).join("\n");
  assert.match(hidden, /display:\s*none/, "a collapsed row in a narrow list still draws Reply and Done");
  // `display: none` rather than clipping: out of the accessibility tree, as they are off the screen.
  assert.doesNotMatch(hidden, /clip|opacity|visibility/, "the acts are hidden from the eye but left to a screen reader");
  // The negative control: no rule anywhere hides Reply or Done on a row that is not collapsed, or on
  // every row.
  for (const block of CSS_CODE.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const rule = block[1].trim();
    if (!/display:\s*none/.test(block[2]) || !/reply-button|done-button/.test(rule)) continue;
    assert.match(rule, /\[data-collapsed="true"\]/, `"${rule}" hides Reply or Done on a row that is not collapsed`);
  }
  // Wide lists keep every control: nothing outside the narrow query takes them off a collapsed row.
  assert.equal(wholeRules(CSS_CODE.replace(body, ""), selector).length, 0, "a wide list takes Reply and Done off a collapsed row too");
  // The controls move as one: pushed to the end of the line, wrapping among themselves only when they
  // are wider than the whole line.
  const acts = cssBlock(".row-acts");
  assert.match(acts, /margin-left:\s*auto/);
  assert.match(acts, /flex-wrap:\s*wrap/);
  assert.match(acts, /justify-content:\s*flex-end/);
});

test("`#377 compact-collapsed-rows`: the name gives way on the header line, and the Thread tag is the size of Reply", () => {
  const name = cssRules(CSS_CODE, "#discord-log li.discord-message > .meta > .msg-author").join("\n");
  assert.match(name, /flex:\s*1 1 3em/, "the name asks the line for more than three letters, so a long one pushes the controls onto a line of their own");
  assert.match(name, /min-width:\s*0/);
  assert.match(name, /max-width:\s*max-content/, "the name grows past its own width, away from the time");
  assert.match(name, /text-overflow:\s*ellipsis/, "a cut name has no ellipsis");
  assert.match(name, /white-space:\s*nowrap/);
  const tag = cssRules(CSS_CODE, ".thread-badge").join("\n");
  const reply = cssBlock(".reply-button");
  for (const property of ["padding", "font-size", "min-height"]) {
    const of = (block) => [...block.matchAll(new RegExp(`(?:^|[;\\s])${property}:\\s*([^;]+);`, "g"))].map((m) => m[1].trim()).pop();
    assert.equal(of(tag), of(reply), `the Thread tag's ${property} is not Reply's`);
  }
});

test("`#377 compact-collapsed-rows`: a row's name carries the whole of itself as its tooltip", async () => {
  const page = newPage();
  await signIn(page);
  const long = "Bartholomew Featherstonehaugh-Ravensworth";
  const rows = await showDiscord(page, [
    message({ id: "100", author: long, author_id: "1000000000000000006", author_is_bot: false, content: "a person" }),
    message({ id: "101", author: "relay", author_id: "1000000000000000008", author_is_bot: true, content: "a bot" }),
  ]);
  const nameOf = (li) => metaOf(li).children.find((node) => String(node.className).split(" ").includes("msg-author"));
  const [person, bot] = rows.map(nameOf);
  assert.equal(person.textContent, long);
  assert.equal(person.getAttribute("title"), long, "the whole name is nowhere a pointer can find it once it is cut");
  assert.equal(bot.getAttribute("title"), `${bot.textContent}`, "a bot's tooltip is not the name its row says");
  assert.match(bot.getAttribute("title"), /\(bot\)$/);
});

test("`#377 compact-collapsed-rows`: a row's controls are held together, in their order, after its name and time", async () => {
  const page = newPage();
  await signIn(page);
  const [li] = await showDiscord(page, [message({ id: "100", content: "a message" })]);
  const meta = metaOf(li);
  const kids = meta.children.map((node) => String(node.className).split(" ")[0]);
  assert.ok(kids.indexOf("row-acts") > kids.indexOf("msg-time"), `the controls are not after the time: ${kids}`);
  const acts = meta.children.find((node) => node.className === "row-acts");
  const inside = acts.children.map((node) => String(node.className).split(" ")[0]);
  for (const name of ["draft-marker", "reply-button", "row-more", "not-noise-button", "done-button"]) {
    assert.ok(inside.includes(name), `${name} is not among the row's controls: ${inside}`);
  }
  assert.deepEqual(inside.filter((name) => ["reply-button", "row-more", "done-button"].includes(name)),
    ["reply-button", "row-more", "done-button"], "the controls are out of their order");
  // Reply's own record of itself still finds it (`#228 reply-visible`).
  assert.equal(li.replyButton, acts.children.find((node) => node.className === "reply-button"));
});

// --- edited messages (#320 slack-edited-mark) ------------------------------------------------------
//
// A message its author changed after sending it says "(edited)" after its time, where the provider
// says so — Slack's `edited.ts`, Discord's `edited_timestamp` — as `edited_timestamp`. When, on hover
// and in the details sheet; never in the body, the search or the voice.

/** A row's edited mark, or undefined when it has none. */
const editedMarkOf = (li) => {
  const meta = li.children.find((node) => node.className === "meta");
  return meta ? meta.children.find((node) => node.className === "edited-mark") : undefined;
};

test("an edited message says (edited) straight after its time; one not edited, or not reported, does not", async () => {
  await inZone("America/New_York", async () => {
    const page = newPage();
    await signIn(page);
    const sent = localAt(2026, 8, 19, 9, 10);
    const changed = localAt(2026, 8, 19, 9, 42);
    const rows = await showDiscord(page, [
      message({ id: "100", content: "deploy at noon, typo fixed", timestamp: sent.toISOString(),
        edited_timestamp: changed.toISOString() }),
      message({ id: "101", content: "never touched", timestamp: localAt(2026, 8, 19, 9, 50).toISOString() }),
      message({ id: "102", content: "an older server's copy", timestamp: localAt(2026, 8, 19, 9, 55).toISOString(),
        edited_timestamp: null }),
    ]);
    const mark = editedMarkOf(rows[0]);
    assert.ok(mark, "the edited message has no mark");
    assert.equal(mark.tagName, "time", "the mark does not say which instant the edit was");
    assert.equal(mark.textContent, "(edited)");
    assert.equal(mark.getAttribute("datetime"), changed.toISOString());
    assert.equal(mark.getAttribute("title"), `Edited Today, Aug 19, Wed, ${shownTime(changed)}`);
    const order = rows[0].children.find((node) => node.className === "meta").children.map((node) => node.className);
    assert.equal(order.indexOf("edited-mark"), order.indexOf("msg-time") + 1, `the mark is not after the time: ${order}`);
    // The negative controls: never edited, and an edit time that is not there.
    assert.equal(editedMarkOf(rows[1]), undefined, "a message never edited is marked");
    assert.equal(editedMarkOf(rows[2]), undefined, "an edit time of null is marked");
    // Not part of what the message says: not in its body, and not a match for a search.
    assert.ok(!rows[0].children.find((node) => node.className === "body").text().includes("edited"));
    await search(page, "edited");
    await page.settle();
    assert.ok(rows.every((li) => li.hasClass("search-hidden")), "the mark made a row match a search");
  });
});

test("an edit's tooltip says its day again at hover, and the details sheet says when for a phone", async () => {
  await inZone("America/New_York", async () => {
    const page = newPage();
    await signIn(page);
    const changed = localAt(2026, 8, 19, 9, 42);
    const rows = await showDiscord(page, [
      message({ id: "100", content: "deploy at noon", timestamp: localAt(2026, 8, 19, 9, 10).toISOString(),
        edited_timestamp: changed.toISOString() }),
      message({ id: "101", content: "never touched", timestamp: localAt(2026, 8, 19, 9, 50).toISOString() }),
    ]);
    const mark = editedMarkOf(rows[0]);
    page.setClock(localAt(2026, 8, 20, 0, 5).getTime());
    await page.documentEvent("pointerover", { target: mark });
    assert.equal(mark.getAttribute("title"), `Edited Yesterday, Aug 19, Wed, ${shownTime(changed)}`,
      "a page left open past midnight still called the edit Today");
    assert.equal(mark.textContent, "(edited)", "the hover changed the mark's words");
    await hold(page, rows[0]);
    const sheet = rows[0].descendants().find((node) => node.className === "msg-details");
    const line = sheet.children.find((node) => node.className === "msg-edited");
    assert.ok(line, "the details sheet does not say when the message was edited");
    assert.match(line.textContent, /^Edited Yesterday, Aug 19, Wed, /);
    await hold(page, rows[1]);
    const plain = rows[1].descendants().find((node) => node.className === "msg-details");
    assert.ok(plain, "the unedited row's details did not open");
    assert.equal(plain.children.find((node) => node.className === "msg-edited"), undefined,
      "an unedited message's details say it was edited");
  });
});

test("an edit with no readable time is still marked, and a combined row says its latest edit", async () => {
  const page = newPage();
  await signIn(page);
  const first = splitPost(
    { edited_timestamp: "2026-08-19T04:35:00.000Z" },
    { edited_timestamp: "2026-08-19T04:40:30.000Z" }
  );
  const rows = await showDiscord(page, [...first,
    message({ id: "3000000000000000001", content: "fixed", edited_timestamp: "sometime" })]);
  assert.equal(rows.length, 2, "the fixture did not produce one combined row and one more");
  assert.equal(editedMarkOf(rows[0]).getAttribute("datetime"), "2026-08-19T04:40:30.000Z",
    "a combined row's mark is not its latest edit");
  assert.equal(rows[0].descendants().filter((node) => node.className === "edited-mark").length, 1);
  const vague = editedMarkOf(rows[1]);
  assert.equal(vague.tagName, "span", "an edit with no readable time claimed an instant");
  assert.equal(vague.textContent, "(edited)");
  assert.equal(vague.getAttribute("title"), "Edited");
  assert.equal(vague.getAttribute("datetime"), null);
});

test("a live copy that does not say when a message was edited keeps the mark a read brought", async () => {
  const page = await reactedThreadPage();
  const read = page.messages.find((m) => m.id === "200");
  page.messages = page.messages.map((m) => (m.id === "200" ? { ...m, edited_timestamp: "2026-08-19T04:45:00.000Z" } : m));
  await pollOnce(page);
  assert.ok(editedMarkOf(rowOf(page, "200")), "a read that brought the edit drew no mark");
  page.timeline = errorResponse(502, "chat_error", "history read timed out");
  await deliver(page, page.stream(), sseMessage({ ...read, content: "main channel announcement, edited again" }));
  await page.settle();
  const kept = savedScopes(page)[scopeKey(CHANNEL.id)].messages.find((m) => m.id === "200");
  assert.equal(kept.content, "main channel announcement, edited again", "the echo was not folded into the store");
  assert.equal(kept.edited_timestamp, "2026-08-19T04:45:00.000Z", "the store's copy lost the edit");
  // Drawn from what the device kept, with the service unreachable: the echo's words, and the mark.
  const again = reloadWith(page.storage, [], (p) => {
    p.threadingSupported = true;
    p.threads = [];
    p.timeline = errorResponse(502, "chat_error", "history read timed out");
  });
  await reopenedChannel(again);
  const row = rowOf(again, "200");
  assert.match(row.children.find((node) => node.className === "body").text(), /edited again/,
    "the reopened row is not the echo's, so its mark proves nothing");
  assert.ok(editedMarkOf(row), "the live echo took the mark away");
});

test("negative control: without the rule, the live echo does take the edit from the store", async () => {
  const page = await reactedThreadPage(await brokenModule([{ file: "src/core/channels/live.ts",
    find: "if (held && held.edited_timestamp && !kept.edited_timestamp) {", replace: "if (false) {" }]));
  const read = page.messages.find((m) => m.id === "200");
  page.messages = page.messages.map((m) => (m.id === "200" ? { ...m, edited_timestamp: "2026-08-19T04:45:00.000Z" } : m));
  await pollOnce(page);
  page.timeline = errorResponse(502, "chat_error", "history read timed out");
  await deliver(page, page.stream(), sseMessage({ ...read, content: "main channel announcement, edited again" }));
  await page.settle();
  const kept = savedScopes(page)[scopeKey(CHANNEL.id)].messages.find((m) => m.id === "200");
  assert.equal(kept.edited_timestamp, undefined, "the control did not take the rule away, so it proves nothing");
});

test("a pinned message that is edited later is redrawn in the Pinned list with its mark", async () => {
  const page = newPage();
  const data = threadData();
  page.threadingSupported = true;
  page.threads = data.threads;
  page.pinned.set(String(data.messages[0].id), storedPin(data.messages[0], 1));
  page.servePinsRevision = true;
  page.pinsRevision = 7;
  await signIn(page);
  await showDiscord(page, data.messages);
  await showPinned(page);
  const pinnedRow = () => page.el("pinned-log").children.find((li) => li.getAttribute("data-id") === String(data.messages[0].id));
  assert.equal(editedMarkOf(pinnedRow()), undefined, "the fixture's pin is already marked");
  page.messages = page.messages.map((m) => (m.id === data.messages[0].id
    ? { ...m, edited_timestamp: "2026-08-19T04:45:00.000Z" } : m));
  await pollOnce(page);
  assert.ok(editedMarkOf(pinnedRow()), "the Pinned list kept the row it drew before the edit");
});

test("the edited mark is drawn in the time's ink, kept whole, and has no hover", () => {
  const mark = cssBlock(".edited-mark");
  assert.match(mark, /white-space: nowrap/);
  assert.match(mark, /flex: 0 0 auto/);
  assert.doesNotMatch(mark, /color:/, "the mark has an ink of its own rather than the time's");
  assert.doesNotMatch(mark, /cursor: pointer/);
  assert.doesNotMatch(CSS_CODE, /\.edited-mark:hover/);
});

// --- `#386 hide-empty-messages` ----------------------------------------------------------------------
//
// The owner, 2026-10-19: "there is a BLANK message from the coding agent... I want to have a
// checked-by-default box in that same settings area that completely filters out empty messages."
// Empty is no text but white space, no reaction, and not the root of a thread anyone has answered.

const HIDE_EMPTY = "vibe-talk.voice.hide-empty";

/**
 * The inbox channel of `#222 unread-replies` with empty messages in it: question C's placeholder (11)
 * an empty reply instead; question B's root (05) emptied, though it has a reply; and after the agent's
 * newest (12), a message of spaces, a line break and a zero-width space (13), a file posted with no
 * words, as the server writes one (14), and a message with no text and one reaction (15).
 */
function emptyChannel(page) {
  const messages = inboxChannel(page);
  const at = (n) => messages.find((m) => m.id === inboxId(n));
  Object.assign(at(11), { content: "", noise: false });
  at(5).content = "";
  messages.push(
    message({ id: inboxId(13), content: " \n​ ", ...CODER }),
    message({ id: inboxId(14), content: "📎 [screenshot.png](<https://chat.example/files/F1/screenshot.png>) · PNG", ...CODER }),
    message({ id: inboxId(15), content: "", reactions: [{ emoji: "👍", count: 1 }], ...CODER }),
  );
  return messages;
}

test("`#386 hide-empty-messages`: ON BY DEFAULT an empty message draws no row in All or Main and is not unread; a file alone, a reaction alone and an answered root keep theirs", async () => {
  const page = newPage();
  const messages = emptyChannel(page);
  await signIn(page);
  assert.equal(page.el("hide-empty-messages").checked, true, "the default is not on");
  await showDiscord(page, messages);
  const all = shownIds(page);
  assert.ok(!all.includes(inboxId(11)) && !all.includes(inboxId(13)), `All drew an empty message: ${all}`);
  // Negative controls: none of these is empty.
  for (const n of [5, 14, 15]) assert.ok(all.includes(inboxId(n)), `All left out ${inboxId(n)}, which is not empty`);
  await pickThread(page, "main");
  assert.deepStrictEqual(shownIds(page), [1, 2, 5, 10, 7, 12, 14, 15].map(inboxId));
  // Question C's empty reply is not one more unread: the agent's two answers are.
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: null, 7: "2 unread" });
});

test("`#386 hide-empty-messages`: turned off, every empty message is drawn and counted again with no read; on, gone again; the choice is kept", async () => {
  const page = newPage();
  const messages = emptyChannel(page);
  await inboxOnMain(page, messages);
  const reads = page.timelineCalls.length;

  await page.el("hide-empty-messages").setChecked(false);
  await page.settle();
  assert.deepStrictEqual(shownIds(page), [1, 2, 5, 10, 7, 12, 13, 14, 15].map(inboxId));
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: null, 7: "3 unread" });
  assert.equal(page.storage.get(HIDE_EMPTY), "0", "the choice was not kept");
  assert.equal(page.timelineCalls.length, reads, "showing what the page holds cost a read");

  await page.el("hide-empty-messages").setChecked(true);
  await page.settle();
  assert.deepStrictEqual(shownIds(page), [1, 2, 5, 10, 7, 12, 14, 15].map(inboxId));
  assert.deepStrictEqual(unreadSaid(page), { 2: "1 unread", 5: null, 7: "2 unread" });
  assert.equal(page.storage.get(HIDE_EMPTY), "1");

  // A page opened with it off draws them from the start.
  const off = newPage(new Map([[HIDE_EMPTY, "0"]]));
  await inboxOnMain(off, emptyChannel(off));
  assert.equal(off.el("hide-empty-messages").checked, false, "the kept choice was not restored");
  assert.ok(shownIds(off).includes(inboxId(13)), "a page opened with the switch off hid an empty message");
});

test("`#386 hide-empty-messages`: an empty message arriving on the stream draws no row; a file arriving alone does", async () => {
  const page = newPage();
  const stream = await withLiveChannel(page, [message({ id: "9500000000000000001", content: "already here" })]);
  await deliver(page, stream, sseMessage(message({ id: "9500000000000000002", content: "   " })));
  assert.deepStrictEqual(shownIds(page), ["9500000000000000001"], "an empty arrival drew a row");
  await deliver(page, stream, sseMessage(message({ id: "9500000000000000003",
    content: "📎 [build.log](<https://chat.example/files/F2/build.log>) · Plain Text" })));
  assert.deepStrictEqual(shownIds(page), ["9500000000000000001", "9500000000000000003"], "a file arriving alone drew no row");
});

test("`#386 hide-empty-messages`: the arrow of a reply to a hidden empty message says so, and walks back through nothing", async () => {
  const page = newPage();
  page.threadingSupported = true;
  await signIn(page);
  await showDiscord(page, [
    message({ id: "9600000000000000001", content: "" }),
    message({ id: "9600000000000000002", content: "that image is the failing run", reply_to: "9600000000000000001" }),
  ]);
  assert.deepStrictEqual(shownIds(page), ["9600000000000000002"]);
  const reads = page.timelineCalls.length;
  await tapArrow(page, rowWithId(page, "9600000000000000002"));
  assert.equal(statusText(page), "Settings hides that empty message.");
  assert.equal(page.timelineCalls.length, reads, "it walked back looking for a message it held and hides");
});

test("`#386 hide-empty-messages`: an empty root keeps its row when only the store knows it was answered — its copy saying 0 replies, or carrying no thread record", async () => {
  const page = newPage();
  page.threadingSupported = true;
  const counted = { id: "spaces/A/threads/counted", root_message_id: "9600000000000000011", is_root: true,
    reply_count: 0, reply_count_exact: false };
  const bare = { id: "spaces/A/threads/bare", root_message_id: "9600000000000000013", is_root: false,
    reply_count: 1, reply_count_exact: true };
  const messages = [
    message({ id: "9600000000000000011", content: "", thread: counted }),
    message({ id: "9600000000000000012", content: "the screenshot shows the timeout", thread: { ...counted, is_root: false } }),
    message({ id: "9600000000000000013", content: "" }),
    message({ id: "9600000000000000014", content: "the log is attached above", thread: bare }),
  ];
  await signIn(page);
  await showDiscord(page, messages);
  const all = shownIds(page);
  assert.ok(all.includes("9600000000000000011"), `the root whose copy says 0 replies, with one held, is hidden: ${all}`);
  assert.ok(all.includes("9600000000000000013"), `the root with no record of its own, a reply to it held, is hidden: ${all}`);
});

test("`#386 hide-empty-messages`: the switch redraws the thread list's unread counts", async () => {
  const page = newPage();
  const messages = emptyChannel(page);
  await signIn(page);
  await showDiscord(page, messages);
  await pickThread(page, "threads");
  const saidOfC = () => cardPart(page, INBOX.c, "side-card-unread");
  assert.equal(saidOfC(), "2 unread", "the card counts the empty reply unread");
  await page.el("hide-empty-messages").setChecked(false);
  await page.settle();
  assert.equal(saidOfC(), "3 unread", "turned off, the card did not count the empty reply");
});

// --- days between rows (#250 date-separators) ---------------------------------------------------------
//
// The owner: "It would also be good to have temporal markers, intelligently. We could have more
// horizontal rule separators, not for collapsed messages but just for date separators. "---- Oct 3, Mon
// ----" for a day separator at midnight. If you filter down to sparse hits, consecutive separators
// should collapse "Oct 3 - 5"." Every time here is built from the device's own calendar, `new Date(y, m,
// d, h, min)`, so a midnight is a midnight in whatever zone the suite runs in, and the page's clock is
// set to a local noon on Monday 12 October 2026.

/** A local time on this device's calendar, as the server writes a timestamp. */
const localStamp = (month, day, hour = 12, minute = 0, year = 2026) => new Date(year, month - 1, day, hour, minute).toISOString();

/** The page's clock at a local time: `#250`'s Today. */
const clockAt = (page, month, day, hour = 12, year = 2026) => page.setClock(new Date(year, month - 1, day, hour, 0).getTime());

/** Each row's lines, as `[id, kind, above, below]`, for the rows drawing any. */
function linesOf(page, list = "discord-log") {
  return page.el(list).children.map((li) => {
    const line = (below) => li.children.find((kid) => kid.hasClass("row-seam") && kid.hasClass("row-seam-below") === below);
    const above = line(false);
    const under = line(true);
    return [li.getAttribute("data-id"), li.getAttribute("data-seam"), above ? lineText(above) : null, under ? lineText(under) : null];
  }).filter(([, , above, below]) => above !== null || below !== null);
}

/** Seven messages over ten days, either side of two midnights; "engine" in three. */
function dayChannel() {
  return [
    message({ id: "1", content: "engine one", timestamp: localStamp(10, 3, 23, 58) }),
    message({ id: "2", content: "plain, a minute past midnight", timestamp: localStamp(10, 4, 0, 1) }),
    message({ id: "3", content: "plain again", timestamp: localStamp(10, 4, 10) }),
    message({ id: "4", content: "engine two", timestamp: localStamp(10, 6, 9) }),
    message({ id: "5", content: "plain yesterday", timestamp: localStamp(10, 11, 20) }),
    message({ id: "6", content: "engine three", timestamp: localStamp(10, 12, 8) }),
    message({ id: "7", content: "plain this morning", timestamp: localStamp(10, 12, 9) }),
  ];
}

test("`#250 date-separators`: a rule above the first message of each day, at the device's midnight, with Yesterday and Today by name and date", async () => {
  const page = newPage();
  clockAt(page, 10, 12);
  await signIn(page);
  await showDiscord(page, dayChannel());
  assert.deepStrictEqual(linesOf(page), [
    ["1", "day", "Oct 3, Sat", null],
    ["2", "day", "Oct 4, Sun", null],
    ["4", "day", "Oct 6, Tue", null],
    ["5", "day", "Yesterday, Oct 11, Sun", null],
    ["6", "day", "Today, Oct 12, Mon", null],
  ], "the days are not ruled off where they begin, or not said as the owner asked");
  const line = page.el("discord-log").children[0].children[0];
  assert.equal(line.className, "row-seam", "the rule is not the row's first child, ahead of its message");
  assert.equal(line.getAttribute("role"), "separator");
  assert.equal(line.getAttribute("aria-label"), "Oct 3, Sat");
  // Its own room, solid, between the rows.
  assert.match(cssBlock('#discord-log li.discord-message[data-seam="day"]'), /--seam-room:\s*var\(--day-room\)/);
  assert.match(cssBlock('#discord-log li.discord-message[data-seam="day"] > .row-seam:not(.row-seam-below)::before'),
    /border-top-style:\s*solid/, "a day's rule is drawn as a gap is");
  // A day of another year says its year.
  page.messages = [message({ id: "9", content: "from last year", timestamp: localStamp(12, 30, 18, 0, 2025) }), ...dayChannel()];
  await reReadChannel(page);
  assert.deepStrictEqual(linesOf(page)[0], ["9", "day", "Dec 30, 2025, Tue", null]);
  // Nothing is formatted by a date formatter per row: the words are the page's own.
  const made = page.dateFormatsMade;
  await reReadChannel(page);
  assert.ok(page.dateFormatsMade - made <= 1, `a redraw of the days made ${page.dateFormatsMade - made} formatters`);
});

test("`#250 date-separators`: filtered to sparse hits, the days in each gap are one range on its line, never a stack of rules", async () => {
  const page = newPage();
  clockAt(page, 10, 12);
  await signIn(page);
  await showDiscord(page, dayChannel());
  await search(page, "engine");
  await page.settle();
  assert.deepStrictEqual(linesOf(page), [
    ["1", "day", "Oct 3, Sat", null],
    ["4", "gap", "… 2 unmatched messages · Oct 4 – 6 …", null],
    ["6", "gap", "… 1 unmatched message · Yesterday – Today …", "… 1 unmatched message …"],
  ], "the days a filter skips are not one range on the gap's line");
  // Each side of the `·` is held together, so a narrow line breaks between the count and the days.
  assert.equal(rowWithId(page, "4").children[0].textContent, "…\u00a02\u00a0unmatched\u00a0messages\u00a0· Oct\u00a04\u00a0–\u2060\u00a06\u00a0…",
    "a gap's line can break inside its count or its days");
  assert.equal(rowWithId(page, "4").children[0].getAttribute("aria-label"), "Show 2 unmatched messages · Oct 4 – 6",
    "a screen reader is read the no-break spaces");
  // A gap that crosses no day says none; one across a month says both months; one before the first
  // match says the days it reaches, as does one after the last.
  await page.el("search-field").setValue("plain");
  assert.deepStrictEqual(linesOf(page), [
    ["2", "gap", "… 1 unmatched message · Oct 3 – 4 …", null],
    ["5", "gap", "… 1 unmatched message · Oct 6 – Yesterday …", null],
    ["7", "gap", "… 1 unmatched message · Today, Oct 12, Mon …", null],
  ]);
  page.messages = [
    message({ id: "21", content: "engine late september", timestamp: localStamp(9, 29, 10) }),
    message({ id: "22", content: "plain", timestamp: localStamp(9, 30, 10) }),
    message({ id: "23", content: "engine early october", timestamp: localStamp(10, 2, 10) }),
    message({ id: "24", content: "plain", timestamp: localStamp(10, 4, 10) }),
    message({ id: "25", content: "plain", timestamp: localStamp(10, 5, 10) }),
  ];
  await reReadChannel(page);
  await page.el("search-field").setValue("engine");
  assert.deepStrictEqual(linesOf(page), [
    ["21", "day", "Sep 29, Tue", null],
    ["23", "gap", "… 1 unmatched message · Sep 30 – Oct 2 …", "… 2 unmatched messages · Oct 4 – 5 …"],
  ]);
  // Cleared, each day has its own rule again.
  await page.el("search-field").setValue("");
  assert.deepStrictEqual(linesOf(page).map(([id, kind]) => [id, kind]),
    [["21", "day"], ["22", "day"], ["23", "day"], ["24", "day"], ["25", "day"]]);
});

test("`#250 date-separators`: a collapsed run's line says the days it brings, and the next day is ruled after it", async () => {
  const page = newPage();
  clockAt(page, 10, 12);
  await signIn(page);
  for (const id of ["2", "3", "4"]) page.dealtWith.add(id);
  await showDiscord(page, dayChannel());
  await chooseReadMode(page, "Fold read");
  assert.equal(rowWithId(page, "2").getAttribute("data-read-run"), "head", "the fixture collapsed no run");
  assert.equal(lineText(readRunLineOf(rowWithId(page, "2"))), "… 3 read messages · Oct 4 – 6 …",
    "the run's line does not say the days it covers");
  assert.deepStrictEqual(linesOf(page), [
    ["1", "day", "Oct 3, Sat", null], ["5", "day", "Yesterday, Oct 11, Sun", null], ["6", "day", "Today, Oct 12, Mon", null],
  ], "a day inside the run has a rule of its own, or the next day has none");
  // A run within one day already said says no day.
  page.dealtWith.delete("4");
  await reReadChannel(page);
  await page.settle();
  assert.equal(lineText(readRunLineOf(rowWithId(page, "2"))), "… 2 read messages · Oct 4, Sun …");
  assert.deepStrictEqual(linesOf(page).map(([id]) => id), ["1", "4", "5", "6"]);
});

test("`#250 date-separators`: a day's rule and the bookmark above one message: the day above, the bookmark nearer the message, each in room of its own", async () => {
  const page = newPage();
  clockAt(page, 10, 12);
  page.serveBookmark = true;
  await signIn(page);
  await showDiscord(page, dayChannel());
  await bookmarkRow(page, "4");
  assert.deepStrictEqual(bookmarkAt(page), { id: "4", place: "above" });
  const row = rowWithId(page, "4");
  assert.deepStrictEqual(row.children.slice(0, 2).map((kid) => kid.className), ["row-seam", "bookmark-line"],
    "the day's rule and the bookmark's line are not in order, the bookmark nearer the message");
  assert.equal(row.getAttribute("data-seam"), "day");
  assert.match(cssBlock('#discord-log li.discord-message[data-seam][data-bookmark="above"] > .row-seam:not(.row-seam-below)'),
    /bottom:\s*calc\(100% \+ 1px \+ var\(--bookmark-room\)\)/, "the day's rule is not drawn above the bookmark's line");
  // In the layout model, the rule stands above the bookmark's line, which stands above the message.
  const rule = row.children[0].getBoundingClientRect();
  assert.ok(rule.bottom <= row.getBoundingClientRect().top, "the day's rule overlaps the message");
});

test("`#250 date-separators`: the first redraw after midnight says Yesterday where it said Today", async () => {
  const page = newPage();
  clockAt(page, 10, 12, 23);
  await signIn(page);
  await showDiscord(page, dayChannel());
  assert.deepStrictEqual(linesOf(page).slice(-2).map(([, , words]) => words), ["Yesterday, Oct 11, Sun", "Today, Oct 12, Mon"]);
  clockAt(page, 10, 13, 0);
  await reReadChannel(page);
  assert.deepStrictEqual(linesOf(page).slice(-2).map(([, , words]) => words), ["Oct 11, Sun", "Yesterday, Oct 12, Mon"],
    "the days were not said again once the device's day had turned");
});

test("`#250 date-separators`: a redraw that changes nothing writes no rule, and reads no row's layout", async () => {
  const run = async (script) => {
    const page = newPage(new Map(), script);
    clockAt(page, 10, 12);
    await signIn(page);
    await showDiscord(page, dayChannel());
    const set = FakeElement.prototype.setAttribute;
    const rect = FakeElement.prototype.getBoundingClientRect;
    const list = page.el("discord-log");
    let written = 0;
    let measured = 0;
    FakeElement.prototype.setAttribute = function counted(name, value) {
      if (name === "data-seam" || name === "data-seam-below" || (this.isRowSeam() && name === "aria-label")) written += 1;
      return set.call(this, name, value);
    };
    FakeElement.prototype.getBoundingClientRect = function counted() {
      if (nodeWithinFixture(list, this)) measured += 1;
      return rect.call(this);
    };
    const rows = [...list.children];
    try {
      // Every redraw passes over the lines: opening the search bar, with nothing typed, is one.
      await search(page, "");
      await page.settle();
    } finally {
      FakeElement.prototype.setAttribute = set;
      FakeElement.prototype.getBoundingClientRect = rect;
    }
    assert.ok(list.children.every((li, at) => li === rows[at]), "the redraw replaced the rows; this proves nothing");
    return { written, measured };
  };
  const drawn = await run(SCRIPT);
  assert.equal(drawn.written, 0, "a redraw that changed nothing wrote the days again");
  // The same redraw with no lines drawn at all reads exactly as much of the layout.
  const bare = await run(brokenScript("function drawSeams(list) {\n", "function drawSeams(list) {\n  if (list) return;\n"));
  assert.equal(drawn.measured, bare.measured, `the lines read ${drawn.measured - bare.measured} rows' layout on a redraw`);
});

test("`#250 date-separators`: each column and the Pinned list say their own days, and a thread its own", async () => {
  const page = newPage();
  clockAt(page, 10, 12);
  const data = threadData();
  const days = [localStamp(10, 3, 9), localStamp(10, 4, 9), localStamp(10, 11, 9), localStamp(10, 12, 8), localStamp(10, 12, 9)];
  data.messages.forEach((m, i) => { m.timestamp = days[i]; });
  page.threadingSupported = true;
  page.threads = data.threads;
  for (const m of [data.messages[0], data.messages[3]]) page.pinned.set(m.id, storedPin(m));
  page.servePinsRevision = true;
  page.pinsRevision = 2;
  await signIn(page);
  await showDiscord(page, data.messages);
  await twoColumns(page);
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  // Main on the left: 200, 201, 203. The first thread on the right: 201, 202.
  assert.deepStrictEqual(linesOf(page, "discord-log").map(([id, , words]) => [id, words]),
    [["200", "Oct 3, Sat"], ["201", "Oct 4, Sun"], ["203", "Today, Oct 12, Mon"]]);
  assert.deepStrictEqual(linesOf(page, "side-log").map(([id, , words]) => [id, words]),
    [["201", "Oct 4, Sun"], ["202", "Yesterday, Oct 11, Sun"]], "the right column does not say its own days");
  await page.setDesk(false);
  await showPinned(page);
  assert.deepStrictEqual(linesOf(page, "pinned-log").map(([id, , words]) => [id, words]),
    [["200", "Oct 3, Sat"], ["203", "Today, Oct 12, Mon"]], "the Pinned list does not say its days");
});

test("`#250 date-separators`: replies gathered under their message, out of time order, say no day and move none", async () => {
  const page = newPage();
  clockAt(page, 10, 12);
  const data = scatteredThread();
  const days = { 597: 3, 598: 3, 599: 3, 600: 3, 601: 4, 602: 5, 603: 5, 604: 6, 605: 7, 606: 8, 607: 9, 608: 10, 609: 10 };
  data.messages.forEach((m, i) => { m.timestamp = localStamp(10, days[m.id], 9, i); });
  page.threadingSupported = true;
  page.threads = data.threads;
  await signIn(page);
  await showDiscord(page, data.messages);
  assert.deepStrictEqual(linesOf(page).map(([id, , words]) => [id, words]), [
    ["597", "Oct 3, Sat"], ["601", "Oct 4, Sun"], ["602", "Oct 5, Mon"], ["604", "Oct 6, Tue"],
    ["605", "Oct 7, Wed"], ["606", "Oct 8, Thu"], ["607", "Oct 9, Fri"], ["608", "Oct 10, Sat"],
  ]);
  await gatherChip(page, "601").click();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), GATHERED);
  assert.deepStrictEqual(linesOf(page).map(([id, , words]) => [id, words]), [
    ["597", "Oct 3, Sat"], ["601", "Oct 4, Sun"], ["605", "Oct 7, Wed"], ["608", "Oct 10, Sat"],
  ], "a gathered reply was ruled off as though it were in time order");
});

// --- one day in the owner's words, and the polish before landing -----------------------------------
//
// The owner's wording for a single day: "Today, Oct 12, Mon" and "Yesterday, Oct 11, Sun" with the
// date, "Oct 9, Fri" otherwise, and "Oct 9, 2025, Thu" when the year is not now's — Today and
// Yesterday across New Year too: "Yesterday, Dec 31, 2025, Wed". One function, `dayLabel(when, now)`,
// says it; a range stays compact. `dayLabel` is the time's tooltip's formatter (`#260 timestamp-hover`),
// whose table above pins its words — today, yesterday, this year, another year, across New Year and
// both clock changes — so a day rule is checked here to BE that function, not a copy of it.

test("`#250 date-separators`: one day is said in the owner's words by `dayLabel`, the page's one wording of a day", async () => {
  const dayLabel = newPage().pure("dayLabel");
  await inZone("America/New_York", () => {
    const at = (year, month, day, hour = 12, minute = 0) => new Date(year, month - 1, day, hour, minute);
    assert.equal(dayLabel(at(2026, 10, 11, 17, 25), at(2026, 10, 12, 15, 56)), "Yesterday, Oct 11, Sun");
    assert.equal(dayLabel(at(2025, 12, 31, 23, 50), at(2026, 1, 1, 0, 30)), "Yesterday, Dec 31, 2025, Wed");
  });
  // A day rule says exactly what `dayLabel` says of the day: the rule IS the function, not a copy.
  assert.match(SCRIPT_CODE, /return dayLabel\(new Date\(year, month, day, 12\), clock\.now\);/,
    "a day's rule words its day without `dayLabel`");
  assert.equal((SCRIPT_CODE.match(/^function dayLabel\(/gm) || []).length, 1, "there is more than one wording of a day");
  assert.equal((SCRIPT_CODE.match(/^const DAY_MONTHS = /gm) || []).length, 1, "the names of the months are kept twice");
});

test("`#250 date-separators`: a range stays compact — across a year, within a month of a past year, and ending Today or Yesterday", async () => {
  const page = newPage();
  const range = page.pure("dayRangeWords");
  const clockNow = page.pure("dayClock");
  clockAt(page, 10, 12);
  const now = clockNow();
  const cases = [
    [20261007, 20261009, "Oct 7 – 9"],
    [20260929, 20261002, "Sep 29 – Oct 2"],
    [20251230, 20260102, "Dec 30, 2025 – Jan 2"],
    [20241230, 20250102, "Dec 30, 2024 – Jan 2, 2025"],
    [20251003, 20251005, "Oct 3 – 5, 2025"],
    [20250929, 20251002, "Sep 29 – Oct 2, 2025"],
    [20261009, 20261012, "Oct 9 – Today"],
    [20261008, 20261011, "Oct 8 – Yesterday"],
    [20261011, 20261012, "Yesterday – Today"],
    [20251230, 20261012, "Dec 30, 2025 – Today"],
    // One day is not a range: it is said as `dayLabel` says it.
    [20261012, 20261012, "Today, Oct 12, Mon"],
    [20251005, 20251005, "Oct 5, 2025, Sun"],
  ];
  for (const [from, to, want] of cases) assert.equal(range(from, to, now), want, `${from} to ${to}`);
  // New Year's Day: the days before it are another year's, and the last of them is Yesterday.
  page.setClock(new Date(2026, 0, 1, 9, 0).getTime());
  const newYear = clockNow();
  assert.equal(range(20251229, 20251231, newYear), "Dec 29, 2025 – Yesterday");
  assert.equal(range(20251231, 20251231, newYear), "Yesterday, Dec 31, 2025, Wed");
  assert.equal(range(20251231, 20260101, newYear), "Yesterday – Today");
});

test("`#250 date-separators`: a search's gaps across a year say both years where they differ from now's, and a past year's month once", async () => {
  const page = newPage();
  clockAt(page, 10, 12);
  await signIn(page);
  const at = (year, month, day) => new Date(year, month - 1, day, 10, 0).toISOString();
  await showDiscord(page, [
    message({ id: "1", content: "engine one", timestamp: at(2025, 10, 2) }),
    message({ id: "2", content: "plain", timestamp: at(2025, 10, 3) }),
    message({ id: "3", content: "plain", timestamp: at(2025, 10, 4) }),
    message({ id: "4", content: "engine two", timestamp: at(2025, 10, 5) }),
    message({ id: "5", content: "plain", timestamp: at(2025, 10, 30) }),
    message({ id: "6", content: "plain", timestamp: at(2025, 11, 2) }),
    message({ id: "7", content: "engine three", timestamp: at(2025, 11, 3) }),
    message({ id: "8", content: "plain", timestamp: at(2025, 12, 30) }),
    message({ id: "9", content: "plain", timestamp: at(2025, 12, 31) }),
    message({ id: "10", content: "engine four", timestamp: at(2026, 1, 2) }),
    message({ id: "11", content: "plain", timestamp: at(2026, 10, 11) }),
  ]);
  await search(page, "engine");
  await page.settle();
  assert.deepStrictEqual(linesOf(page), [
    ["1", "day", "Oct 2, 2025, Thu", null],
    ["4", "gap", "… 2 unmatched messages · Oct 3 – 5, 2025 …", null],
    ["7", "gap", "… 2 unmatched messages · Oct 30 – Nov 3, 2025 …", null],
    ["10", "gap", "… 2 unmatched messages · Dec 30, 2025 – Jan 2 …", "… 1 unmatched message · Yesterday, Oct 11, Sun …"],
  ], "a gap across a year, or within a past year's month, is not said compactly");
});

test("`#250 date-separators`: past midnight a channel nobody writes in says Yesterday where it said Today, though the refresh that finds nothing draws nothing", async () => {
  /**
   * A channel read at 23:55 through a server with a change record, and polled at 00:05 with nothing
   * new: within `TIMELINE_RECONCILE_MS` of the read, so each poll is a delta that brings nothing.
   */
  const quietMidnight = async (script, typed = null) => {
    const page = newPage(new Map(), script);
    page.setClock(new Date(2026, 9, 12, 23, 55).getTime());
    // A provider with threads, whose reads go through the timeline and can be deltas; none in it.
    page.threadingSupported = true;
    page.threads = [];
    page.messages = dayChannel();
    const server = forwardTimeline(page);
    await signIn(page);
    await showDiscord(page, page.messages);
    if (typed !== null) {
      await search(page, typed);
      await page.settle();
    }
    // The first poll reads the view as a delta from then on, and brings nothing.
    await pollOnce(page);
    const before = linesOf(page);
    const rows = [...page.el("discord-log").children];
    const made = page.createdTags.length;
    const reads = server.reads.length;
    page.setClock(new Date(2026, 9, 13, 0, 5).getTime());
    await pollOnce(page);
    assert.ok(server.reads.length > reads && server.reads.slice(reads).every((read) => read.after !== null),
      "the poll past midnight was not a delta");
    assert.ok(page.el("discord-log").children.every((li, at) => li === rows[at]), "the quiet refresh rebuilt the rows");
    assert.equal(builtSince(page, made, "li"), 0, "the quiet refresh built rows");
    return { page, before, after: linesOf(page) };
  };
  const words = (lines) => lines.slice(-2).map(([, , said]) => said);
  const plain = await quietMidnight(SCRIPT);
  assert.deepStrictEqual(words(plain.before), ["Yesterday, Oct 11, Sun", "Today, Oct 12, Mon"]);
  assert.deepStrictEqual(words(plain.after), ["Oct 11, Sun", "Yesterday, Oct 12, Mon"], "midnight on a quiet channel left Today on yesterday");
  // Under a search, the gaps' days turn too; and a quiet refresh later the same night writes nothing.
  const searched = await quietMidnight(SCRIPT, "engine");
  assert.deepStrictEqual(searched.before.at(-1), ["6", "gap", "… 1 unmatched message · Yesterday – Today …", "… 1 unmatched message …"]);
  assert.deepStrictEqual(searched.after.at(-1), ["6", "gap", "… 1 unmatched message · Oct 11 – Yesterday …", "… 1 unmatched message …"],
    "midnight under a search left a gap's days as they were");
  const set = FakeElement.prototype.setAttribute;
  let written = 0;
  FakeElement.prototype.setAttribute = function counted(name, value) {
    if (name === "data-seam" || name === "data-seam-below" || (this.isRowSeam() && name === "aria-label")) written += 1;
    return set.call(this, name, value);
  };
  try {
    searched.page.setClock(new Date(2026, 9, 13, 0, 8).getTime());
    await pollOnce(searched.page);
  } finally {
    FakeElement.prototype.setAttribute = set;
  }
  assert.equal(written, 0, "a quiet refresh on the same day drew the lines again");
  // The control: without the turn of the day noticed there, the quiet refresh leaves Today standing,
  // so the refresh above really was the one that draws nothing.
  const control = await quietMidnight(brokenScript("      seamsAfterMidnight();\n", ""));
  assert.deepStrictEqual(words(control.after), ["Yesterday, Oct 11, Sun", "Today, Oct 12, Mon"],
    "the control relabelled the days anyway, so the quiet refresh was not the one under test");
});

test("`#249 search-highlight` x `#250 date-separators`: a line drawn above a row, the bookmark put on it and a hit marked beside a link leave the focus where it was", async () => {
  /** Focus a control in a row, then draw a gap above it, mark the hits by it, and bookmark another row. */
  const focusAcross = async (script) => {
    const page = newPage(new Map(), script);
    clockAt(page, 10, 12);
    page.serveBookmark = true;
    await signIn(page);
    const messages = dayChannel();
    messages[6] = { ...messages[6], content: "plain this morning, see [the notes](https://example.com/n) for more plain words",
      content_html: `<p>plain this morning, see <a href="https://example.com/n" ${LINK_ATTRS}>the notes</a> for more plain words</p>\n` };
    await showDiscord(page, messages);
    FakeElement.detachBlurs = true;
    const kept = {};
    const row = rowWithId(page, "7");
    const link = row.descendants().find((node) => node.tagName === "a");
    // The bar open first, which takes the focus to its field; then a link in the row holds it.
    await page.el("search-toggle").click();
    await page.settle();
    link.focus();
    // The search leaves 6 out before 7, so a gap's line goes in at the head of 7's row.
    await page.el("search-field").setValue("plain");
    await page.settle();
    assert.equal(row.getAttribute("data-seam"), "gap", "the fixture drew no line above the row");
    kept.seam = focused(page) === link;
    // The hits are marked in the paragraph the link stands in, either side of it.
    link.focus();
    await page.el("search-field").dispatch("keydown", { key: "Enter" });
    await page.settle();
    assert.deepStrictEqual(hitsIn(row), ["plain", "plain"], "the fixture marked nothing beside the link");
    kept.marks = focused(page) === link;
    link.focus();
    await page.el("search-field").setValue("");
    assert.deepStrictEqual(hitsIn(row), []);
    kept.unmarked = focused(page) === link;
    // The bookmark's line goes in at the head of 4's row, after its day's rule; its ⋯ item has the focus.
    await page.el("search-toggle").click();
    await page.settle();
    const item = rowWithId(page, "4").descendants().find((node) => node.className === "row-bookmark-button");
    item.focus();
    await item.click();
    await page.settle();
    assert.deepStrictEqual(rowWithId(page, "4").children.slice(0, 2).map((kid) => kid.className), ["row-seam", "bookmark-line"]);
    kept.bookmark = focused(page) === item;
    return { kept, lost: FakeElement.focusLost };
  };
  const { kept, lost } = await focusAcross(SCRIPT);
  assert.deepStrictEqual(kept, { seam: true, marks: true, unmarked: true, bookmark: true }, "a pass over the rows took the focus off a control in one");
  assert.equal(lost, 0, "a pass over the rows took something with the focus out of the document");
  // The controls: each line put in by writing the row's children again, as before. The fixture loses
  // the focus as a browser does, so the checks above can fail.
  const seam = await focusAcross(brokenScript("    else row.insertBefore(line, row.firstChild);", "    else row.replaceChildren(line, ...row.children);"));
  assert.equal(seam.kept.seam, false, "the control drew the line above the row and kept the focus, so the check proves nothing");
  const marks = await focusAcross(brokenScript(
    "      for (const made of pieces) parent.insertBefore(made, piece.node);\n      parent.removeChild(piece.node);",
    "      parent.replaceChildren(...[...parent.childNodes].flatMap((kid) => (kid === piece.node ? pieces : [kid])));"));
  assert.equal(marks.kept.marks, false, "the control marked the hits and kept the focus, so the check proves nothing");
  const bookmark = await focusAcross(brokenScript(
    "      row.insertBefore(line, first !== null && first === row.seamLine ? first.nextSibling : first);",
    "      row.replaceChildren(...(first === row.seamLine ? [first] : []), line, ...row.children.filter((kid) => kid !== row.seamLine));"));
  assert.equal(bookmark.kept.bookmark, false, "the control drew the bookmark and kept the focus, so the check proves nothing");
});

test("`#249 search-highlight`: the rows follow every keystroke, and the hits are marked once the typing pauses — at once on Enter, and on clearing", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [
    message({ id: "1", content: "the engine stalled at noon" }),
    message({ id: "2", content: "nothing about it here" }),
  ]);
  await search(page, "e");
  for (const typed of ["en", "eng", "engine"]) await page.el("search-field").setValue(typed);
  assert.deepStrictEqual(passing(page), ["1"], "the rows did not follow the keystroke");
  assert.deepStrictEqual(hitsIn(rows[0]), [], "a keystroke marked the hits before the typing paused");
  assert.equal(marksPending(page), 1, "each keystroke left a marking of its own pending");
  await typingPaused(page);
  assert.deepStrictEqual(hitsIn(rows[0]), ["engine"], "the pause did not mark the hits");
  assert.equal(marksPending(page), 0);
  // Enter is the reader saying the words are typed.
  await page.el("search-field").setValue("engine noon");
  assert.deepStrictEqual(hitsIn(rows[0]), ["engine"], "the marks followed the keystroke");
  await page.el("search-field").dispatch("keydown", { key: "Enter" });
  await page.settle();
  assert.deepStrictEqual(hitsIn(rows[0]), ["engine", "noon"], "Enter did not mark the hits at once");
  assert.equal(marksPending(page), 0, "Enter left a marking pending");
  // Cleared, no mark outlives its words, and nothing waits to put one back.
  await page.el("search-field").setValue("");
  assert.deepStrictEqual(rows.flatMap(hitsIn), [], "a mark outlived the cleared field");
  assert.equal(marksPending(page), 0, "clearing left a marking pending");
  // Closing the bar with a marking pending: nothing is marked after it shuts.
  await page.el("search-field").setValue("stalled");
  await page.el("search-toggle").click();
  await page.settle();
  assert.equal(marksPending(page), 0, "closing the bar left a marking pending");
  assert.deepStrictEqual(rows.flatMap(hitsIn), []);
});

test("`#249 search-highlight`: marking a paragraph drawn in many pieces writes only the text nodes it splits", async () => {
  const page = newPage();
  await signIn(page);
  const pieces = Array.from({ length: 60 }, (_unused, i) => `<strong>w${i}</strong> engine `).join("");
  const rows = await showDiscord(page, [message({ id: "1", content: "many pieces", content_html: `<p>${pieces}</p>\n` })]);
  const paragraph = bodyOf(rows[0]).descendants().find((node) => node.tagName === "p");
  const kids = [...paragraph.children];
  const replace = FakeElement.prototype.replaceChildren;
  const insert = FakeElement.prototype.insertBefore;
  let replaced = 0;
  let inserted = 0;
  FakeElement.prototype.replaceChildren = function counted(...made) {
    if (this === paragraph) replaced += 1;
    return replace.apply(this, made);
  };
  FakeElement.prototype.insertBefore = function counted(node, ref) {
    if (this === paragraph) inserted += 1;
    return insert.call(this, node, ref);
  };
  try {
    await typeAndPause(page, "engine");
  } finally {
    FakeElement.prototype.replaceChildren = replace;
    FakeElement.prototype.insertBefore = insert;
  }
  assert.equal(hitsIn(rows[0]).length, 60);
  assert.equal(replaced, 0, "the paragraph's children were written again to mark one piece");
  // Each " engine " is three pieces where it was one: a space, the mark, a space.
  assert.equal(inserted, 60 * 3, `marking 60 hits put ${inserted} nodes into the paragraph`);
  assert.ok(kids.filter((kid) => kid.tagName === "strong").every((kid) => kid.parentNode === paragraph),
    "an element beside a hit left the paragraph");
});

test("`#249 search-highlight`: in summary mode a folded match's hits past its fold are said under its summary, the one place they show", async () => {
  const page = newPage();
  await signIn(page);
  const content = `${longMessage("rollout")} Then the engine stalled at the end.`;
  const rows = await showDiscord(page, [message({ id: "7000000000000000031", content })]);
  await turnSummariesOn(page);
  assert.equal(rows[0].getAttribute("data-summarised"), "true", "the fixture never summarised");
  assert.equal(bodyOf(rows[0]).hidden, true, "the summary did not take the message's place");
  await typeAndPause(page, "engine");
  const line = excerptOf(rows[0]);
  assert.ok(line, "a summarised match says nothing of its hits");
  assert.equal(line.hidden, false, "the line under a summarised match is hidden");
  assert.match(line.text(), /engine stalled at the end/);
  assert.deepStrictEqual(line.descendants().filter((n) => n.tagName === "mark").map((n) => n.text()), ["engine"]);
  // Nothing in the stylesheet hides it under a summary: only the Links view does.
  const hiding = [...CSS_CODE.matchAll(/([^{}]+)\{([^{}]*)\}/g)]
    .filter((rule) => /search-excerpt/.test(rule[1]) && /display:\s*none/.test(rule[2]))
    .map((rule) => rule[1].trim());
  assert.ok(hiding.every((selector) => selector.includes('data-links-view="true"')), `a rule hides the line: ${hiding}`);
});

test("`#249 search-highlight`: with rich headers off the words a header restates are marked where they show, and the switch needs no marking again", async () => {
  const page = newPage();
  await signIn(page);
  await page.el("rich-headers").setChecked(false);
  await page.settle();
  const rows = await showDiscord(page, [message({ id: "100", content: HEALTH, heading: HEALTH_HEADING })]);
  await typeAndPause(page, "backup");
  const source = rows[0].descendants().find((node) => node.hasClass("rh-src"));
  assert.ok(source, "the fixture's body does not carry the words its header restates");
  assert.deepStrictEqual(source.descendants().filter((n) => n.tagName === "mark").map((n) => n.text()), ["Backup"],
    "with the header off, the hit in the words it restates is not marked");
  // ...and in the header too, hidden while it is off, so turning it on shows its hit with no new pass.
  assert.deepStrictEqual(hitsIn({ children: [headOf(rows[0])] }), ["Backup"]);
  const made = page.createdTags.length;
  await page.el("rich-headers").setChecked(true);
  await page.settle();
  assert.equal(builtSince(page, made, "mark"), 0, "the switch marked the hits again");
  assert.match(cssBlock(':root[data-rich-headers="off"] .rh-head'), /display: none/);
});
