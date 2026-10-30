// The app page's tests: two columns on a desk: the cards, their connectors, focus and the card over a thread.
//
// One feature's file of the suite tests/js/voice_page.test.mjs runs, against the fixture in
// ./fixture.mjs. Run with the rest of the suite, or alone: node --test tests/js/page/columns.mjs.

import assert from "node:assert/strict";

import { CHANNEL, CSS_CODE, DESKTOP_QUERY, DISCORD_POLL_MS, FakeElement, HTML, HTML_CODE, LINE_PX,
  PAGE_CODE, RUN_TAIL, SCRIPT, SCRIPT_CODE, THREADS_MAIN_STORAGE, TWO_COLUMN_QUERY, UI_STATE_KEY,
  assertCurrent, assertMarkupContains, backlog, brokenModule, brokenScript, brokenScriptEach,
  builtSince, cardIds, cardOf, cardPart, cardRow, cardsListed, cardsUp, chooseReadMode, cssBlock,
  cssBlockIn, cssBlockOutside, cssRules, cssRulesElsewhere, cssWithoutIn, deliver, doneButton,
  drawnLinks, errorResponse, firstCardInView, focusName, focused, forwardTimeline, freshMessageData,
  headerCard, inTwoColumns, json, layColumnsOut, listCardSays, litCards, litRows, liveReply,
  manyThreadColumns, manyThreads, markupHolds, mediaBodyOf, message, mouseTo, newPage,
  nodeWithinFixture, openerOf, optionLabel, pickThread, pinChip, pollOnce, press, pressHere,
  reReadChannel, readModeSaid, readModeTo, readViewsSince, reloadWith, reopenedChannel, replyHint,
  rowWithId, runId, runThread, savedUiState, scopedPage, search, selectedId, showDiscord, shownIds,
  sideAtFoot, sideIds, sideRow, signIn, sourceConstant, sseMessage, sseUpdate, startOf, tapPin,
  test, threadBadge, threadButton, threadData, threadOptions, threadPage, threadedService,
  turnTodoOn, twoColumnServer, twoColumns, unreadChip, viewOf, wideCard } from "./fixture.mjs";

const COLUMNS_STORAGE = "vibe-talk.voice.columns";

// --- `#373 two-column-full-width` -----------------------------------------------------------------
//
// The owner, 2026-10-19: "I asked a while ago for full-width messages from the coding agent in
// two-column mode. Why don't I have this? In this screenshot it shows wasted space." Each column held its
// list to the one-column reading width, and the desk's speaker insets cut the agent's tile shorter again.
// And, correcting the first version: "MY messages are still partial width to make them visually
// distinct. The agents usually-long messages are full column. That's how we previously did it on narrow
// phone columns." So every row but the owner's spans its column; the owner's keep their inset.
// tests/desktop_layout_browser.py measures the rows in Chromium; this pins the rules that place them.

/** The parts of the two columns that must span their column there, and not elsewhere. */
const TWO_COLUMN_SPANS = ["#pane-discord", "> #connection-banner", "> #channel-freshness-anchor", "#side-heading", "#side-freshness", "#side-list", "#side-pane"];

/**
 * What is wrong with two columns' widths against `#373`, as sentences; empty when nothing is. A function
 * of the stylesheet, so each negative control can hand it one broken.
 */
function twoColumnWidthProblems(css) {
  const problems = [];
  const two = (selector) => `#screen-main[data-columns="two"] ${selector}`;
  const inside = (selector) => cssRules(mediaBodyOf(css, TWO_COLUMN_QUERY), selector).join("\n");
  for (const part of TWO_COLUMN_SPANS) {
    if (!/max-width:\s*none/.test(inside(two(part)))) problems.push(`${part.replace("> ", "")} is still held to the reading width in two columns`);
  }
  // The speakers: in two columns the agent's tile and other people's rows take the column, through
  // properties whose fallbacks are a desk's insets, and the owner's rows keep the desk's inset.
  const vars = inside('#screen-main[data-columns="two"]');
  const value = (name) => (new RegExp(`${name}:\\s*([^;]+);`).exec(vars) || [])[1];
  if (!/^0(?:px)?$/.test(value("--desk-agent-inset") || "")) problems.push("the agent's tile keeps a desk's inset in two columns");
  if (!/^0(?:px)?$/.test(value("--desk-third-inset") || "")) problems.push("other people's rows keep a desk's inset in two columns");
  const desk = (selector) => cssRules(mediaBodyOf(css, DESKTOP_QUERY), selector).join("\n");
  const agent = desk('#discord-log li.discord-message[data-who="coder"]');
  if (!/margin-right:\s*var\(--desk-agent-inset,\s*min\(15%,\s*6rem\)\)/.test(agent)) problems.push("the agent's desk inset is not read through --desk-agent-inset");
  if (!/margin-inline:\s*var\(--desk-third-inset,\s*min\(7%,\s*3rem\)\)/.test(desk('#discord-log li.discord-message[data-who="human"]'))) {
    problems.push("a third party's desk inset is not read through --desk-third-inset");
  }
  // The owner's rows keep their partial width: the desk's inset as a number, nothing in two columns
  // overriding it, and nothing setting a property it could be read through.
  const owner = desk('#discord-log li.discord-message[data-who="me"]');
  const ownerTwo = [...mediaBodyOf(css, TWO_COLUMN_QUERY).matchAll(/([^{}]+)\{([^{}]*)\}/g)]
    .filter((rule) => /data-columns="two"/.test(rule[1]) && /data-who="me"|\.mine\b/.test(rule[1]) && /margin/.test(rule[2]));
  if (!/margin-left:\s*min\(15%,\s*6rem\)/.test(owner) || ownerTwo.length > 0 || /--desk-owner-inset:/.test(css)) {
    problems.push("the owner's rows lose their partial width in two columns");
  }
  // One column keeps its reading width: nothing else sets the properties, nor uncaps the lists.
  const elsewhere = cssWithoutIn(css, TWO_COLUMN_QUERY);
  if (/--desk-(?:agent|third)-inset:/.test(elsewhere)) problems.push("a speaker's two-column inset is set outside two columns");
  if (!/max-width:\s*var\(--reading-width\)/.test(desk("#pane-discord"))) problems.push("one column lost its reading width");
  // The things placed by the column follow it.
  if (!/width:\s*calc\(50% - 1\.2rem\)/.test(inside(two("> #scroll-tools")))) problems.push("the chips over the left list keep the reading width");
  // Since `#406 command-bar-one-row` the desk's dock spans the window in every view, two columns included.
  if (!/--dock-inset:\s*0\.6rem/.test(desk("#dock"))) problems.push("the dock keeps the reading width's inset in two columns");
  return problems;
}

test("`#373 two-column-full-width`: in two columns the lists span their columns, every row but the owner's across it; one column keeps its width", () => {
  assert.deepStrictEqual(twoColumnWidthProblems(CSS_CODE), []);
});

test("`#373 two-column-full-width`: the check refuses a capped list, a desk inset in two columns, an owner's row across the column, and a wide single column (negative controls)", () => {
  const capped = CSS_CODE.replace('#screen-main[data-columns="two"] #pane-discord,\n', "");
  assert.notEqual(capped, CSS_CODE, "the negative control changed nothing");
  assert.deepStrictEqual(twoColumnWidthProblems(capped), ["#pane-discord is still held to the reading width in two columns"]);
  const inset = CSS_CODE.replace(/--desk-agent-inset:\s*0px;/, "");
  assert.notEqual(inset, CSS_CODE, "the negative control changed nothing");
  assert.deepStrictEqual(twoColumnWidthProblems(inset), ["the agent's tile keeps a desk's inset in two columns"]);
  const literal = CSS_CODE.replace("margin-right: var(--desk-agent-inset, min(15%, 6rem));", "margin-right: min(15%, 6rem);");
  assert.notEqual(literal, CSS_CODE, "the negative control changed nothing");
  assert.deepStrictEqual(twoColumnWidthProblems(literal), ["the agent's desk inset is not read through --desk-agent-inset"]);
  const third = CSS_CODE.replace(/--desk-third-inset:\s*0px;/, "--desk-third-inset: 0.75rem;");
  assert.notEqual(third, CSS_CODE, "the negative control changed nothing");
  assert.deepStrictEqual(twoColumnWidthProblems(third), ["other people's rows keep a desk's inset in two columns"]);
  // The owner's rows across the column: by a two-column rule, or through a property, as the first version did.
  const ownerInBlock = CSS_CODE.replace(/(#screen-main\[data-columns="two"\] \{\s*--desk-third-inset)/,
    '#screen-main[data-columns="two"] #discord-log li.discord-message[data-who="me"] {\n    margin-left: 0;\n  }\n\n  $1');
  assert.notEqual(ownerInBlock, CSS_CODE, "the negative control changed nothing");
  assert.deepStrictEqual(twoColumnWidthProblems(ownerInBlock), ["the owner's rows lose their partial width in two columns"]);
  const ownerVar = CSS_CODE.replace(/(--desk-third-inset:\s*0px;)/, "$1\n    --desk-owner-inset: 0px;")
    .replace(/(#discord-log li\.discord-message\[data-who="me"\],\n\s*#pinned-log li\.discord-message\[data-who="me"\] \{\n\s*)margin-left: min\(15%, 6rem\);/,
      "$1margin-left: var(--desk-owner-inset, min(15%, 6rem));");
  assert.notEqual(ownerVar, CSS_CODE, "the negative control changed nothing");
  assert.deepStrictEqual(twoColumnWidthProblems(ownerVar), ["the owner's rows lose their partial width in two columns"]);
  const everywhere = `${CSS_CODE}\n:root { --desk-agent-inset: 0px; }\n`;
  assert.deepStrictEqual(twoColumnWidthProblems(everywhere), ["a speaker's two-column inset is set outside two columns"]);
  const chips = CSS_CODE.replace("width: calc(50% - 1.2rem);", "width: calc(min(var(--reading-width), 50%) - 1.2rem);");
  assert.notEqual(chips, CSS_CODE, "the negative control changed nothing");
  assert.deepStrictEqual(twoColumnWidthProblems(chips), ["the chips over the left list keep the reading width"]);
});

test("Ctrl+\\ AND Cmd+\\ turn two columns on and off like the layout button, and say when the window is too narrow", async () => {
  for (const fields of [{ ctrlKey: true }, { metaKey: true }]) {
    const page = newPage();
    const data = threadData();
    page.threadingSupported = true;
    page.threads = data.threads;
    page.messages = data.messages;
    await signIn(page);
    await showDiscord(page, data.messages);
    page.wideDesk = true;
    await press(page, page.el("scroll-area"), "\\", fields);
    await page.settle();
    assert.ok(inTwoColumns(page), `${JSON.stringify(fields)} \\ did not give two columns on a wide desk`);
    await press(page, page.el("scroll-area"), "\\", fields);
    await page.settle();
    assert.ok(!inTwoColumns(page), `${JSON.stringify(fields)} \\ did not go back to one column`);
    page.wideDesk = false;
    await press(page, page.el("scroll-area"), "\\", fields);
    await page.settle();
    assert.ok(!inTwoColumns(page), "two columns appeared in a narrow window");
    assert.match(page.el("status").textContent, /wider window/, "a narrow window did not say why nothing changed");
  }
});

test("`#281 view-keys`: the thread list is the right column on its own, across the window at every width, over the left column kept behind it", async () => {
  // The stylesheet's, since this fixture lays nothing out: drawn outside the desk's block, so a phone
  // has it too, in the list's own cell, under the floating line and over the left column.
  const column = cssBlockOutside(TWO_COLUMN_QUERY, '#screen-main[data-columns="list"] > #side-column');
  assert.match(column, /display:\s*grid/, "the thread list is not drawn outside the desk's block");
  assert.match(column, /grid-row:\s*3/);
  assert.match(column, /grid-column:\s*1/);
  assert.match(column, /padding-top:\s*calc\(var\(--float-line\)/, "the head of the list is under the search glass");
  // The left column is laid out still, out of sight, so coming back to it finds the reader where they were.
  assert.match(cssBlockOutside(TWO_COLUMN_QUERY, '#screen-main[data-columns="list"] > #scroll-area'), /visibility:\s*hidden/);
  assert.match(cssBlockOutside(TWO_COLUMN_QUERY, '#screen-main[data-columns="list"] #channel-freshness'), /visibility:\s*hidden/,
    "the left column's pill speaks over the thread list");
  // The same cards as two columns, by the same rules: none of them is the desk's alone any more.
  for (const selector of [".side-card", ".side-card-text", "#side-heading", "#side-scroll", "#side-more-threads"]) {
    assert.ok(cssRulesElsewhere(TWO_COLUMN_QUERY, selector) > 0, `${selector} is drawn only on a desk`);
  }

  const page = await threadPage();
  await press(page, page.el("scroll-area"), "3", { ctrlKey: true });
  await page.settle();
  assert.equal(page.el("screen-main").getAttribute("data-columns"), "list");
  assert.ok(page.el("side-column").getClientRects().length > 0, "the thread list is not on screen on a phone");
  assert.equal(page.el("side-list").hidden, false);
  assert.equal(page.el("side-close").hidden, true, "an X with nothing to close");
  assert.equal(page.el("side-unpack").hidden, false, "Unpack msgs is not over the thread list");
  assert.deepStrictEqual(cardIds(page), threadData().threads.map((thread) => thread.id));
  // On a phone two columns are not offered, and Ctrl+4 says why it did nothing.
  assert.ok(!threadOptions(page).some(([value]) => value === "columns"), "a phone was offered two columns");
  await press(page, page.el("side-scroll"), "4", { ctrlKey: true });
  await page.settle();
  assert.match(page.el("status").textContent, /wider window/);
  assert.equal(page.el("thread-select").value, "threads", "Ctrl+4 in a narrow window left the thread list");
  // A layout keyed on another keyboard: Ctrl+& is Ctrl+1 on AZERTY, by the place of its key.
  const azerty = await press(page, page.el("side-scroll"), "&", { ctrlKey: true, code: "Digit1" });
  await page.settle();
  assert.equal(azerty.defaulted, false);
  assert.equal(page.el("thread-select").value, "main", "Ctrl+1 on AZERTY did not choose Main");
  // But Shift held is another key: Ctrl+Shift+2 on QWERTY is Ctrl+@, on the key Ctrl+2 is read from, and
  // chooses nothing. On AZERTY, Shift types the digit itself, which is read as that digit.
  const shifted = await press(page, page.el("scroll-area"), "@", { ctrlKey: true, shiftKey: true, code: "Digit2" });
  await page.settle();
  assert.equal(shifted.defaulted, true, "Ctrl+Shift+2 was taken");
  assert.equal(page.el("thread-select").value, "main", "Ctrl+Shift+2, Ctrl+@, chose All");
  const typed = await press(page, page.el("scroll-area"), "2", { ctrlKey: true, shiftKey: true, code: "Digit2" });
  await page.settle();
  assert.equal(typed.defaulted, false);
  assert.equal(page.el("thread-select").value, "flat", "Ctrl+Shift+é on AZERTY, which types 2, did not choose All");
  // A channel that is itself one thread has no list to show.
  const { page: scoped } = await scopedPage();
  await press(scoped, scoped.el("scroll-area"), "3", { ctrlKey: true });
  await scoped.settle();
  assert.equal(scoped.el("screen-main").hasAttribute("data-columns"), false);
  assert.match(scoped.el("status").textContent, /single thread/);
});

test("`#229 desktop-two-column`: the layout button is a toggle in the bar, offered on a channel with threads and drawn only on a desk 1000px wide", async () => {
  assertMarkupContains("bar-pack", "column-toggle");
  const bar = HTML_CODE.slice(HTML_CODE.indexOf('id="bar-pack"'));
  assert.ok(bar.indexOf('id="thread-select"') < bar.indexOf('id="column-toggle"'),
    "the layout button is not beside the view picker it greys out");
  const tag = /<button\s+id="column-toggle"[^>]*>/.exec(HTML_CODE)[0];
  assert.match(tag, /aria-pressed="false"/, "the button does not say whether two columns are on");
  assert.match(tag, /aria-label="Two columns"/, "the button has no fixed name");
  assert.match(tag, /(?:^|\s)hidden(?=[\s>])/, "the button ships on screen, before anything has decided it is offered");
  // Drawn only inside the block, whose query is the desk's AND a width for two columns. Outside it,
  // neither the button nor the right column is ever drawn: a phone and a narrow window are unchanged.
  assert.match(cssBlockOutside(TWO_COLUMN_QUERY, "#column-toggle"), /display:\s*none/);
  assert.match(cssBlockIn(TWO_COLUMN_QUERY, "#column-toggle"), /display:\s*flex/);
  assert.match(cssBlockOutside(TWO_COLUMN_QUERY, "#side-column"), /display:\s*none/);
  assert.match(cssBlockIn(TWO_COLUMN_QUERY, '#screen-main[data-columns="two"] > #side-column'), /display:\s*grid/);
  assert.match(cssBlockIn(TWO_COLUMN_QUERY, '#screen-main[data-columns="two"]'),
    /grid-template-columns:\s*minmax\(0, 1fr\) minmax\(0, 1fr\)/);
  // ...and it is the stylesheet's to say, never a query the script reads.
  assert.doesNotMatch(PAGE_CODE, /matchMedia|innerWidth|outerWidth|screen\.width/);

  const page = await threadPage();
  assert.equal(page.el("column-toggle").hidden, false, "not offered on a channel whose provider has threads");
  assert.equal(page.el("column-toggle").getAttribute("aria-pressed"), "false");
  await page.el("view-switch").click();
  await page.settle();
  assert.equal(page.el("column-toggle").hidden, true, "offered over the call's transcript");
  const flat = newPage();
  await signIn(flat);
  await showDiscord(flat, [message({ content: "no threads on this provider" })]);
  assert.equal(flat.el("column-toggle").hidden, true, "offered where the provider has no threads");
});

test("`#229 desktop-two-column`: pressed in a narrow window it waits for a wide one; narrowed, one column comes back as it was, and the choice is kept", async () => {
  const page = await threadPage();
  assert.equal(page.el("thread-select").value, "flat");
  await page.el("column-toggle").click();
  await page.settle();
  assert.equal(page.el("column-toggle").getAttribute("aria-pressed"), "true", "the button does not say it is pressed");
  assert.equal(page.storage.get(COLUMNS_STORAGE), "two", "the choice was not kept on the device");
  // `#281 view-keys`. Not asked of the stylesheet until the window can draw them: the layout they were
  // chosen from stands meanwhile, one column here, and the thread list where they were chosen from it.
  assert.equal(page.el("screen-main").hasAttribute("data-columns"), false, "a narrow window was asked for two columns");
  assert.ok(!inTwoColumns(page), "a narrow window was given two columns");
  assert.equal(page.el("thread-select").disabled, false, "one column on screen, and the view picker greyed out");
  assert.equal(page.el("thread-select").hasAttribute("aria-disabled"), false, "one column on screen, and the view picker greyed out");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "202", "203", "204"], "a narrow window left All");

  await page.setDesk(true);
  assert.ok(inTwoColumns(page), "widening the window did not bring two columns");
  // `#281 view-keys`. Two columns are the picker's fourth view, which it says, rather than greying out.
  assert.equal(page.el("thread-select").hasAttribute("aria-disabled"), false, "the view picker greyed out");
  assert.equal(page.el("thread-select").value, "columns", "the view picker does not say two columns are up");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "the left column is not Main");
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  assert.deepStrictEqual(sideIds(page), ["201", "202"], "the thread was not selected on the right");

  await page.setDesk(false);
  assert.ok(!inTwoColumns(page));
  assert.equal(page.el("thread-select").disabled, false, "one column again, and the view picker still grey");
  assert.equal(page.el("thread-select").hasAttribute("aria-disabled"), false, "one column again, and the view picker still grey");
  assert.equal(page.el("thread-select").value, "flat", "one column again is not the view last chosen in one");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "202", "203", "204"]);
  assert.equal(page.storage.get(COLUMNS_STORAGE), "two", "narrowing the window forgot the choice");
  // Main in two columns was the layout's, not a choice, and All put back was not one either: the
  // reader chose no view here, so the channel still opens in its default.
  assert.deepStrictEqual(JSON.parse(page.storage.get(UI_STATE_KEY)).channels, [],
    "two columns recorded a view the reader did not choose");

  await page.setDesk(true);
  assert.ok(inTwoColumns(page));
  assert.deepStrictEqual(sideIds(page), ["201", "202"], "widening again lost the thread selected");
  assert.equal(page.el("side-close").hidden, false);

  // The button again: one column, for good, and said so.
  await page.el("column-toggle").click();
  await page.settle();
  assert.ok(!inTwoColumns(page));
  assert.equal(page.el("screen-main").hasAttribute("data-columns"), false);
  assert.equal(page.storage.get(COLUMNS_STORAGE), "one");
  assert.equal(page.el("column-toggle").getAttribute("aria-pressed"), "false");
  assert.equal(page.el("thread-select").value, "flat");
});

test("`#229 desktop-two-column`: a reload on a wide desk opens the channel in two columns, Main on the left", async () => {
  const first = await threadPage();
  await twoColumns(first);
  const data = threadData();
  const page = reloadWith(first.storage, data.messages, (p) => {
    p.threadingSupported = true;
    p.threads = data.threads;
    p.wideDesk = true;
  });
  await reopenedChannel(page);
  await page.settle();
  assert.ok(inTwoColumns(page), "the reload forgot two columns");
  assert.equal(page.el("column-toggle").getAttribute("aria-pressed"), "true");
  assert.equal(page.el("thread-select").value, "columns");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"]);
  assert.equal(page.el("side-list").hidden, false, "the right column did not open on its list of threads");
  // ...and a narrow window reloads in one column, in the view chosen there.
  const narrow = reloadWith(page.storage, data.messages, (p) => {
    p.threadingSupported = true;
    p.threads = data.threads;
  });
  await reopenedChannel(narrow);
  await narrow.settle();
  assert.ok(!inTwoColumns(narrow));
  assert.equal(narrow.el("thread-select").value, "flat");
  assert.equal(narrow.el("thread-select").disabled, false);
});

test("`#281 view-keys`: in two columns the view picker says so and chooses, and Ctrl+1 to Ctrl+4 leave them for each view", async () => {
  const page = await threadPage();
  await twoColumns(page);
  const picker = page.el("thread-select");
  // `#231 two-column-polish` greyed it here; two columns are its fourth view now, in the keys' order.
  assert.equal(picker.hasAttribute("aria-disabled"), false, "the view picker is still greyed in two columns");
  assert.equal(picker.value, "columns");
  assert.deepStrictEqual(threadOptions(page).slice(0, 4),
    [["main", "Main"], ["flat", "All"], ["threads", "Thread list"], ["columns", "Two columns"]]);
  // The bare digits are retired: they type nothing here and change nothing.
  for (const key of ["1", "2", "3"]) {
    const event = await press(page, page.el("scroll-area"), key);
    assert.equal(event.defaulted, true, `${key} was taken`);
  }
  assert.ok(inTwoColumns(page), "a bare digit changed the layout");
  // Ctrl+1: one column, Main.
  let event = await press(page, page.el("scroll-area"), "1", { ctrlKey: true });
  assert.equal(event.defaulted, false, "Ctrl+1 was left to the browser's tab switch");
  await page.settle();
  assert.ok(!inTwoColumns(page), "Ctrl+1 left two columns up");
  assert.equal(picker.value, "main");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"]);
  // Ctrl+4 back, then Ctrl+2: All.
  await press(page, page.el("scroll-area"), "4", { ctrlKey: true });
  await page.settle();
  assert.ok(inTwoColumns(page), "Ctrl+4 did not bring two columns back");
  assert.equal(picker.value, "columns");
  event = await press(page, page.el("scroll-area"), "2", { metaKey: true });
  assert.equal(event.defaulted, false);
  await page.settle();
  assert.ok(!inTwoColumns(page));
  assert.equal(picker.value, "flat");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "202", "203", "204"]);
  // ...and the picker chooses as the keys do.
  picker.value = "columns";
  await picker.dispatch("change");
  await page.settle();
  assert.ok(inTwoColumns(page), "choosing Two columns in the picker did not give them");
  picker.value = "threads";
  await picker.dispatch("change");
  await page.settle();
  assert.equal(page.el("screen-main").getAttribute("data-columns"), "list", "choosing Thread list did not show it");
  assert.equal(picker.value, "threads");
  assert.equal(page.el("column-toggle").getAttribute("aria-pressed"), "false", "the layout button still says two columns");
  // From the box of a thread selected on the right: one column, and the keys on the list on screen — not
  // left in a box the stylesheet has just taken away, where the browser drops the focus on the body.
  await press(page, page.el("side-scroll"), "4", { ctrlKey: true });
  await page.settle();
  // Its replies button: a click on the card itself only highlights it (`#279 thread-card-focus`).
  await openerOf(cardRow(page, threadData().threads[0].id)).click();
  await page.settle();
  assert.equal(page.el("side-pane").hidden, false, "a card's replies button did not select its thread on the right");
  for (const [key, value] of [["1", "main"], ["2", "flat"]]) {
    if (!inTwoColumns(page)) {
      await press(page, page.el("scroll-area"), "4", { ctrlKey: true });
      await page.settle();
    }
    assert.equal(page.el("side-pane").hidden, false, "two columns came back without the thread on the right");
    event = await press(page, page.el("side-compose-text"), key, { ctrlKey: true });
    await page.settle();
    assert.equal(event.defaulted, false);
    assert.equal(picker.value, value);
    assert.ok(focused(page) && nodeWithinFixture(page.el("scroll-area"), focused(page)),
      `Ctrl+${key} in the right column's box left the focus on ${focusName(page)}`);
  }
});

test("`#229 desktop-two-column`: with no thread selected, a card per thread, newest activity at the foot (`#261`), saying the summariser's name and summary or else its first message", async () => {
  const page = newPage();
  const data = threadData();
  const [first, second] = data.threads;
  first.display_name = "runner-wedged";
  first.summary = "The overnight runner hung and was restarted by hand.";
  // A name with no summary yet is not "name: nothing": the first message says more.
  second.display_name = "half-named";
  page.threadingSupported = true;
  page.threads = data.threads;
  await signIn(page);
  await showDiscord(page, data.messages);
  await twoColumns(page);
  assert.equal(page.el("side-list").hidden, false);
  assert.equal(page.el("side-pane").hidden, true);
  assert.equal(page.el("side-close").hidden, true, "an X with nothing to close");
  assert.equal(page.el("side-title").textContent, "Thread list");
  // `#261 two-column-connectors`: newest activity at the foot, as the channel's own lists are.
  assert.deepStrictEqual(cardIds(page), [first.id, second.id], "the cards are not newest activity at the foot");
  assert.equal(cardPart(page, first.id, "side-card-text"), "runner-wedged: The overnight runner hung and was restarted by hand.");
  assert.equal(cardPart(page, second.id, "side-card-text"), "second thread root");
  // `#277 thread-card-focus`: how long since it moved, and beside that the button into it with its count.
  assert.match(cardPart(page, first.id, "side-card-facts"), /^(now|\d+[mhdwy]|\d+d\d+h)$/,
    "the card does not say how long since the thread moved");
  assert.match(cardPart(page, first.id, "side-card-replies"), /^1 reply$/, "the card does not say how many replies it has");
  for (const id of cardIds(page)) {
    assert.ok(page.el("side-cards").children.find((li) => li.getAttribute("data-id") === id).hasAttribute("data-card"));
  }
  // At most the reader's collapsed lines (`#425 collapsed-lines`, never under one line of clamp on
  // a card), cut with an ellipsis, and fitted below that most (`#482 thread-card-fit`): the
  // stylesheet's, since this fixture lays nothing out.
  // Outside the two-column block since `#281 view-keys`: the thread list draws the same cards at any width.
  const text = cssBlockOutside(TWO_COLUMN_QUERY, ".side-card-text");
  assert.match(text, /-webkit-line-clamp:\s*max\(1, var\(--collapsed-lines\)\)/);
  assert.match(text, /overflow:\s*hidden/);
  assert.match(text, /max-height:\s*calc\(max\(1, var\(--collapsed-lines\)\) \* 1\.35em\)/, "a card is not held to at most its collapsed lines");
  assert.doesNotMatch(text, /(^|[^-])height:\s*calc/, "a short card still reserves its collapsed lines");
  // `#257 summary-truncation`, since `#277 thread-card-focus`: save the card highlighted, which shows its
  // whole text, so nothing the clamp cuts is out of reach. The card under the
  // mouse or the keys does not: see `#277`'s own tests below. Outside the two columns' block since `#281
  // view-keys`, the thread list's cards being the same cards.
  const whole = cssBlockOutside(TWO_COLUMN_QUERY, ".side-card-row[data-highlighted] > .side-card > .side-card-text");
  assert.match(whole, /-webkit-line-clamp:\s*none/, "the highlighted card is still clamped");
  assert.match(whole, /overflow:\s*visible/, "the highlighted card still hides what runs over");
  assert.match(whole, /(^|[^-])height:\s*auto/, "the highlighted card is still a fixed height");
  assert.doesNotMatch(whole, /min-height:/, "a highlighted short card still reserves its collapsed lines");
  // A long first message is given whole, its line breaks folded, for the clamp to cut.
  page.threads = [];
  const long = message({ id: "210", content: "a long first message\n\nthat runs over several lines " + "and on ".repeat(40),
    thread: { id: "spaces/A/threads/long", root_message_id: "210", is_root: true, reply_count: 1, reply_count_exact: true } });
  const answer = message({ id: "211", content: "answered", thread: { ...long.thread, is_root: false } });
  page.messages = [...data.messages, long, answer];
  await page.setDesk(true);
  page.expireTimers(DISCORD_POLL_MS);
  await page.settle();
  await page.settle();
  const said = cardPart(page, "spaces/A/threads/long", "side-card-text");
  // `#489 card-root-lines`: line by line, as the message is drawn; `#528 thread-card-markdown-and-cap`:
  // stopping between them, after the first paragraph, where the second would take the card past K.
  assert.equal(said, "a long first message…", `the long card says ${said}`);
});

test("`#229 desktop-two-column`: every way into a thread selects it on the right — its tag, its N replies, a card — and a plain tap does not", async () => {
  const page = await threadPage();
  await twoColumns(page);
  const [first, second] = threadData().threads;
  // A tap on a message folds and unfolds it, as in one column: it does not select its thread.
  await rowWithId(page, "201").click();
  await page.settle();
  assert.equal(page.el("side-list").hidden, false, "a plain tap on a root selected its thread");
  // N replies, which in one column gathers the replies under the root.
  await threadButton(rowWithId(page, "201")).click();
  await page.settle();
  assert.deepStrictEqual(sideIds(page), ["201", "202"], "N replies did not select the thread on the right");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "N replies changed the left column");
  // `#298 thread-card-header`: under its card, which says what its card in the list says.
  assert.equal(headerCard(page).text, "first thread root", "the heading is not the thread's card");
  assert.match(headerCard(page).age, /^(now|\d+[mhdwy]|\d+d\d+h)$/);
  assert.equal(headerCard(page).count, "1 reply · 1 unread");
  assert.equal(page.el("side-close").hidden, false);
  assert.equal(page.el("side-pane").hidden, false);
  assert.equal(page.el("side-list").hidden, true);
  // Its tag, on the other root.
  await threadBadge(rowWithId(page, "203")).click();
  await page.settle();
  assert.deepStrictEqual(sideIds(page), ["203", "204"]);
  assert.equal(headerCard(page).text, "second thread root");
  // The X: back to the cards, and a card selects.
  await page.el("side-close").click();
  await page.settle();
  assert.equal(page.el("side-list").hidden, false, "the X did not go back to the list");
  assert.equal(page.el("side-close").hidden, true);
  assert.equal(page.el("side-thread-card").hidden, true, "the thread's card stayed over the cards");
  assert.equal(page.el("side-title").textContent, "Thread list");
  assert.equal(page.el("side-title").hidden, false);
  const reads = page.timelineCalls.length;
  await openerOf(cardRow(page, first.id)).click();
  await page.settle();
  assert.deepStrictEqual(sideIds(page), ["201", "202"], "a card did not select its thread");
  // Drawn from what the page holds: All was read, and covers both threads.
  assert.deepStrictEqual(readViewsSince(page, reads).filter((view) => view === "thread"), [],
    "a thread the page holds was read again to be selected");
  assert.equal(page.el("thread-select").value, "columns", "selecting on the right moved the view picker");
  assert.ok(second);
});

test("`#229 desktop-two-column`: the right column's composer posts into its thread, and the left column's into Main", async () => {
  const page = await threadPage();
  // The server's answer to a post in a thread carries the thread, as a provider's does.
  const answer = page.replyResponse;
  page.replyResponse = async (path, options) => {
    const response = await answer(path, options);
    const body = JSON.parse(options.body);
    if (!body.thread_id) return response;
    const sent = JSON.parse(await response.text());
    const thread = { id: body.thread_id, root_message_id: "201", is_root: false, reply_count: null, reply_count_exact: false };
    return json(200, { ...sent, posted: { ...sent.posted, thread } });
  };
  await twoColumns(page);
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  assert.equal(page.el("side-compose-text").getAttribute("aria-label"), "Reply in this thread");
  assert.equal(page.el("channel-compose-text").getAttribute("aria-label"), "Message the main channel",
    "the left column's box stopped posting to Main");
  page.el("side-compose-text").value = "answered from the right";
  await page.el("side-send").click();
  await page.settle();
  assert.equal(page.repliesPosted.length, 1);
  assert.equal(page.repliesPosted[0].body.thread_id, "spaces/A/threads/one & two", "the right column's box did not post in its thread");
  assert.equal(page.el("side-compose-text").value, "", "the box was not emptied");
  assert.ok(sideIds(page).includes("9000000000000000001"), "the acknowledged reply was not drawn in the thread");
  assert.ok(!shownIds(page).includes("9000000000000000001"), "a thread's reply was drawn on Main");
  page.el("channel-compose-text").value = "and one to Main";
  await page.el("channel-send").click();
  await page.settle();
  assert.equal(page.repliesPosted.length, 2);
  assert.equal(page.repliesPosted[1].body.thread_id, undefined, "the left column's box posted into the thread");
});

test("`#229 desktop-two-column`: a live reply goes to the thread on the right and is read once, as the thread; a live message to Main, read once as Main", async () => {
  const { page, server, data } = await twoColumnServer();
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  const first = data.messages[1].thread;
  const reads = server.reads.length;
  const reply = server.post(message({ id: "205", content: "a second answer", thread: { ...first, is_root: false } }));
  await deliver(page, page.stream(), sseMessage(reply));
  await page.settle();
  assert.deepStrictEqual(sideIds(page), ["201", "202", "205"], "the live reply did not reach the thread on the right");
  assert.ok(!shownIds(page).includes("205"), "a thread's reply was drawn on Main");
  // One read, of the thread: in full, the first time — it was drawn from All's page, and has no
  // forward position of its own yet — and never one of Main as well.
  assert.deepStrictEqual(server.reads.slice(reads).map((read) => read.view), ["thread"],
    `the live reply was not read once, as the thread: ${JSON.stringify(server.reads.slice(reads))}`);
  assert.equal(threadButton(rowWithId(page, "201")).textContent, "2 replies", "Main's root did not count the reply");
  // The next is read as what changed in the thread since.
  const again = server.reads.length;
  const third = server.post(message({ id: "207", content: "a third answer", thread: { ...first, is_root: false } }));
  await deliver(page, page.stream(), sseMessage(third));
  await page.settle();
  assert.deepStrictEqual(sideIds(page), ["201", "202", "205", "207"]);
  assert.deepStrictEqual(server.reads.slice(again).map((read) => [read.view, read.after !== null]), [["thread", true]],
    `the second live reply was not read once, as the thread's delta: ${JSON.stringify(server.reads.slice(again))}`);

  const later = server.reads.length;
  const posted = server.post(message({ id: "206", content: "to the channel itself" }));
  await deliver(page, page.stream(), sseMessage(posted));
  await page.settle();
  assert.ok(shownIds(page).includes("206"), "a live message to Main did not reach the left column");
  assert.ok(!sideIds(page).includes("206"), "a Main message was drawn in the thread");
  assert.deepStrictEqual(server.reads.slice(later).map((read) => read.view), ["main"]);
});

test("`#229 desktop-two-column`: the poll reads Main and the thread on the right, one delta each and never the same view twice, and a poll that brings nothing builds nothing in either column", async () => {
  const { page, server } = await twoColumnServer();
  await threadBadge(rowWithId(page, "203")).click();
  await page.settle();
  // The first poll reads the thread in full — drawn from All's page, it has no forward position of
  // its own yet — and Main as what changed. From then on, both as what changed.
  await pollOnce(page);
  await page.settle();
  const left = [...page.el("discord-log").children];
  const right = [...page.el("side-log").children];
  const made = page.createdTags.length;
  const reads = server.reads.length;
  await pollOnce(page);
  await page.settle();
  const polled = server.reads.slice(reads).map((read) => [read.view, read.after !== null]);
  assert.deepStrictEqual(polled.map(([view]) => view).sort(), ["main", "thread"], `the poll read ${JSON.stringify(polled)}`);
  assert.ok(polled.every(([, delta]) => delta), "a poll of a view read in this page was not a delta");
  assert.ok(page.el("discord-log").children.every((li, at) => li === left[at]), "the left column's rows were replaced");
  assert.ok(page.el("side-log").children.every((li, at) => li === right[at]), "the right column's rows were replaced");
  assert.equal(builtSince(page, made, "li"), 0, "a poll that brought nothing built rows");
  assert.equal(builtSince(page, made, "button"), 0, "a poll that brought nothing built controls");
  assert.ok(page.el("discord-log").children.every((li, at) => li === left[at]), "the left column's rows were replaced");
  assert.ok(page.el("side-log").children.every((li, at) => li === right[at]), "the right column's rows were replaced");
  // With the cards up, the poll reads the thread list, at most once a minute, and not Main twice.
  await page.el("side-close").click();
  await page.settle();
  const before = server.reads.length;
  await pollOnce(page);
  await page.settle();
  const views = server.reads.slice(before).map((read) => read.view);
  assert.equal(views.filter((view) => view === "main").length, 1, `Main was read ${JSON.stringify(views)}`);
  assert.ok(views.filter((view) => view === "threads").length <= 1);
  assert.ok(!views.includes("thread"), "a thread no longer shown was polled");
});

test("`#229 desktop-two-column`: one read mode for both columns, and Done in one column is Done in the other", async () => {
  const page = await threadPage();
  await twoColumns(page);
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  // Done on the root, in the thread on the right: Main's row for it greys too.
  await doneButton(sideRow(page, "201")).click();
  await page.settle();
  assert.equal(sideRow(page, "201").getAttribute("data-archived"), "true");
  assert.equal(rowWithId(page, "201").getAttribute("data-archived"), "true", "Done on the right did not reach Main's row");
  assert.deepStrictEqual(page.dismissCalls.map((call) => call.messages), [["201"]]);
  // Hide read takes the read messages out of both columns.
  await turnTodoOn(page);
  assert.ok(!shownIds(page).includes("201") || unreadChip(page, "201"), "Hide read kept a read root with nothing unread");
  assert.ok(!sideIds(page).includes("201"), "Hide read did not reach the right column");
  assert.ok(sideIds(page).includes("202"), "Hide read hid the unread reply");
  // Done on the reply, on the right: the root's count of unread replies on Main goes.
  await doneButton(sideRow(page, "202")).click();
  await page.settle();
  assert.ok(!sideIds(page).includes("202"));
  assert.equal(unreadChip(page, "201"), null, "Main still counts an unread reply that is Done on the right");
});

test("`#229 desktop-two-column`: the search and Links reach both columns, and the count speaks for both", async () => {
  const page = await threadPage();
  await twoColumns(page);
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  await page.el("search-toggle").click();
  await page.el("search-field").setValue("answer");
  await page.settle();
  const visible = (list) => page.el(list).children.filter((li) => !li.hasClass("search-hidden")).map((li) => li.getAttribute("data-id"));
  assert.deepStrictEqual(visible("side-log"), ["202"], "the search did not reach the right column");
  // `#606 server-side-search`: in Main a thread stands for its replies, so the roots of the threads
  // whose held replies match stay.
  assert.deepStrictEqual(visible("discord-log"), ["201", "203"], "the left column was not searched");
  assert.equal(page.el("search-count").textContent, "3 of 5 loaded", "the count does not speak for both columns");
  assert.equal(page.el("search-empty").hidden, true, "the left column said its list was empty");
  assert.equal(page.el("side-search-empty").hidden, true);
  await page.el("search-field").setValue("root");
  await page.settle();
  assert.deepStrictEqual(visible("discord-log"), ["201", "203"]);
  assert.deepStrictEqual(visible("side-log"), ["201"]);
  assert.equal(page.el("search-count").textContent, "3 of 5 loaded");
  await page.el("search-toggle").click();
  await page.settle();
  assert.deepStrictEqual(visible("side-log"), ["201", "202"], "closing the search left the right column filtered");
});

test("`#229 desktop-two-column`: [ and ] move the keys between the columns, → selects a Main message's thread on the right, ← goes back, Esc closes the thread, and → opens a card", async () => {
  const page = await threadPage();
  await twoColumns(page);
  page.el("scroll-area").focus();
  await press(page, page.el("scroll-area"), "]");
  assert.equal(focused(page), page.el("side-scroll"), `] did not give the keys to the right column: ${focusName(page)}`);
  await press(page, focused(page), "j");
  const card = page.el("side-cards").children[0];
  assert.equal(focused(page), card, `j did not select the first card: ${focusName(page)}`);
  await press(page, card, "j");
  assert.equal(focused(page), page.el("side-cards").children[1]);
  // `#277 thread-card-focus`: → goes into a card's thread; Enter highlights the card (its own test, below).
  await press(page, focused(page), "ArrowRight");
  await page.settle();
  // The second card, the newer thread: the cards have their newest at the foot (`#261`).
  assert.deepStrictEqual(sideIds(page), ["203", "204"], "→ on a card did not select its thread");
  await press(page, page.el("side-scroll"), "[");
  assert.ok(focused(page) === page.el("scroll-area") || page.el("discord-log").children.includes(focused(page)),
    `[ did not give the keys to the left column: ${focusName(page)}`);
  // → on Main's other root: its thread on the right, the keys with it.
  const root = rowWithId(page, "201");
  root.focus();
  FakeElement.focusVisible = true;
  await press(page, root, "ArrowRight");
  await page.settle();
  assert.deepStrictEqual(sideIds(page), ["201", "202"], "→ did not select the thread on the right");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "→ left Main");
  assert.ok(focused(page) === page.el("side-scroll") || page.el("side-log").children.includes(focused(page)),
    `→ did not give the keys to the right column: ${focusName(page)}`);
  // ← from the right: back to Main, the thread kept.
  await press(page, focused(page), "ArrowLeft");
  assert.ok(focused(page) === page.el("scroll-area") || page.el("discord-log").children.includes(focused(page)),
    `← did not go back to the left column: ${focusName(page)}`);
  assert.deepStrictEqual(sideIds(page), ["201", "202"], "← closed the thread");
  // Esc in the right column is its X.
  await press(page, page.el("side-scroll"), "Escape");
  await page.settle();
  assert.equal(page.el("side-list").hidden, false, "Esc in the right column did not close its thread");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "Esc changed the left column");
  // The keys are listed, and [ and ] are not on a phone's list as if they did something there.
  assert.ok(page.el("keyboard-keys").text().includes("In two columns"), "the list of keys does not name [ and ]");
});

test("`#239 single-selection` x `#229 desktop-two-column`: one selection per column and one lit message on the page; the mouse into the other column takes the selection, and the keys, there", async () => {
  const page = await threadPage();
  await twoColumns(page);
  page.el("scroll-area").focus();
  await press(page, page.el("scroll-area"), "Home");
  assert.deepStrictEqual(litRows(page), ["discord-log:200"]);
  // With no thread selected the right column's cards: the mouse over one takes the selection there,
  // and ] after [ gives the focus back to that card, not to the column around it.
  const card = page.el("side-cards").children[1];
  await mouseTo(page, card, 900, 120);
  assert.deepStrictEqual(litRows(page), [`side-cards:${card.getAttribute("data-id")}`], "the card under the mouse is not the one lit");
  assert.equal(focused(page), card);
  await pressHere(page, "[");
  assert.deepStrictEqual(litRows(page), ["discord-log:200"]);
  await pressHere(page, "]");
  assert.deepStrictEqual(litRows(page), [`side-cards:${card.getAttribute("data-id")}`]);
  assert.equal(focused(page), card, `] gave the focus to ${focusName(page)}, not to the card lit`);
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  assert.deepStrictEqual(sideIds(page), ["201", "202"]);
  page.el("scroll-area").focus();
  await press(page, page.el("scroll-area"), "Home");
  assert.deepStrictEqual(litRows(page), ["discord-log:200"]);
  // The mouse into the right column, over the thread's reply: the selection, the focus and the keys go there.
  await mouseTo(page, sideRow(page, "202"), 900, 60);
  assert.deepStrictEqual(litRows(page), ["side-log:202"], "the left column's message stayed lit beside the right's");
  assert.equal(focused(page), sideRow(page, "202"), `the focus stayed in the left column: ${focusName(page)}`);
  await pressHere(page, "k");
  assert.deepStrictEqual(litRows(page), ["side-log:201"], "k did not move in the column the mouse chose");
  // Back over the left column: its message under the mouse takes it, and the right's goes out.
  await mouseTo(page, rowWithId(page, "203"), 300, 120);
  assert.deepStrictEqual(litRows(page), ["discord-log:203"]);
  assert.equal(focused(page), rowWithId(page, "203"));
  // ] and [ hand the keys across: each column's own selection is lit again, and only that one.
  await pressHere(page, "]");
  assert.deepStrictEqual(litRows(page), ["side-log:201"], "] did not light the right column's selection alone");
  assert.equal(focused(page), sideRow(page, "201"));
  await pressHere(page, "[");
  assert.deepStrictEqual(litRows(page), ["discord-log:203"], "[ did not light the left column's selection alone");
  // A redraw of both columns leaves the column in use as it was.
  await page.el("search-toggle").click();
  await page.el("search-toggle").click();
  await page.settle();
  assert.deepStrictEqual(litRows(page), ["discord-log:203"], "a redraw lit the other column's selection");
});

test("`#229 desktop-two-column`: leaving two columns returns to the view last chosen in one, and the thread selected comes back with two", async () => {
  const page = await threadPage("main");
  await pickThread(page, "thread:spaces/A/threads/another");
  assert.equal(page.el("thread-select").value, "thread:spaces/A/threads/another");
  // `#298 thread-card-header`: under its card, and named by the picker's option.
  const said = wideCard(page).text;
  const name = optionLabel(page, "thread:spaces/A/threads/another").split(" · ").at(-1);
  assert.equal(said, "second thread root");
  // Into two columns from a thread: Main on the left, that thread on the right.
  await twoColumns(page);
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"]);
  assert.deepStrictEqual(sideIds(page), ["203", "204"], "the thread open in one column did not go to the right");
  await page.el("column-toggle").click();
  await page.settle();
  assert.equal(page.el("thread-select").value, "thread:spaces/A/threads/another",
    "one column again is not the view last chosen there");
  await page.el("column-toggle").click();
  await page.settle();
  assert.deepStrictEqual(sideIds(page), ["203", "204"], "the thread selected was forgotten");
  // Named as one column named it, and under the card it was under there (`#298 thread-card-header`).
  assert.equal(page.el("side-scroll").getAttribute("aria-label"), `Thread: ${name}`);
  assert.equal(headerCard(page).text, said);
});

test("`#229 desktop-two-column`: a thread selected on the right lands on its first unread message, unfolded, at the head of its column", async () => {
  const page = newPage();
  const messages = runThread(page, { tail: RUN_TAIL });
  await signIn(page);
  await showDiscord(page, messages);
  await twoColumns(page);
  const leftTop = page.el("scroll-area").scrollTop;
  await threadBadge(rowWithId(page, runId(0))).click();
  await page.settle();
  const answer = sideRow(page, runId(500));
  assert.ok(answer, "the thread was not drawn on the right");
  assert.equal(answer.getAttribute("data-collapsed"), "false", "the answer the reader came for is folded on the right");
  const top = answer.getBoundingClientRect().top;
  const head = page.el("side-scroll").getBoundingClientRect().top;
  assert.ok(top >= head && top <= head + LINE_PX + 8, `the answer landed at ${top}px, not at the head of the right column`);
  // The left column was not moved by a landing on the right.
  assert.equal(page.el("scroll-area").scrollTop, leftTop, "the landing on the right scrolled the left column");
  assert.deepStrictEqual(shownIds(page), [runId(0)], "the left column is not Main");
});

test("`#229 desktop-two-column`: a pin made in either column is drawn in both", async () => {
  const page = await threadPage();
  await twoColumns(page);
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  await tapPin(page, sideRow(page, "201"));
  assert.equal(page.pinCalls.length, 1);
  assert.equal(pinChip(sideRow(page, "201")).hidden, false, "the pin is not shown where it was made");
  assert.equal(pinChip(rowWithId(page, "201")).hidden, false, "the pin is not shown on Main's row for the same message");
  await tapPin(page, rowWithId(page, "201"));
  assert.equal(pinChip(rowWithId(page, "201")).hidden, true);
  assert.equal(pinChip(sideRow(page, "201")).hidden, true, "an unpin on Main did not reach the right column");
});

test("`#229 desktop-two-column`: the search narrows the cards as it narrows the Threads list", async () => {
  const page = await threadPage();
  await twoColumns(page);
  const [first, second] = threadData().threads;
  await page.el("search-toggle").click();
  await page.el("search-field").setValue("second");
  await page.settle();
  const shown = () => page.el("side-cards").children.filter((li) => !li.hasClass("search-hidden")).map((li) => li.getAttribute("data-id"));
  assert.deepStrictEqual(shown(), [second.id], "the search did not reach the cards");
  await page.el("search-toggle").click();
  await page.settle();
  assert.deepStrictEqual(shown(), [first.id, second.id], "closing the search left a card hidden");
});

test("`#229 desktop-two-column`: another channel shows its own threads on the right, and coming back finds the thread selected there", async () => {
  const page = newPage();
  const data = threadData();
  const other = { id: "1110000000000000002", label: "second team", writable: true };
  page.channels = [{ ...CHANNEL }, other];
  page.threadingSupported = true;
  page.threads = data.threads;
  await signIn(page);
  await showDiscord(page, data.messages);
  await twoColumns(page);
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  assert.deepStrictEqual(sideIds(page), ["201", "202"]);
  page.el("discord-channel").value = other.id;
  await page.el("discord-channel").dispatch("change");
  await page.settle();
  assert.ok(inTwoColumns(page), "changing channel left two columns");
  assert.equal(page.el("side-list").hidden, false, "the new channel did not open on its cards");
  assert.deepStrictEqual(sideIds(page), [], "the last channel's thread is still drawn on the right");
  page.el("discord-channel").value = CHANNEL.id;
  await page.el("discord-channel").dispatch("change");
  await page.settle();
  assert.deepStrictEqual(sideIds(page), ["201", "202"], "coming back did not find the thread selected there");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"]);
});

test("`#229 desktop-two-column`: a reopen in two columns reads nothing before sign-in has answered", async () => {
  const first = await threadPage();
  await twoColumns(first);
  const data = threadData();
  let answer = null;
  const page = reloadWith(first.storage, data.messages, (p) => {
    p.threadingSupported = true;
    p.threads = data.threads;
    p.wideDesk = true;
    const config = p.clientConfig;
    p.clientConfig = (...args) => new Promise((resolve) => {
      answer = () => resolve(config(...args));
    });
  });
  await page.settle();
  await page.settle();
  assert.equal(page.tab(), "discord");
  assert.ok(inTwoColumns(page), "the reopen did not draw two columns from what the device saved");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "the left column is not Main, drawn from the device");
  assert.deepStrictEqual(page.timelineCalls, [], "a read went out on a token nobody has proved yet");
  assert.ok(answer, "sign-in never asked for the client configuration");
  answer();
  await page.settle();
  await page.settle();
  await page.settle();
  const views = page.timelineCalls.map(viewOf);
  assert.ok(views.includes("main"), `the reopen did not read Main once sign-in answered: ${JSON.stringify(views)}`);
  assert.equal(views.filter((view) => view === "main").length, 1, `Main was read more than once: ${JSON.stringify(views)}`);
});

test("`#229 desktop-two-column`: keys for the page — another channel, the Pinned filter — act for the page with the keys in the right column", async () => {
  const page = newPage();
  const data = threadData();
  const other = { id: "1110000000000000002", label: "second team", writable: true };
  page.channels = [{ ...CHANNEL }, other];
  page.threadingSupported = true;
  page.threads = data.threads;
  await signIn(page);
  await showDiscord(page, data.messages);
  await twoColumns(page);
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  // Shift+P from the right column: the Pinned filter, which is the left column's list.
  await press(page, page.el("side-scroll"), "P", { shiftKey: true });
  assert.equal(page.el("pinned-filter").getAttribute("aria-pressed"), "true", "Shift+P from the right column did nothing");
  assert.equal(page.el("pinned-log").hidden, false, "the Pinned filter is not the left column's list");
  assert.equal(page.el("side-pinned-log").hidden, true, "the Pinned filter took the right column's list");
  assert.equal(page.el("side-log").hidden, false, "the right column's thread went behind the Pinned filter");
  await press(page, page.el("side-scroll"), "Escape");
  await page.settle();
  // The other channel holds a message of its own.
  const serve = page.timeline;
  page.timeline = (path, options) => {
    if (!String(path).includes(other.id)) return serve(path, options);
    const held = page.messages;
    page.messages = [message({ id: "300", content: "in the second team" })];
    try {
      return serve(path, options);
    } finally {
      page.messages = held;
    }
  };
  // Alt+↓ from the right column: the next channel, the left column on its Main, the right on its cards.
  await press(page, page.el("side-scroll"), "ArrowDown", { altKey: true });
  await page.settle();
  await page.settle();
  assert.equal(page.el("discord-channel").value, other.id, "Alt+↓ from the right column did not change channel");
  assert.equal(page.el("thread-select").value, "columns", "the new channel is not in two columns");
  assert.deepStrictEqual(shownIds(page), ["300"], "the left column did not read the new channel");
  assert.equal(page.el("side-list").hidden, false, "the right column did not go to the new channel's cards");
  assert.equal(page.el("channel-compose-text").getAttribute("aria-label"), "Message the main channel");
});

// --- two columns, the review's follow-ups (#231 two-column-polish) ------------------------------
//
// What the review of `#229 desktop-two-column` found and did not hold it for: a card's reply count
// that lagged the reply its unread chip had counted, a right column whose failed refresh said
// nothing, two regions both called "Thread", a greyed picker that explained itself only to a
// hovering mouse, a thread read twice when it was selected under a poll, no guard on the cards'
// no-op refresh, and a dead declaration.

/**
 * A card's facts without its unread chip: how long since its thread moved, and its reply count — the
 * second said by its button into the thread since `#277 thread-card-focus`, beside the first.
 */
const cardFacts = (page, id) =>
  [cardPart(page, id, "side-card-facts"), cardPart(page, id, "side-card-replies")].filter(Boolean).join(" · ");

test("`#231 two-column-polish`: a live reply counts on its thread's card at once, with its unread chip, and not a minute later at the thread list's next read", async () => {
  const { page, server, data } = await twoColumnServer();
  const [first, second] = data.threads;
  assert.match(cardFacts(page, first.id), / · 1 reply$/);
  const reads = server.reads.length;
  // Into the thread whose count is exact, and into the one whose count the provider estimates.
  for (const [id, root] of [["205", data.messages[1]], ["206", data.messages[3]]]) {
    const reply = server.post(message({ id, content: `a live answer ${id}`, thread: { ...root.thread, is_root: false } }));
    await deliver(page, page.stream(), sseMessage(reply));
    await page.settle();
  }
  assert.deepStrictEqual(server.reads.slice(reads).filter((read) => read.view === "threads"), [],
    "the thread list was read again, so this does not show the card counting the reply by itself");
  assert.match(cardFacts(page, first.id), / · 2 replies$/,
    "the card still gives the count the thread list last reported, one short of the reply it counts as unread");
  assert.match(cardFacts(page, second.id), / · about 2 replies$/,
    "an estimated count did not take the live reply, or stopped saying it is an estimate");
  assert.ok(cardPart(page, first.id, "side-card-unread"), "the live reply was not counted as unread on its card");
  // Main's root says the same count, by the same rule.
  assert.equal(threadBadge(rowWithId(page, "201")).textContent, "Thread(2)");
});

/**
 * The channel in two columns, each thread reporting three replies of which the store holds one: the
 * usual case behind Main, where All's newest page holds a busy thread's latest replies only. The
 * first thread's count is exact, the second's the provider's estimate.
 */
const twoColumnsWithOlderReplies = (script = SCRIPT) => twoColumnServer((_page, data) => {
  for (const summary of data.threads) {
    summary.reply_count = 3;
    for (const held of data.messages.filter((m) => m.thread && m.thread.id === summary.id)) {
      held.thread = { ...held.thread, reply_count: 3 };
    }
  }
}, script);

test("`#231 two-column-polish`: a live reply counts on its thread's card and on Main's root when the store does not hold the thread's older replies, and a replayed one is not counted twice", async () => {
  /** Two live replies, one into each thread, and a replay of one more no read has brought. */
  const replies = async (script) => {
    const { page, server, data } = await twoColumnsWithOlderReplies(script);
    const [first, second] = data.threads;
    // Main is read as what changed from the poll's first round on, which brings no root that did not
    // change: the fixture's server does not count a reply on its root, as no read here may.
    await pollOnce(page);
    const before = { first: cardFacts(page, first.id), second: cardFacts(page, second.id) };
    const reads = server.reads.length;
    for (const [id, root] of [["205", data.messages[1]], ["206", data.messages[3]]]) {
      const reply = server.post(message({ id, content: `a live answer ${id}`, thread: { ...root.thread, is_root: false } }));
      await deliver(page, page.stream(), sseMessage(reply));
      await page.settle();
    }
    const live = {
      first: cardFacts(page, first.id), second: cardFacts(page, second.id),
      unread: cardPart(page, first.id, "side-card-unread"),
      badge: threadBadge(rowWithId(page, "201")).textContent,
      replies: threadButton(rowWithId(page, "201")) ? threadButton(rowWithId(page, "201")).textContent : null,
    };
    // A copy the stream replays, of a reply posted before it attached: a read since may have counted
    // it, so the page leaves the count to the next read that brings one.
    const replayed = server.post(message({ id: "207", content: "a replayed answer",
      thread: { ...data.messages[1].thread, is_root: false } }));
    await deliver(page, page.stream(), sseMessage(replayed, { replayed: true }));
    await page.settle();
    assert.deepStrictEqual(server.reads.slice(reads).filter((read) => read.view === "threads" || read.view === "thread"), [],
      "the thread list or a thread was read, which brings counts, so this does not show the page counting the replies");
    return { before, live, afterReplay: cardFacts(page, first.id), replayBadge: threadBadge(rowWithId(page, "201")).textContent };
  };
  const { before, live, afterReplay, replayBadge } = await replies(SCRIPT);
  assert.match(before.first, / · 3 replies$/);
  assert.match(before.second, / · about 3 replies$/);
  assert.match(live.first, / · 4 replies$/,
    "the card still gives the count the thread list last reported, a reply short of what its unread chip counts");
  assert.match(live.second, / · about 4 replies$/, "an estimated count did not take the live reply");
  assert.equal(live.unread, "2+ unread");
  assert.equal(live.badge, "Thread(4)", "Main's root did not count the live reply");
  assert.equal(live.replies, "4 replies", "the root's N replies did not count the live reply");
  assert.match(afterReplay, / · 4 replies$/, "a replayed reply was counted on top of a count that may hold it");
  assert.equal(replayBadge, "Thread(4)");

  // The controls. Without the count a live reply adds, the card and the root stay where the reads left
  // them; counting a replayed copy too counts it on the card.
  const uncounted = await replies(await brokenModule([{ file: "src/core/channels/live.ts",
    find: "  const fresh = !held && !replayed && read === null && thread !== null", replace: "  const fresh = false && thread !== null" }]));
  assert.match(uncounted.live.first, / · 3 replies$/, "the negative control counted the reply, so the check above proves nothing");
  assert.equal(uncounted.live.badge, "Thread(3)");
  const replayCounted = await replies(await brokenModule([{ file: "src/core/channels/live.ts",
    find: "  const fresh = !held && !replayed && read === null && thread !== null", replace: "  const fresh = !held && read === null && thread !== null" }]));
  assert.match(replayCounted.afterReplay, / · 5 replies$/,
    "the negative control did not count the replayed reply, so the check above proves nothing");
});

test("`#231 two-column-polish`: the right column is one named region, called what it shows — Threads, or Thread: and its name", async () => {
  // The scroller is the region, as #scroll-area is the left column's, and the column round it is not
  // a second one: two nested regions both called "Thread" were said twice on every way in.
  const column = HTML_CODE.slice(HTML_CODE.indexOf('id="side-column"'), HTML_CODE.indexOf('id="pull-refresh"'));
  assert.equal((column.match(/role="region"/g) || []).length, 1, "the right column holds more than one region");
  assert.doesNotMatch(/<div\s+id="side-column"[^>]*>/.exec(HTML_CODE)[0], /role=|aria-label=/,
    "the right column is a region round its own region");
  assert.match(/<div\s+id="side-scroll"[^>]*>/.exec(HTML_CODE)[0], /role="region"/);
  const page = await threadPage();
  await twoColumns(page);
  const region = page.el("side-scroll");
  assert.equal(region.getAttribute("aria-label"), "Thread list");
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  assert.equal(region.getAttribute("aria-label"), "Thread: First discussion", "the region does not name the thread it shows");
  // `#298 thread-card-header`. The heading is the thread's card now, in the card's words; the region,
  // the one thing a screen reader announces, keeps the thread's name, and the card is no second region.
  assert.equal(page.el("side-title").hidden, true, "the words \"Thread list\" stayed over a thread");
  assert.equal(headerCard(page).text, "first thread root");
  assert.equal(page.el("side-thread-card").getAttribute("role"), null, "the thread's card is a second region");
  await threadBadge(rowWithId(page, "203")).click();
  await page.settle();
  assert.equal(region.getAttribute("aria-label"), "Thread: Second discussion");
  await page.el("side-close").click();
  await page.settle();
  assert.equal(region.getAttribute("aria-label"), "Thread list", "the X left the region naming a thread it no longer shows");
});

/**
 * The channel in two columns, its second thread reporting an exact count of two replies while the
 * store holds one: All's page does not hold that thread whole, so selecting it reads it behind the
 * rows the store holds (`storeLacksReplies`).
 */
const twoColumnsWithAThreadToRead = (script = SCRIPT) => twoColumnServer((_page, data) => {
  const second = data.threads[1];
  Object.assign(second, { reply_count: 2, reply_count_exact: true });
  for (const held of data.messages.filter((m) => m.thread && m.thread.id === second.id)) {
    held.thread = { ...held.thread, reply_count: 2, reply_count_exact: true };
  }
}, script);

/** Hold every read of `view` on `server` until the returned function is called. */
function holdView(server, view) {
  let open = () => {};
  const opened = new Promise((resolve) => {
    open = resolve;
  });
  server.hold = { respond: (path) => (viewOf(path) === view ? opened.then(() => server.answer(path)) : server.answer(path)) };
  return () => {
    server.hold = null;
    open();
  };
}

test("`#231 two-column-polish`: a thread selected while the poll's read of Main is on the wire is read once — the poll's own read of it collapses into the selection's", async () => {
  const { page, server } = await twoColumnsWithAThreadToRead();
  const reads = server.reads.length;
  const release = holdView(server, "main");
  page.expireTimers(DISCORD_POLL_MS);
  await page.settle();
  // Selected under the poll: drawn from the rows the store holds, its read queued behind Main's.
  await threadBadge(rowWithId(page, "203")).click();
  await page.settle();
  assert.deepStrictEqual(sideIds(page), ["203", "204"], "the thread was not drawn on the right from what the page holds");
  release();
  for (let i = 0; i < 4; i += 1) await page.settle();
  const views = server.reads.slice(reads).map((read) => read.view);
  assert.equal(views.filter((view) => view === "thread").length, 1,
    `the thread was read more than once between its selection and the end of the poll: ${JSON.stringify(views)}`);
  assert.equal(views.filter((view) => view === "main").length, 1, `Main was not read once: ${JSON.stringify(views)}`);
  // ...and the next poll reads it, as what changed since: collapsing a read is not losing the thread.
  const next = server.reads.length;
  await pollOnce(page);
  assert.deepStrictEqual(server.reads.slice(next).filter((read) => read.view === "thread").map((read) => read.after !== null), [true],
    "the next poll did not read the thread on the right as a delta");
});

test("`#231 two-column-polish`: on a channel whose changes carry additions alone, an edit's read of the thread on the right is not collapsed into a read of what changed", async () => {
  /**
   * An edit to a reply in the thread on the right, with Main's read for it on the wire, and a live
   * reply into that thread meanwhile, whose read of what changed goes out once Main's lands: the
   * edit's own read of the thread comes after it, and that one only adds.
   */
  const edited = async (script) => {
    const { page, server, data } = await twoColumnServer(null, script);
    await threadBadge(rowWithId(page, "201")).click();
    await page.settle();
    // The server sees creations only, and the page learns so from the poll's first round.
    server.complete = false;
    await pollOnce(page);
    const reads = server.reads.length;
    const release = holdView(server, "main");
    server.edit("202", { content: "first thread answer, corrected" });
    await deliver(page, page.stream(), sseUpdate("edit-202", page.messages.find((m) => m.id === "202")));
    await page.settle();
    const reply = server.post(message({ id: "205", content: "a live answer meanwhile",
      thread: { ...data.messages[1].thread, is_root: false } }));
    await deliver(page, page.stream(), sseMessage(reply));
    await page.settle();
    release();
    for (let i = 0; i < 4; i += 1) await page.settle();
    const row = sideRow(page, "202");
    return {
      reads: server.reads.slice(reads).map((read) => `${read.view}${read.after === null ? "" : " delta"}`),
      text: row ? row.text() : "",
      ids: sideIds(page),
    };
  };
  const { reads, text, ids } = await edited(SCRIPT);
  assert.deepStrictEqual(reads, ["main", "thread delta", "thread"],
    "the edit's read of the thread on the right was not a read in full after the live reply's delta");
  assert.match(text, /first thread answer, corrected/, "the edit to the thread on the right was never read");
  assert.ok(ids.includes("205"), "the live reply left the thread on the right");
  // The control: any read of the thread since counts, a read of what changed too. The edit's read is
  // then collapsed into one that could not carry it, and the edit is never shown.
  const control = await edited(brokenScript("    (sent.full || forwardCursor(reason) !== null);", "    true;"));
  assert.deepStrictEqual(control.reads, ["main", "thread delta"],
    "the negative control still read the thread in full, so the check above proves nothing");
  assert.doesNotMatch(control.text, /corrected/);
});

const THREAD_DIRECTORY_REFRESH_MS = sourceConstant("THREAD_DIRECTORY_REFRESH_MS");

/** Answer every read of `view` on `server` with a gateway error, and every other read as it would be. */
function failView(server, view) {
  const failed = errorResponse(502, "chat_error", "the chat service refused the read");
  server.hold = { respond: (path) => (viewOf(path) === view ? failed() : server.answer(path)) };
}

test("`#231 two-column-polish`: a failed refresh of the thread on the right says so on its own line, in the pill's words, and the next read that lands takes it away", async () => {
  // The line is part of the right column, under its heading, and hidden until it has something to say.
  assertMarkupContains("side-column", "side-freshness");
  const tag = /<p\s+id="side-freshness"[^>]*>/.exec(HTML_CODE)[0];
  assert.match(tag, /role="status"/);
  assert.match(tag, /(?:^|\s)hidden(?=[\s>])/, "the line ships on screen, before anything has been read");
  // The right column's own parts, wherever it is drawn: beside Main, or as the thread list (`#281 view-keys`).
  assert.match(cssBlockOutside(TWO_COLUMN_QUERY, "#side-freshness"), /grid-row:\s*2/);
  assert.match(cssBlockOutside(TWO_COLUMN_QUERY, "#side-scroll"), /grid-row:\s*3/,
    "the list is not placed below the line, so the line hidden would leave the list in the wrong row");
  const { page, server } = await twoColumnServer();
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  const line = page.el("side-freshness");
  assert.equal(line.hidden, true, "a thread drawn from a read that succeeded says it is stale");
  failView(server, "thread");
  await pollOnce(page);
  assert.equal(line.hidden, false, "a failed refresh of the right column said nothing");
  assert.equal(line.getAttribute("data-state"), "failed");
  assert.match(line.textContent, /^Refresh failed · showing messages from \d{2}:\d{2}$/);
  assert.deepStrictEqual(sideIds(page), ["201", "202"], "the failure took the thread's rows away");
  // Said over the rows it is about, so not again in the error panel; and the left column, which read
  // its own view, is current, and says so.
  assert.equal(page.el("error-wrap").hidden, true, "a failed refresh of the right column raised the error panel");
  assertCurrent(page, "the right column's failure was said for the left column");
  server.hold = null;
  await pollOnce(page);
  assert.equal(line.hidden, true, "the next read that landed left the right column saying its refresh failed");
  assert.equal(line.getAttribute("data-state"), "fresh");
});

test("`#231 two-column-polish`: a failed read of the thread list behind the cards says so on the right column's line, until one lands", async () => {
  const { page, server } = await twoColumnServer();
  const line = page.el("side-freshness");
  assert.equal(page.el("side-list").hidden, false, "the cards are not up");
  assert.equal(line.hidden, true);
  failView(server, "threads");
  page.setClock(page.clock() + THREAD_DIRECTORY_REFRESH_MS + 1000);
  const calls = page.timelineCalls.length;
  await pollOnce(page);
  assert.ok(page.timelineCalls.slice(calls).map(viewOf).includes("threads"), "the poll did not read the thread list");
  assert.equal(line.hidden, false, "a failed read of the thread list behind the cards said nothing");
  assert.match(line.textContent, /^Refresh failed · showing threads from \d{2}:\d{2}$/);
  assert.ok(cardIds(page).length > 0, "the failure took the cards away");
  assert.equal(page.el("error-wrap").hidden, true);
  // A failed read is asked again at the next poll, not a minute later, and its landing ends the line.
  server.hold = null;
  await pollOnce(page);
  assert.equal(line.hidden, true, "the thread list's next read landed, and the line still says it failed");
  // Selecting a thread speaks for the thread; closing it again, for the cards.
  failView(server, "threads");
  page.setClock(page.clock() + THREAD_DIRECTORY_REFRESH_MS + 1000);
  await pollOnce(page);
  assert.equal(line.hidden, false);
  await openerOf(cardRow(page, cardIds(page)[0])).click();
  await page.settle();
  assert.equal(line.hidden, true, "the thread on the right says the thread list's failure");
  await page.el("side-close").click();
  await page.settle();
  assert.equal(line.hidden, false, "back on the cards, the thread list's failure is no longer said");
});

test("`#231 two-column-polish` x `#226 page-energy-profile`: with the cards up, a refresh that brings nothing draws nothing, in the cards as in Main", async () => {
  /** A poll with the cards up, late enough that the thread list is read again, which brings nothing. */
  const quietPoll = async (script) => {
    const { page, server } = await twoColumnServer(null, script);
    assert.equal(page.el("side-list").hidden, false, "the cards are not up");
    // The poll's first round reads Main as a delta from then on; this is the round under test.
    await pollOnce(page);
    page.setClock(page.clock() + THREAD_DIRECTORY_REFRESH_MS + 1000);
    const cards = [...page.el("side-cards").children];
    const rows = [...page.el("discord-log").children];
    const made = page.createdTags.length;
    const reads = server.reads.length;
    await pollOnce(page);
    const views = server.reads.slice(reads).map((read) => read.view).sort();
    assert.deepStrictEqual(views, ["main", "threads"], `the poll did not read Main and the thread list: ${JSON.stringify(views)}`);
    return { page, cards, rows, made };
  };
  const { page, cards, rows, made } = await quietPoll(SCRIPT);
  assert.ok(cards.length > 0);
  assert.equal(builtSince(page, made, "li"), 0, "a refresh that brought nothing built rows or cards");
  assert.equal(builtSince(page, made, "button"), 0, "a refresh that brought nothing built controls");
  assert.ok(page.el("side-cards").children.length === cards.length &&
    page.el("side-cards").children.every((li, at) => li === cards[at]), "the cards were replaced");
  assert.ok(page.el("discord-log").children.every((li, at) => li === rows[at]), "Main's rows were replaced");
  // The control: the cards redrawn on every pass, as they were before they kept what they were drawn
  // from. The same refresh has to build them again, or the check above could not see it.
  // `#532 shift-o-latency`: and nothing kept from the pass before, which a pass that changed nothing keeps.
  const control = await quietPoll(brokenScriptEach(["  if (signature !== sideCardsDrawn) {", "  if (true) {"],
    ['const keep = sideCardsDrawn !== "" && sideCardsKept.whole === whole;', "const keep = false;"]));
  assert.ok(builtSince(control.page, control.made, "li") >= control.cards.length,
    "the negative control did not redraw the cards, so the check above proves nothing");
});

test("`#281 view-keys`: the view picker chooses in two columns, where `#231` greyed it — a picker like any other, and a thread chosen there opens on the right", async () => {
  // Still described by a note outside the strip's pack, hidden: what it says while the picker is inert.
  const tag = /<select\s+id="thread-select"[^>]*>/.exec(HTML_CODE)[0];
  assert.match(tag, /aria-describedby="thread-select-note"/, "the picker has no description to say why it is inert");
  assert.match(/<p\s+id="thread-select-note"[^>]*>/.exec(HTML_CODE)[0], /(?:^|\s)hidden(?=[\s>])/);
  assert.ok(!markupHolds("bar-pack", "thread-select-note"), "the note is a member of the strip's pack");
  assert.doesNotMatch(SCRIPT_CODE, /pickerGreyed|refuseGreyedPicker/, "the greyed picker's refusals are still there");
  const page = await threadPage();
  const picker = page.el("thread-select");
  const note = page.el("thread-select-note");
  await twoColumns(page);
  assert.equal(picker.disabled, false);
  assert.equal(picker.hasAttribute("aria-disabled"), false, "the picker still says it is unavailable in two columns");
  assert.equal(note.textContent, "", "a picker that chooses carries a reason it is inert");
  assert.match(picker.title, /^Which view of the channel to read: Main, All, the thread list, two columns, or one thread\.$/);
  // Its own keys are its own, as on any picker: the browser opens it and chooses with it.
  const arrow = await press(page, picker, "ArrowDown");
  assert.equal(arrow.defaulted, true, "↓ on the picker was kept from the browser");
  // Escape is the way off it, back to the list.
  await press(page, picker, "Escape");
  assert.ok(focused(page) === page.el("scroll-area") || page.el("discord-log").children.includes(focused(page)),
    `Escape on the picker did not go back to the list: ${focusName(page)}`);
  // A thread chosen in two columns is selected on the right; the picker goes on saying two columns.
  const [first] = threadData().threads;
  await pickThread(page, `thread:${first.id}`);
  assert.ok(inTwoColumns(page), "choosing a thread left two columns");
  assert.deepStrictEqual(sideIds(page), ["201", "202"], "the thread chosen was not selected on the right");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "the left column left Main");
  assert.equal(picker.value, "columns");
  // All chosen there: one column, All.
  await pickThread(page, "flat");
  assert.ok(!inTwoColumns(page));
  assert.equal(picker.value, "flat");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "202", "203", "204"]);
  // A channel without threads: disabled, as it always was, and its description says why.
  const flat = newPage();
  await signIn(flat);
  await showDiscord(flat, [message({ content: "no threads on this provider" })]);
  assert.equal(flat.el("thread-select").disabled, true);
  assert.match(flat.el("thread-select-note").textContent, /does not offer threads here/);
  assert.deepStrictEqual(threadOptions(flat), [["main", "Main"]], "a channel without threads offered views it cannot show");
});

test("`#281 view-keys` x `#226 page-energy-profile`: a touch on the view picker in two columns reads the thread list at most once a minute", async () => {
  // A touch on the picker reads the thread list, for the picker to offer, as it does in one column.
  // Not every touch: the read is reused for a minute, so tabbing past it or opening it again is free.
  const { page, server } = await twoColumnServer();
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  page.setClock(page.clock() + THREAD_DIRECTORY_REFRESH_MS + 1000);
  const picker = page.el("thread-select");
  const touch = async () => {
    const reads = server.reads.length;
    await picker.dispatch("focus");
    await picker.dispatch("pointerdown", { pointerType: "mouse" });
    await page.settle();
    return server.reads.slice(reads).map((read) => read.view);
  };
  assert.deepStrictEqual(await touch(), ["threads"], "a touch on the picker did not read the thread list for it");
  assert.deepStrictEqual(await touch(), [], "a second touch within the minute read the thread list again");
});

/** The top-level bindings of `code` that nothing else in it names: declared, and never used. */
function unusedBindings(code) {
  return [...code.matchAll(/^(?:let|const|var) ([A-Za-z_$][\w$]*)/gm)].map((found) => found[1]).filter((name) => {
    // A use is the name standing alone, or spread (`...name`); `.name` is someone else's property.
    const uses = code.match(new RegExp(`(?:^|[^\\w$.]|\\.\\.\\.)${name.replace(/\$/g, "\\$")}(?![\\w$])`, "gm")) || [];
    return uses.length < 2;
  });
}

test("`#231 two-column-polish`: no top-level binding of web/voice.js is declared and never used", () => {
  assert.deepStrictEqual(unusedBindings(SCRIPT_CODE), [], "web/voice.js declares something nothing uses");
  // The control: one more such declaration, `#229`'s `let sideMark = null` as it was, is found.
  assert.deepStrictEqual(unusedBindings(`${SCRIPT_CODE}\nlet sideMark = null;\n`), ["sideMark"],
    "the check does not find a declaration nothing uses, so it proves nothing");
});

test("`#261 two-column-connectors`: the cards have the newest thread at the foot and open scrolled to it, and a thread that moves goes to the foot, followed only by a reader there", async () => {
  const run = async (script) => {
    const { page, server, data } = await manyThreadColumns(16, null, script);
    const area = page.el("side-scroll");
    const ids = data.threads.map((thread) => thread.id);
    assert.deepStrictEqual(cardIds(page), ids, "the cards are not oldest activity first, the newest at the foot");
    assert.ok(area.scrollHeight > area.clientHeight + 200, "the cards do not run past the column's foot, so this proves nothing");
    assert.ok(sideAtFoot(page), `the cards opened at ${area.scrollTop}px, not at the foot`);
    // A reply into the oldest thread: its card goes to the foot, and the reader there follows it.
    await liveReply(page, server, data.messages[0], "400");
    assert.deepStrictEqual(cardIds(page), [...ids.slice(1), ids[0]], "the thread that moved did not go to the foot");
    assert.ok(sideAtFoot(page), "the reader at the foot did not follow the thread that moved there");
    // Up the list: the card the reader is on stays where it is while a thread above it moves to the foot.
    area.scrollTop = 160;
    const anchor = firstCardInView(page);
    const at = anchor.getBoundingClientRect().top;
    const above = cardIds(page).indexOf(anchor.getAttribute("data-id")) - 1;
    assert.ok(above >= 0, "no card above the one in view, so this proves nothing");
    const moving = data.threads.findIndex((thread) => thread.id === cardIds(page)[above]);
    await liveReply(page, server, data.messages[2 * moving], "401");
    assert.equal(cardIds(page)[cardIds(page).length - 1], data.threads[moving].id, "the thread that moved did not go to the foot");
    const card = page.el("side-cards").children.find((li) => li.getAttribute("data-id") === anchor.getAttribute("data-id"));
    return { page, moved: card.getBoundingClientRect().top - at, foot: sideAtFoot(page) };
  };
  const { moved, foot } = await run(SCRIPT);
  assert.equal(moved, 0, `the card the reader was on moved ${moved}px when a thread above it went to the foot`);
  assert.equal(foot, false, "a reader up the list was taken to the foot");
  // The control: with the place not kept, the card moves up by the card that left above it.
  const loose = await run(brokenScript("  change();\n  placeCardAnchor(anchor, follow);\n", "  change();\n"));
  assert.ok(loose.moved < 0, "the negative control kept the card in place, so the check above proves nothing");
});

// --- the card the reader was held by, moved (#612 view-stability) ----------------------------------
//
// The owner, 2026-10-27: "I just experienced a screen jump. I was in threaded view reading a message and it
// jumped off the screen. It was probably bc of some new message causing a thread to jump to the bottom. But
// we should ALWAYS prioritize view stability. Whatever I'm looking at should not jump away." The cards held
// their reader by one card, the first on screen, half off the top; a reply in that card's thread took it to
// the foot, and the reader after it. Each check below reads every row on screen before the change and asks
// that every one still drawn where it was, but the card that moved, is within a pixel of where it stood.

/** Every row of `list` that shows in `area`, by id, with the top of its box. */
const rowsOnScreen = (page, list, area, idOf = (li) => li.getAttribute("data-id")) => {
  const frame = page.el(area);
  const top = frame.getBoundingClientRect().top;
  return new Map(page.el(list).children.filter((li) => {
    const box = li.getBoundingClientRect();
    return !li.hidden && !li.hasClass("search-hidden") && box.bottom > top && box.top < top + frame.clientHeight;
  }).map((li) => [idOf(li), li.getBoundingClientRect().top]));
};

/** How far each row of `before` (`rowsOnScreen`) but `moved` has gone since, by id: the rows that moved. */
const rowsMoved = (page, list, before, moved, idOf = (li) => li.getAttribute("data-id")) => {
  const now = new Map(page.el(list).children.map((li) => [idOf(li), li.getBoundingClientRect().top]));
  return [...before].filter(([id]) => id !== moved)
    .map(([id, top]) => [id, now.has(id) ? now.get(id) - top : NaN])
    .filter(([, by]) => !(Math.abs(by) <= 1));
};

test("`#612 view-stability`: a reply moving the card the reader is held by to the foot — the first on screen, half off the top; one in the middle of the screen; the highlighted one — moves nothing else on screen; a reply below the screen moves nothing; and the card half off the top, grown by an edit, grows over the top", async () => {
  // `upto`: how many of the steps below to take, for a negative control that goes wrong at one of them.
  const run = async (script, upto = Infinity) => {
    const { page, server, data } = await manyThreadColumns(16, null, script);
    const area = page.el("side-scroll");
    const threadOf = (id) => data.threads.findIndex((thread) => thread.id === id);
    const replyTo = (id, n) => liveReply(page, server, data.messages[2 * threadOf(id)], String(n));
    const found = [];
    // Up the list, the first card on screen half scrolled off the top.
    area.scrollTop = 160;
    const first = firstCardInView(page);
    const box = first.getBoundingClientRect();
    area.scrollTop += Math.round((area.getBoundingClientRect().top - box.top) + box.height / 2);
    await area.dispatch("scroll");
    const half = firstCardInView(page);
    assert.ok(half.getBoundingClientRect().top < area.getBoundingClientRect().top, "the first card is not half off the top, so this proves nothing");
    let before = rowsOnScreen(page, "side-cards", "side-scroll");
    assert.ok(before.size >= 3, "too few cards on screen to prove anything");
    await replyTo(half.getAttribute("data-id"), 400);
    assert.equal(cardIds(page)[cardIds(page).length - 1], half.getAttribute("data-id"), "the card answered did not go to the foot");
    found.push(["the first card on screen moved to the foot", rowsMoved(page, "side-cards", before, half.getAttribute("data-id"))]);
    if (found.length >= upto) return found;
    assert.equal(sideAtFoot(page), false, "a reader up the list was taken to the foot");
    // A card in the middle of the screen: the cards above it stay where they are, and the rest close up under them.
    before = rowsOnScreen(page, "side-cards", "side-scroll");
    const middle = [...before.keys()][2];
    await replyTo(middle, 401);
    const above = new Map([...before].slice(0, 2));
    found.push(["a card in the middle of the screen moved to the foot", rowsMoved(page, "side-cards", above, middle)]);
    if (found.length >= upto) return found;
    // The highlighted card, which the reader is held by while it shows (`#277 thread-card-focus`), moves alone:
    // the first card fully shown that stayed, the one under it, holds the rest.
    const lit = [...rowsOnScreen(page, "side-cards", "side-scroll").keys()][1];
    await page.el("side-cards").children.find((li) => li.getAttribute("data-id") === lit).children[0].click();
    await page.settle();
    assert.deepStrictEqual(litCards(page), [lit], "the click did not highlight the card");
    before = rowsOnScreen(page, "side-cards", "side-scroll");
    await replyTo(lit, 402);
    found.push(["the highlighted card moved to the foot", rowsMoved(page, "side-cards", new Map([...before].slice(2)), lit)]);
    if (found.length >= upto) return found;
    assert.equal(sideAtFoot(page), false, "the reader went with the highlighted card to the foot");
    // A reply into a thread whose card is below the screen, and an edit of a root above it: nothing on screen moves.
    before = rowsOnScreen(page, "side-cards", "side-scroll");
    const shown = new Set(before.keys());
    const below = cardIds(page).slice(cardIds(page).indexOf([...before.keys()].pop()) + 1).find((id) => !shown.has(id));
    await replyTo(below, 403);
    found.push(["a reply below the screen", rowsMoved(page, "side-cards", before, null)]);
    // The card half off the top, its root edited longer: it grows over the top of the column, not over the
    // cards under it.
    before = rowsOnScreen(page, "side-cards", "side-scroll");
    const over = [...before.keys()][0];
    assert.ok(before.get(over) < area.getBoundingClientRect().top, "the first card is not half off the top, so this proves nothing");
    const root = data.messages[2 * threadOf(over)];
    server.edit(String(root.id), { content: `${root.content}, edited and longer. ${"More words. ".repeat(30)}` });
    await deliver(page, page.stream(), sseUpdate("edit-1", page.messages.find((m) => String(m.id) === String(root.id))));
    await page.settle();
    const grew = page.el("side-cards").children.find((li) => li.getAttribute("data-id") === over).getBoundingClientRect();
    assert.ok(grew.bottom - grew.top > 150, "the edit did not grow the card, so this proves nothing");
    found.push(["the card half off the top grown by an edit", rowsMoved(page, "side-cards", before, over)]);
    return found;
  };
  for (const [what, moved] of await run(SCRIPT)) {
    assert.deepStrictEqual(moved, [], `${what}, and the rows on screen moved by (id, px): ${JSON.stringify(moved)}`);
  }
  // The controls. Held by the first card showing alone, as before, the cards on screen go with it to the foot;
  // held by the first card shown whole, but followed wherever it goes, they go with the highlighted card.
  const FOLLOWED = ["  const stayed = follow ? null : stayedCardRow(anchor);\n", "  const stayed = null;\n"];
  const first = await run(brokenScriptEach(FOLLOWED,
    ["    rows.find((one) => seen(one) && whole(one)) || rows.find(seen);\n", "    rows.find(seen);\n"]), 1);
  assert.ok(first[0][1].length > 0, "the first negative control left the cards where they were, so the check above proves nothing");
  const lit = await run(brokenScriptEach(FOLLOWED), 3);
  assert.ok(lit[2][1].length > 0, "the second negative control left the cards where they were, so the check above proves nothing");
});

test("`#612 view-stability`: a thread unpacked and being read, its replies filling the screen, is followed to the foot when a reply comes, every line on screen where it was", async () => {
  const run = async (script) => {
    // Ten threads, the fourth with thirteen answers, all older than the fifth's root: its block is mid-list.
    const messages = [];
    const threads = [];
    for (let i = 0; i < 10; i += 1) {
      const id = `spaces/A/threads/t${i}`;
      const rootId = String(300 + 2 * i);
      const count = i === 3 ? 13 : 1;
      const record = { id, root_message_id: rootId, is_root: true, reply_count: count, reply_count_exact: true };
      const root = message({ id: rootId, content: `thread ${i} root`, thread: record });
      messages.push(root);
      let answer = null;
      for (let n = 0; n < count; n += 1) {
        answer = message({ id: n === 0 ? String(301 + 2 * i) : String(500 + n), content: `thread ${i} answer ${n}`,
          thread: { ...record, is_root: false } });
        messages.push(answer);
      }
      threads.push({ id, root, title: `Thread ${i}`, reply_count: count, reply_count_exact: true, updated_at: answer.timestamp });
    }
    const page = newPage(new Map(), script);
    page.threadingSupported = true;
    page.threads = threads;
    page.messages = messages;
    const server = forwardTimeline(page);
    await signIn(page);
    await showDiscord(page, messages);
    await press(page, page.el("scroll-area"), "3", { ctrlKey: true });
    assert.equal(page.el("screen-main").getAttribute("data-columns"), "list", "the premise is the Threads view");
    // Unpack all (`#440`: Pack all, Unpack unread, Unpack all).
    await page.el("side-unpack").click();
    await page.el("side-unpack").click();
    await page.settle();
    const block = page.el("side-cards").children.filter((li) => li.getAttribute("data-unpack-thread") === "spaces/A/threads/t3");
    assert.ok(block.length >= 13, `the fourth thread did not unpack its answers (${block.length} rows)`);
    const area = page.el("side-scroll");
    area.scrollTop += Math.round(block[3].getBoundingClientRect().top - area.getBoundingClientRect().top);
    await area.dispatch("scroll");
    const before = rowsOnScreen(page, "side-cards", "side-scroll");
    const own = new Set(block.map((li) => li.getAttribute("data-id")));
    assert.ok(before.size >= 3 && [...before.keys()].every((id) => own.has(id)),
      `the fourth thread's replies do not fill the screen, so this proves nothing: ${[...before.keys()]}`);
    await liveReply(page, server, threads[3].root, "600");
    assert.equal(cardIds(page).filter((id) => id && id.startsWith("spaces/"))
      .pop(), "spaces/A/threads/t3", "the thread answered did not go to the foot");
    return rowsMoved(page, "side-cards", before, null);
  };
  const moved = await run(SCRIPT);
  assert.deepStrictEqual(moved, [], `the lines being read moved by (id, px): ${JSON.stringify(moved)}`);
  // The control: held by the cards that stayed, the thread being read goes to the foot from under the reader.
  const loose = await run(brokenScript("  if (heaviest !== null && px > still) {\n", "  if (false) {\n"));
  assert.ok(loose.length > 0, "the negative control left the lines where they were, so the check above proves nothing");
});

test("`#612 view-stability`: in the Threads list in one column, a reply moving the first card on screen to the foot moves no other card on screen; a reply below the screen moves none", async () => {
  const run = async (script) => {
    const data = manyThreads(16);
    const page = newPage(new Map(), script);
    page.threadingSupported = true;
    page.threads = data.threads;
    page.messages = data.messages;
    const server = forwardTimeline(page);
    await signIn(page);
    await showDiscord(page, data.messages);
    await page.el("channel-view-threads").click();
    await page.settle();
    const list = page.el("thread-list");
    assert.equal(list.hidden, false, "the Threads list is not on screen");
    const idOf = (li) => li.getAttribute("data-context-id");
    const ids = () => list.children.map(idOf);
    const area = page.el("scroll-area");
    assert.ok(area.scrollHeight > area.clientHeight + 200, "the cards do not run past the foot, so this proves nothing");
    area.scrollTop = Math.round((area.scrollHeight - area.clientHeight) * 0.4);
    await area.dispatch("scroll");
    const top = area.getBoundingClientRect().top;
    const first = list.children.find((li) => li.getBoundingClientRect().bottom > top);
    area.scrollTop += Math.round(first.getBoundingClientRect().top - top + first.getBoundingClientRect().height / 2);
    await area.dispatch("scroll");
    let before = rowsOnScreen(page, "thread-list", "scroll-area", idOf);
    const held = [...before.keys()][0];
    assert.ok(before.size >= 3 && list.children.find((li) => idOf(li) === held).getBoundingClientRect().top < top,
      "the first card is not half off the top among others, so this proves nothing");
    const index = data.threads.findIndex((thread) => thread.id === held);
    await liveReply(page, server, data.messages[2 * index], "400");
    assert.equal(ids()[ids().length - 1], held, "the card answered did not go to the foot");
    const found = [["the first card on screen moved to the foot", rowsMoved(page, "thread-list", before, held, idOf)]];
    // The control follows the card to the foot, where nothing is below the screen.
    if (script !== SCRIPT) return found;
    before = rowsOnScreen(page, "thread-list", "scroll-area", idOf);
    const shown = new Set(before.keys());
    const below = ids().slice(ids().indexOf([...before.keys()].pop()) + 1).find((id) => !shown.has(id) && id !== held);
    await liveReply(page, server, data.messages[2 * data.threads.findIndex((thread) => thread.id === below)], "401");
    found.push(["a reply below the screen", rowsMoved(page, "thread-list", before, null, idOf)]);
    return found;
  };
  for (const [what, moved] of await run(SCRIPT)) {
    assert.deepStrictEqual(moved, [], `${what}, and the cards on screen moved by (id, px): ${JSON.stringify(moved)}`);
  }
  const loose = await run(brokenScript("    if (!neighbourStayed(position.anchorId, now, around.near)) {",
    "    if (false) {"));
  assert.ok(loose[0][1].length > 0, "the negative control left the cards where they were, so the check above proves nothing");
});

test("`#261 two-column-connectors`: closing a thread comes back to the card it was chosen from; the way to older threads is above the oldest card", async () => {
  const { page, data } = await manyThreadColumns(16);
  const area = page.el("side-scroll");
  area.scrollTop = 0;
  const card = page.el("side-cards").children[2];
  const at = card.getBoundingClientRect().top;
  await openerOf(card).click();
  await page.settle();
  assert.equal(page.el("side-list").hidden, true, "the card did not select its thread");
  await page.el("side-close").click();
  await page.settle();
  const back = page.el("side-cards").children.find((li) => li.getAttribute("data-id") === card.getAttribute("data-id"));
  assert.equal(back.getBoundingClientRect().top, at, "closing the thread did not come back to the card it was chosen from");
  // From the foot, back to the foot.
  area.scrollTop = area.scrollHeight;
  const last = page.el("side-cards").children[page.el("side-cards").children.length - 1];
  await openerOf(last).click();
  await page.settle();
  await page.el("side-close").click();
  await page.settle();
  assert.ok(sideAtFoot(page), "a thread chosen at the foot did not come back to the foot");
  assert.ok(markupHolds("side-list", "side-more-threads"));
  const html = HTML.slice(HTML.indexOf('id="side-list"'));
  assert.ok(html.indexOf('id="side-more-threads"') < html.indexOf('id="side-cards"'),
    "the way to older threads is not above the oldest card");
  assert.ok(data.threads.length === 16);
});

// --- the read mode over the cards (#432 read-mode-unify; Unread only, #259, before it) ---------------
//
// The owner, 2026-10-12, of #259 thread-list-unread-filter: "Some have "N unread" and others are fully
// read... I would like a toggle ... to filter it to unread only." And 2026-10-22, #432: "I think we should
// unify it actually ... the thread view will respect show/hide/collapse for unread ... a thread card with
// any unread-children acts as an unread, and with 100% read acts as read." So Unread only is gone: the read
// mode filters the cards as it filters every list.

/** The ids of the cards the filters leave on screen, in order. */
const shownCardIds = (page) => page.el("side-cards").children
  .filter((li) => li.hasAttribute("data-card") && !li.hidden && !li.hasClass("search-hidden")).map((li) => li.getAttribute("data-id"));

/** The ids of the cards saying their thread has unread replies, by their chip. */
const unreadCardIds = (page) => page.el("side-cards").children
  .filter((li) => cardPart(page, li.getAttribute("data-id"), "side-card-unread")).map((li) => li.getAttribute("data-id"));

/** `manyThreadColumns`, with the answers of the threads numbered in `read` marked Done on the server. */
const readThreadColumns = (count, read, script = SCRIPT, store = null) =>
  manyThreadColumns(count, (p) => {
    for (const i of read) p.dealtWith.add(String(301 + 2 * i));
    if (store) for (const [key, value] of store) p.storage.set(key, value);
  }, script);

/** `manyThreadColumns`, the threads numbered in `read` read whole: root and answer marked Done. */
const wholeReadColumns = (count, read, script = SCRIPT) =>
  manyThreadColumns(count, (p) => {
    for (const i of read) {
      p.dealtWith.add(String(300 + 2 * i));
      p.dealtWith.add(String(301 + 2 * i));
    }
  }, script);

test("`#432 read-mode-unify`: Unread only is gone from the cards' heading, whose chips are Unpack msgs, Main channel and the order, and the read mode is the cards' filter", async () => {
  assert.doesNotMatch(HTML_CODE, /id="side-unread-only"/, "Unread only is still on the page");
  const head = HTML_CODE.slice(HTML_CODE.indexOf('id="side-heading"'), HTML_CODE.indexOf('id="side-thread-card"'));
  const order = ["side-title", "side-unpack", "side-main", "side-order"].map((id) => head.indexOf(`id="${id}"`));
  assert.ok(order.every((at, i) => at >= 0 && (i === 0 || at > order[i - 1])), `the heading's chips are not in order: ${order}`);
  assert.match(cssBlock("#side-unpack"), /margin-inline-start:\s*auto/, "the chips do not stand at the heading's far end");
  assert.doesNotMatch(SCRIPT, /side-unread-only|SIDE_UNREAD_KEY|sideUnreadOnly/, "voice.js still keeps Unread only");
});

test("`#432 read-mode-unify`: Hide read keeps exactly the threads with an unread message — a reply or the root — and Show read every card; one unread reply makes a thread unread", async () => {
  const { page, server, data: held } = await wholeReadColumns(6, [0, 2, 3]);
  assert.deepStrictEqual(shownCardIds(page).length, 6, "Show read hides a card");
  await readModeTo(page, "Hide read");
  const unread = ["spaces/A/threads/t1", "spaces/A/threads/t4", "spaces/A/threads/t5"];
  assert.deepStrictEqual(shownCardIds(page), unread, "Hide read did not keep exactly the threads with an unread message");
  assert.equal(page.el("side-list-state").hidden, true, "a list with cards in it says it is empty");
  // A reply arriving in a read thread makes it unread: its card comes back, at the foot.
  await liveReply(page, server, held.messages[0], "400");
  assert.deepStrictEqual(shownCardIds(page), [...unread, "spaces/A/threads/t0"],
    "a read thread one reply came into is not unread under Hide read");
  await readModeTo(page, "Show read");
  assert.equal(shownCardIds(page).length, 6, "back to Show read, every card did not come back");
  // A thread whose replies are all read but whose root is not is unread too (the root is one of its messages).
  const rootOnly = await readThreadColumns(2, [0, 1]);
  await readModeTo(rootOnly.page, "Hide read");
  assert.deepStrictEqual(unreadCardIds(rootOnly.page), [], "the premise is no unread reply");
  assert.deepStrictEqual(shownCardIds(rootOnly.page), ["spaces/A/threads/t0", "spaces/A/threads/t1"],
    "a thread with its root unread was taken for read");
});

test("`#432 read-mode-unify`: under Hide read with nothing unread the cards say so, and with the search open Hide read steps aside as it does for messages", async () => {
  const { page } = await wholeReadColumns(4, [0, 1, 2, 3]);
  await readModeTo(page, "Hide read");
  assert.deepStrictEqual(shownCardIds(page), []);
  assert.equal(page.el("side-list-state").hidden, false, "an emptied list says nothing");
  assert.equal(page.el("side-list-state").textContent, "No unread threads.");
  await readModeTo(page, "Show read");
  assert.equal(page.el("side-list-state").hidden, true, "the empty state stayed with the cards back");
  const searched = (await wholeReadColumns(5, [0, 2])).page;
  await readModeTo(searched, "Hide read");
  await searched.el("search-toggle").click();
  await searched.el("search-field").setValue("thread 2");
  await searched.settle();
  // `#262 search-load-older`: a filter choosing rows lifts Hide read, so a read thread the search finds shows.
  assert.deepStrictEqual(shownCardIds(searched), ["spaces/A/threads/t2"], "the search did not find a read thread under Hide read");
  await searched.el("search-field").setValue("nothing like this");
  await searched.settle();
  assert.equal(searched.el("side-list-state").textContent, "No threads match that search — all of this channel is loaded.");
});

test("`#432 read-mode-unify` x `#243 filter-keeps-selection`: the card selected stays where it was as Hide read goes on and off", async () => {
  const run = async (script) => {
    // Every other thread read, so the cards left still run past the column's foot.
    const read = Array.from({ length: 15 }, (_, i) => 2 * i);
    const { page } = await wholeReadColumns(30, read, script);
    const area = page.el("side-scroll");
    area.scrollTop = 600;
    const card = page.el("side-cards").children.find((li) => {
      const box = li.getBoundingClientRect();
      return li.hasAttribute("data-card") && !li.hasAttribute("data-thread-read") && box.top > 40 && box.bottom < area.clientHeight - 40;
    });
    assert.ok(card, "no unread card in view, so this proves nothing");
    await mouseTo(page, card, 900, 100);
    const id = card.getAttribute("data-id");
    const top = () => cardRow(page, id).getBoundingClientRect().top;
    const at = top();
    await readModeTo(page, "Hide read");
    const on = top() - at;
    await readModeTo(page, "Show read");
    return { on, off: top() - at };
  };
  const kept = await run(SCRIPT);
  assert.equal(kept.on, 0, `Hide read moved the card selected by ${kept.on}px`);
  assert.equal(kept.off, 0, `back to Show read moved the card selected by ${kept.off}px`);
});

/** What h says on the status line of each mode (`READ_MODE_SAID`). */
const READ_MODE_STATUS = {
  show: "Show read: everything shown.",
  fold: "Fold read: read runs folded.",
  hide: "Hide read: read ones hidden.",
};

/** The channel of `threadData` with messages `read` Done, in view `view` ("thread" for its first thread). */
async function readModePage(view, read = ["200", "201", "202"]) {
  const page = newPage();
  const data = threadData();
  page.threadingSupported = true;
  page.threads = data.threads;
  for (const id of read) page.dealtWith.add(id);
  await signIn(page);
  await showDiscord(page, data.messages);
  await pickThread(page, view === "thread" ? `thread:${data.threads[0].id}` : view);
  await page.settle();
  return page;
}

/** The run lines a list draws: the words of each, in order. */
const runLines = (page, list) => page.el(list).children.filter((li) => li.getAttribute("data-read-run") === "head" && !li.hidden)
  .map((li) => String(li.readRunLine ? li.readRunLine.textContent : ""));

test("`#432 read-mode-unify`: h is the button's own act in All, Main, a thread and the Threads view — the button's words, aria and the list move together — and says the mode", async () => {
  for (const view of ["flat", "main", "thread", "threads"]) {
    const page = await readModePage(view);
    const target = view === "threads" ? page.el("side-scroll") : page.el("scroll-area");
    for (const [mode, label, pressed] of [["fold", "Fold read", "mixed"], ["hide", "Hide read", "true"], ["show", "Show read", "false"]]) {
      await press(page, target, "h");
      assert.deepStrictEqual(readModeSaid(page), [label, pressed, mode], `${view}: h did not move the button to ${label}`);
      assert.equal(page.el("status").textContent, READ_MODE_STATUS[mode], `${view}: h did not say ${label}`);
    }
  }
});

test("`#432 read-mode-unify`: Fold read by h draws the run lines the button draws, in All", async () => {
  const lines = async (how) => {
    const page = await readModePage("flat");
    if (how === "key") await press(page, page.el("scroll-area"), "h");
    else {
      await page.el("todo-filter").click();
      await page.settle();
    }
    return runLines(page, "discord-log");
  };
  const byKey = await lines("key");
  assert.ok(byKey.length > 0 && byKey[0].replace(/\s+/g, " ").startsWith("… 3 read messages"), `h drew no run line: ${byKey}`);
  assert.deepStrictEqual(byKey, await lines("button"), "h and the button draw different lines");
});

test("`#432 read-mode-unify`: under Fold read each run of read threads over the cards is one line saying how many, the newest card never in one; a tap opens it, and a thread with one unread reply breaks a run", async () => {
  // t0, t1, t2 read whole, t3 unread, t4 read alone, t5 read but newest: one run of three.
  const { page } = await wholeReadColumns(6, [0, 1, 2, 4, 5]);
  await readModeTo(page, "Fold read");
  const card = (i) => cardRow(page, `spaces/A/threads/t${i}`);
  assert.deepStrictEqual(runLines(page, "side-cards"), ["… 3 read threads"], "the run of read threads is not one line saying three");
  assert.equal(card(0).getAttribute("data-read-run"), "head");
  assert.ok(card(1).hidden && card(2).hidden, "the run's other cards are drawn");
  assert.ok(!card(3).hidden && !card(4).hidden && !card(5).hidden, "a card outside the run is hidden");
  assert.equal(card(4).hasAttribute("data-read-run"), false, "a read thread alone was folded");
  assert.equal(card(5).hasAttribute("data-read-run"), false, "the newest card joined a run");
  // A tap on the line opens the run, and it stays open through a redraw.
  await card(0).readRunLine.click();
  await page.settle();
  assert.deepStrictEqual(runLines(page, "side-cards"), [], "the tap did not open the run");
  assert.ok([0, 1, 2].every((i) => !card(i).hidden), "the opened run's cards are not all drawn");
  await page.el("side-unpack").click();
  await page.el("side-unpack").click();
  await page.settle();
  assert.deepStrictEqual(runLines(page, "side-cards"), [], "a redraw folded the opened run again");
  // Another read mode and back folds it again (as a list of messages does).
  await readModeTo(page, "Show read");
  assert.deepStrictEqual(runLines(page, "side-cards"), [], "Show read drew a run line");
  await readModeTo(page, "Fold read");
  assert.deepStrictEqual(runLines(page, "side-cards"), ["… 3 read threads"]);
  // One unread reply in t1 makes it unread, and the run of three is broken.
  const unreadReply = await manyThreadColumns(6, (p) => {
    for (const i of [0, 1, 2, 4, 5]) p.dealtWith.add(String(300 + 2 * i));
    for (const i of [0, 2, 4, 5]) p.dealtWith.add(String(301 + 2 * i));
  });
  await readModeTo(unreadReply.page, "Fold read");
  assert.deepStrictEqual(runLines(unreadReply.page, "side-cards"), [], "a thread with an unread reply joined a run of read threads");
});

test("`#432 read-mode-unify`: the search opens the runs of read threads while it chooses cards, as it opens runs of messages, and they fold again when it closes", async () => {
  const { page } = await wholeReadColumns(6, [0, 1, 2, 4, 5]);
  await readModeTo(page, "Fold read");
  assert.deepStrictEqual(runLines(page, "side-cards"), ["… 3 read threads"], "the premise is a folded run");
  await page.el("search-toggle").click();
  await page.el("search-field").setValue("thread 1");
  await page.settle();
  assert.deepStrictEqual(runLines(page, "side-cards"), [], "the search left the run folded");
  assert.deepStrictEqual(shownCardIds(page), ["spaces/A/threads/t1"], "the search did not find a card inside the run");
  await page.el("search-field").setValue("");
  await page.el("search-toggle").click();
  await page.settle();
  assert.deepStrictEqual(runLines(page, "side-cards"), ["… 3 read threads"], "the run did not fold again once the search closed");
});

test("`#432 read-mode-unify`: unpacked, a folded run of read threads takes their unpacked rows behind its line, and Enter on the line opens it", async () => {
  const { page } = await wholeReadColumns(4, [0, 1, 2]);
  await press(page, page.el("scroll-area"), "3", { ctrlKey: true });
  assert.equal(page.el("screen-main").getAttribute("data-columns"), "list", "the premise is the Threads view");
  // Unpack all (`#440`: Pack all, Unpack unread, Unpack all), so the read threads are unpacked too.
  await page.el("side-unpack").click();
  await page.el("side-unpack").click();
  await page.settle();
  await readModeTo(page, "Fold read");
  const blockOf = (i) => page.el("side-cards").children.filter((li) => li.getAttribute("data-unpack-thread") === `spaces/A/threads/t${i}`);
  assert.deepStrictEqual(runLines(page, "side-cards").filter((words) => words.includes("thread")), ["… 3 read threads"]);
  assert.ok([0, 1, 2].every((i) => blockOf(i).every((li) => li.hidden)), "a folded thread's unpacked rows are drawn");
  assert.ok(blockOf(3).some((li) => !li.hidden), "the unread thread's rows are hidden");
  const head = cardRow(page, "spaces/A/threads/t0");
  await press(page, head, "Enter");
  await page.settle();
  assert.ok(!runLines(page, "side-cards").some((words) => words.includes("thread")), "Enter on the line did not open the run");
  assert.ok(blockOf(1).some((li) => !li.hidden), "the opened run's unpacked rows are not drawn");
});

test("`#432 read-mode-unify`: with Main channel on, a run takes the read main-channel messages among the cards too, and says both", async () => {
  const page = newPage(new Map([[THREADS_MAIN_STORAGE, "on"]]));
  const data = freshMessageData();
  page.threadingSupported = true;
  page.threads = data.threads;
  page.messages = data.messages;
  for (const id of ["200", "201", "202", "203", "204"]) page.dealtWith.add(id);
  threadedService(page);
  await signIn(page);
  await showDiscord(page, data.messages);
  await pickThread(page, "threads");
  await page.settle();
  assert.deepStrictEqual(cardsListed(page), ["main:200", "card:spaces/A/threads/one & two", "card:spaces/A/threads/another", "main:210"],
    "the premise: Main's 200, both cards, then 210");
  await readModeTo(page, "Fold read");
  assert.deepStrictEqual(runLines(page, "side-cards"), ["… 2 read threads and 1 read message"],
    "the run of Main's read message and two read threads does not say both");
});

test("`#432 read-mode-unify`: a record that says Collapse read, from before Fold read was its name, opens in Fold read", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog());
  page.storage.set(UI_STATE_KEY, JSON.stringify({ ...savedUiState(page), read: "collapse" }));
  const older = reloadWith(page.storage, backlog());
  await reopenedChannel(older);
  assert.deepStrictEqual(readModeSaid(older), ["Fold read", "mixed", "fold"], "a saved Collapse read did not open as Fold read");
  assert.equal(savedUiState(older).read, "fold", "the record was not saved again under the new name");
});

// --- the links between Main's roots and their cards (#261 two-column-connectors) ---------------------
//
// The owner, 2026-10-12: "Some git diff style tools show connections between hunks on the left and the
// right with connector arrows (the kind with only right angle bends that leave the box on the left,
// travel through the middle vertical gap, and connect to the right side). I want such connectors": in
// the thread's colour; drawn where both ends are on screen; the highlighted message or card drawing
// its own even off screen; and, left highlighted, "after say a 1.5 second delay start scrolling the
// other column smoothly to bring its counterpart onto the screen". This fixture lays nothing out across
// the page, so the tests below give the columns a 1440px desk's horizontal places (`layColumnsOut`),
// and real Chromium measures the drawing itself (tests/two_column_browser.py).

const COLUMN_GLIDE_MS = sourceConstant("COLUMN_GLIDE_MS");
const LINK_HEAD_PX = sourceConstant("LINK_HEAD_PX");

/** Main's row of the root of thread `t<i>`, and that thread's card. */
const rootRow = (page, i) => rowWithId(page, String(300 + 2 * i));
const threadCard = (page, i) => page.el("side-cards").children.find((li) => li.getAttribute("data-id") === `spaces/A/threads/t${i}`);

/** Where a link meets a root: level with the middle of its Thread(N). */
const rootLinkY = (row) => {
  const tag = threadBadge(row).getBoundingClientRect();
  return (tag.top + tag.bottom) / 2;
};
const cardLinkY = (card) => {
  const box = card.getBoundingClientRect();
  return (box.top + box.bottom) / 2;
};
/** Whether viewport y `y` shows in the column scrolled by `area`. */
const inColumnView = (area, y) => y >= area.getBoundingClientRect().top && y <= area.getBoundingClientRect().top + area.clientHeight;

const LINK_GLIDE_MARGIN_PX = sourceConstant("LINK_GLIDE_MARGIN_PX");
const LANDING_GAP_PX = sourceConstant("REPLY_LANDING_GAP_PX");

/** Scroll Main until the tag of thread `t<i>`'s root is at viewport y `y`, as the reader would. */
async function rootAt(page, i, y) {
  const left = page.el("scroll-area");
  left.scrollTop += rootLinkY(rootRow(page, i)) - y;
  await left.dispatch("scroll");
  await page.nextFrame();
  assert.equal(rootLinkY(rootRow(page, i)), y, "Main cannot be scrolled to put the root there");
}

/** Where a glide in the column `area` brings an end, to be level with `y`: as near as its margins allow. */
const glideLevel = (area, y) => {
  const top = area.getBoundingClientRect().top;
  return Math.max(top + LINK_GLIDE_MARGIN_PX, Math.min(top + area.clientHeight - LINK_GLIDE_MARGIN_PX, y));
};

/** Two columns over `count` threads, with frames, laid out across a desk; `arrange` as `newPage`'s. */
async function linkedColumns(count, arrange = null, script = SCRIPT) {
  const columns = await manyThreadColumns(count, (p) => {
    p.enableAnimationFrames();
    if (arrange) arrange(p);
  }, script);
  await columns.page.nextFrame();
  return columns;
}

test("`#261 two-column-connectors`: the links are one SVG over the main screen, drawn only in two columns, hidden from assistive technology, never in the way of a press", () => {
  assertMarkupContains("screen-main", "column-links");
  const tag = /<svg\s+id="column-links"[^>]*>/.exec(HTML_CODE)[0];
  assert.match(tag, /aria-hidden="true"/, "the links are not hidden from assistive technology");
  assert.match(cssBlockOutside(TWO_COLUMN_QUERY, "#column-links"), /display:\s*none/, "the links can be drawn in one column");
  const drawn = cssBlockIn(TWO_COLUMN_QUERY, '#screen-main[data-columns="two"] > #column-links');
  assert.match(drawn, /pointer-events:\s*none/, "the links can take a press meant for a row or a card");
  assert.match(drawn, /position:\s*absolute/);
  assert.match(cssBlockIn(TWO_COLUMN_QUERY, "#column-links .column-link"), /stroke:\s*hsl\(var\(--thread-hue, 210\) 55% 50%\)/,
    "a link is not drawn in its thread's colour, as the card's edge is");
  assert.match(cssBlockOutside(TWO_COLUMN_QUERY, ".side-card-row"), /border-left:\s*3px solid hsl\(var\(--thread-hue, 210\) 55% 50%\)/,
    "the card's edge is no longer the colour a link is drawn in");
  // A reduced-motion reader's glide is a jump: the stylesheet says so, and the script reads it.
  assert.match(CSS_CODE, /@media \(prefers-reduced-motion: reduce\) \{\s*#screen-main \{\s*--column-glide: auto;/,
    "the stylesheet does not tell the page a reduced-motion reader's glide should jump");
  assert.doesNotMatch(PAGE_CODE, /matchMedia|prefers-reduced-motion/, "web/voice.js grew a second opinion about motion");
});

test("`#261 two-column-connectors`: a root on Main and its thread's card are joined while both are on screen, out of the root's right edge level with its tag and into the card's left edge, in the thread's colour", async () => {
  const restore = layColumnsOut();
  try {
    const run = async (script) => {
      const { page } = await linkedColumns(16, null, script);
      const left = page.el("scroll-area");
      const right = page.el("side-scroll");
      /** The threads whose root and card both show, by where each link meets them. */
      const bothShown = () => [...Array(16).keys()].filter((i) =>
        inColumnView(left, rootLinkY(rootRow(page, i))) && inColumnView(right, cardLinkY(threadCard(page, i))))
        .map((i) => `spaces/A/threads/t${i}`);
      const snapshots = [];
      for (const [leftTop, rightTop] of [[left.scrollHeight, right.scrollHeight], [0, right.scrollHeight], [0, 0], [300, 120]]) {
        left.scrollTop = leftTop;
        right.scrollTop = rightTop;
        await left.dispatch("scroll");
        await right.dispatch("scroll");
        await page.nextFrame();
        snapshots.push({ expected: bothShown(), drawn: drawnLinks(page) });
      }
      return { page, snapshots };
    };
    const { page, snapshots } = await run(SCRIPT);
    assert.ok(snapshots[0].expected.length > 0 && snapshots[2].expected.length > 0, "no pair is on screen, so this proves nothing");
    assert.deepStrictEqual(snapshots[1].expected, [], "a pair is on screen with Main at the top and the cards at the foot, so this proves less");
    for (const { expected, drawn } of snapshots) {
      assert.deepStrictEqual(drawn.map((link) => link.thread).sort(), [...expected].sort(),
        "the links are not exactly the pairs with both ends on screen");
      for (const link of drawn) assert.equal(link.clip, "both");
    }
    // The last place: each link's ends on the boxes it joins, its lane in the gap, its colour the thread's.
    for (const link of snapshots[3].drawn) {
      const i = Number(link.thread.replace(/^.*t/, ""));
      const row = rootRow(page, i).getBoundingClientRect();
      const card = threadCard(page, i).getBoundingClientRect();
      const [fx, fy, lane, ty, end] = link.line;
      assert.equal(fx, row.right, "a link does not leave its root's right edge");
      assert.equal(fy, rootLinkY(rootRow(page, i)), "a link does not leave its root level with its tag");
      assert.ok(fy >= row.top && fy <= row.bottom);
      assert.equal(end + LINK_HEAD_PX, card.left, "a link's arrowhead does not reach its card's left edge");
      assert.equal(link.marks[link.marks.length - 6], card.left, "the arrowhead's point is not on the card's edge");
      assert.equal(ty, cardLinkY(threadCard(page, i)), "a link does not arrive at its card's middle");
      assert.ok(lane > 708 && lane < card.left - LINK_HEAD_PX, `a link's lane at ${lane} is not in the gap between the columns`);
      const hue = threadCard(page, i).style.getPropertyValue("--thread-hue");
      assert.equal(link.hue, hue, "a link is not the colour of its card's edge");
      assert.equal(link.marksHue, hue);
      assert.equal(threadBadge(rootRow(page, i)).style.getPropertyValue("--thread-hue"), hue, "a link is not the colour of its root's tag");
    }
    const lanes = snapshots[3].drawn.map((link) => link.line[2]);
    assert.equal(new Set(lanes).size, lanes.length, "two links share a lane");
    // The control: drawn whatever is on screen, the links are more than the pairs on screen.
    const loose = await run(brokenScript("    if (end.fromSeen && end.toSeen) {", "    if (true) {"));
    assert.notDeepStrictEqual(loose.snapshots[1].drawn.map((link) => link.thread), [],
      "the negative control drew nothing either, so the check above proves nothing");
  } finally {
    restore();
  }
});

test("`#261 two-column-connectors`: the selected root or card draws its link with the other end off screen, clipped at the other column's edge and pointing up or down to it", async () => {
  const restore = layColumnsOut();
  try {
    const { page } = await linkedColumns(16);
    const left = page.el("scroll-area");
    const right = page.el("side-scroll");
    left.scrollTop = 0;
    await left.dispatch("scroll");
    await page.nextFrame();
    assert.deepStrictEqual(drawnLinks(page), [], "a pair is on screen, so this proves less");
    // A root on Main, its card above the right column's top: drawn, pointing up, ending at that edge.
    await mouseTo(page, rootRow(page, 0), 300, 40);
    assert.deepStrictEqual(litRows(page), ["discord-log:300"]);
    await page.nextFrame();
    let [link] = drawnLinks(page);
    assert.ok(link, "the selected root's link was not drawn with its card off screen");
    assert.equal(link.thread, "spaces/A/threads/t0");
    assert.equal(link.clip, "up", "the link does not point up to a card above the column");
    assert.equal(link.selected, true);
    assert.ok(cardLinkY(threadCard(page, 0)) < right.getBoundingClientRect().top, "the card is on screen, so this proves nothing");
    const top = right.getBoundingClientRect().top;
    assert.equal(link.line[link.line.length - 1], top + 2 + LINK_HEAD_PX, "the line does not stop at the arrowhead under the column's edge");
    assert.equal(link.marks[link.marks.length - 5], top + 2, "the arrowhead does not point at the column's top edge");
    // A card at the foot, its root below Main's foot: pointing down.
    await mouseTo(page, threadCard(page, 15), 900, 300);
    assert.deepStrictEqual(litRows(page), ["side-cards:spaces/A/threads/t15"]);
    await page.nextFrame();
    [link] = drawnLinks(page);
    assert.equal(link.thread, "spaces/A/threads/t15");
    assert.equal(link.clip, "down", "the link does not point down to a root below the column");
    assert.ok(rootLinkY(rootRow(page, 15)) > left.clientHeight, "the root is on screen, so this proves nothing");
    // Moved on to another root, its link takes the last one's place.
    await mouseTo(page, rootRow(page, 1), 300, 200);
    await page.nextFrame();
    assert.deepStrictEqual(drawnLinks(page).map((one) => [one.thread, one.clip]), [["spaces/A/threads/t1", "up"]]);
  } finally {
    restore();
  }
});

test("`#261 two-column-connectors`: left selected for 1.5 s with its other end off screen, the other column scrolls that end level with it, smoothly, and the link is whole", async () => {
  const restore = layColumnsOut();
  try {
    const { page } = await linkedColumns(16);
    const left = page.el("scroll-area");
    const right = page.el("side-scroll");
    await rootAt(page, 8, 120);
    await mouseTo(page, rootRow(page, 8), 300, 120);
    await page.nextFrame();
    assert.deepStrictEqual(drawnLinks(page).filter((link) => link.selected).map((link) => link.clip), ["up"],
      "the card is on screen already, so this proves nothing");
    assert.equal([...page.timers.values()].filter((timer) => timer.ms === COLUMN_GLIDE_MS).length, 1, "no wait was armed for the glide");
    assert.equal(COLUMN_GLIDE_MS, 1500);
    assert.equal(page.expireTimers(COLUMN_GLIDE_MS), 1);
    assert.deepStrictEqual((right.glides || []).map((glide) => glide.behavior), ["smooth"], "the right column did not glide, smoothly");
    assert.equal(cardLinkY(threadCard(page, 8)), glideLevel(right, rootLinkY(rootRow(page, 8))),
      "the card was not brought level with its root");
    assert.equal(left.glides, undefined, "the column the selection is in was scrolled");
    await right.dispatch("scroll");
    await page.nextFrame();
    assert.deepStrictEqual(drawnLinks(page).filter((link) => link.selected).map((link) => link.clip), ["both"],
      "the link is not whole once its card is on screen");
    // From the right: a card whose root is below Main's foot brings the root up, level with it.
    right.scrollTop = right.scrollHeight;
    await right.dispatch("scroll");
    await mouseTo(page, threadCard(page, 15), 900, 300);
    page.expireTimers(COLUMN_GLIDE_MS);
    assert.deepStrictEqual((left.glides || []).map((glide) => glide.behavior), ["smooth"], "Main did not glide to the card's root");
    assert.ok(inColumnView(left, rootLinkY(rootRow(page, 15))), "the root was not brought on screen");
    // Already whole, nothing moves: a root whose card is on screen arms no glide that scrolls.
    const glides = right.glides.length;
    await mouseTo(page, rootRow(page, 15), 300, 200);
    page.expireTimers(COLUMN_GLIDE_MS);
    assert.equal(right.glides.length, glides, "a column was scrolled to an end already on screen");
  } finally {
    restore();
  }
});

test("`#261 two-column-connectors`: the glide waits for nothing once the selection moves or the reader scrolls that column, and jumps for a reader who asks for reduced motion", async () => {
  const restore = layColumnsOut();
  try {
    // The selection moves on before the wait is up: the wait is for the new one, never the old.
    let { page } = await linkedColumns(16);
    await rootAt(page, 7, 40);
    await mouseTo(page, rootRow(page, 7), 300, 40);
    await mouseTo(page, rootRow(page, 8), 300, 300);
    assert.deepStrictEqual(litRows(page), ["discord-log:316"]);
    assert.equal([...page.timers.values()].filter((timer) => timer.ms === COLUMN_GLIDE_MS).length, 1, "the old wait is still armed beside the new one");
    page.expireTimers(COLUMN_GLIDE_MS);
    assert.equal(cardLinkY(threadCard(page, 8)), glideLevel(page.el("side-scroll"), rootLinkY(rootRow(page, 8))),
      "the glide did not bring the new selection's card level with it");
    // The reader scrolls the other column while it waits: theirs to move, and it stays where they put it.
    ({ page } = await linkedColumns(16));
    const right = page.el("side-scroll");
    page.el("scroll-area").scrollTop = 0;
    await mouseTo(page, rootRow(page, 0), 300, 40);
    right.scrollTop -= 60;
    await right.dispatch("scroll");
    const put = right.scrollTop;
    page.expireTimers(COLUMN_GLIDE_MS);
    assert.equal(right.glides, undefined, "the column the reader scrolled was glided anyway");
    assert.equal(right.scrollTop, put);
    // A thread on the right instead of the cards: its one link, from its root to the card at the top of
    // the column (`#298 thread-card-header`), and no glide, the wait for root 0 dropped with the cards.
    ({ page } = await linkedColumns(16));
    page.el("scroll-area").scrollTop = 0;
    await mouseTo(page, rootRow(page, 0), 300, 40);
    await openerOf(threadCard(page, 3)).click();
    await page.settle();
    await page.nextFrame();
    page.expireTimers(COLUMN_GLIDE_MS);
    assert.equal(page.el("side-scroll").glides, undefined, "a glide ran with a thread on the right");
    assert.equal(page.el("scroll-area").glides, undefined, "a glide ran with a thread on the right");
    assert.deepStrictEqual(drawnLinks(page).map((link) => [link.thread, link.selected, link.dimmed]),
      [["spaces/A/threads/t3", true, false]], "the thread on the right is not joined to its root alone");
    // Reduced motion: the same glide, at once.
    ({ page } = await linkedColumns(16, (p) => p.preferReducedMotion()));
    page.el("scroll-area").scrollTop = 0;
    await mouseTo(page, rootRow(page, 0), 300, 40);
    page.expireTimers(COLUMN_GLIDE_MS);
    assert.deepStrictEqual(page.el("side-scroll").glides.map((glide) => glide.behavior), ["auto"],
      "a reader who asks for reduced motion was given a smooth glide");
  } finally {
    restore();
  }
});

test("`#280 scroll-stability` x `#261`: a selection a redraw hands on, its card gone under Hide read, glides nothing", async () => {
  const restore = layColumnsOut();
  try {
    const run = async (rest) => {
      // The fixture's own timeline, kept from before the change record replaces it: its newest page of
      // Main says which replies are Done (`reply_dismissals`), which is how a Done made on another device
      // reaches this one.
      const { page } = await linkedColumns(16, (p) => { p.plainTimeline = p.timeline; });
      // `#432 read-mode-unify`: Hide read, which took over from Unread only, filters the cards.
      await readModeTo(page, "Hide read");
      const left = page.el("scroll-area");
      const right = page.el("side-scroll");
      left.scrollTop = 0;
      await left.dispatch("scroll");
      right.scrollTop = 0;
      await right.dispatch("scroll");
      await page.nextFrame();
      // The reader's choice: the first card, its root on screen at Main's head, so nothing glides.
      await mouseTo(page, threadCard(page, 0), 900, 60);
      assert.deepStrictEqual(litRows(page), ["side-cards:spaces/A/threads/t0"]);
      page.expireTimers(COLUMN_GLIDE_MS);
      assert.equal(left.glides, undefined, "a card whose root was on screen glided Main");
      // The reader goes down Main to its foot, and the first thread is read on another device: its card
      // goes under Hide read, and the selection is handed to the card now first, whose root is far
      // above Main's foot.
      left.scrollTop = left.scrollHeight;
      await left.dispatch("scroll");
      await page.nextFrame();
      page.dealtWith.add("300");
      page.dealtWith.add("301");
      page.timeline = page.plainTimeline;
      await reReadChannel(page);
      await page.nextFrame();
      const handed = litRows(page);
      assert.ok(!shownCardIds(page).includes("spaces/A/threads/t0"), "the read card is still shown, so this proves nothing");
      const armed = [...page.timers.values()].filter((timer) => timer.ms === COLUMN_GLIDE_MS).length;
      // The control's reader then moves the mouse onto the card handed the selection, and rests there.
      if (rest) await mouseTo(page, threadCard(page, 1), 900, 80);
      page.expireTimers(COLUMN_GLIDE_MS);
      return { handed, armed, glides: left.glides, at: left.scrollTop, foot: left.scrollHeight - left.clientHeight };
    };
    const kept = await run(false);
    assert.equal(kept.armed, 0, "a selection nobody chose armed a glide");
    assert.equal(kept.glides, undefined, "a selection nobody chose glided Main away from the reader");
    assert.equal(kept.at, kept.foot, "Main moved from where the reader put it");
    // THE CONTROL: the mouse resting on that same card glides Main up to its root, so the test can tell.
    const control = await run(true);
    assert.deepStrictEqual(control.handed, ["side-cards:spaces/A/threads/t1"], "the selection was not handed to the next card");
    assert.ok(control.glides && control.glides.length === 1, "the control did not glide, so the test cannot tell");
  } finally {
    restore();
  }
});

/** The mouse leaving the window: nothing under it, and the root element's `pointerleave`. */
async function mouseOutOfWindow(page) {
  FakeElement.hovered = null;
  await page.documentElement.dispatch("pointerleave", { pointerType: "mouse", target: page.documentElement, relatedTarget: null });
  await page.settle();
}

test("`#280 scroll-stability` x `#261`: the mouse crossing a card on its way elsewhere glides nothing, though the card stays selected; resting on it glides, even with the card moved out from under it, and coming back to it waits again", async () => {
  // The owner, in two columns: "it jumped me to the bottom of the screen in the main message view... I
  // don't think I touched anything." The mouse crossed the cards on its way to the dock, the last card
  // it passed stayed selected, and 1.5 s later Main slid to that card's root near its foot.
  const restore = layColumnsOut();
  try {
    /** Main at its head, the cards at their foot: the last card on screen, its root far below Main's foot. */
    const crossing = async (script = SCRIPT) => {
      const { page } = await linkedColumns(16, null, script);
      const left = page.el("scroll-area");
      const right = page.el("side-scroll");
      left.scrollTop = 0;
      await left.dispatch("scroll");
      right.scrollTop = right.scrollHeight;
      await right.dispatch("scroll");
      await page.nextFrame();
      assert.ok(!inColumnView(left, rootLinkY(rootRow(page, 15))), "the card's root is on screen, so this proves nothing");
      assert.ok(inColumnView(right, cardLinkY(threadCard(page, 15))), "the card is off screen, so this proves nothing");
      return { page, left, right };
    };
    const armed = (page) => [...page.timers.values()].filter((timer) => timer.ms === COLUMN_GLIDE_MS).length;
    const dock = (page) => page.el("compose-text");

    // Across the card and on to the dock.
    let { page, left, right } = await crossing();
    await mouseTo(page, threadCard(page, 15), 900, 300);
    assert.deepStrictEqual(litRows(page), ["side-cards:spaces/A/threads/t15"]);
    assert.equal(armed(page), 1, "the mouse arriving on the card armed no wait, so this proves nothing");
    await mouseTo(page, dock(page), 1100, 795);
    assert.equal(armed(page), 0, "the wait outlived the mouse moving on from the card");
    assert.deepStrictEqual(litRows(page), ["side-cards:spaces/A/threads/t15"], "the card did not stay selected, so this proves less");
    page.expireTimers(COLUMN_GLIDE_MS);
    assert.equal(left.glides, undefined, "the mouse crossing a card on its way to the dock glided Main");
    assert.equal(left.scrollTop, 0, "Main moved with nothing touched");
    // Back onto the card and resting there: that is a dwell, and it glides.
    await mouseTo(page, threadCard(page, 15), 905, 302);
    assert.equal(armed(page), 1, "the mouse coming back to the selected card armed no wait");
    page.expireTimers(COLUMN_GLIDE_MS);
    assert.deepStrictEqual((left.glides || []).map((glide) => glide.behavior), ["smooth"], "the mouse resting on the card did not glide Main");
    // Moving about inside the card after its glide arms nothing more.
    await mouseTo(page, threadCard(page, 15), 910, 306);
    assert.equal(armed(page), 0, "the mouse moving inside a card whose glide ran armed another");

    // Out of the window straight from the card, with no move outside it to say so.
    ({ page, left } = await crossing());
    await mouseTo(page, threadCard(page, 15), 900, 300);
    await mouseOutOfWindow(page);
    assert.equal(armed(page), 0, "the wait outlived the mouse leaving the window");
    page.expireTimers(COLUMN_GLIDE_MS);
    assert.equal(left.glides, undefined, "the mouse leaving the window from a card glided Main");

    // The card moving out from under a pointer standing still — drawn whole, the cards above it shorter —
    // and the hand on the mouse moving it a few pixels, over the column's own background: not left.
    ({ page, left, right } = await crossing());
    await mouseTo(page, threadCard(page, 15), 900, 300);
    await mouseTo(page, right, 903, 303);
    assert.equal(armed(page), 1, "a few pixels off a card that moved dropped the wait");
    page.expireTimers(COLUMN_GLIDE_MS);
    assert.deepStrictEqual((left.glides || []).map((glide) => glide.behavior), ["smooth"],
      "the mouse resting where it chose a card that then moved did not glide Main");

    // THE CONTROL: with a mouse that moved on still counted as staying, the crossing glides.
    const loose = await crossing(brokenScript("  if (pointerOn(followed ? followed.row : null)) return true;", "  return true;"));
    await mouseTo(loose.page, threadCard(loose.page, 15), 900, 300);
    await mouseTo(loose.page, dock(loose.page), 1100, 795);
    loose.page.expireTimers(COLUMN_GLIDE_MS);
    assert.ok(loose.left.glides && loose.left.glides.length === 1, "the control did not glide, so the test cannot tell");
  } finally {
    restore();
  }
});

test("`#280 scroll-stability` x `#261`: ] and [ handing the keys to a column glide nothing, even onto a card whose root is off screen", async () => {
  const restore = layColumnsOut();
  try {
    const { page } = await linkedColumns(16);
    const left = page.el("scroll-area");
    const right = page.el("side-scroll");
    left.scrollTop = 0;
    await left.dispatch("scroll");
    right.scrollTop = right.scrollHeight;
    await right.dispatch("scroll");
    await page.nextFrame();
    const armed = () => [...page.timers.values()].filter((timer) => timer.ms === COLUMN_GLIDE_MS).length;
    // The mouse once over the last card, then away to the dock: the card is the right column's selection.
    await mouseTo(page, threadCard(page, 15), 900, 300);
    await mouseTo(page, page.el("compose-text"), 5, 790);
    // Whatever that did is the test above's; Main is put back at its head.
    page.expireTimers(COLUMN_GLIDE_MS);
    left.glides = undefined;
    left.scrollTop = 0;
    await left.dispatch("scroll");
    await page.nextFrame();
    // [ gives the keys to Main and ] back to the right column, whose selection is that card again.
    await pressHere(page, "[");
    assert.equal(armed(), 0, "[ armed a glide");
    await pressHere(page, "]");
    assert.deepStrictEqual(litRows(page), ["side-cards:spaces/A/threads/t15"], "] did not light the card, so this proves nothing");
    assert.ok(!inColumnView(left, rootLinkY(rootRow(page, 15))), "the card's root is on screen, so this proves nothing");
    assert.equal(armed(), 0, "] onto a card armed a glide of Main");
    page.expireTimers(COLUMN_GLIDE_MS);
    assert.equal(left.glides, undefined, "] glided Main to the card's root");
    assert.equal(left.scrollTop, 0, "Main moved for a key that only handed the keys across");
    // The keys moving the selection, j and k, glide nothing either.
    await pressHere(page, "k");
    await pressHere(page, "j");
    assert.equal(armed(), 0, "j or k armed a glide");
    // THE CONTROL: the mouse resting on that card is a dwell, and Main glides, so the case is one that can.
    await mouseTo(page, threadCard(page, 15), 905, 302);
    page.expireTimers(COLUMN_GLIDE_MS);
    assert.ok(left.glides && left.glides.length === 1, "the mouse resting on the card did not glide Main, so the test cannot tell");
  } finally {
    restore();
  }
});

test("`#261 two-column-connectors` x `#226 page-energy-profile`: one frame for a burst of scrolls, nothing written when nothing moved, and nothing at all hidden, in one column or with a thread on the right", async () => {
  const restore = layColumnsOut();
  try {
    const { page } = await linkedColumns(16);
    const left = page.el("scroll-area");
    const right = page.el("side-scroll");
    assert.ok(drawnLinks(page).length > 0, "no links are drawn, so this proves nothing");
    for (let i = 0; i < 5; i += 1) {
      await left.dispatch("scroll");
      await right.dispatch("scroll");
    }
    assert.equal(page.frames.length, 1, "a burst of scrolls asked for more than one frame");
    // A frame in which nothing moved writes nothing, and makes nothing.
    const write = FakeElement.prototype.setAttribute;
    let writes = 0;
    FakeElement.prototype.setAttribute = function counted(name, value) {
      if (this.tagName === "path") writes += 1;
      return write.call(this, name, value);
    };
    const made = page.createdTags.length;
    try {
      await page.nextFrame();
    } finally {
      FakeElement.prototype.setAttribute = write;
    }
    assert.equal(writes, 0, "a frame in which nothing moved wrote to the links");
    assert.equal(page.createdTags.length, made);
    // Hidden: a scroll asks for no frame; shown again, one, and the links are drawn as they are now.
    await page.setVisibility("hidden");
    left.scrollTop = 0;
    await left.dispatch("scroll");
    assert.equal(page.frames.length, 0, "a hidden page asked for a frame to draw links in");
    await page.setVisibility("visible");
    assert.equal(page.frames.length, 1, "coming back to the page did not draw the links again");
    await page.nextFrame();
    assert.deepStrictEqual(drawnLinks(page), [], "the links were not drawn again for where the columns are now");
    // One column: the links go at once, and a scroll asks for nothing.
    left.scrollTop = left.scrollHeight;
    await left.dispatch("scroll");
    await page.nextFrame();
    assert.ok(drawnLinks(page).length > 0);
    await page.el("column-toggle").click();
    await page.settle();
    assert.deepStrictEqual(page.el("column-links").children, [], "the links stayed in one column");
    await left.dispatch("scroll");
    assert.equal(page.frames.length, 0, "a scroll in one column asked for a frame");
    // Without frames at all — the fixture's ordinary page — nothing is measured for links.
    const plain = await manyThreadColumns(4);
    assert.deepStrictEqual(plain.page.el("column-links").children, []);
  } finally {
    restore();
  }
});

test("`#261 two-column-connectors`: the jump to a thread's newest message goes with the thread, and is not left over the cards", async () => {
  const page = newPage();
  const messages = runThread(page, { tail: RUN_TAIL });
  await signIn(page);
  await showDiscord(page, messages);
  await twoColumns(page);
  await threadBadge(rowWithId(page, runId(0))).click();
  await page.settle();
  const side = page.el("side-scroll");
  side.scrollTop = 0;
  await side.dispatch("scroll");
  assert.equal(page.el("side-jump-newest").hidden, false, "the thread scrolled up offers no jump, so this proves nothing");
  await page.el("side-close").click();
  await page.settle();
  assert.equal(page.el("side-jump-newest").hidden, true, "the thread's jump to its newest message was left over the cards");
});

test("the search over the cards finds a thread by the replies the page holds, each term in the card or its replies, and says \"in replies\"", async () => {
  const { page, server, data } = await twoColumnServer();
  const [first, second] = data.threads;
  const shown = () => page.el("side-cards").children.filter((li) => !li.hidden && !li.hasClass("search-hidden"))
    .map((li) => li.getAttribute("data-id"));
  await page.el("search-toggle").click();
  // In every card's own words: found, nothing more said.
  await page.el("search-field").setValue("root");
  await page.settle();
  assert.deepStrictEqual(shown(), [first.id, second.id]);
  assert.equal(replyHint(page, first.id), null, "a card found by its own words says it was found in its replies");
  // In no card's own words, in both threads' replies: both, each saying where.
  await page.el("search-field").setValue("answer");
  await page.settle();
  assert.deepStrictEqual(shown(), [first.id, second.id], "the search did not reach the replies the page holds");
  assert.equal(replyHint(page, first.id), "in replies");
  assert.equal(replyHint(page, second.id), "in replies");
  assert.equal(page.el("side-cards").children[0].getAttribute("data-search-in"), "replies");
  // One term in the card, one in a reply: found; one in neither: not.
  await page.el("search-field").setValue("first answer");
  await page.settle();
  assert.deepStrictEqual(shown(), [first.id], "a term in the card and one in its replies did not find it together");
  assert.equal(replyHint(page, first.id), "in replies");
  assert.equal(replyHint(page, second.id), null, "a card the search hides still says it was found");
  // A reply that arrives is searched once it is held.
  await page.el("search-field").setValue("deploy");
  await page.settle();
  assert.deepStrictEqual(shown(), [], "a reply the page does not hold yet was found");
  const reply = server.post(message({ id: "205", content: "the deploy went out", thread: { ...data.messages[3].thread, is_root: false } }));
  await deliver(page, page.stream(), sseMessage(reply));
  await page.settle();
  assert.deepStrictEqual(shown(), [second.id], "a reply the page now holds was not searched");
  assert.equal(replyHint(page, second.id), "in replies");
  // Closed, every card is back and none says it was found.
  await page.el("search-toggle").click();
  await page.settle();
  assert.deepStrictEqual(shown(), [first.id, second.id]);
  assert.equal(replyHint(page, first.id), null);
  assert.equal(replyHint(page, second.id), null, "closing the search left a card saying it was found in its replies");
  assert.equal(page.el("side-cards").children[1].hasAttribute("data-search-in"), false);
});

test("`#277 thread-card-focus`: every card stays the reader's collapsed lines — under the mouse, the keys' ring and Tab's — and only a highlighted card shows all of it", async () => {
  // Anywhere in the stylesheet: since `#281 view-keys` the cards' rules stand outside the two columns'
  // block, for the thread list's cards, which are the same cards across the window.
  for (const lit of ['.side-card-row[data-selected="true"] > .side-card > .side-card-text', ".side-card:focus-visible > .side-card-text"]) {
    assert.deepStrictEqual(cssRules(CSS_CODE, lit), [], `${lit} still shows the card whole, so the card under the mouse grows`);
  }
  const whole = cssBlockOutside(TWO_COLUMN_QUERY, ".side-card-row[data-highlighted] > .side-card > .side-card-text");
  assert.match(whole, /-webkit-line-clamp:\s*none/, "a highlighted card is still clamped");
  assert.match(whole, /overflow:\s*visible/, "a highlighted card still hides what runs over");
  assert.match(whole, /(^|[^-])height:\s*auto/, "a highlighted card is still a fixed height");
  assert.doesNotMatch(whole, /min-height:/, "a highlighted short card still reserves its collapsed lines (`#482 thread-card-fit`)");
  const { page, data } = await twoColumnServer();
  const [first, second] = data.threads;
  await mouseTo(page, cardRow(page, first.id), 900, 60);
  assert.deepStrictEqual(litRows(page), [`side-cards:${first.id}`], "the card under the mouse is not the one selected");
  assert.deepStrictEqual(litCards(page), [], "the card under the mouse was highlighted");
  for (const id of [first.id, second.id]) {
    assert.equal(cardOf(page, id).getAttribute("aria-expanded"), "false", "a card does not say it is contracted");
  }
  await press(page, cardRow(page, first.id), "j");
  assert.deepStrictEqual(litRows(page), [`side-cards:${second.id}`]);
  assert.deepStrictEqual(litCards(page), [], "the keys walking through the cards highlighted one");
});

test("`#277 thread-card-focus`: a click highlights a card and shows it whole without entering its thread; another card takes the highlight, the same one again puts it out, and the mouse and a redraw leave it", async () => {
  const { page, server, data } = await manyThreadColumns(4);
  const [a, b] = [data.threads[1].id, data.threads[2].id];
  await cardOf(page, a).click();
  await page.settle();
  assert.ok(cardsUp(page), "a click on a card entered its thread");
  assert.deepStrictEqual(litCards(page), [a], "a click on a card did not highlight it");
  assert.equal(cardOf(page, a).getAttribute("aria-expanded"), "true", "the highlighted card does not say it is expanded");
  assert.equal(cardOf(page, b).getAttribute("aria-expanded"), "false");
  // The mouse over another card selects it; the highlight stays where it was put.
  await mouseTo(page, cardRow(page, b), 900, 200);
  assert.deepStrictEqual(litRows(page), [`side-cards:${b}`]);
  assert.deepStrictEqual(litCards(page), [a], "the mouse moved the highlight");
  // A redraw of the cards — a reply moving another thread to the foot — keeps it on its thread's card.
  await liveReply(page, server, data.messages[0], "400");
  assert.equal(cardIds(page).at(-1), data.threads[0].id, "the thread that moved did not go to the foot, so nothing was redrawn");
  assert.deepStrictEqual(litCards(page), [a], "a redraw of the cards lost the highlight");
  assert.equal(cardOf(page, a).getAttribute("aria-expanded"), "true");
  // Another card pressed takes it; the same one pressed again puts it out.
  await cardOf(page, b).click();
  await page.settle();
  assert.deepStrictEqual(litCards(page), [b], "another card did not take the highlight");
  assert.equal(cardOf(page, a).getAttribute("aria-expanded"), "false");
  await cardOf(page, b).click();
  await page.settle();
  assert.deepStrictEqual(litCards(page), [], "the highlighted card pressed again did not put the highlight out");
  assert.ok(cardsUp(page));
});

test("`#277 thread-card-focus`: while a card is highlighted every other card's edge and unread count, and every other link between the columns, go grey, and the highlighted card's link is drawn over them", async () => {
  const others = ".side-cards:has(> .side-card-row[data-highlighted]:not([hidden]):not(.search-hidden)) > .side-card-row:not([data-highlighted])";
  // The cards' rules stand outside the two columns' block, for the thread list's too (`#281 view-keys`).
  assert.match(cssBlockOutside(TWO_COLUMN_QUERY, others), /border-left-color:\s*var\(--thread-grey\)/, "the other cards keep their colours");
  assert.match(cssBlockOutside(TWO_COLUMN_QUERY, `${others} .side-card-unread`), /color:\s*var\(--muted\)/,
    "the other cards' unread counts keep the accent");
  assert.match(cssBlockIn(TWO_COLUMN_QUERY, "#column-links .column-link[data-dimmed]"), /stroke:\s*var\(--thread-grey\)/,
    "the other links keep their colours");
  assert.match(cssBlockIn(TWO_COLUMN_QUERY, "#column-links .column-link-marks[data-dimmed]"), /fill:\s*var\(--thread-grey\)/);
  // A token of each scheme, as this page's colours are: one declared for a single scheme draws nothing in the other.
  assert.equal((CSS_CODE.match(/--thread-grey:\s*#[0-9a-f]{6};/g) || []).length, 2, "the grey is not declared for both schemes");
  const restore = layColumnsOut();
  try {
    const { page } = await linkedColumns(16);
    const left = page.el("scroll-area");
    const right = page.el("side-scroll");
    left.scrollTop = left.scrollHeight;
    right.scrollTop = right.scrollHeight;
    await left.dispatch("scroll");
    await right.dispatch("scroll");
    await page.nextFrame();
    const before = drawnLinks(page);
    assert.ok(before.length >= 2, `only ${before.length} links are drawn, so this proves nothing`);
    assert.deepStrictEqual(before.filter((link) => link.dimmed), [], "a link is grey with no card highlighted");
    const number = (thread) => Number(thread.replace(/^.*t/, ""));
    const lit = before[1].thread;
    await threadCard(page, number(lit)).children[0].click();
    await page.settle();
    await page.nextFrame();
    const after = drawnLinks(page);
    assert.ok(after.length >= 2);
    for (const link of after) {
      if (link.thread === lit) {
        assert.equal(link.dimmed, false, "the highlighted card's link is grey");
        assert.equal(link.selected, true, "the highlighted card's link is not drawn heavy");
      } else {
        assert.equal(link.dimmed, true, `the link of ${link.thread} is not grey beside the highlighted card`);
        assert.equal(link.selected, false, `the link of ${link.thread} is drawn heavy beside the highlighted card`);
      }
    }
    assert.equal(after.at(-1).thread, lit, "the highlighted card's link is not drawn over the others");
    // The mouse on another card: its link stays grey, and the highlighted card's stays heavy.
    const other = after.find((link) => link.thread !== lit).thread;
    await mouseTo(page, threadCard(page, number(other)), 900, 300);
    assert.deepStrictEqual(litRows(page), [`side-cards:${other}`]);
    await page.nextFrame();
    assert.deepStrictEqual(drawnLinks(page).filter((link) => !link.dimmed).map((link) => link.thread), [lit],
      "the card under the mouse took the highlighted card's link");
    // Put out: every link in its colour again.
    await threadCard(page, number(lit)).children[0].click();
    await page.settle();
    await page.nextFrame();
    assert.deepStrictEqual(drawnLinks(page).filter((link) => link.dimmed), [], "a link stayed grey with the highlight put out");
  } finally {
    restore();
  }
});

test("`#277 thread-card-focus`: a card's \"N replies · M unread\" is a button of its own, and the way into its thread", async () => {
  const { page, data } = await twoColumnServer();
  const [first] = data.threads;
  const open = openerOf(cardRow(page, first.id));
  assert.ok(open, "a card has no way into its thread of its own");
  assert.equal(open.getAttribute("type"), "button");
  assert.notEqual(open, cardOf(page, first.id), "the way in is the card's own button, which highlights it");
  assert.match(open.text(), /^1 reply( · 1 unread)?$/, `the way in says ${open.text()}`);
  assert.equal(open.getAttribute("aria-label"), `Open this thread, ${open.text()}`,
    "a screen reader is not told the button opens the thread, in the words it shows");
  assert.match(cardPart(page, first.id, "side-card-facts"), /^(now|\d+[mhdwy]|\d+d\d+h)$/,
    "the card's facts still give the count its button gives");
  await open.click();
  await page.settle();
  assert.deepStrictEqual(sideIds(page), ["201", "202"], "the replies button did not enter its thread");
  assert.equal(page.el("side-close").hidden, false);
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "entering a card's thread changed the left column");
});

test("`#277 thread-card-focus`: → enters the highlighted card's thread, or with none the card the keys are on; ← comes back to the cards on that card, and Esc puts the highlight out", async () => {
  const { page, data } = await twoColumnServer();
  const [first, second] = data.threads;
  page.el("scroll-area").focus();
  await press(page, page.el("scroll-area"), "]");
  await press(page, focused(page), "j");
  assert.equal(focused(page), cardRow(page, first.id), `j did not select the first card: ${focusName(page)}`);
  // Enter on a card is a press on it: highlighted, and not entered.
  await press(page, focused(page), "Enter");
  assert.ok(cardsUp(page), "Enter on a card entered its thread");
  assert.deepStrictEqual(litCards(page), [first.id], "Enter on a card did not highlight it");
  // The keys move on; → goes into the highlighted card's thread.
  await press(page, focused(page), "j");
  assert.equal(focused(page), cardRow(page, second.id));
  await press(page, focused(page), "ArrowRight");
  await page.settle();
  assert.deepStrictEqual(sideIds(page), ["201", "202"], "→ did not enter the highlighted card's thread");
  assert.ok(focused(page) === page.el("side-scroll") || page.el("side-log").children.includes(focused(page)),
    `→ did not take the keys into the thread: ${focusName(page)}`);
  // ← out of a thread entered from a card: back to the cards, the keys on the card entered, still highlighted.
  await press(page, focused(page), "ArrowLeft");
  await page.settle();
  assert.ok(cardsUp(page), "← from a thread entered from its card did not come back to the cards");
  assert.equal(focused(page), cardRow(page, first.id), `← did not come back to the card entered: ${focusName(page)}`);
  assert.deepStrictEqual(litCards(page), [first.id], "coming back out lost the highlight");
  // Esc puts the highlight out and leaves the cards up.
  await press(page, focused(page), "Escape");
  await page.settle();
  assert.deepStrictEqual(litCards(page), [], "Esc did not put the highlight out");
  assert.ok(cardsUp(page));
  // With none highlighted, → enters the card the keys are on.
  await press(page, focused(page), "j");
  assert.equal(focused(page), cardRow(page, second.id));
  await press(page, focused(page), "ArrowRight");
  await page.settle();
  assert.deepStrictEqual(sideIds(page), ["203", "204"], "→ did not enter the thread of the card the keys are on");
  // Esc there is the X, as it was: back to the cards, on that card.
  await press(page, focused(page), "Escape");
  await page.settle();
  assert.ok(cardsUp(page), "Esc in a thread entered from its card did not come back to the cards");
  // A thread selected from Main is left by ← for Main, as it was.
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  await press(page, page.el("scroll-area"), "]");
  assert.ok(focused(page) === page.el("side-scroll") || page.el("side-log").children.includes(focused(page)));
  await press(page, focused(page), "ArrowLeft");
  assert.ok(focused(page) === page.el("scroll-area") || page.el("discord-log").children.includes(focused(page)),
    `← from a thread selected from Main did not go back to Main: ${focusName(page)}`);
  assert.deepStrictEqual(sideIds(page), ["201", "202"], "← closed a thread selected from Main");
});

test("`#277 thread-card-focus`: → on a card's own button, where Tab leaves the focus, enters that card's thread, whichever card is highlighted", async () => {
  const { page, data } = await twoColumnServer();
  const [first, second] = data.threads;
  await cardOf(page, first.id).click();
  await page.settle();
  assert.deepStrictEqual(litCards(page), [first.id]);
  await press(page, openerOf(cardRow(page, second.id)), "ArrowRight");
  await page.settle();
  assert.deepStrictEqual(sideIds(page), ["203", "204"], "→ on a card's replies button entered the highlighted card's thread instead");
});

test("`#277 thread-card-focus`: Esc with no card highlighted leaves the cards where the reader has them", async () => {
  const { page } = await manyThreadColumns(16);
  const area = page.el("side-scroll");
  area.scrollTop = 120;
  await area.dispatch("scroll");
  const card = firstCardInView(page);
  await press(page, card, "Escape");
  await page.settle();
  assert.ok(cardsUp(page));
  assert.equal(area.scrollTop, 120, "Esc on the cards moved them");
});

test("`#277 thread-card-focus`: the Threads list in one column is the same — contracted, a click highlights a card and greys the rest, its replies button or → enters it, ← comes back to it, Esc puts it out", async () => {
  const others = "#thread-list:has(> .thread-card[data-highlighted]:not([hidden]):not(.search-hidden)) > .thread-card:not([data-highlighted])";
  assert.match(cssBlock(others), /border-inline-start-color:\s*var\(--thread-grey\)/, "the other cards keep their colours");
  assert.match(cssBlock(".thread-card-text"), /display:\s*none/, "a card that is not highlighted shows what its thread is about");
  assert.match(cssBlock(".thread-card[data-highlighted] .thread-card-text"), /display:\s*block/, "a highlighted card does not show it");
  const page = await threadPage();
  const [first, second] = threadData().threads;
  await page.el("channel-view-threads").click();
  await page.settle();
  const list = page.el("thread-list");
  const row = (id) => list.children.find((li) => li.getAttribute("data-context-id") === id);
  assert.equal(list.hidden, false, "the Threads list is not on screen");
  assert.equal(row(first.id).children[0].getAttribute("aria-expanded"), "false");
  await row(first.id).children[0].click();
  await page.settle();
  assert.equal(list.hidden, false, "a click on a card entered its thread");
  assert.deepStrictEqual(litCards(page, "thread-list"), [first.id], "a click did not highlight the card");
  assert.equal(row(first.id).children[0].getAttribute("aria-expanded"), "true");
  // → enters it, and ← comes back to the list, the card still highlighted.
  page.el("scroll-area").focus();
  await press(page, page.el("scroll-area"), "ArrowRight");
  await page.settle();
  assert.equal(page.el("thread-select").value, `thread:${first.id}`, "→ did not enter the highlighted card's thread");
  await press(page, page.el("scroll-area"), "ArrowLeft");
  await page.settle();
  assert.equal(page.el("thread-list").hidden, false, "← did not come back to the Threads list");
  assert.deepStrictEqual(litCards(page, "thread-list"), [first.id], "coming back lost the highlight");
  // Esc puts it out.
  await press(page, page.el("scroll-area"), "Escape");
  assert.deepStrictEqual(litCards(page, "thread-list"), [], "Esc did not put the highlight out");
  assert.equal(page.el("thread-list").hidden, false);
  // The other card's replies button enters its thread.
  const open = openerOf(row(second.id), "thread-card-open");
  assert.ok(open, "a card of the Threads list has no way into its thread of its own");
  assert.match(open.text(), /^(about )?1 reply/);
  await open.click();
  await page.settle();
  assert.equal(page.el("thread-select").value, `thread:${second.id}`, "the replies button did not enter its thread");
});

test("`#277 thread-card-focus`: the list of keys says Enter highlights a card, → enters the highlighted card's thread, and Esc puts the highlight out", async () => {
  const page = newPage();
  await signIn(page);
  const keys = page.el("keyboard-keys").text();
  assert.match(keys, /On a thread card, highlight it and show all of it/, "the list of keys does not say what Enter does to a card");
  assert.match(keys, /On the thread cards, into the highlighted card's thread/, "the list of keys does not say what → does on the cards");
  assert.match(keys, /takes the highlight off a thread card/, "the list of keys does not say Esc puts a highlight out");
});

test("`#280 scroll-stability` x `#277 thread-card-focus`: a card highlighted by a click under the mouse glides Main to its root while the mouse rests on it; the mouse crossing other cards or moving on, and Enter on a card, glide nothing", async () => {
  // The highlighted card is what the glide follows (`#277 thread-card-focus`), and only the mouse resting
  // on it starts the wait (`#280 scroll-stability`): the owner's jump with nothing touched was the mouse
  // crossing the cards, and a highlight must not bring it back.
  const restore = layColumnsOut();
  try {
    /** Main at its head, the cards at their foot: the last two cards on screen, their roots far below Main's foot. */
    const cardsAtFoot = async (script = SCRIPT) => {
      const { page } = await linkedColumns(16, null, script);
      const left = page.el("scroll-area");
      const right = page.el("side-scroll");
      left.scrollTop = 0;
      await left.dispatch("scroll");
      right.scrollTop = right.scrollHeight;
      await right.dispatch("scroll");
      await page.nextFrame();
      for (const i of [14, 15]) {
        assert.ok(!inColumnView(left, rootLinkY(rootRow(page, i))), `t${i}'s root is on screen, so this proves nothing`);
        assert.ok(inColumnView(right, cardLinkY(threadCard(page, i))), `t${i}'s card is off screen, so this proves nothing`);
      }
      return { page, left };
    };
    const armed = (page) => [...page.timers.values()].filter((timer) => timer.ms === COLUMN_GLIDE_MS).length;
    /** The mouse onto card `t<i>`'s button at (`x`, `y`), and a click there, the mouse staying. */
    const clickCard = async (page, i, x, y) => {
      const button = threadCard(page, i).children[0];
      await mouseTo(page, button, x, y);
      await page.documentEvent("pointerdown", { target: button, pointerType: "mouse" });
      await button.click();
      await page.settle();
    };
    /** Enter on card `t<i>` with the keys, the mouse having crossed it on the way to the dock. */
    const enterCard = async (page, i) => {
      await mouseTo(page, threadCard(page, i), 900, 300);
      await mouseTo(page, page.el("compose-text"), 5, 790);
      await pressHere(page, "[");
      await pressHere(page, "]");
      assert.deepStrictEqual(litRows(page), [`side-cards:spaces/A/threads/t${i}`], "] did not light the card, so this proves nothing");
      await pressHere(page, "Enter");
      await page.settle();
      assert.deepStrictEqual(litCards(page), [`spaces/A/threads/t${i}`], "Enter did not highlight the card, so this proves nothing");
    };

    // A click highlights the card under the mouse, and the mouse resting there glides Main to its root.
    let { page, left } = await cardsAtFoot();
    await clickCard(page, 15, 900, 300);
    assert.deepStrictEqual(litCards(page), ["spaces/A/threads/t15"], "the click did not highlight the card");
    assert.equal(armed(page), 1, "a card highlighted under the mouse armed no wait");
    page.expireTimers(COLUMN_GLIDE_MS);
    assert.deepStrictEqual((left.glides || []).map((glide) => glide.behavior), ["smooth"],
      "the mouse resting on the card it highlighted did not glide Main to its root");

    // Highlighted, the mouse goes on to another card and rests there: that card is selected, the
    // highlight stays, and nothing glides, though that card's root is off screen too.
    ({ page, left } = await cardsAtFoot());
    await clickCard(page, 15, 900, 300);
    await mouseTo(page, threadCard(page, 14), 900, 200);
    assert.deepStrictEqual(litRows(page), ["side-cards:spaces/A/threads/t14"], "the mouse did not select the other card, so this proves less");
    assert.deepStrictEqual(litCards(page), ["spaces/A/threads/t15"], "the mouse moved the highlight");
    assert.equal(armed(page), 0, "a wait outlived the mouse leaving the highlighted card, or the card beside it armed one");
    page.expireTimers(COLUMN_GLIDE_MS);
    assert.equal(left.glides, undefined, "the mouse resting on a card beside the highlighted one glided Main");
    assert.equal(left.scrollTop, 0, "Main moved with the mouse on another card");
    // Back onto the highlighted card, resting: that waits again, and glides.
    await mouseTo(page, threadCard(page, 15), 905, 302);
    assert.equal(armed(page), 1, "the mouse coming back to the highlighted card armed no wait");
    page.expireTimers(COLUMN_GLIDE_MS);
    assert.ok(left.glides && left.glides.length === 1, "the mouse resting on the highlighted card again did not glide Main");

    // Highlighted, and the mouse crossing on to the dock: nothing glides.
    ({ page, left } = await cardsAtFoot());
    await clickCard(page, 15, 900, 300);
    await mouseTo(page, page.el("compose-text"), 1100, 795);
    assert.equal(armed(page), 0, "the wait outlived the mouse moving on from the highlighted card");
    page.expireTimers(COLUMN_GLIDE_MS);
    assert.equal(left.glides, undefined, "the mouse leaving the highlighted card for the dock glided Main");
    assert.equal(left.scrollTop, 0, "Main moved with the mouse on the dock");

    // Enter highlighting a card glides nothing: the keys chose it, not a mouse resting on it.
    ({ page, left } = await cardsAtFoot());
    await enterCard(page, 15);
    assert.equal(armed(page), 0, "Enter on a card armed a glide of Main");
    page.expireTimers(COLUMN_GLIDE_MS);
    assert.equal(left.glides, undefined, "Enter on a card glided Main to its root");
    assert.equal(left.scrollTop, 0, "Main moved for a key");

    // THE CONTROL: a highlight that arms whoever made it glides on Enter, so the test can tell.
    const loose = await cardsAtFoot(brokenScript(
      "if (litCards.get(channel) === id && inputKind === \"mouse\" && pointerOn(row)) dwellColumnGlide(pane, row);",
      "if (litCards.get(channel) === id) dwellColumnGlide(pane, row);"));
    await enterCard(loose.page, 15);
    loose.page.expireTimers(COLUMN_GLIDE_MS);
    assert.ok(loose.left.glides && loose.left.glides.length === 1, "the control did not glide, so the test cannot tell");
  } finally {
    restore();
  }
});

// --- the card at the top of an open thread (#298 thread-card-header) --------------------------------
//
// The owner, 2026-10-14: "When I drill into a given thread card to see the messages inside it, I would
// like to KEEP that same style ... I would like to see the SAME colorful card move up to the top of the
// screen and KEEP its connector linkage over to the main channel message on the left column ... We can
// still sneak in an X button somewhere for a mouse-only exit back to all-threads view." These say what
// the page does with the card at the top of a thread on the right; tests/thread_card_header_browser.py
// measures it in Chromium at a desk, where the stylesheet's fixed height and its colours are drawn.

test("`#298 thread-card-header`: the card at the top of a thread is in the right column's heading, outside its scroller, a list card by its classes and its look, with the X in it", () => {
  assertMarkupContains("side-heading", "side-thread-card");
  assert.ok(!markupHolds("side-scroll", "side-thread-card"), "the card is inside the scroller, where it would scroll away");
  for (const part of ["side-thread-card-text", "side-thread-card-age", "side-thread-card-count", "side-close"]) {
    assertMarkupContains("side-thread-card", part);
  }
  const tag = /<div\s+id="side-thread-card"[^>]*>/.exec(HTML_CODE)[0];
  assert.match(tag, /class="side-card-row thread-card-header"/, "the card at the top is not drawn by the list card's rules");
  assert.match(tag, /(?:^|\s)hidden(?=[\s>])/, "the card ships on screen, over the cards");
  assert.doesNotMatch(tag, /role=|aria-label=/, "the card is a region of its own beside the column's one");
  assert.match(/<button\s+id="side-thread-card-text"[^>]*>/.exec(HTML_CODE)[0], /class="side-card"/);
  assert.match(/<button\s+id="side-close"[^>]*>/.exec(HTML_CODE)[0], /aria-label="Close thread"/);
  assert.doesNotMatch(HTML_CODE, /id="side-facts"/, "the old facts are still in the heading beside the card");
  // Ringed and tinted as a highlighted card is — by its own rule, never `data-highlighted`'s, which
  // unclamps a card's text — and grey for a thread nobody has answered.
  const look = cssBlockOutside(TWO_COLUMN_QUERY, ".side-card-row.thread-card-header");
  const lit = cssBlockOutside(TWO_COLUMN_QUERY, ".side-card-row[data-highlighted]");
  assert.match(look, /border-color:\s*hsl\(var\(--thread-hue, 210\) 55% 50%\)/, "the card at the top is not ringed in its colour");
  const background = (rule) => (/(?:^|[;{\s])background:\s*([^;]+);/.exec(rule) || [])[1];
  assert.ok(background(lit), "the highlighted card has no tint to compare with");
  assert.equal(background(look), background(lit), "the card at the top is not tinted as a highlighted card");
  assert.match(cssBlockOutside(TWO_COLUMN_QUERY, '.side-card-row.thread-card-header[data-thread-state="start"]'),
    /border-color:\s*var\(--thread-grey\)/, "a thread nobody has answered spends a colour on its card");
  assert.deepStrictEqual(cssRules(CSS_CODE, ".thread-card-header[data-highlighted]"), []);
  // Fitted as a list card is (`#482 thread-card-fit`): the list card's text, at most the reader's
  // clamped lines, and an X of a fixed size on the facts line, which the count beside it is shorter than.
  assert.match(cssBlockOutside(TWO_COLUMN_QUERY, ".side-card-text"), /max-height:\s*calc\(max\(1, var\(--collapsed-lines\)\) \* 1\.35em\)/);
  assert.match(cssBlockOutside(TWO_COLUMN_QUERY, ".thread-card-close"), /(^|[^-])height:\s*1\.75rem/);
  // From review, larger type on a phone: the facts line no longer fits on one line at 150% type on a
  // 360px phone, and the age, the only column that could give way, was squeezed under its own words
  // and drawn over the count, while an X sized in rem grew off the screen. The age is never narrower
  // than its words; the count gives way, wrapping its parts whole, two of its lines being the facts
  // line's least height so that wrapping moves nothing; and the X is a thumb's 44px at any type size.
  // tests/thread_card_header_browser.py measures it at 130% and 150% type.
  assert.match(look, /grid-template-columns:\s*minmax\(min-content, 1fr\) minmax\(min-content, auto\) auto;/,
    "the age can be squeezed under its own words");
  assert.match(look, /grid-template-rows:\s*auto minmax\(1\.75rem, auto\);/, "the facts line has no least height for two lines of count");
  const count = cssBlockOutside(TWO_COLUMN_QUERY, ".thread-card-count");
  assert.doesNotMatch(count, /white-space:\s*nowrap/, "the count cannot give way");
  assert.match(count, /line-height:\s*calc\(1\.75rem \/ 2\);/, "two lines of the count are not the facts line's least height");
  assert.match(cssBlockOutside(TWO_COLUMN_QUERY, ".thread-card-count > span"), /display:\s*inline-block;/,
    "the count's parts do not wrap whole");
  assert.match(CSS_CODE, /@media \(pointer: coarse\) \{\s*\.thread-card-close \{\s*width: 44px;\s*height: 44px;\s*\}\s*\}/,
    "the X under a finger is not 44px whatever the type size");
  assert.ok(cssRules(CSS_CODE, ".thread-card-close").every((rule) => !/rem/.test(rule) || /1\.75rem/.test(rule)),
    "the X under a finger grows with the type");
  // `#388 thread-header-collapse`. Folded, a bar of its own edge and tint, 14px from edge to edge, with
  // nothing in it but its text's button: out of sight, and 6px past the bar above and below, the bar's
  // 24px press target. tests/thread_header_collapse_browser.py measures it in Chromium.
  const folded = cssBlockOutside(TWO_COLUMN_QUERY, ".side-card-row.thread-card-header[data-folded]");
  assert.match(folded, /(^|[^-])height:\s*14px/, "the bar is not 14px tall");
  assert.match(folded, /(^|[^-])padding:\s*0/);
  assert.doesNotMatch(folded, /(^|[;{\s])(border|background)(-[a-z-]+)?:/, "the bar is not drawn in the card's own edge and tint");
  assert.match(cssBlockOutside(TWO_COLUMN_QUERY, ".thread-card-header[data-folded] > :not(.side-card)"), /display:\s*none/);
  assert.match(cssBlockOutside(TWO_COLUMN_QUERY, ".thread-card-header[data-folded] > .side-card"), /inset:\s*-6px 0/,
    "the bar's press target is not 6px past it above and below");
  // Over a thread the heading gives up its rule and its least height, the card having its border, and
  // holds the card at its top.
  const heading = cssBlockOutside(TWO_COLUMN_QUERY, "#side-heading:has(> #side-thread-card:not([hidden]))");
  assert.match(heading, /border-bottom:\s*0/);
  assert.match(heading, /align-items:\s*flex-start/);
  // In two columns it stands where the cards do, for the line from its root.
  for (const selector of ['#screen-main[data-columns="two"] .thread-card-header', '#screen-main[data-columns="two"] .side-cards']) {
    assert.match(cssBlockIn(TWO_COLUMN_QUERY, selector), /margin-inline-start:\s*2\.5rem/, `${selector} is not inset for the links' lanes`);
  }
});

test("`#298 thread-card-header`: every way into a thread on the right puts its card at the top — its card's replies button, its Thread(N), the picker, → on its card — saying what its card says, in its colour", async () => {
  const { page, data } = await twoColumnServer((_page, held) => {
    Object.assign(held.threads[0], { display_name: "runner-wedged", summary: "The overnight runner hung and was restarted by hand." });
  });
  const [first, second] = data.threads;
  const said = listCardSays(page, first.id);
  assert.equal(said.text, "runner-wedged: The overnight runner hung and was restarted by hand.");
  assert.equal(headerCard(page).shown, false, "the card at the top is over the cards");
  const ways = [
    ["its card's replies button", () => openerOf(cardRow(page, first.id)).click()],
    ["its Thread(N)", () => threadBadge(rowWithId(page, "201")).click()],
    ["the picker", async () => {
      page.el("thread-select").value = `thread:${first.id}`;
      await page.el("thread-select").dispatch("change");
    }],
    ["→ on its card", async () => {
      await mouseTo(page, cardRow(page, first.id), 900, 60);
      await press(page, cardRow(page, first.id), "ArrowRight");
    }],
  ];
  for (const [way, enter] of ways) {
    if (!cardsUp(page)) {
      await page.el("side-close").click();
      await page.settle();
    }
    await enter();
    await page.settle();
    assert.deepStrictEqual(sideIds(page), ["201", "202"], `${way} did not open the thread on the right`);
    const header = headerCard(page);
    assert.equal(header.shown, true, `${way}: no card at the top of the thread`);
    assert.equal(header.thread, first.id);
    assert.equal(header.text, said.text, `${way}: the card at the top does not say what its card says`);
    assert.equal(header.title, said.text, `${way}: the whole text is not the card's tooltip`);
    assert.equal(header.age, said.age, `${way}: the card at the top does not say how long since the thread moved`);
    assert.equal(header.count, said.count, `${way}: the card at the top does not count what its card counts`);
    assert.equal(header.unread, "1 unread", `${way}: the unread is not drawn in the accent`);
    assert.equal(header.hue, said.hue, `${way}: the card at the top is not its card's colour`);
    assert.equal(header.state, "open");
    assert.equal(header.highlighted, false, "the card at the top carries the highlight that unclamps a card");
    assert.equal(page.el("side-title").hidden, true, `${way}: "Thread list" is still over the thread`);
    assert.equal(page.el("side-close").hidden, false);
    assert.equal(page.el("side-scroll").getAttribute("aria-label"), "Thread: runner-wedged", `${way}: the region lost the thread's name`);
  }
  // From one thread to another: the card says the other's.
  await threadBadge(rowWithId(page, "203")).click();
  await page.settle();
  assert.equal(headerCard(page).thread, second.id);
  assert.equal(headerCard(page).text, listCardSays(page, second.id).text);
  assert.equal(headerCard(page).hue, listCardSays(page, second.id).hue);
  // Back to the cards, and the card goes with the thread.
  await page.el("side-close").click();
  await page.settle();
  assert.equal(headerCard(page).shown, false, "the card at the top stayed over the cards");
  assert.equal(page.el("side-title").hidden, false);
});

test("`#284 redraw-keeps-pane`: in two columns, with the focus lost while the reader is in the right column, a redraw of Main leaves the keys there — Esc still closes the right column's thread — and a redraw of the right column gives its own row the focus back", async () => {
  const FIX = "    if (had || (lost && pane === active)) row.focus({ preventScroll: true });\n";
  const run = async (script, redraw) => {
    const { page, server, data } = await twoColumnServer(null, script);
    const [first] = data.threads;
    // A selection drawn in Main, the focus on it, as the mouse over a message leaves it.
    await mouseTo(page, rowWithId(page, "200"), 200, 40);
    assert.deepStrictEqual(litRows(page), ["discord-log:200"], "the premise: Main's message is not selected");
    assert.equal(selectedId(page), "200", "the premise: the focus is not on Main's message");
    // Into a thread on the right by its card's replies button, which gives the keys to that column
    // (`#298 thread-card-header`); then a click on the card at its top where it takes no focus, its age,
    // and Chromium puts the focus on the body.
    await openerOf(cardRow(page, first.id)).click();
    await page.settle();
    assert.equal(page.el("side-pane").hidden, false, "the premise: the thread did not open on the right");
    await page.documentEvent("pointerdown", { target: page.el("side-thread-card-age"), pointerType: "mouse" });
    await page.loseFocus();
    assert.equal(focused(page), null, `the premise: ${focusName(page)} holds the focus`);
    const before = rowWithId(page, "200");
    if (redraw === "main") {
      // A live message in Main, which draws Main again.
      await deliver(page, page.stream(), sseMessage(server.post(message({ id: "209", content: "a new message in Main" }))));
      assert.ok(rowWithId(page, "209"), "the premise: the live message was not drawn in Main");
      assert.notEqual(rowWithId(page, "200"), before, "the premise: Main was not drawn again, so this proves nothing");
    }
    const held = focused(page);
    const where = held === null ? "nothing" : nodeWithinFixture(page.el("scroll-area"), held) ? "Main" : "elsewhere";
    // Main's selection is kept on its message's new row, for when the reader goes back there.
    const kept = rowWithId(page, "200").getAttribute("tabindex") === "0";
    await pressHere(page, "Escape");
    await page.settle();
    return { where, kept, closed: cardsUp(page) };
  };
  // The premise: with nothing redrawn, Esc with the focus on nothing acts in the right column.
  const still = await run(SCRIPT, null);
  assert.equal(still.closed, true, "with nothing redrawn Esc did not close the right column's thread, so this proves nothing");
  const redrawn = await run(SCRIPT, "main");
  assert.equal(redrawn.where, "nothing", `a redraw of Main gave the focus to ${redrawn.where}, a pane the reader was not in`);
  assert.equal(redrawn.closed, true, "after a redraw of Main, Esc no longer closed the right column's thread");
  assert.equal(redrawn.kept, true, "a redraw of Main beside the reader lost Main's selection");
  // The control: the focus given back in whichever pane was redrawn, as before. Main takes it, and with it
  // the keys, and Esc there leaves the right column's thread open.
  const anywhere = await run(brokenScript(FIX, "    if (lost) row.focus({ preventScroll: true });\n"), "main");
  assert.equal(anywhere.where, "Main", "the negative control did not give Main the focus, so the check above proves nothing");
  assert.equal(anywhere.closed, false, "the negative control closed the thread anyway, so the check above proves nothing");

  // In the right column, the focus lost and the column drawn again under the reader — a live reply in its
  // thread — the same message's new row takes it back there, as a redraw of the one column always did.
  const { page, server, data } = await twoColumnServer();
  const thread = data.messages[1].thread;
  await openerOf(cardRow(page, data.threads[0].id)).click();
  await page.settle();
  await mouseTo(page, sideRow(page, "202"), 900, 120);
  assert.equal(selectedId(page), "202", "the premise: the right column's reply is not selected");
  const old = sideRow(page, "202");
  await page.loseFocus();
  await deliver(page, page.stream(), sseMessage(server.post(message({ id: "205", content: "a second answer", thread: { ...thread, is_root: false } }))));
  assert.notEqual(sideRow(page, "202"), old, "the premise: the right column was not drawn again, so this proves nothing");
  assert.equal(focused(page), sideRow(page, "202"), `a redraw of the right column under its reader left the focus on ${focusName(page)}`);
});

test("`#298 thread-card-header`: the card at the top counts the thread's replies and unread as they change — a live reply, a Done — and a pass that changes nothing writes nothing to it", async () => {
  const run = async (script) => {
    const page = newPage(new Map(), script);
    const data = threadData();
    page.threadingSupported = true;
    page.threads = data.threads;
    page.messages = data.messages;
    const server = forwardTimeline(page);
    await signIn(page);
    await showDiscord(page, data.messages);
    await twoColumns(page);
    await openerOf(cardRow(page, data.threads[0].id)).click();
    await page.settle();
    const counted = [headerCard(page).count];
    const first = data.messages[1].thread;
    const reply = server.post(message({ id: "205", content: "a second answer", thread: { ...first, is_root: false } }));
    await deliver(page, page.stream(), sseMessage(reply));
    await page.settle();
    counted.push(headerCard(page).count);
    await doneButton(sideRow(page, "202")).click();
    await page.settle();
    counted.push(headerCard(page).count);
    // A poll that brings nothing, and the rows passed over again: nothing written to the card.
    const count = page.el("side-thread-card-count");
    const words = page.el("side-thread-card-text").children[0];
    const replace = count.replaceChildren;
    let writes = 0;
    count.replaceChildren = (...kids) => {
      writes += 1;
      return replace.call(count, ...kids);
    };
    const write = FakeElement.prototype.setAttribute;
    FakeElement.prototype.setAttribute = function counted(name, value) {
      if (this === page.el("side-thread-card") || this === page.el("side-thread-card-text")) writes += 1;
      return write.call(this, name, value);
    };
    const text = words.textContent;
    try {
      page.expireTimers(DISCORD_POLL_MS);
      await page.settle();
      await page.el("search-toggle").click();
      await page.el("search-toggle").click();
      await page.settle();
    } finally {
      FakeElement.prototype.setAttribute = write;
    }
    assert.equal(words.textContent, text);
    return { counted, writes };
  };
  const { counted, writes } = await run(SCRIPT);
  assert.deepStrictEqual(counted, ["1 reply · 1 unread", "2 replies · 2 unread", "2 replies · 1 unread"],
    "the card at the top did not follow the live reply and the Done");
  assert.equal(writes, 0, "a pass that changed nothing wrote to the card at the top");
  // The control: drawn on every pass, the card is written to by a pass that changes nothing.
  const always = await run(brokenScript("  if (threadCardsDrawn.get(card.id) === signature) return;\n", ""));
  assert.ok(always.writes > 0, "the negative control wrote nothing either, so the check above proves nothing");
});

test("`#388 thread-header-collapse`: a press on the card at the top of the right column's thread — its text, its age, its count — folds it to a bar, and a press on the bar opens it; its X and a link in it do not; its text's button says aria-expanded; the choice is the device's, for every thread", async () => {
  const FOLD_KEY = "vibe-talk.voice.thread-card-folded";
  const run = async (script) => {
    const { page, data } = await twoColumnServer(null, script);
    const [first] = data.threads;
    await openerOf(cardRow(page, first.id)).click();
    await page.settle();
    const card = page.el("side-thread-card");
    // The browser hands the card the press on whatever in it was pressed: the fixture does not bubble.
    const pressOn = (target) => card.dispatch("click", { target });
    const seen = [headerCard(page)];
    await pressOn(page.el("side-thread-card-text"));
    seen.push({ ...headerCard(page), stored: page.storage.get(FOLD_KEY) });
    await pressOn(page.el("side-thread-card-text"));
    seen.push({ ...headerCard(page), stored: page.storage.get(FOLD_KEY) });
    await pressOn(page.el("side-thread-card-age"));
    seen.push(headerCard(page));
    await pressOn(page.el("side-thread-card-count"));
    seen.push(headerCard(page));
    // Its X, and a link in it, are not the card: neither folds it.
    const link = new FakeElement("", "a");
    card.append(link);
    await pressOn(link);
    seen.push(headerCard(page));
    card.removeChild(link);
    await pressOn(page.el("side-close"));
    seen.push(headerCard(page));
    return { page, seen };
  };
  const { page, seen } = await run(SCRIPT);
  const said = seen.map(({ folded, expanded, stored }) => (stored === undefined ? { folded, expanded } : { folded, expanded, stored }));
  assert.deepStrictEqual(said, [
    { folded: false, expanded: "true" },
    { folded: true, expanded: "false", stored: "on" },
    { folded: false, expanded: "true", stored: "off" },
    { folded: true, expanded: "false" },
    { folded: false, expanded: "true" },
    { folded: false, expanded: "true" },
    { folded: false, expanded: "true" },
  ], "a press on the card did not fold and open it, or one on its X or a link did");
  // Folded, the card is still the thread's, and so is the next thread's: one choice for every thread.
  await page.el("side-thread-card").dispatch("click", { target: page.el("side-thread-card-text") });
  await threadBadge(rowWithId(page, "203")).click();
  await page.settle();
  assert.equal(headerCard(page).thread, page.el("side-thread-card").getAttribute("data-thread"));
  assert.notEqual(headerCard(page).thread, seen[0].thread, "the badge did not open another thread, so this proves nothing");
  assert.equal(headerCard(page).folded, true, "another thread's card did not keep the fold");
  assert.equal(wideCard(page).folded, true, "the card across the window does not share the choice");
  // Its X, on the card opened again, closes the thread: the press goes to it, never to the card.
  await page.el("side-thread-card").dispatch("click", { target: page.el("side-thread-card-text") });
  await page.el("side-close").click();
  await page.settle();
  assert.ok(cardsUp(page), "the X did not close the thread");
  // The control: a card that takes every press inside it as its own folds on its X and on a link.
  const blind = await run(brokenScript(
    "if (tag === \"a\" || (tag === \"button\" && node !== leftEl(`${card.id}-text`))) return;", ""));
  assert.deepStrictEqual(blind.seen.slice(-2).map(({ folded }) => folded), [true, false],
    "the negative control did not fold on the link and open again on the X, so the check above proves nothing");
});

test("`#388 thread-header-collapse`: the fold is kept on the device — a reload draws the card at the top folded, in two columns and across the window; with nothing kept, open", async () => {
  const FOLD_KEY = "vibe-talk.voice.thread-card-folded";
  const reopened = async (stored, script = SCRIPT) => {
    const { page, data } = await twoColumnServer(null, script, new Map(stored === null ? [] : [[FOLD_KEY, stored]]));
    await openerOf(cardRow(page, data.threads[0].id)).click();
    await page.settle();
    const right = headerCard(page);
    return { right: [right.folded, right.expanded], wide: [wideCard(page).folded, wideCard(page).expanded] };
  };
  assert.deepStrictEqual(await reopened("on"), { right: [true, "false"], wide: [true, "false"] }, "the reload forgot the fold");
  assert.deepStrictEqual(await reopened("off"), { right: [false, "true"], wide: [false, "true"] });
  assert.deepStrictEqual(await reopened(null), { right: [false, "true"], wide: [false, "true"] }, "with nothing kept the card is folded");
  // The control: a page that reads nothing kept draws the card open after the reload.
  const forgot = await reopened("on", brokenScript("return deviceSettings().get(DEVICE_SETTINGS.threadCardFolded);", "return false;"));
  assert.deepStrictEqual(forgot.right, [false, "true"], "the negative control kept the fold too, so the check above proves nothing");
  // Across the window: a thread from the picker, the card read from the same choice.
  const page = await threadPage("main");
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  await page.el("thread-card").dispatch("click", { target: page.el("thread-card-age") });
  assert.equal(page.storage.get(FOLD_KEY), "on");
  assert.deepStrictEqual([wideCard(page).folded, wideCard(page).expanded], [true, "false"]);
  // A press on the X across the window is not the card's. (Folded, the X is not drawn: the stylesheet
  // hides everything in the bar but its button, which this fixture does not lay out.)
  await page.el("thread-card").dispatch("click", { target: page.el("thread-back") });
  assert.equal(wideCard(page).folded, true, "a press on the X across the window opened the card");
  await page.el("thread-card").dispatch("click", { target: page.el("thread-card-text") });
  await page.el("thread-back").click();
  await page.settle();
  assert.equal(page.el("thread-heading").hidden, true, "the X did not leave the thread");
  assert.equal(wideCard(page).folded, false, "leaving the thread folded the card");
});

test("`#298 thread-card-header`: over a thread on the right its one link joins its root on Main to the card at the top, as the selection's — whole, or clipped at Main's edge while the root is off it — and the cards' links come back with the cards", async () => {
  const restore = layColumnsOut();
  try {
    const { page } = await linkedColumns(16);
    const left = page.el("scroll-area");
    const right = page.el("side-scroll");
    // Another thread's card highlighted in the list: the open thread's link is not greyed beside it.
    await threadCard(page, 3).children[0].click();
    await page.settle();
    assert.deepStrictEqual(litCards(page), ["spaces/A/threads/t3"]);
    await openerOf(threadCard(page, 8)).click();
    await page.settle();
    await rootAt(page, 8, 120);
    let links = drawnLinks(page);
    assert.equal(links.length, 1, `over a thread the links are ${JSON.stringify(links.map((one) => one.thread))}, not its one`);
    let [link] = links;
    assert.equal(link.thread, "spaces/A/threads/t8");
    assert.equal(link.selected, true, "the thread's link is not drawn as the selection's");
    assert.equal(link.dimmed, false);
    assert.equal(link.clip, "both");
    const header = page.el("side-thread-card");
    const box = header.getBoundingClientRect();
    const row = rootRow(page, 8).getBoundingClientRect();
    const [fx, fy, lane, ty, end] = link.line;
    assert.equal(fx, row.right, "the link does not leave its root's right edge");
    assert.equal(fy, rootLinkY(rootRow(page, 8)), "the link does not leave its root level with its tag");
    assert.equal(end + LINK_HEAD_PX, box.left, "the link's arrowhead does not reach the card's left edge");
    assert.equal(link.marks[link.marks.length - 6], box.left, "the arrowhead's point is not on the card's edge");
    assert.equal(ty, (box.top + box.bottom) / 2, "the link does not arrive at the card's middle");
    assert.ok(lane > 708 && lane < box.left - LINK_HEAD_PX, `the link's lane at ${lane} is not in the gap`);
    assert.equal(link.hue, header.style.getPropertyValue("--thread-hue"), "the link is not the card's colour");
    // Main scrolled until the root is above it, then below it: clipped at its edge, pointing the way.
    for (const [top, way] of [[left.scrollHeight, "up"], [0, "down"]]) {
      left.scrollTop = top;
      await left.dispatch("scroll");
      await page.nextFrame();
      assert.ok(!inColumnView(left, rootLinkY(rootRow(page, 8))), `the root is on screen at ${top}, so this proves nothing`);
      [link] = drawnLinks(page);
      assert.deepStrictEqual([link.thread, link.clip, link.selected], ["spaces/A/threads/t8", way, true],
        `with the root ${way === "up" ? "above" : "below"} Main the link does not point ${way} to it`);
    }
    // Scrolling the thread moves nothing the link goes to.
    right.scrollTop = 0;
    await right.dispatch("scroll");
    await page.nextFrame();
    assert.deepStrictEqual(drawnLinks(page).map((one) => one.clip), ["down"]);
    // Back to the cards: their links, not the thread's.
    await page.el("side-close").click();
    await page.settle();
    left.scrollTop = left.scrollHeight;
    right.scrollTop = right.scrollHeight;
    await left.dispatch("scroll");
    await page.nextFrame();
    links = drawnLinks(page);
    assert.ok(links.length > 1, `the cards' links did not come back: ${JSON.stringify(links)}`);
    assert.ok(links.every((one) => !one.selected || one.thread !== "spaces/A/threads/t8"), "the thread's link outlived it");
  } finally {
    restore();
  }
});

test("`#298 thread-card-header`: the link to the card at the top goes from a collapsed run's line when the root is in the run, up from Main's top for a root older than Main holds, and nowhere for a root the search hides", async () => {
  const restore = layColumnsOut();
  try {
    // Fold read, every thread but the newest read: the roots in one run, under its line.
    const reads = manyThreads(16).messages.slice(0, 30).map((m) => String(m.id));
    let { page } = await linkedColumns(16, (p) => { p.dealtWith = new Set(reads); });
    await chooseReadMode(page, "Fold read");
    const head = page.el("discord-log").children.find((li) => li.getAttribute("data-read-run") === "head");
    assert.ok(head && rootRow(page, 3).hidden, "the root is not hidden in a collapsed run, so this proves nothing");
    page.el("thread-select").value = "thread:spaces/A/threads/t3";
    await page.el("thread-select").dispatch("change");
    await page.settle();
    page.el("scroll-area").scrollTop = 0;
    await page.el("scroll-area").dispatch("scroll");
    await page.nextFrame();
    let [link] = drawnLinks(page);
    assert.ok(link && link.thread === "spaces/A/threads/t3", "no link from the root in a collapsed run");
    const line = head.getBoundingClientRect();
    assert.ok(link.line[1] >= line.top && link.line[1] <= line.bottom, "the link does not leave the run's line");
    assert.equal(link.line[0], line.right);
    // The search hiding the root among Main's rows: no link at all; cleared, the link again.
    ({ page } = await linkedColumns(16));
    await openerOf(threadCard(page, 8)).click();
    await page.settle();
    await rootAt(page, 8, 120);
    assert.equal(drawnLinks(page).length, 1);
    await search(page, "thread 9 root");
    await page.settle();
    await page.nextFrame();
    assert.ok(rootRow(page, 8).hasClass("search-hidden"), "the search did not hide the root, so this proves nothing");
    assert.deepStrictEqual(drawnLinks(page), [], "a root the search hides is joined to the card at the top");
    await search(page, "");
    await page.settle();
    await page.nextFrame();
    assert.deepStrictEqual(drawnLinks(page).map((one) => one.thread), ["spaces/A/threads/t8"]);
    // A root older than every row Main holds: a stub from the card up to Main's top.
    page = newPage(new Map(), SCRIPT, (p) => p.enableAnimationFrames());
    const old = { id: "spaces/A/threads/old", root_message_id: "290", is_root: true, reply_count: 1, reply_count_exact: true };
    const oldRoot = message({ id: "290", content: "an old question", thread: old });
    const data = manyThreads(4);
    const oldAnswer = message({ id: "399", content: "a late answer", thread: { ...old, is_root: false } });
    page.threadingSupported = true;
    page.threads = [...data.threads, { id: old.id, root: oldRoot, title: "Old question", reply_count: 1,
      reply_count_exact: true, updated_at: oldAnswer.timestamp }];
    page.messages = [...data.messages, oldAnswer];
    forwardTimeline(page);
    await signIn(page);
    await showDiscord(page, page.messages);
    await twoColumns(page);
    await page.settle();
    page.el("thread-select").value = `thread:${old.id}`;
    await page.el("thread-select").dispatch("change");
    await page.settle();
    await page.nextFrame();
    assert.ok(!shownIds(page).includes("290"), "Main holds the old root, so this proves nothing");
    assert.equal(headerCard(page).text, "an old question");
    [link] = drawnLinks(page);
    assert.ok(link, "no link for a root older than Main holds");
    assert.deepStrictEqual([link.thread, link.clip, link.selected], [old.id, "up", true],
      "the link to a root older than Main holds does not point up from Main's top");
    const top = page.el("scroll-area").getBoundingClientRect().top;
    assert.equal(link.marks[1], top + 2, "the stub's arrowhead does not point at Main's top edge");
    // A root Main does not hold has no place to glide Main to.
    await mouseTo(page, page.el("side-thread-card"), 900, 40);
    page.expireTimers(COLUMN_GLIDE_MS);
    assert.equal(page.el("scroll-area").glides, undefined, "Main glided to a root it does not hold");
  } finally {
    restore();
  }
});

test("`#298 thread-card-header`: the mouse resting on the card at the top glides Main to its root, once each time the thread opens, smoothly or at once under reduced motion; a mouse passing over it glides nothing", async () => {
  const restore = layColumnsOut();
  try {
    for (const reduced of [false, true]) {
      const { page } = await linkedColumns(16, reduced ? (p) => p.preferReducedMotion() : null);
      const left = page.el("scroll-area");
      const header = page.el("side-thread-card");
      await openerOf(threadCard(page, 8)).click();
      await page.settle();
      left.scrollTop = 0;
      await left.dispatch("scroll");
      await page.nextFrame();
      assert.ok(!inColumnView(left, rootLinkY(rootRow(page, 8))), "the root is on screen, so this proves nothing");
      // Passing over: on it, and off to the dock before the wait is up.
      await mouseTo(page, header, 900, 40);
      await mouseTo(page, page.el("dock"), 600, 780);
      page.expireTimers(COLUMN_GLIDE_MS);
      assert.equal(left.glides, undefined, "a mouse passing over the card glided Main");
      // Resting on it.
      await mouseTo(page, header, 900, 40);
      assert.equal(page.expireTimers(COLUMN_GLIDE_MS), 1, "no wait was armed for the mouse resting on the card");
      assert.deepStrictEqual(left.glides.map((glide) => glide.behavior), [reduced ? "auto" : "smooth"],
        "Main did not glide to the card's root, as the reader's system asks");
      const box = header.getBoundingClientRect();
      // Level with the card, unless that would cut off the root's head, the date rule over it included,
      // at Main's top edge: then just low enough to show it whole (`#324 live-pill-covers-row`).
      const root = rootRow(page, 8);
      const whole = rootLinkY(root) - startOf(root) + left.getBoundingClientRect().top + LANDING_GAP_PX;
      assert.equal(rootLinkY(root), Math.max(glideLevel(left, (box.top + box.bottom) / 2), whole),
        "the root was not brought level with the card, or not whole");
      assert.ok(startOf(root) >= left.getBoundingClientRect().top, "the glide left the root's head above Main's top edge");
      await left.dispatch("scroll");
      await page.nextFrame();
      assert.deepStrictEqual(drawnLinks(page).map((one) => one.clip), ["both"], "the link is not whole once the root is on screen");
      // Once for this opening.
      left.scrollTop = 0;
      await left.dispatch("scroll");
      await mouseTo(page, header, 950, 60);
      page.expireTimers(COLUMN_GLIDE_MS);
      assert.equal(left.glides.length, 1, "the card glided Main twice in one opening");
      if (reduced) continue;
      // ...and once more for the next.
      await page.el("side-close").click();
      await page.settle();
      await openerOf(threadCard(page, 8)).click();
      await page.settle();
      left.scrollTop = 0;
      await left.dispatch("scroll");
      await mouseTo(page, header, 900, 40);
      page.expireTimers(COLUMN_GLIDE_MS);
      assert.equal(left.glides.length, 2, "the thread opened again did not let the card glide Main again");
    }
  } finally {
    restore();
  }
});

test("`#298 thread-card-header` x `#284 redraw-keeps-pane`: a click on a card's replies button gives the keys to its thread on the right, so a redraw of Main leaves them there and Esc closes it; a key pressed on the card at the top acts on the right column", async () => {
  const run = async (script) => {
    const page = newPage(new Map(), script);
    const data = threadData();
    page.threadingSupported = true;
    page.threads = data.threads;
    page.messages = data.messages;
    const server = forwardTimeline(page);
    await signIn(page);
    await showDiscord(page, data.messages);
    await twoColumns(page);
    page.el("scroll-area").focus();
    await openerOf(cardRow(page, data.threads[0].id)).click();
    await page.settle();
    const keys = focused(page) !== null && nodeWithinFixture(page.el("side-scroll"), focused(page));
    const posted = server.post(message({ id: "206", content: "to the channel itself" }));
    await deliver(page, page.stream(), sseMessage(posted));
    await page.settle();
    assert.ok(shownIds(page).includes("206"), "the live message did not redraw Main");
    await press(page, focused(page), "Escape");
    await page.settle();
    const closed = cardsUp(page);
    // A key on the card at the top, the keys given to Main first.
    if (!cardsUp(page)) {
      await page.el("side-close").click();
      await page.settle();
    }
    await openerOf(cardRow(page, data.threads[0].id)).click();
    await page.settle();
    await press(page, page.el("side-scroll"), "[");
    await press(page, page.el("side-thread-card-text"), "Escape");
    await page.settle();
    return { keys, closed, fromCard: cardsUp(page) };
  };
  assert.deepStrictEqual(await run(SCRIPT), { keys: true, closed: true, fromCard: true },
    "the keys did not follow a click into the thread on the right, or a key on its card went to Main");
  // The controls: the keys left where the click found them, and the heading's keys left to Main.
  const stayed = await run(brokenScript("  if (sideShowing()) focusList(sidePane);\n", ""));
  assert.equal(stayed.closed, false, "the negative control closed the thread anyway, so the check above proves nothing");
  const heading = await run(brokenScript(
    "    if (pane === sidePane && node && nodeWithin(leftEl(\"side-heading\"), node)) return pane;\n", ""));
  assert.equal(heading.fromCard, false, "the negative control closed the thread from its card anyway, so the check above proves nothing");
});

// --- the card moving up, and the same card over a thread across the window (#298, part 2) -------------
//
// The owner: "When we go to single column thread view Cmd-3 an drill in I'd like the same thing. One
// unified thread list view and one unified intra-thread view." And of the card: "I would like to see the
// SAME colorful card move up to the top of the screen ... So it just moves up and disgorges its internal
// messages underneath". tests/thread_card_header_browser.py watches the move in Chromium, frame by frame.

test("`#298 thread-card-header`: a thread across the window is headed by the same card, outside its scroller, with the X in it; two lines of it on a phone; and its move up is the stylesheet's, none under reduced motion", () => {
  assertMarkupContains("channel-navigation", "thread-heading");
  assertMarkupContains("thread-heading", "thread-card");
  assert.ok(!markupHolds("scroll-area", "thread-card"), "the card is inside the scroller, where it would scroll away");
  for (const part of ["thread-card-text", "thread-card-age", "thread-card-count", "thread-back"]) {
    assertMarkupContains("thread-card", part);
  }
  // The right column's card, part for part: the same classes, so the same rules draw both.
  const tagOf = (id) => /<[a-z]+\s+id="[^"]+"[^>]*>/.exec(HTML_CODE.slice(HTML_CODE.indexOf(`id="${id}"`) - 12))[0];
  const classes = (id) => (/class="([^"]*)"/.exec(tagOf(id)) || [])[1];
  for (const [wide, side] of [["thread-card", "side-thread-card"], ["thread-card-text", "side-thread-card-text"],
    ["thread-card-age", "side-thread-card-age"], ["thread-card-count", "side-thread-card-count"], ["thread-back", "side-close"]]) {
    assert.ok(classes(wide), `#${wide} has no classes`);
    assert.equal(classes(wide), classes(side), `#${wide} is not drawn by #${side}'s rules`);
  }
  assert.doesNotMatch(tagOf("thread-card"), /role=|aria-label=/, "the card is a region of its own");
  assert.match(tagOf("thread-back"), /aria-label="Close thread"/);
  for (const gone of ["thread-title", "thread-facts"]) {
    assert.doesNotMatch(HTML_CODE, new RegExp(`id="${gone}"`), `the old heading's #${gone} is still beside the card`);
  }
  assert.doesNotMatch(HTML_CODE, /← Back/, "the X is still a button saying Back");
  // The heading holds the card at its top, and draws no rule of its own under the card's border.
  assert.match(cssBlockOutside(TWO_COLUMN_QUERY, "#thread-heading"), /align-items:\s*flex-start/);
  assert.match(cssBlockOutside(TWO_COLUMN_QUERY, "#channel-navigation:has(> #thread-heading:not([hidden]))"), /border-bottom:\s*0/);
  // On a phone, two lines of it at most — the reader's count under that, one at least (`#425
  // collapsed-lines`) — and fitted below that most (`#482 thread-card-fit`).
  const phone = cssBlockIn("(max-width: 599.98px)", "#thread-card:not([data-folded]) > .side-card > .side-card-text");
  assert.match(phone, /-webkit-line-clamp:\s*max\(1, min\(var\(--collapsed-lines\), 2\)\)/);
  assert.match(phone, /max-height:\s*calc\(max\(1, min\(var\(--collapsed-lines\), 2\)\) \* 1\.35em\)/, "the phone's card is not held to two lines at most");
  assert.doesNotMatch(phone, /(^|[^-])height:\s*calc/, "the phone's card still reserves its two lines");
  // The move up: a transform from where the card in the list was, for as long as the stylesheet says,
  // with the thread fading in under it — nothing laid out — and none of it for a reader who asked for
  // less motion. The script reads the length, and asks no media query.
  assert.match(CSS_CODE, /#screen-main \{\s*--card-flight: 220ms;\s*\}/);
  assert.match(CSS_CODE, /@keyframes card-flight \{\s*from \{\s*transform: translate\(var\(--flight-x, 0px\), var\(--flight-y, 0px\)\);\s*\}\s*\}/,
    "the card does not start where its card in the list was, by a transform alone");
  assert.match(CSS_CODE, /@keyframes thread-arrive \{\s*from \{\s*opacity: 0;\s*\}\s*\}/);
  assert.match(cssBlockOutside(TWO_COLUMN_QUERY, ".thread-card-header[data-flying]"), /animation:\s*card-flight var\(--card-flight\)/);
  // `#472 gate-flake-hardening`. Held where it starts until it starts: a busy browser can draw a frame
  // between the move being set and its clock starting, and without the fill that frame showed the
  // card at its place, before it flew there from the list.
  assert.match(cssBlockOutside(TWO_COLUMN_QUERY, ".thread-card-header[data-flying]"),
    /animation:\s*card-flight var\(--card-flight\) cubic-bezier\([^)]*\) backwards;/,
    "the card at the top can be drawn at its place before it moves");
  assert.match(cssBlockOutside(TWO_COLUMN_QUERY, "#pane-discord[data-arriving]"), /animation:\s*thread-arrive /);
  assert.match(CSS_CODE, /@media \(prefers-reduced-motion: reduce\) \{\s*#screen-main \{\s*--card-flight: 0ms;\s*\}\s*\.thread-card-header\[data-flying\],\s*#pane-discord\[data-arriving\],\s*#side-pane\[data-arriving\] \{\s*animation: none;/,
    "the card moves, or the thread fades, for a reader who asked for less motion");
  assert.doesNotMatch(PAGE_CODE, /matchMedia|prefers-reduced-motion/, "web/voice.js grew a second opinion about motion");
});

test("`#298 thread-card-header`: every way into a thread across the window puts the same card at the top — the thread list's replies button, Thread(N) in Main and in All, the picker — and its X says where it goes and goes there, as Esc and ← do", async () => {
  const page = await threadPage("main");
  const first = page.threads[0].id;
  await pickThread(page, "threads");
  const said = listCardSays(page, first);
  assert.equal(said.text, "first thread root");
  const back = page.el("thread-back");
  const expect = (way, to) => {
    const card = wideCard(page);
    assert.equal(card.shown, true, `${way}: no card at the top of the thread`);
    assert.deepStrictEqual([card.thread, card.text, card.title, card.age, card.count, card.hue, card.state],
      [first, said.text, said.text, said.age, said.count, said.hue, "open"], `${way}: the card at the top is not the thread's card`);
    assert.equal(card.highlighted, false, `${way}: the card carries the highlight that unclamps a card`);
    assert.equal(back.getAttribute("title"), to, `${way}: the X does not say where it goes`);
  };
  // From the thread list: its X, Esc and ← each go back to the list.
  for (const out of ["the X", "Esc", "←"]) {
    await openerOf(cardRow(page, first)).click();
    await page.settle();
    expect(`the thread list (${out})`, "Back to the thread list");
    if (out === "the X") await back.click();
    else await press(page, page.el("scroll-area"), out === "Esc" ? "Escape" : "ArrowLeft");
    await page.settle();
    assert.equal(page.el("screen-main").getAttribute("data-columns"), "list", `${out} did not go back to the thread list`);
  }
  // Thread(N) in Main, then in All; the picker from Main.
  await pickThread(page, "main");
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  expect("Thread(N) in Main", "Back to Main");
  await back.click();
  await page.settle();
  assert.equal(page.el("thread-select").value, "main", "the X did not go back to Main");
  await pickThread(page, "flat");
  await threadBadge(rowWithId(page, "202")).click();
  await page.settle();
  expect("Thread(N) in All", "Back to All");
  await back.click();
  await page.settle();
  assert.equal(page.el("thread-select").value, "flat", "the X did not go back to All");
  await pickThread(page, "main");
  await pickThread(page, `thread:${first}`);
  expect("the picker", "Back to Main");
  // Another thread: the card is that one's.
  await pickThread(page, `thread:${page.threads[1].id}`);
  assert.equal(wideCard(page).text, "second thread root");
  assert.equal(wideCard(page).thread, page.threads[1].id);
});

test("`#298 thread-card-header`: the card across the window counts the thread's replies and unread as they change — a live reply, a Done — and a pass that changes nothing writes nothing to it", async () => {
  const run = async (script) => {
    const page = newPage(new Map(), script);
    const data = threadData();
    page.threadingSupported = true;
    page.threads = data.threads;
    page.messages = data.messages;
    const server = forwardTimeline(page);
    await signIn(page);
    await showDiscord(page, data.messages);
    await pickThread(page, `thread:${data.threads[0].id}`);
    const counted = [wideCard(page).count];
    const first = data.messages[1].thread;
    const reply = server.post(message({ id: "205", content: "a second answer", thread: { ...first, is_root: false } }));
    await deliver(page, page.stream(), sseMessage(reply));
    await page.settle();
    counted.push(wideCard(page).count);
    await doneButton(rowWithId(page, "202")).click();
    await page.settle();
    counted.push(wideCard(page).count);
    // A poll that brings nothing, and the rows passed over again: nothing written to the card.
    const count = page.el("thread-card-count");
    const words = page.el("thread-card-text").children[0];
    const replace = count.replaceChildren;
    let writes = 0;
    count.replaceChildren = (...kids) => {
      writes += 1;
      return replace.call(count, ...kids);
    };
    const write = FakeElement.prototype.setAttribute;
    FakeElement.prototype.setAttribute = function counted(name, value) {
      if ([page.el("thread-card"), page.el("thread-card-text"), page.el("thread-back")].includes(this)) writes += 1;
      return write.call(this, name, value);
    };
    const text = words.textContent;
    try {
      page.expireTimers(DISCORD_POLL_MS);
      await page.settle();
      await page.el("search-toggle").click();
      await page.el("search-toggle").click();
      await page.settle();
    } finally {
      FakeElement.prototype.setAttribute = write;
    }
    assert.equal(words.textContent, text);
    return { counted, writes };
  };
  const { counted, writes } = await run(SCRIPT);
  assert.deepStrictEqual(counted, ["1 reply · 1 unread", "2 replies · 2 unread", "2 replies · 1 unread"],
    "the card across the window did not follow the live reply and the Done");
  assert.equal(writes, 0, "a pass that changed nothing wrote to the card across the window");
  // The controls: drawn on every pass, the card is written to by a pass that changes nothing; drawn
  // only with the heading, it misses the Done.
  const always = await run(brokenScript("  if (threadCardsDrawn.get(card.id) === signature) return;\n", ""));
  assert.ok(always.writes > 0, "the negative control wrote nothing either, so the check above proves nothing");
  const heading = await run(brokenScript(
    "  if (!sideActive && selection().view === \"thread\") paintThreadCard(leftEl(\"thread-card\"), openThreadChoice(), tally);\n", ""));
  assert.notDeepStrictEqual(heading.counted, counted, "the negative control counted the Done anyway, so the check above proves nothing");
});

test("`#298 thread-card-header`: a thread opened from its card on screen on the right has its card at the top start where that card was and move up, the thread fading in under it and the line following it a frame at a time, until the animations end or their time is up", async () => {
  const restore = layColumnsOut();
  try {
    const { page } = await linkedColumns(16, (p) => p.allowCardFlight());
    const header = page.el("side-thread-card");
    const pane = page.el("side-pane");
    const area = page.el("side-scroll");
    const card = threadCard(page, 8);
    area.scrollTop += card.getBoundingClientRect().top - area.getBoundingClientRect().top - 120;
    await area.dispatch("scroll");
    await rootAt(page, 8, 300);
    const from = card.getBoundingClientRect();
    assert.ok(from.top > 0 && from.top < area.getBoundingClientRect().top + area.clientHeight, "the card is not on screen, so this proves nothing");
    await openerOf(card).click();
    await page.settle();
    const to = header.getBoundingClientRect();
    assert.equal(header.hasAttribute("data-flying"), true, "the card at the top did not move up from its card");
    assert.equal(header.style.getPropertyValue("--flight-x"), `${from.left - to.left}px`);
    assert.equal(header.style.getPropertyValue("--flight-y"), `${from.top - to.top}px`, "the card at the top does not start where its card was");
    assert.ok(from.top - to.top > 0, "the card at the top starts above its place, so this proves nothing");
    assert.equal(pane.hasAttribute("data-arriving"), true, "the thread did not fade in under its card");
    // The line follows the card up, drawn every frame while it moves: the box a browser gives a moving
    // card is where its transform has it, which this fixture, laying nothing out, is told frame by frame.
    const read = header.getBoundingClientRect;
    const ends = [];
    try {
      for (const lower of [90, 45, 10]) {
        header.getBoundingClientRect = () => ({ ...to, top: to.top + lower, bottom: to.bottom + lower });
        assert.ok(await page.nextFrame() >= 1, `no frame was asked for with the card ${lower}px from its place`);
        const [link] = drawnLinks(page);
        assert.deepStrictEqual(link && [link.thread, link.clip], ["spaces/A/threads/t8", "both"], "the card moving up has no whole line");
        ends.push(link.line[3] - (to.top + to.bottom) / 2);
      }
    } finally {
      header.getBoundingClientRect = read;
    }
    assert.deepStrictEqual(ends, [90, 45, 10], "the line did not follow the card up, frame by frame");
    // Each part ends with its own animation; another's ending in it is not its own.
    await header.dispatch("animationend", { target: page.el("side-thread-card-text") });
    assert.equal(header.hasAttribute("data-flying"), true, "an animation of something in the card ended its move");
    await header.dispatch("animationend", { target: header });
    assert.equal(header.hasAttribute("data-flying"), false, "the card's move did not end with its animation");
    assert.equal(pane.hasAttribute("data-arriving"), true, "the card's move ended the thread's fade with it");
    await pane.dispatch("animationend", { target: pane });
    assert.equal(pane.hasAttribute("data-arriving"), false, "the fade did not end with its animation");
    await page.nextFrame();
    await page.nextFrame();
    assert.equal(await page.nextFrame(), 0, "frames went on being asked for once the card had come to rest");
    assert.equal(page.expireTimers(320), 0, "the fallback outlived the animations it stands in for");
    // No animation ending, as for a card no longer drawn: the time is up at the length and a little.
    await page.el("side-close").click();
    await page.settle();
    await openerOf(threadCard(page, 8)).click();
    await page.settle();
    assert.equal(header.hasAttribute("data-flying"), true);
    assert.equal(page.expireTimers(320), 1, "no fallback was armed for a move whose animation never ends");
    assert.equal(header.hasAttribute("data-flying"), false);
    assert.equal(pane.hasAttribute("data-arriving"), false);
  } finally {
    restore();
  }
});

test("`#298 thread-card-header`: the card moves up only from a card on screen — Thread(N) on Main with the card showing, yes; the picker with it scrolled away, one thread to another, a stylesheet that gives it no time, or a reader who asked for less motion, no", async () => {
  const restore = layColumnsOut();
  try {
    const showCard = async (page, i) => {
      const area = page.el("side-scroll");
      area.scrollTop += threadCard(page, i).getBoundingClientRect().top - area.getBoundingClientRect().top - 120;
      await area.dispatch("scroll");
      await page.nextFrame();
    };
    const moved = (page) => [page.el("side-thread-card").hasAttribute("data-flying"), page.el("side-pane").hasAttribute("data-arriving")];
    const run = async (script) => {
      const { page } = await linkedColumns(16, (p) => p.allowCardFlight(), script);
      const seen = {};
      // Thread(N) on Main, its card on screen.
      await showCard(page, 8);
      await rootAt(page, 8, 120);
      await threadBadge(rootRow(page, 8)).click();
      await page.settle();
      seen.tag = moved(page);
      // One thread to another.
      page.expireTimers(320);
      await threadBadge(rootRow(page, 9)).click();
      await page.settle();
      seen.another = moved(page);
      // The picker, its card scrolled away.
      await page.el("side-close").click();
      await page.settle();
      const area = page.el("side-scroll");
      area.scrollTop = area.scrollHeight;
      await area.dispatch("scroll");
      await page.nextFrame();
      assert.ok(threadCard(page, 0).getBoundingClientRect().bottom < area.getBoundingClientRect().top, "the card is on screen, so this proves nothing");
      page.el("thread-select").value = "thread:spaces/A/threads/t0";
      await page.el("thread-select").dispatch("change");
      await page.settle();
      seen.away = moved(page);
      return seen;
    };
    assert.deepStrictEqual(await run(SCRIPT), { tag: [true, true], another: [false, false], away: [false, false] },
      "the card moved up from where no card showed, or not from where one did");
    // The control: asked whether the card shows nowhere, it moves from a card scrolled away.
    const blind = await run(brokenScript(" || !rowInView(sidePane, row)) return null;", ") return null;"));
    assert.deepStrictEqual(blind.away, [true, true], "the negative control did not move the card either, so the check above proves nothing");
    // Reduced motion, and a stylesheet that says nothing: never.
    for (const arrange of [(p) => p.preferReducedMotion(), null]) {
      const { page } = await linkedColumns(16, arrange);
      await showCard(page, 8);
      await openerOf(threadCard(page, 8)).click();
      await page.settle();
      assert.deepStrictEqual(moved(page), [false, false], `the card moved ${arrange ? "for a reader who asked for less motion" : "with no time to do it in"}`);
      assert.equal(page.el("side-thread-card").hidden, false);
    }
    const { page } = await linkedColumns(16, (p) => p.preferReducedMotion(), brokenScript("  if (!(ms > 0)) return;\n", ""));
    await showCard(page, 8);
    await openerOf(threadCard(page, 8)).click();
    await page.settle();
    assert.equal(page.el("side-thread-card").hasAttribute("data-flying"), true, "the negative control kept still too, so the check above proves nothing");
  } finally {
    restore();
  }
});

test("`#298 thread-card-header`: from the thread list, the card moves up across layouts — from its place in the list to the top of the window, into the card over the thread — with no line drawn", async () => {
  const run = async (script) => {
    const page = newPage(new Map(), script, (p) => {
      p.allowCardFlight();
      p.enableAnimationFrames();
    });
    const data = threadData();
    page.threadingSupported = true;
    page.threads = data.threads;
    await signIn(page);
    await showDiscord(page, data.messages);
    await pickThread(page, "threads");
    // The card at the foot of the list, which has a card above it: below the top of the window.
    const id = cardIds(page).at(-1);
    const from = cardRow(page, id).getBoundingClientRect();
    await openerOf(cardRow(page, id)).click();
    await page.settle();
    return { page, from, id };
  };
  const { page, from, id } = await run(SCRIPT);
  const card = page.el("thread-card");
  assert.equal(page.el("screen-main").hasAttribute("data-columns"), false, "the thread did not open across the window");
  assert.equal(wideCard(page).shown, true);
  assert.equal(card.hasAttribute("data-flying"), true, "the card did not move up from the thread list");
  const to = card.getBoundingClientRect();
  assert.equal(card.style.getPropertyValue("--flight-y"), `${from.top - to.top}px`, "the card does not start where the list's card was");
  assert.ok(from.top - to.top > 0, "the list's card is above the top, so this proves nothing");
  assert.equal(page.el("pane-discord").hasAttribute("data-arriving"), true, "the thread did not fade in under the card");
  assert.equal(page.el("side-thread-card").hasAttribute("data-flying"), false);
  await page.nextFrame();
  assert.deepStrictEqual(page.el("column-links").children, [], "a line was drawn across the window, with no Main to draw it to");
  await card.dispatch("animationend", { target: card });
  await page.el("pane-discord").dispatch("animationend", { target: page.el("pane-discord") });
  assert.equal(card.hasAttribute("data-flying"), false);
  assert.equal(page.el("pane-discord").hasAttribute("data-arriving"), false);
  // Back to the list, and in again: it moves again.
  await page.el("thread-back").click();
  await page.settle();
  await openerOf(cardRow(page, id)).click();
  await page.settle();
  assert.equal(card.hasAttribute("data-flying"), true, "the card did not move up the second time");
  // The control: with no place taken from the list, nothing moves.
  const still = await run(brokenScript("  const from = threadListOnScreen() ? cardFlightStart(String(id)) : null;\n", "  const from = null;\n"));
  assert.equal(still.page.el("thread-card").hasAttribute("data-flying"), false, "the negative control moved the card anyway");
});
