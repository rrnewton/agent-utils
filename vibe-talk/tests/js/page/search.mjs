// The app page's tests: search: on screen, further back, in the threads' replies, and on the server.
//
// One feature's file of the suite tests/js/voice_page.test.mjs runs, against the fixture in
// ./fixture.mjs. Run with the rest of the suite, or alone: node --test tests/js/page/search.mjs.

import assert from "node:assert/strict";

import { ALICE, CHANNEL, CODER, CSS, FakeElement, HOLD_MS, HTML, HTML_CODE, LINK_ATTRS, PAGE_CODE,
  RECORD_EPOCH, SCRIPT_CODE, UNPACK_STORAGE, allLeavingOut, assertMarkupContains, assistantSays,
  authorChannel, authorState, backlog, beforeOf, boxesOf, builtSince, cardRow, channelSummaryText,
  chooseAuthor, chooseReadMode, collapsedRows, cssBlock, cssRules, deliver, excerptOf, foldButton,
  gate, headOf, hitsIn, inTwoColumns, json, lineText, longMessage, marksPending, message,
  messageCacheStatus, messageRows, newPage, nodeWithinFixture, openerOf, pagedThreads, pickThread,
  press, reReadChannel, reloadPage, repliesPage, replyHint, rowWithId, ruledRows, said,
  savedUiState, search, searchDay, searchHistory, searchMoment, showDiscord, showPinned, shownIds,
  signIn, sourceConstant, sseMessage, startTalking, startTalkingNamed, state, statusText, storedPin,
  tapArrow, tapRow, test, threadData, threadHistory, threadPage, threadsHistory, timelineAnswer,
  touchAt, turnTodoOn, twoColumns, typeAndPause, unfiltered, unpackPage, unpackRows, viewOf, youSay } from "./fixture.mjs";

/** Everything the record holds for one call, in the order it was said. */
function recorded(page, id, texts) {
  page.storedTurns.set(
    id,
    texts.map((text, i) => ({
      speaker: i % 2 === 0 ? "you" : "agent",
      text,
      at_ms: RECORD_EPOCH + i * 60_000,
    }))
  );
}

const TALKED_ABOUT = [
  "the mac runner stalled",
  "which runner do you mean",
  "the mac one, again",
  "a runner on the mac",
];

test("EVERY TERM HAS TO MATCH, WHICH IS WHAT MAKES A SECOND WORD WORTH TYPING", async () => {
  // AND, not OR. A reader types a second word to narrow, and a filter that widened on it would be
  // actively worse than no filter: the more they said about what they wanted, the more they would
  // get. Substring rather than whole-word, because "runner" has to find "runners".
  const page = newPage();
  recorded(page, "conv", TALKED_ABOUT);
  await signIn(page);
  await page.settle();
  assert.equal(unfiltered(page, "transcript").length, 4, "the record did not come back");

  await search(page, "runner");
  assert.deepStrictEqual(unfiltered(page, "transcript").map(said), [
    "the mac runner stalled",
    "which runner do you mean",
    "a runner on the mac",
  ]);

  await search(page, "runner mac");
  assert.deepStrictEqual(
    unfiltered(page, "transcript").map(said),
    ["the mac runner stalled", "a runner on the mac"],
    "a second term widened the search instead of narrowing it"
  );
});

test("A QUOTED PHRASE IS ONE TERM, AND HAS TO BE FOUND WHOLE", async () => {
  // The clause the owner spelled out — "allowing double quoting to group them into one string" —
  // and the fixture is built so that the two readings really do differ: `a runner on the mac`
  // contains both words and not the phrase, so a quoted search that still matched it would be a
  // search where the quotes changed nothing.
  const page = newPage();
  recorded(page, "conv", TALKED_ABOUT);
  await signIn(page);
  await page.settle();

  await search(page, `"mac runner"`);
  assert.deepStrictEqual(unfiltered(page, "transcript").map(said), ["the mac runner stalled"]);

  // ...and an UNTERMINATED quote groups to the end of what has been typed, rather than waiting for
  // its partner. This is the state the field is in for every keystroke between the two quotes, and
  // treating it as two loose words there makes the list flicker through a query nobody asked for.
  await search(page, `"mac runner`);
  assert.deepStrictEqual(
    unfiltered(page, "transcript").map(said),
    ["the mac runner stalled"],
    "a half-typed phrase was read as two separate words"
  );
});

test("THE COUNT NAMES ITS DENOMINATOR, BECAUSE THE SCREEN IS ONLY A SUFFIX OF THE RECORD", async () => {
  // The honesty clause, and `#128 transcript-history` is what made it necessary: the transcript on
  // screen is a bounded window onto a much longer record, so "no matches" and "never said" are
  // different answers. A bare "1" would let a reader conclude the second when the page only knows
  // the first.
  const page = newPage();
  recorded(page, "conv", TALKED_ABOUT);
  await signIn(page);
  await page.settle();
  assert.equal(page.el("search-count").textContent, "", "a count stands with nothing to count");

  await search(page, "runner mac");
  assert.equal(page.el("search-count").textContent, "2 of 4 loaded");
  assert.equal(page.el("search-count").hidden, false, "the count is not on screen to be read");

  // An empty field is not a filter, and must not read as "0 of 4".
  await search(page, "   ");
  assert.equal(page.el("search-count").textContent, "");
  assert.equal(unfiltered(page, "transcript").length, 4, "whitespace hid the whole record");
});

test("a rule the PAGE drew goes away with the rows it was explaining", async () => {
  // A seam says "the agent below this never heard the words above it". Left standing over a
  // filtered list it says that about two rows that are no longer adjacent, which is a claim about
  // the record that is simply false. It declares nothing searchable, which is how the filter knows.
  const page = newPage();
  recorded(page, "older", ["the mac runner stalled"]);
  recorded(page, "newer", ["and again this morning"]);
  await signIn(page);
  await page.settle();
  assert.ok(ruledRows(page).length > 0, "the fixture drew no seam to hide");

  await search(page, "runner");
  assert.deepStrictEqual(unfiltered(page, "transcript").map(said), ["the mac runner stalled"]);
  assert.equal(
    ruledRows(page).filter((li) => !li.hasClass("search-hidden")).length,
    0,
    "a boundary between two calls is still drawn between rows that are not beside each other"
  );
});

test("ONE QUERY FILTERS BOTH LISTS, BECAUSE A READER SWITCHES VIEWS WITHOUT SWITCHING SEARCH", async () => {
  // "It should work both for raw discord messages and for a transcript of the voice agent
  // discussion" — and they are one switch apart. A filter that applied to whichever list happened
  // to be up when it was typed would come off the moment the reader looked at the other one.
  const page = newPage();
  recorded(page, "conv", TALKED_ABOUT);
  await signIn(page);
  await page.settle();
  await search(page, "runner");

  await showDiscord(page, [
    message({ id: "1", content: "the runner is back" }),
    message({ id: "2", content: "deploy finished" }),
  ]);
  assert.equal(
    unfiltered(page, "discord-log").length,
    1,
    "the channel arrived unfiltered under a search that was already on"
  );
  // ...and the count now speaks for the list the reader is actually looking at.
  assert.equal(page.el("search-count").textContent, "1 of 2 loaded");

  await page.el("view-switch").click();
  await page.settle();
  assert.equal(page.el("search-count").textContent, "3 of 4 loaded", "the count stayed behind");
});

test("A TURN ARRIVING DURING A SEARCH ARRIVES FILTERED", async () => {
  // The failure this closes is quiet and total: a row appended by a path that does not re-apply
  // the filter lands among the matches, and the reader — who is reading a filtered list — takes
  // it for one. It reads as a search that silently stopped working.
  const page = newPage();
  await signIn(page);
  await startTalking(page);
  assistantSays(page, "the mac runner stalled");
  await page.settle();

  await search(page, "runner");
  assistantSays(page, "the deploy finished");
  assistantSays(page, "the other runner is fine");
  await page.settle();

  assert.deepStrictEqual(unfiltered(page, "transcript").map(said), [
    "the mac runner stalled",
    "the other runner is fine",
  ]);
});

test("THE FILTER HIDES WITH A CLASS OF ITS OWN, NEVER WITH THE ATTRIBUTE", async () => {
  // Load-bearing, and not a style preference. `hidden` is where the page records the OTHER reasons
  // a row is off screen — to-do mode, archived, which channel view is up. A filter that borrowed
  // it would leave every row it hid looking like a row hidden for one of those reasons, and
  // clearing the search would then put back rows the reader had archived.
  const page = newPage();
  recorded(page, "conv", TALKED_ABOUT);
  await signIn(page);
  await page.settle();

  await search(page, "stalled");
  const missing = messageRows(page, "transcript").filter((row) => row.hasClass("search-hidden"));
  assert.equal(missing.length, 3, "nothing was filtered, so this proves nothing");
  for (const row of missing) {
    assert.equal(row.hidden, false, "the filter wrote the attribute that means something else");
  }
  // ...and the row keeps whatever else it was wearing, which is what says who said it.
  assert.ok(
    missing.every((row) => row.hasClass("mine") || row.hasClass("theirs")),
    "filtering a row forgot which side of the transcript it is on"
  );
});

test("CLOSING THE GLASS PUTS EVERY ROW BACK, BECAUSE A FILTER NOBODY CAN SEE IS THE WORST STATE", async () => {
  // A query parked behind a shut control is the one outcome worse than no search at all: the
  // messages are gone and nothing on the screen says why. Closing clears; it is not a second
  // switch. Escape does the same thing, because on a desktop that is the key a hand reaches for
  // and on a phone keyboard it is the only one there is.
  const page = newPage();
  recorded(page, "conv", TALKED_ABOUT);
  await signIn(page);
  await page.settle();

  await search(page, "stalled");
  assert.equal(unfiltered(page, "transcript").length, 1);
  assert.equal(page.el("search-toggle").getAttribute("aria-pressed"), "true");

  await page.el("search-toggle").click();
  assert.equal(unfiltered(page, "transcript").length, 4, "closing the glass left the filter on");
  assert.equal(page.el("search-field").value, "", "the query is parked behind a shut control");
  assert.equal(page.el("search-field").hidden, true);
  assert.equal(page.el("search-toggle").getAttribute("aria-pressed"), "false");

  await search(page, "stalled");
  assert.equal(unfiltered(page, "transcript").length, 1);
  await page.el("search-field").dispatch("keydown", { key: "Escape" });
  assert.equal(unfiltered(page, "transcript").length, 4, "Escape left the filter on");
  assert.equal(page.el("search-field").hidden, true, "Escape left the field open");
});

test("NOTHING MATCHED IS SAID WHERE THE MESSAGES WERE, NOT ONLY IN THE CORNER", async () => {
  // The state this closes is a blank screen. Every row is still IN the list — hidden by a class,
  // which is the whole design — so neither pane's own empty state fires, and the only thing
  // reporting what happened would be "0 of 4 loaded" in 0.75rem grey type at the far end of the
  // search bar. A reader who does not find that concludes the page broke.
  const page = newPage();
  recorded(page, "conv", TALKED_ABOUT);
  await signIn(page);
  await page.settle();
  assert.equal(page.el("search-empty").hidden, true, "it is standing with nothing to explain");

  await search(page, "kubernetes");
  assert.equal(unfiltered(page, "transcript").length, 0, "the fixture matched something after all");
  assert.equal(page.el("search-empty").hidden, false, "a blank list and no word about why");
  assert.equal(page.el("search-count").textContent, "0 of 4 loaded");

  // ...and it goes the moment there is anything to look at again, because a "nothing matched"
  // standing over matches is worse than no notice at all.
  await search(page, "runner");
  assert.equal(page.el("search-empty").hidden, true, "it stayed up over a list with rows in it");
});

test("a list that was empty BEFORE the search says why in its own words, not the filter's", async () => {
  // The notice claims the search emptied the list. Over a transcript that never had a turn in it
  // that is simply false, and it would also be covering the page's own invitation to start one.
  const page = newPage();
  await signIn(page);
  await page.settle();
  assert.equal(page.el("transcript").children.length, 0, "the fixture arrived with a record");

  await search(page, "kubernetes");
  assert.equal(page.el("search-empty").hidden, true, "the filter took the blame for an empty page");
  assert.equal(page.el("empty-state").hidden, false, "the invitation to start talking went away");
});

test("WITH THE BAR IN THE HEADER, SEARCHING LEAVES IT THERE, BECAUSE THE FIELD FLOATS OVER THE LIST", async () => {
  // REWRITTEN BY `#197 floating-search`. This used to be the one collision `#129 message-search`
  // had with `#58 control-bar`: the field opened in the header row, two elastic things in a 375px
  // header is a search field about a word wide, and so the bar yielded the row for the duration.
  // The field opens over the top of the LIST now, in both placements, so the collision is gone and
  // so is the yielding — a reader who put the bar up there keeps it while they search.
  const page = newPage();
  recorded(page, "conv", TALKED_ABOUT);
  await signIn(page);
  await page.settle();
  await page.el("open-settings").click();
  page.el("bar-placement").value = "top";
  await page.el("bar-placement").dispatch("change");
  await page.el("close-settings").click();
  assert.equal(page.el("control-bar").hidden, false, "the fixture never got the bar up there");
  assert.equal(page.el("topbar").hidden, false, "the bar is in a header that is not shown");

  await search(page, "runner");
  assert.equal(page.el("control-bar").hidden, false, "opening the search took the bar away");
  assert.equal(page.el("topbar").hidden, false, "opening the search took the header away");
  assert.equal(page.el("search-field").hidden, false, "the field did not open");
  assert.equal(unfiltered(page, "transcript").length, 3, "the search stopped filtering up here");

  await page.el("search-toggle").click();
  assert.equal(page.el("control-bar").hidden, false, "shutting the glass moved the bar");
  assert.equal(unfiltered(page, "transcript").length, 4);
});

test("THE GLASS GROWS INTO A BAR OVER THE LIST, THE PILL GIVES WAY, AND CLOSING FOLDS IT BACK", async () => {
  // `#197 floating-search`. The glass and the freshness pill share one line over the top of the
  // list, so the open bar — field, count and glass, along that line — would be drawn straight over
  // the pill if the pill stayed. It gives way, exactly as it gives way to a pull, and comes back
  // the moment the bar folds. `data-searching` on #screen-main is what web/voice.css keys the
  // bar's shape and the room at the head of the list off; asserted here because nothing else can
  // see whether the script ever sets it.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [
    message({ id: "1", content: "the mac runner stalled" }),
    message({ id: "2", content: "a different subject" }),
  ]);
  const pill = page.el("channel-freshness");
  const searching = () => page.el("screen-main").hasAttribute("data-searching");
  assert.equal(pill.hidden, false, "the fixture never put the pill on the line");
  assert.equal(searching(), false, "the bar ships open");
  assert.equal(page.el("topbar").hidden, true, "the header came back over the channel");

  await page.el("search-toggle").click();
  assert.equal(searching(), true, "the bar did not open over the list");
  assert.equal(page.el("search-field").hidden, false, "the field did not open");
  assert.equal(pill.hidden, true, "the open bar is drawn over the pill");
  assert.equal(page.el("topbar").hidden, true, "opening the search spent a header row after all");
  // The pill's ROOM stays reserved while the pill steps aside, as it does for a pull: it is keyed
  // to there being something to say, and the bar's own room replaces it in the stylesheet.
  assert.equal(page.el("pane-discord").hasAttribute("data-freshness"), true);

  await page.el("search-field").setValue("stalled");
  assert.equal(unfiltered(page, "discord-log").length, 1, "the floating field does not filter");
  assert.equal(page.el("search-count").hidden, false, "the count is not on the bar");
  assert.equal(page.el("search-count").textContent, "1 of 2 loaded");

  await page.el("search-toggle").click();
  assert.equal(searching(), false, "closing left the bar's room at the head of the list");
  assert.equal(pill.hidden, false, "the pill did not come back when the bar folded");
  assert.equal(page.el("search-field").hidden, true);
  assert.equal(unfiltered(page, "discord-log").length, 2, "folding the bar left the filter on");

  // Escape folds it the same way.
  await search(page, "stalled");
  assert.equal(pill.hidden, true);
  await page.el("search-field").dispatch("keydown", { key: "Escape" });
  assert.equal(searching(), false, "Escape left the bar open");
  assert.equal(pill.hidden, false, "Escape left the pill hidden");
});

test("the open bar makes room for itself at the head of the list, in place of the pill's", () => {
  // The bar floats, so without room reserved under it the first match — the one a filter leaves
  // at the very top — would be underneath it. The room is padding inside the scroller, as
  // `#32 freshness-pill-overlap` first made room for the pill, and it REPLACES the pill's room rather
  // than stacking under it: the pill is not on screen while the bar is. Since `#324
  // live-pill-covers-row` the pill's room is the list's top edge, and is made only while not searching.
  assert.match(
    cssBlock("#screen-main[data-searching] #scroll-area"),
    /padding-top:\s*calc\(var\(--float-line\) \+ var\(--search-bar\) \/ 2 \+ 0\.35rem\)/,
    "the list does not make room for the open bar"
  );
  assert.equal(cssRules(CSS, "#screen-main:not([data-searching]) > #scroll-area:has(> #pane-discord[data-freshness]:not([hidden]))").length, 1, "the pill's room is not one rule");
  assert.equal(cssRules(CSS, "#pane-discord[data-freshness]").length, 0,
    "the pill's room is padding inside the scroller again, which scrolls away");
  // ...and the bar is a panel of its own, so the count is not printed onto a message underneath.
  const open = cssBlock("#screen-main[data-searching] #search-float");
  assert.match(open, /background:\s*var\(--panel\)/);
  assert.match(open, /height:\s*var\(--search-bar\)/);
  assert.match(open, /left:\s*calc\(var\(--float-edge\) \+ env\(safe-area-inset-left\)\)/);
});

test("every list of messages is a list the filter reaches", () => {
  // The completeness rule, and the reason it is a test rather than a comment: the lists are
  // declared in the markup and the filter's set of them is declared in the script, so a fifth one
  // added to the page is unsearchable until somebody remembers this file. A reader who searches
  // and sees a list unchanged concludes the feature is broken, which — for that list — it is.
  const declared = /const SEARCH_LISTS = \[([^\]]*)\]/.exec(SCRIPT_CODE);
  assert.ok(declared, "web/voice.js no longer declares SEARCH_LISTS");
  const filtered = [...declared[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]);
  // `#channel-summary` is built in the same idiom and holds no messages: it is the one-row seam
  // naming the channel, which `#63 status-line-placement` moved to the head of its own list so it
  // would scroll with what it describes. There is nothing in it for a query to match, and hiding
  // the channel's own name because it does not contain the search terms would read as a bug.
  //
  // It is EXCLUDED BY NAME rather than by a rule about what it contains, so that a fifth list
  // added to the page still fails here until somebody says which of the two kinds it is.
  const NOT_MESSAGES = ["channel-summary"];
  // `#229 desktop-two-column`. The right column's lists stand in for the channel pane's, by
  // `COLUMN_IDS`, and the filter reaches them the way everything else does: it runs once in each
  // column, and `el()` gives it that column's list for each id. So each counts as its counterpart.
  const table = /const COLUMN_IDS = \{([\s\S]*?)\n\};/.exec(SCRIPT_CODE);
  assert.ok(table, "web/voice.js no longer declares COLUMN_IDS");
  const standsFor = new Map([...table[1].matchAll(/"([^"]+)": "([^"]+)"/g)].map((m) => [m[2], m[1]]));
  const inMarkup = [...new Set([...HTML.matchAll(/<ol id="([^"]+)" class="messages"/g)]
    .map((m) => standsFor.get(m[1]) || m[1]))]
    .filter((id) => !NOT_MESSAGES.includes(id));
  assert.ok(inMarkup.length >= 4, "the markup scan stopped finding the lists it is about");
  assert.deepStrictEqual(
    [...filtered].sort(),
    [...inMarkup].sort(),
    "a list of messages on the page is one the search cannot touch"
  );
});

test("a vendor id that would have to be mangled to be legal becomes a LOCAL id instead", async () => {
  // Stripping illegal bytes makes two different vendor ids collapse onto one stored conversation
  // — `a/b` and `ab` are the same key once the slash is gone — and the two calls then interleave
  // in one transcript with nothing saying so.
  const page = newPage();
  await startTalkingNamed(page, "call/one");
  youSay(page, "first call");
  await page.settle();

  const second = newPage();
  await startTalkingNamed(second, "callone");
  youSay(second, "second call");
  await second.settle();

  const idOf = (p) =>
    /conversations\/([^/]+)\/turns/.exec(p.storeCalls.find((c) => c.startsWith("POST")))[1];
  assert.match(idOf(page), /^local-/, "a mangled vendor id was used as the key anyway");
  assert.notEqual(
    idOf(page),
    idOf(second),
    "two different calls were recorded into one stored conversation"
  );
  // The control: an id the server already accepts is passed through untouched, or every
  // conversation would be unmatchable against the vendor's own record of the same call.
  const clean = newPage();
  await startTalkingNamed(clean, "conv_from_vendor");
  youSay(clean, "hello");
  await clean.settle();
  assert.equal(idOf(clean), "conv_from_vendor");
});

test("CLEAR EMPTIES THE SCREEN AND ERASES NOTHING; FORGET ERASES AND LEAVES THE SCREEN", async () => {
  // The two records must not be able to disagree about what erases what. This is the assertion
  // that catches Clear quietly becoming a delete, and Forget quietly wiping the screen.
  const page = newPage();
  await startTalkingNamed(page);
  youSay(page, "what happened overnight");
  await page.settle();
  assert.equal(page.storedTurns.size, 1, "nothing was stored, so this test proves nothing");

  await page.el("clear-view").click(); // arms
  await page.el("clear-view").click(); // clears
  await page.settle();
  assert.equal(
    page.storedTurns.size,
    1,
    "Clear erased the durable record, which the settings screen promises it does not"
  );
  assert.equal(
    page.storeCalls.filter((c) => c.startsWith("DELETE")).length,
    0,
    `Clear issued a DELETE: ${page.storeCalls.join(", ")}`
  );

  // The control: the other button really does erase, so the assertion above is about Clear and
  // not about a page that cannot delete at all.
  const before = page.el("transcript").children.length;
  await page.el("forget-conversations").click();
  await page.settle();
  assert.equal(page.storedTurns.size, 0, "Forget stored conversations erased nothing");
  assert.equal(
    page.el("transcript").children.length,
    before,
    "Forget emptied the screen, which is Clear's job and not this one's"
  );
  assert.match(page.el("storage-state").textContent, /Erased 1 stored conversation\b/);
});

test("A STORE THAT IS DOWN NEVER INTERRUPTS THE CALL", async () => {
  // The owner is driving. A dead store must cost the record, not the conversation — and it must
  // say so once, in Settings, rather than raising a panel on every turn.
  const page = newPage();
  await startTalkingNamed(page);
  page.storeStatus = 503;

  youSay(page, "first");
  await page.settle();
  assistantSays(page, "second");
  youSay(page, "third");
  await page.settle();

  assert.equal(state(page), "live", "a failing store hung up the call");
  assert.equal(page.el("error").hidden, true, `the store raised an error panel: ${page.el("error").textContent}`);
  assert.equal(
    page.storeCalls.filter((c) => c.startsWith("POST")).length,
    1,
    `the page kept retrying a store it already knows is broken: ${page.storeCalls.join(", ")}`
  );
  assert.match(
    page.el("storage-state").textContent,
    /Not recording/,
    "a store that gave up has to say so where it can be read afterwards"
  );
  assert.equal(
    page.el("transcript").children.filter((li) => li.className !== "seam").length,
    3,
    "the lines must still reach the screen when only the recording failed"
  );
});

test("a conversation id from the vendor is sanitised before it becomes a path", async () => {
  // The id is vendor-controlled text that is about to be interpolated into a URL. The server
  // refuses a bad one, but the page must not be the thing that sends it.
  const page = newPage();
  await startTalkingNamed(page, "../../etc/passwd");
  youSay(page, "hello");
  await page.settle();

  const posted = page.storeCalls.filter((c) => c.startsWith("POST"));
  assert.equal(posted.length, 1, `nothing was recorded: ${page.storeCalls.join(", ")}`);
  assert.ok(
    !posted[0].includes(".."),
    `the page put a traversal in the URL it requested: ${posted[0]}`
  );
  assert.match(posted[0], /^POST \/api\/v1\/conversations\/[A-Za-z0-9_-]+\/turns$/);
});

test("the settings screen states which control erases the record and which does not", () => {
  // Two controls, two different things erased. The page has to say which is which in the place
  // the button is, or the owner learns it by losing something.
  const note = HTML.slice(HTML.indexOf('id="continuity-note"'));
  assert.match(note, /leaves the stored record on the server untouched/);
  assert.match(note, /Forget stored conversations/);
  const settings = HTML.slice(HTML.indexOf("Stored conversations"));
  assert.match(settings, /not encrypted/, "an unencrypted record has to say so where it is offered");
  assert.match(settings, /id="forget-conversations"/);
});

// --- the hits, drawn where they are (#249 search-highlight) ----------------------------------------
//
// The owner: "if I search for the word "engine" let's say, it's still weird that we don't highlight
// where the word hits within the filtered messages that have the word." Each hit is a `<mark>` built
// from text nodes, never from markup; the row says exactly what it said; and a folded message whose
// hit is beyond its fold says where under it.

/** A node as markup, its own text and its text nodes included, so a test can say exactly what is drawn. */
function drawnMarkup(node) {
  if (node.nodeType === 3) return node.textContent;
  const tag = String(node.tagName).toLowerCase();
  const cls = node.className ? ` class="${node.className}"` : "";
  return `<${tag}${cls}>${node.textContent}${node.children.map(drawnMarkup).join("")}</${tag}>`;
}

/** What a row says, the lines drawn between rows (`#249`, `#250`) left out. */
const rowWords = (li) => li.children.filter((kid) => !kid.hasClass("row-seam")).map((kid) => kid.text()).join("");

test("`#249 search-highlight`: every hit of every term is marked where it is, whatever its case, and the row says what it said", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [
    message({ id: "1", content: "The Engine stalled; restart the ENGINE and the engine room." }),
    message({ id: "2", content: "nothing about it here" }),
    // A word half in bold is one word on screen, and one hit.
    message({ id: "3", content: "**en**gine notes", content_html: "<p><strong>en</strong>gine notes</p>\n" }),
  ]);
  const before = rows.map(rowWords);
  await typeAndPause(page, "engine");
  assert.deepStrictEqual(hitsIn(rows[0]), ["Engine", "ENGINE", "engine"], "a hit is unmarked, or marked in the wrong case");
  assert.deepStrictEqual(hitsIn(rows[1]), [], "a row the search hides was marked");
  assert.equal(drawnMarkup(boxesOf(rows[2])[0]),
    '<div class="md"><p><strong><mark class="search-hit">en</mark></strong><mark class="search-hit">gine</mark> notes</p>\n</div>',
    `a word half in bold is not marked whole, in its two pieces: ${drawnMarkup(boxesOf(rows[2])[0])}`);
  assert.deepStrictEqual(rows.map(rowWords), before, "marking the hits changed what a row says");
  // Two terms, and a quoted phrase: each term's hits; a phrase's only where it is whole.
  await typeAndPause(page, "restart engine");
  assert.deepStrictEqual(hitsIn(rows[0]), ["Engine", "restart", "ENGINE", "engine"]);
  await typeAndPause(page, "\"engine room\"");
  assert.deepStrictEqual(hitsIn(rows[0]), ["engine room"], "a phrase was marked word by word");
  // Overlapping hits of two terms are one mark.
  await typeAndPause(page, "engi gine");
  assert.deepStrictEqual(hitsIn(rows[0]), ["Engine", "ENGINE", "engine"], "overlapping hits were marked twice");
  // Cleared, every mark goes and the text is whole again, a text node as it was.
  await page.el("search-field").setValue("");
  assert.deepStrictEqual(rows.flatMap(hitsIn), [], "a mark outlived the search");
  assert.equal(drawnMarkup(boxesOf(rows[2])[0]), '<div class="md"><p><strong>en</strong>gine notes</p>\n</div>');
  assert.deepStrictEqual(rows.map(rowWords), before);
  await typeAndPause(page, "engine");
  await page.el("search-toggle").click();
  await page.settle();
  assert.deepStrictEqual(rows.flatMap(hitsIn), [], "closing the search left its marks");
});

test("`#249 search-highlight`: hits in code, in a link's words, a link's address alone, the author and a rich header's pieces are all shown", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [
    message({ id: "1", content: "run `engine --check` first",
      content_html: "<p>run <code>engine --check</code> first</p>\n" }),
    message({ id: "2", content: "see [the engine docs](https://example.com/manual) and [this](https://example.com/engine)",
      content_html: `<p>see <a href="https://example.com/manual" ${LINK_ATTRS}>the engine docs</a> and ` +
        `<a href="https://example.com/engine" ${LINK_ATTRS}>this</a></p>\n` }),
    message({ id: "3", content: "an engine report", ...ALICE, author: "engineer" }),
    message({ id: "4", content: "[engine-bot] Engine check, 9:05 AM ET. All green.",
      heading: { tag: "engine-bot", time: "9:05 AM ET", title: "Engine check", cut: 39 } }),
  ]);
  await typeAndPause(page, "engine");
  assert.deepStrictEqual(hitsIn(rows[0]), ["engine"], "a hit in code is not marked");
  assert.deepStrictEqual(hitsIn(rows[1]), ["engine"], "a hit in a link's words is not marked");
  const anchors = rows[1].descendants().filter((node) => node.tagName === "a");
  assert.deepStrictEqual(anchors.map((a) => a.hasClass("search-hit-link")), [false, true],
    "only the link whose address alone holds the word is outlined");
  const author = rows[2].descendants().find((node) => node.hasClass("msg-author"));
  assert.equal(author.getAttribute("data-search-hit"), "", "an author whose name holds the word is not lit");
  assert.equal(author.text(), "engineer", "lighting the author changed its name");
  // The header's pieces, each on its own; and the words they restate, hidden while the header shows.
  const head = headOf(rows[3]);
  assert.deepStrictEqual(head.children.map((piece) => [piece.className, piece.text(), hitsIn({ children: [piece] }).length]),
    [["rh-tag", "engine-bot", 1], ["rh-title", "Engine check", 1], ["rh-time", "@9:05 AM ET", 0]]);
  // Under Links, the links drawn in place of the text.
  await page.el("links-filter").click();
  await page.settle();
  const list = rows[1].children.find((node) => node.className === "row-links");
  assert.deepStrictEqual(list.descendants().filter((node) => node.tagName === "mark").map((node) => node.text()), ["engine"],
    "a hit in the Links view's list is not marked");
  assert.ok(list.children[1].hasClass("search-hit-link"), "the Links view does not outline the link found by its address");
  await page.el("search-toggle").click();
  await page.settle();
  assert.equal(author.getAttribute("data-search-hit"), null, "the author stayed lit after the search");
  assert.deepStrictEqual(anchors.map((a) => a.hasClass("search-hit-link")), [false, false]);
});

test("`#249 search-highlight`: markup in a message stays text, and a mark is built of text nodes, never parsed", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [
    message({ id: "1", content: "<b>engine</b> & <script>engine()</script>" }),
  ]);
  const templates = page.templatesParsed;
  await typeAndPause(page, "engine");
  assert.deepStrictEqual(hitsIn(rows[0]), ["engine", "engine"]);
  assert.equal(rows[0].text().includes("<b>engine</b> & <script>engine()</script>"), true, "the text is not what was written");
  for (const tag of ["b", "script"]) assert.ok(!page.createdTags.includes(tag), `the text became a <${tag}>`);
  assert.equal(page.templatesParsed, templates, "marking parsed something as HTML");
  // The page has one HTML sink, and marking added none.
  assert.equal((PAGE_CODE.match(/\.innerHTML\s*=/g) || []).length, 1, "a second place parses HTML");
});

test("`#249 search-highlight`: a combined row is marked in each of its messages, and a turn of the call is marked too", async () => {
  const page = newPage();
  await signIn(page);
  const first = message({ id: "1", content: "the engine part one" });
  const second = message({ id: "2", content: "and the engine part two",
    timestamp: new Date(Date.parse(first.timestamp) + 1500).toISOString() });
  const rows = await showDiscord(page, [first, second]);
  assert.equal(rows.length, 1, "the fixture made no combined row");
  await typeAndPause(page, "engine");
  assert.deepStrictEqual(boxesOf(rows[0]).map((box) => box.descendants().filter((n) => n.tagName === "mark").length), [1, 1]);
  // The call's transcript.
  await page.el("search-toggle").click();
  await startTalking(page);
  assistantSays(page, "the engine is warm");
  await page.settle();
  await typeAndPause(page, "warm");
  const turn = unfiltered(page, "transcript").find((li) => li.text().includes("engine is warm"));
  assert.deepStrictEqual(hitsIn(turn), ["warm"], "a turn of the call was not marked");
  assert.equal(said(turn), "the engine is warm");
});

test("`#249 search-highlight`: a folded message whose hit is past its fold says where, under it, until it is opened", async () => {
  const page = newPage();
  await signIn(page);
  const late = `${"plain words without it. ".repeat(14)}Then the engine stalled at noon, and later the engine ` +
    `restarted. ${"more plain words here. ".repeat(4)}`;
  const early = `The engine first. ${"plain words without it. ".repeat(14)}`;
  const many = Array.from({ length: 5 }, (_unused, i) => `${"filler words go on here. ".repeat(4)}engine ${i}.`).join(" ");
  const rows = await showDiscord(page, [
    message({ id: "1", content: late }),
    message({ id: "2", content: early }),
    message({ id: "3", content: many }),
    message({ id: "4", content: "a short engine note" }),
  ]);
  assert.equal(rows[0].getAttribute("data-collapsed"), "true", "the fixture's long message is not folded");
  await typeAndPause(page, "engine");
  const line = excerptOf(rows[0]);
  assert.ok(line, "a folded message with a hit past its fold says nothing about it");
  assert.equal(line.hidden, false);
  assert.match(line.text(), /^Further down: …plain words without it\. Then the engine stalled at noon, and later the engine restarted\. more plain words here\.…$/,
    `the line does not show the hits with words either side: ${line.text()}`);
  assert.deepStrictEqual(line.descendants().filter((n) => n.tagName === "mark").map((n) => n.text()), ["engine", "engine"]);
  // After the text and the count of what its fold hides (`#244 elision-word-count`), before what
  // follows them; and gone from sight while the message is open.
  const order = rows[0].children.map((kid) => kid.className);
  assert.ok(order.indexOf("elided") === order.indexOf("body clamped") + 1 && order.indexOf("search-excerpt") === order.indexOf("elided") + 1,
    `the line is not straight after the text and its count of hidden words: ${order}`);
  await foldButton(rows[0]).click();
  await page.settle();
  assert.equal(line.hidden, true, "the line stayed under an open message, whose hits are in its text");
  await foldButton(rows[0]).click();
  await page.settle();
  assert.equal(line.hidden, false);
  // A hit in the opening words is on screen already, and is not repeated.
  assert.equal(excerptOf(rows[1]), undefined, "a hit the fold shows was repeated under it");
  // Past three places, it says how many more.
  assert.match(excerptOf(rows[2]).text(), /\+2 more$/, `the line does not say how many more: ${excerptOf(rows[2]).text()}`);
  // A message too short to fold has no line.
  assert.equal(excerptOf(rows[3]), undefined);
  // Under Links the row shows its links, and the line goes with its text; cleared, it goes.
  assert.match(cssBlock('.messages li[data-links-view="true"] > .search-excerpt'), /display:\s*none !important/);
  await page.el("search-field").setValue("");
  assert.equal(excerptOf(rows[0]), undefined, "the line outlived the search");
  // A tap on it is a tap on the row, which opens the message.
  await typeAndPause(page, "engine");
  await tapRowAt(page, excerptOf(rows[0]));
  assert.equal(rows[0].getAttribute("data-collapsed"), "false", "a tap on the line did not open the message");
});

/** A tap on `node`, inside a row, as a finger makes it: on the row it is in. */
async function tapRowAt(page, node) {
  let row = node;
  while (row && !row.hasClass("discord-message")) row = row.parentNode;
  await row.dispatch("click", { target: node });
  await page.settle();
}

test("`#249 search-highlight`: a redraw with the same search writes nothing, and reads no row's layout", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, Array.from({ length: 12 }, (_unused, i) =>
    message({ id: String(i + 1), content: i % 2 ? `engine report ${i}` : `${"long words. ".repeat(30)}engine at the end ${i}` })));
  await typeAndPause(page, "engine");
  const marks = page.el("discord-log").descendants().filter((n) => n.tagName === "mark").length;
  assert.ok(marks >= 12, `the fixture marked ${marks} hits`);
  const made = page.createdTags.length;
  const replace = FakeElement.prototype.replaceChildren;
  const rect = FakeElement.prototype.getBoundingClientRect;
  let replaced = 0;
  let measured = 0;
  const list = page.el("discord-log");
  FakeElement.prototype.replaceChildren = function counted(...kids) {
    if (nodeWithinFixture(list, this)) replaced += 1;
    return replace.apply(this, kids);
  };
  FakeElement.prototype.getBoundingClientRect = function counted() {
    if (nodeWithinFixture(list, this)) measured += 1;
    return rect.call(this);
  };
  try {
    // The same query again — a redraw, as a poll's or a keystroke's — and the same rows.
    await page.el("search-field").setValue("engine");
    await page.settle();
    assert.equal(marksPending(page), 0, "the same search put off a marking it has already made");
  } finally {
    FakeElement.prototype.replaceChildren = replace;
    FakeElement.prototype.getBoundingClientRect = rect;
  }
  assert.equal(builtSince(page, made, "mark"), 0, "the same search marked its hits again");
  assert.equal(replaced, 0, "the same search wrote into a row");
  assert.ok(measured <= 4, `the same search read the layout of ${measured} rows`);
});

/**
 * Each row's gap lines as `[id, above, below]`, the words of each or null, for the rows drawing any:
 * the days a line says (`#250 date-separators`) and a day rule of its own left out.
 */
function seamsOf(page, list = "discord-log") {
  const words = (line) => (line ? lineText(line).replace(/ · [^…]+ …$/, " …") : null);
  return page.el(list).children.map((li) => {
    const line = (below) => li.children.find((kid) => kid.hasClass("row-seam") && kid.hasClass("row-seam-below") === below);
    const above = li.getAttribute("data-seam") === "gap" ? line(false) : null;
    const under = li.getAttribute("data-seam-below") === "gap" ? line(true) : null;
    return [li.getAttribute("data-id"), words(above), words(under)];
  }).filter(([, above, below]) => above !== null || below !== null);
}

test("`#249 search-highlight`: a search draws a line in each gap it leaves, counting the messages in it, before the first match and after the last", async () => {
  const page = newPage();
  await signIn(page);
  const messages = authorChannel();
  // 4 and 5 are one combined row: a gap counts messages, not rows.
  messages[4] = { ...messages[4], timestamp: new Date(Date.parse(messages[3].timestamp) + 1500).toISOString(), ...CODER };
  const rows = await showDiscord(page, messages);
  assert.ok(rows.some((li) => li.getAttribute("data-ids") === "4 5"), "the fixture made no combined row");
  assert.deepStrictEqual(seamsOf(page), [], "a line was drawn with no filter on");
  await search(page, "deploy");
  await page.settle();
  // deploy is in 4 (with 5), 8 and 12; 1-3 lead, 6-7 between, 9-11 between, nothing trails.
  assert.deepStrictEqual(seamsOf(page), [
    ["4", "… 3 unmatched messages …", null],
    ["8", "… 2 unmatched messages …", null],
    ["12", "… 3 unmatched messages …", null],
  ]);
  const line = rows.find((li) => li.getAttribute("data-id") === "8").children[0];
  assert.equal(line.className, "row-seam", "the line is not the row's first child, ahead of its message");
  // `#439 search-unmatched-separators`: a gap's line is pressed to show its run, as a run's line is.
  assert.equal(line.tagName, "button", "a gap's line is not a control to a screen reader either");
  assert.match(line.getAttribute("aria-label"), /^Show 2 unmatched messages( · .+)?$/);
  assert.equal(rows.find((li) => li.getAttribute("data-id") === "8").getAttribute("data-seam"), "gap");
  // A trailing gap is drawn below the last match; one message is said as one.
  await page.el("search-field").setValue("step 8");
  assert.deepStrictEqual(seamsOf(page), [["8", "… 7 unmatched messages …", "… 4 unmatched messages …"]]);
  await page.el("search-field").setValue("message 11");
  assert.deepStrictEqual(seamsOf(page), [["11", "… 10 unmatched messages …", "… 1 unmatched message …"]]);
  assert.equal(rows.find((li) => li.getAttribute("data-id") === "11").getAttribute("data-seam-below"), "gap");
  // Cleared, every line goes, and its row's attribute with it.
  await page.el("search-field").setValue("");
  assert.deepStrictEqual(seamsOf(page), [], "a line outlived the search");
  assert.ok(rows.every((li) => li.getAttribute("data-seam") !== "gap" && !li.hasAttribute("data-seam-below")),
    "a row kept the attribute of a gap that went");
  // Whose messages and Links choose rows too, and draw their gaps; Pinned alone is its own list.
  await chooseAuthor(page, "mine");
  assert.deepStrictEqual(seamsOf(page), [
    ["3", "… 2 unmatched messages …", null], ["6", "… 2 unmatched messages …", null],
    ["9", "… 2 unmatched messages …", null], ["12", "… 2 unmatched messages …", null],
  ]);
  await chooseAuthor(page, "everyone");
  await page.el("links-filter").click();
  await page.settle();
  assert.deepStrictEqual(seamsOf(page), [["6", "… 5 unmatched messages …", null], ["7", null, "… 5 unmatched messages …"]]);
});

test("`#249 search-highlight`: the Pinned list draws gaps only while a filter chooses among the pins", async () => {
  const page = newPage();
  const messages = authorChannel();
  for (const n of [2, 3, 4, 8, 11]) page.pinned.set(String(n), storedPin(messages[n - 1], n));
  page.servePinsRevision = true;
  page.pinsRevision = 5;
  await signIn(page);
  await showDiscord(page, messages);
  await showPinned(page);
  assert.deepStrictEqual(seamsOf(page, "pinned-log"), [], "the pins drew a gap where nothing is hidden");
  await page.el("search-field").setValue("deploy");
  assert.deepStrictEqual(seamsOf(page, "pinned-log"), [
    ["4", "… 2 unmatched messages …", null], ["8", null, "… 1 unmatched message …"],
  ]);
});

test("`#249 search-highlight`: a gap never meets a collapsed run, and the lines are drawn in room of their own, quietly; a gap's is a control, a day's is not", async () => {
  const page = newPage();
  await signIn(page);
  const messages = authorChannel();
  for (const m of messages) if (Number(m.id) < 10) page.dealtWith.add(m.id);
  await showDiscord(page, messages);
  await chooseReadMode(page, "Fold read");
  assert.ok(collapsedRows(page).length > 0, "the fixture collapsed nothing");
  assert.deepStrictEqual(seamsOf(page), [], "Fold read drew a gap");
  await search(page, "deploy");
  await page.settle();
  assert.deepStrictEqual(collapsedRows(page), [], "a run stayed collapsed beside a gap");
  assert.deepStrictEqual(seamsOf(page).map(([id]) => id), ["4", "8", "12"]);
  // The stylesheet: fixed room above the row, the line in it, the run's quiet dashed shape.
  assert.match(cssBlock('#discord-log li.discord-message[data-seam="gap"]'), /--seam-room:\s*var\(--gap-room\)/);
  assert.match(cssBlock("#discord-log li.discord-message[data-seam]"), /margin-top:\s*var\(--seam-room\)/);
  assert.match(cssBlock("#discord-log li.discord-message[data-seam-below]"), /margin-bottom:\s*var\(--gap-room\)/);
  const line = cssBlock("#discord-log .row-seam");
  assert.match(line, /position:\s*absolute/);
  assert.match(line, /bottom:\s*calc\(100% \+ 1px\)/);
  assert.match(line, /pointer-events:\s*none/, "a stray tap could land on a day's line");
  // `#439 search-unmatched-separators`: but a gap's line is a button in that shape, and takes its tap.
  assert.match(cssBlock("#discord-log button.row-seam"), /pointer-events:\s*auto/, "a gap's line takes no tap");
  assert.match(cssBlock("#discord-log button.row-seam"), /cursor:\s*pointer/);
  assert.match(line, /color:\s*var\(--muted\)/);
  assert.match(line, /filter:\s*none !important/, "a receding row would drain the line with it");
  assert.match(cssBlock("#discord-log .row-seam::before"), /border-top:\s*1px dashed var\(--edge\)/);
  assert.match(cssBlock("#pinned-log .row-seam"), /position:\s*absolute/, "the Pinned list draws no lines");
  // A row the line is drawn on recedes in its box, as the bookmark's does, so the line keeps its colour.
  assert.match(cssBlock("#discord-log li.discord-message[data-seam][data-who]"), /opacity:\s*1/);
  assert.match(cssBlock("#discord-log li.discord-message[data-seam]::after"), /--row-veil/);
  // ...and the bookmark's line, when it is on the same row, nearer the row, above it.
  assert.match(cssBlock('#discord-log li.discord-message[data-seam][data-bookmark="above"]'),
    /margin-top:\s*calc\(var\(--seam-room\) \+ var\(--bookmark-room\)\)/);
});

test("`#249 search-highlight`: a redraw that changes nothing writes no line, and the lines move with the search", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, authorChannel());
  await search(page, "deploy");
  await page.settle();
  const lines = page.el("discord-log").descendants().filter((node) => node.hasClass("row-seam"));
  assert.equal(lines.length, 3);
  const set = FakeElement.prototype.setAttribute;
  let written = 0;
  FakeElement.prototype.setAttribute = function counted(name, value) {
    if (name === "data-seam" || name === "data-seam-below" || (lines.includes(this) && name === "aria-label")) written += 1;
    return set.call(this, name, value);
  };
  try {
    await page.el("search-field").setValue("deploy");
    await page.settle();
  } finally {
    FakeElement.prototype.setAttribute = set;
  }
  assert.equal(written, 0, "a redraw that changed nothing wrote the lines again");
  assert.equal(builtSince(page, 0, "div") > 0, true);
  assert.deepStrictEqual(page.el("discord-log").descendants().filter((node) => node.hasClass("row-seam")), lines,
    "the same search made its lines again");
});

test("`#439 search-unmatched-separators`: tapping an unmatched line shows that run, and only that run, until the search changes", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, authorChannel());
  await search(page, "deploy");
  await page.settle();
  const shownIds = () => rows.filter((li) => !li.hasClass("search-hidden")).map((li) => li.getAttribute("data-id"));
  assert.deepStrictEqual(shownIds(), ["4", "8", "12"]);
  // The gap above 8 stands for 5, 6 and 7. It is a button, as a collapsed run's line is.
  const line = rows.find((li) => li.getAttribute("data-id") === "8").children
    .find((kid) => kid.hasClass("row-seam") && !kid.hasClass("row-seam-below"));
  assert.equal(line.tagName, "button", "an unmatched line is not a control a tap can open");
  assert.match(line.getAttribute("aria-label"), /^Show 3 unmatched messages/);
  // A finger on the line is on the line, not on the row under it: no hold, no swipe.
  await rows.find((li) => li.getAttribute("data-id") === "8").dispatch("pointerdown",
    { pointerId: 1, pointerType: "touch", clientX: 10, clientY: 10, target: line });
  assert.equal(page.expireTimers(HOLD_MS), 0, "a finger on an unmatched line armed its row's hold");
  await line.click();
  await page.settle();
  assert.deepStrictEqual(shownIds(), ["4", "5", "6", "7", "8", "12"], "the tap did not show exactly its run");
  assert.deepStrictEqual(seamsOf(page), [
    ["4", "… 3 unmatched messages …", null],
    ["12", "… 3 unmatched messages …", null],
  ], "another run's line moved, or the opened one stayed");
  // What the bar counts does not change: shown is not matched.
  assert.match(page.el("search-count").textContent, /^3 of 12 loaded$/);
  // A trailing run opens from the line below its last match.
  await page.el("search-field").setValue("step 8");
  await page.settle();
  assert.deepStrictEqual(shownIds(), ["8"], "changing the search kept a run the old one had opened");
  const below = rows.find((li) => li.getAttribute("data-id") === "8").children
    .find((kid) => kid.hasClass("row-seam") && kid.hasClass("row-seam-below"));
  await below.click();
  await page.settle();
  assert.deepStrictEqual(shownIds(), ["8", "9", "10", "11", "12"]);
  assert.deepStrictEqual(seamsOf(page), [["8", "… 7 unmatched messages …", null]]);
});

test("`#439 search-unmatched-separators`: with Fold read and a search both on, a read run is one unmatched line, and closing the search folds it again", async () => {
  const page = newPage();
  await signIn(page);
  const messages = authorChannel();
  for (const m of messages) if (Number(m.id) < 10) page.dealtWith.add(m.id);
  const rows = await showDiscord(page, messages);
  await chooseReadMode(page, "Fold read");
  assert.ok(collapsedRows(page).length > 0, "the fixture folded nothing");
  await search(page, "deploy");
  await page.settle();
  // One line, the search's: no collapsed run stands beside a gap, read and unmatched alike.
  assert.deepStrictEqual(collapsedRows(page), [], "a read run stayed collapsed under the search");
  assert.deepStrictEqual(seamsOf(page).map(([id]) => id), ["4", "8", "12"]);
  const line = rows.find((li) => li.getAttribute("data-id") === "8").children
    .find((kid) => kid.hasClass("row-seam") && !kid.hasClass("row-seam-below"));
  await line.click();
  await page.settle();
  assert.deepStrictEqual(rows.filter((li) => !li.hasClass("search-hidden")).map((li) => li.getAttribute("data-id")),
    ["4", "5", "6", "7", "8", "12"], "the read messages the line stood for were not shown by it");
  assert.deepStrictEqual(collapsedRows(page), [], "showing them collapsed them again under the search");
  await page.el("search-toggle").click();
  await page.settle();
  assert.deepStrictEqual(seamsOf(page), [], "an unmatched line outlived the search");
  assert.ok(collapsedRows(page).length > 0, "closing the search did not put Fold read back");
});

test("`#439 search-unmatched-separators`: the search stays on screen and its lines are drawn inside a thread opened under it", async () => {
  const page = await threadPage();
  await search(page, "root");
  await page.settle();
  await pickThread(page, "thread:spaces/A/threads/one & two");
  assert.equal(page.el("thread-heading").hidden, false, "the thread did not open");
  assert.equal(page.el("search-field").hidden, false, "the search went off screen inside the thread");
  assert.equal(page.el("search-count").hidden, false, "the count went with it");
  assert.match(page.el("search-count").textContent, /^1 of 2 loaded$/, "the count does not speak for the thread");
  assert.deepStrictEqual(seamsOf(page), [["201", null, "… 1 unmatched message …"]],
    "no line in the thread says what the search leaves out of it");
  const below = page.el("discord-log").children[0].children
    .find((kid) => kid.hasClass("row-seam") && kid.hasClass("row-seam-below"));
  await below.click();
  await page.settle();
  assert.deepStrictEqual(page.el("discord-log").children.filter((li) => !li.hasClass("search-hidden"))
    .map((li) => li.getAttribute("data-id")), ["201", "202"], "the line did not show the thread's rest");
});

test("`#439 search-unmatched-separators`: among the cards a gap counts the messages in its threads, exactly or as at least, and opens", async () => {
  const page = await threadPage();
  await twoColumns(page);
  const cards = page.el("side-cards").children;
  assert.equal(cards.length, 2, "the fixture drew other than its two cards");
  // The first thread is two messages, counted exactly; the second's count is not exact, and this
  // page holds its root and one reply: at least two.
  await search(page, "second");
  await page.settle();
  assert.deepStrictEqual(seamsOf(page, "side-cards"),
    [["spaces/A/threads/another", "… 2 unmatched messages in 1 thread …", null]],
    "the card the search keeps does not say what whole thread it skipped");
  await page.el("search-field").setValue("first");
  await page.settle();
  assert.deepStrictEqual(seamsOf(page, "side-cards"),
    [["spaces/A/threads/one & two", null, "… at least 2 unmatched messages in 1 thread …"]],
    "an inexact thread is not counted as at least what this page holds of it");
  const below = cards.find((li) => li.getAttribute("data-id") === "spaces/A/threads/one & two").children
    .find((kid) => kid.hasClass("row-seam") && kid.hasClass("row-seam-below"));
  assert.equal(below.tagName, "button", "a card gap's line is not a control");
  assert.match(below.getAttribute("aria-label"), /^Show at least 2 unmatched messages in 1 thread$/);
  await below.click();
  await page.settle();
  assert.deepStrictEqual(cards.filter((li) => !li.hasClass("search-hidden") && !li.hidden)
    .map((li) => li.getAttribute("data-id")), ["spaces/A/threads/one & two", "spaces/A/threads/another"],
    "the tap did not show the thread the line stood for");
  assert.deepStrictEqual(seamsOf(page, "side-cards"), [], "the opened card's line stayed");
  // Typing away and back to the same words does not bring the opened run back un-tapped.
  await page.el("search-field").setValue("first thread");
  await page.settle();
  await page.el("search-field").setValue("first");
  await page.settle();
  assert.deepStrictEqual(cards.filter((li) => !li.hasClass("search-hidden") && !li.hidden)
    .map((li) => li.getAttribute("data-id")), ["spaces/A/threads/one & two"],
    "a search typed away and back resurrected an opened card run");
  // The bar still counts matches, not what was shown by hand.
  assert.match(page.el("search-count").textContent, /loaded|thread/);
  await page.el("search-field").setValue("thread root");
  await page.settle();
  assert.deepStrictEqual(seamsOf(page, "side-cards"), [], "a card gap outlived a search that matches both cards");
});

test("`#439 search-unmatched-separators`: the Threads list in one column draws the same lines over its cards", async () => {
  const page = await threadPage();
  await page.el("channel-view-threads").click();
  await page.settle();
  const list = page.el("thread-list");
  assert.equal(list.hidden, false);
  await search(page, "First");
  await page.settle();
  assert.deepStrictEqual(list.children.filter((li) => !li.hasClass("search-hidden"))
    .map((li) => li.getAttribute("data-context-id")), ["spaces/A/threads/one & two"]);
  const first = list.children[0];
  const below = first.children.find((kid) => kid.hasClass("row-seam") && kid.hasClass("row-seam-below"));
  assert.ok(below, "no line under the kept card counts the thread left out");
  assert.equal(lineText(below), "… at least 2 unmatched messages in 1 thread …");
  assert.equal(first.getAttribute("data-seam-below"), "gap");
  await below.click();
  await page.settle();
  assert.deepStrictEqual(list.children.filter((li) => !li.hasClass("search-hidden"))
    .map((li) => li.getAttribute("data-context-id")),
    ["spaces/A/threads/one & two", "spaces/A/threads/another"], "the tap did not show the other thread's card");
  await page.el("search-toggle").click();
  await page.settle();
  assert.ok(list.children.every((li) => li.getAttribute("data-seam") !== "gap" && !li.hasAttribute("data-seam-below")),
    "a card kept a gap's mark after the search closed");
  // The stylesheet: a card keeps a gap's room above itself, and the line stands in it.
  assert.match(cssBlock('#thread-list .thread-card[data-seam="gap"]'), /margin-top:\s*var\(--gap-room\)/);
  assert.match(cssBlock("#side-cards .row-seam.card-seam"), /position:\s*absolute/);
  assert.match(cssBlock("#side-cards .row-seam.card-seam::before"), /border-top:\s*1px dashed var\(--edge\)/);
});

test("`#249 search-highlight`: the cards of the right column are marked where the search finds them", async () => {
  const page = await threadPage();
  await twoColumns(page);
  await typeAndPause(page, "root");
  const cards = page.el("side-cards").children.filter((li) => !li.hasClass("search-hidden"));
  assert.ok(cards.length > 0, "no card matched the fixture's search");
  for (const card of cards) {
    const text = card.descendants().find((node) => node.hasClass("side-card-text"));
    assert.deepStrictEqual(text.descendants().filter((node) => node.tagName === "mark").map((node) => node.text()), ["root"],
      `a card's text is not marked: ${text.text()}`);
  }
  await page.el("search-toggle").click();
  await page.settle();
  assert.equal(page.el("side-cards").descendants().filter((node) => node.tagName === "mark").length, 0, "a card kept its mark");
});

// --- searching further back -------------------------------------------------------------------------
//
// `#262 search-load-older`. The owner, 2026-10-12: "searching for a term XYZ in two column view found
// zero hits; in one column, scrolling up while searching loaded older pages and found 3 ... 2 column
// view should not PRECLUDE loading hits". These check the page's half: Hide read stepping aside under a
// filter, the count in messages, and the bounded walk the step-back control and Enter start.

test("`#262 search-load-older`: Hide read steps aside while a filter chooses rows, and comes back when the search closes", async () => {
  const page = newPage();
  const data = threadData();
  page.threadingSupported = true;
  page.threads = data.threads;
  // The announcement is read: Hide read leaves it out.
  page.dealtWith = new Set(["200"]);
  await signIn(page);
  await showDiscord(page, data.messages);
  await chooseReadMode(page, "Hide read");
  assert.ok(!shownIds(page).includes("200"), "Hide read drew a read message");
  assert.equal(page.el("inbox-note").hidden, false, "Hide read's note is not up");
  // A search for it: read, and found, greyed as read rows are.
  await search(page, "announcement");
  await page.settle();
  const found = rowWithId(page, "200");
  assert.ok(found && !found.hasClass("search-hidden"), "the read match was not drawn under the search");
  assert.equal(found.getAttribute("data-archived"), "true", "the read match is not drawn as read");
  assert.equal(page.el("inbox-note").hidden, true, "Hide read's note stayed up while it stood aside");
  assert.equal(page.el("search-empty").hidden, true, "the list said nothing matched");
  assert.equal(page.el("todo-filter-label").text(), "Hide read", "the read mode the reader chose changed");
  assert.equal(savedUiState(page).read, "hide", "the read mode kept on this device changed");
  // Another keystroke is no redraw: only the first term and the last flip it.
  const drawn = rowWithId(page, "200");
  await search(page, "announcement main");
  await page.settle();
  assert.equal(rowWithId(page, "200"), drawn, "a keystroke under the search drew the list again");
  // Closed, Hide read is back.
  await page.el("search-toggle").click();
  await page.settle();
  assert.ok(!shownIds(page).includes("200"), "closing the search did not bring Hide read back");
  assert.equal(page.el("inbox-note").hidden, false);
  // Links and whose messages are filters choosing rows too.
  await search(page, "");
  await page.el("author-filter").click();
  await page.settle();
  await page.el("author-filter").click();
  await page.settle();
  assert.equal(authorState(page)[0], "theirs");
  assert.ok(shownIds(page).includes("200"), "Theirs under Hide read left a read message of theirs out");
});

test("`#262 search-load-older`: without threads Hide read is a list of its own, and stays in force under a search", async () => {
  const page = newPage();
  await signIn(page);
  page.dealtWith = new Set(["8000000000000000000"]);
  await showDiscord(page, backlog(3));
  await turnTodoOn(page);
  assert.ok(!shownIds(page).includes("8000000000000000000"));
  await search(page, "message");
  await page.settle();
  assert.ok(!shownIds(page).includes("8000000000000000000"), "the search drew a message Hide read's own list left out");
  assert.equal(page.el("inbox-note").hidden, false);
});

test("`#262 search-load-older`: the count is in messages, so what is shown and the gaps' lines add up to it", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [
    message({ id: "8600000000000000001", content: "first half of a long post about the engine", timestamp: "2026-08-19T04:31:00.000Z" }),
    message({ id: "8600000000000000002", content: "second half of it", timestamp: "2026-08-19T04:31:01.000Z" }),
    message({ id: "8600000000000000003", content: "lunch", timestamp: "2026-08-19T05:31:00.000Z" }),
    message({ id: "8600000000000000004", content: "dinner", timestamp: "2026-08-19T06:31:00.000Z" }),
  ]);
  assert.equal(page.el("discord-log").children.length, 3, "the fixture did not combine the post's halves");
  await search(page, "engine");
  await page.settle();
  assert.equal(page.el("search-count").textContent, "2 of 4 loaded", "a combined row was counted as one");
});

/** A read held until `open()`. */
function searchHold() {
  let open = () => {};
  const promise = new Promise((resolve) => { open = resolve; });
  return { promise, open };
}

/** The rows of `list` the filters let through, by id. */
const matchedIds = (page, list = "discord-log") =>
  page.el(list).children.filter((li) => !li.hidden && !li.hasClass("search-hidden")).map((li) => li.getAttribute("data-id"));

const SEARCH_OLDER_PAGES = sourceConstant("SEARCH_OLDER_PAGES");
const SEARCH_OLDER_TAP_MS = sourceConstant("SEARCH_OLDER_TAP_MS");

/** The older reads so far. */
const searchReads = (page) => page.pagesServed.filter((path) => /[?&]before=/.test(path));

/** Let a walk run to its end. */
async function settleWalk(page) {
  for (let i = 0; i < 40; i += 1) await page.settle();
}

/** A page signed in on `searchHistory`'s channel, at its newest page. */
async function searchPage(options = {}) {
  const page = newPage();
  const all = searchHistory(page, options);
  await signIn(page);
  await showDiscord(page, page.messages);
  return { page, all };
}

test("`#262 search-load-older`: while a filter chooses rows the step back offers to search further back, naming the day it reaches", async () => {
  const { page, all } = await searchPage({ content: (i) => (i === 2 ? "the engine stalled" : `message ${i}`) });
  const chip = page.el("load-older");
  assert.equal(chip.hidden, false);
  assert.equal(chip.textContent, "Older messages", "the step back offered a search with no filter on");
  assert.equal(chip.getAttribute("aria-label"), null);
  await search(page, "engine");
  await page.settle();
  const day = searchMoment(page, all[20].timestamp);
  assert.equal(chip.textContent, `Search before ${day}`);
  assert.equal(chip.disabled, false);
  assert.match(chip.getAttribute("aria-label"),
    /^Search before .+: fetches up to 250 older messages from Discord and searches them here\. Enter in the search field does the same\.$/);
  // `#578 search-status-clarity`. The empty sentence says how many it searched, where, and how far
  // back with no gaps: a list read back from the newest page by cursor is one stretch.
  assert.equal(page.el("search-empty").hidden, false);
  assert.equal(page.el("search-empty").textContent,
    `Searched 4 messages in this browser, a complete history back to ${day} (no gaps). None match.`);
  assert.equal(page.el("search-in-all").hidden, true, "Search All was offered where there are no threads");
  // Links alone is a filter choosing rows; so is whose messages.
  await search(page, "");
  await page.settle();
  assert.equal(chip.textContent, "Older messages", "an open search with nothing typed offered a search");
  await page.el("links-filter").click();
  await page.settle();
  assert.equal(chip.textContent, `Search before ${day}`, "Links alone did not offer a search further back");
  await page.el("links-filter").click();
  await page.el("author-filter").click();
  await page.settle();
  assert.equal(chip.textContent, `Search before ${day}`, "Mine alone did not offer a search further back");
  // Over the Pinned list there is nothing older to find.
  await page.el("pinned-filter").click();
  await page.settle();
  assert.equal(chip.hidden, true, "the step back was offered over the Pinned list");
  // A thumb's target: the 44px square drawn as nothing.
  assert.match(cssBlock("#load-older::after"), /inset:\s*min\(0px, calc\(\(100% - 2\.75rem\) \/ 2\)\)/);
  assert.match(cssBlock("#side-older::after"), /content:\s*""/);
  assert.match(cssBlock("#load-older"), /position:\s*relative/);
});

test("`#262 search-load-older`: a tap reads older pages until one brings three new matches, and says what it found under the step back", async () => {
  const hits = new Set([25, 21, 17, 16]);
  const { page, all } = await searchPage({ pages: 8, content: (i) => (hits.has(i) ? `engine note ${i}` : `message ${i}`) });
  await search(page, "engine");
  await page.settle();
  assert.deepStrictEqual(matchedIds(page), []);
  await page.el("load-older").click();
  await settleWalk(page);
  assert.equal(searchReads(page).length, 3, "the walk did not stop at the end of the page that brought the third match");
  assert.deepStrictEqual(matchedIds(page), [16, 17, 21, 25].map((i) => all[i].id));
  const day = searchMoment(page, all[16].timestamp);
  assert.equal(page.el("search-older-note").hidden, false);
  assert.equal(page.el("search-older-note").textContent,
    `Found 4 more matches among 12 older messages fetched from Discord, back to ${day}.`);
  assert.match(HTML_CODE, /<p id="search-older-note" class="hint search-older-note" role="status" hidden><\/p>/);
  assert.match(HTML_CODE, /<p id="side-search-older-note" class="hint search-older-note" role="status" hidden><\/p>/);
  assert.equal(page.el("load-older").textContent, `Search before ${day}`);
  assert.equal(page.el("search-empty").hidden, true);
  // Nothing of it went to the floating status line.
  assert.ok(page.el("status-line").hidden || !/Found/.test(page.el("status").textContent), "the walk's result went to the status line");
  // The note stays while the search does; another search takes it away.
  await page.el("links-filter").click();
  await page.settle();
  assert.equal(page.el("search-older-note").hidden, true, "a change of filter left the last walk's note up");
  // The count is in messages: every match, of everything loaded.
  await page.el("links-filter").click();
  await page.settle();
  assert.equal(page.el("search-count").textContent, "4 of 16 loaded");
});

test("`#262 search-load-older`: a walk reads at most five pages, and one that finds nothing says how far back in the empty sentence", async () => {
  const { page, all } = await searchPage({ pages: 10 });
  await search(page, "engine");
  await page.settle();
  await page.el("load-older").click();
  await settleWalk(page);
  assert.equal(searchReads(page).length, SEARCH_OLDER_PAGES, "the walk was not bounded");
  // `#578 search-status-clarity`. A press that found nothing says what it fetched, and from where: over a
  // list the search had emptied it used to say nothing, and the owner took it for a press that did nothing.
  const back = searchMoment(page, all[16].timestamp);
  assert.equal(page.el("search-older-note").hidden, false, "a walk that found nothing said nothing");
  assert.equal(page.el("search-older-note").textContent,
    `None of the 20 older messages fetched from Discord, back to ${back}, match.`);
  assert.equal(page.el("search-empty").textContent,
    `Searched 24 messages in this browser, a complete history back to ${back} (no gaps). None match.`);
  assert.equal(page.el("load-older").textContent, `Search before ${back}`, "the control did not move back");
  // Another tap is another walk, from where this one stopped.
  await page.el("load-older").click();
  await settleWalk(page);
  assert.equal(searchReads(page).length, SEARCH_OLDER_PAGES + 4, "the second tap did not walk on to the start");
  assert.equal(page.el("load-older").hidden, true);
  const first = searchMoment(page, all[0].timestamp);
  assert.equal(page.el("search-older-note").textContent,
    `Reached the start of this channel (${first}); none of the 16 older messages fetched from Discord match.`);
  assert.equal(page.el("search-empty").textContent,
    `Searched all 40 messages of this channel in this browser, back to the first one (${first}). None match.`);
});

test("`#262 search-load-older`: the walk starts no page its last page's time says would end past fifteen seconds", async () => {
  const { page, all } = await searchPage({ pages: 10, content: (i) => (i === 33 ? "engine" : `message ${i}`) });
  page.olderTakes = 6000;
  await search(page, "engine");
  await page.settle();
  await page.el("load-older").click();
  await settleWalk(page);
  assert.equal(searchReads(page).length, 2, "a page that would end past the budget was started");
  assert.equal(page.el("search-older-note").textContent,
    `Found 1 more match among 8 older messages fetched from Discord in 15 s, back to ${searchMoment(page, all[28].timestamp)}.`);
});

test("`#262 search-load-older`: Enter in the search field walks back where the step back offers it, and not while composing", async () => {
  const { page } = await searchPage({ pages: 10 });
  await search(page, "");
  let event = await press(page, page.el("search-field"), "Enter");
  assert.equal(searchReads(page).length, 0, "Enter with nothing typed read older pages");
  await page.el("search-field").setValue("engine");
  await page.settle();
  event = await press(page, page.el("search-field"), "Enter", { isComposing: true });
  await settleWalk(page);
  assert.equal(searchReads(page).length, 0, "Enter that an input method was composing with walked back");
  event = await press(page, page.el("search-field"), "Enter");
  assert.equal(event.defaulted, false, "the field's Enter was left to the page's keys");
  await settleWalk(page);
  assert.equal(searchReads(page).length, SEARCH_OLDER_PAGES, "Enter in the field did not search further back");
  // The list of keys says so.
  assert.ok(page.el("keyboard-keys").text().includes("In the search field: search older messages"), "the list of keys does not name Enter in the search field");
});

test("`#262 search-load-older`: Stop aborts the read on the wire, says Stopping… until it settles, and is no failure; a second tap at once is the same tap", async () => {
  const { page } = await searchPage({ pages: 10 });
  page.enableAbort();
  await search(page, "engine");
  await page.settle();
  const chip = page.el("load-older");
  page.olderHold = searchHold();
  void chip.click();
  await page.settle();
  assert.equal(chip.textContent, "Fetching older messages… · Stop");
  assert.equal(chip.disabled, false);
  // A double tap does not stop the walk it started.
  void chip.click();
  await page.settle();
  assert.equal(chip.textContent, "Fetching older messages… · Stop", "a second tap at once stopped the walk");
  page.setClock(page.clock() + SEARCH_OLDER_TAP_MS);
  void chip.click();
  assert.equal(chip.textContent, "Stopping…");
  assert.equal(chip.disabled, true);
  assert.equal(page.olderOptions[0].signal.aborted, true, "Stop did not abort the read on the wire");
  await settleWalk(page);
  assert.equal(searchReads(page).length, 1, "the walk read on after Stop");
  assert.equal(page.el("error").hidden, true, "Stop was reported as a failure");
  assert.match(chip.textContent, /^Search before /);
  assert.equal(chip.disabled, false);
  assert.equal(page.el("search-older-note").textContent, "Stopped before any older messages arrived.");
});

test("`#262 search-load-older`: a message arriving mid-walk that matches neither stops it nor counts as found", async () => {
  const { page, all } = await searchPage({ pages: 10, content: (i) => (i === 30 ? "engine trouble" : `message ${i}`) });
  const stream = page.stream();
  await search(page, "engine");
  await page.settle();
  page.olderHold = searchHold();
  const hold = page.olderHold;
  void page.el("load-older").click();
  await page.settle();
  await deliver(page, stream, sseMessage(message({ id: "8700000000000009999", content: "engine is back",
    timestamp: new Date(Date.UTC(2026, 7, 19, 13, 0, 0)).toISOString() })));
  assert.ok(matchedIds(page).includes("8700000000000009999"), "the live match is not shown");
  hold.open();
  await settleWalk(page);
  assert.equal(searchReads(page).length, SEARCH_OLDER_PAGES, "a live match stopped the walk, or one older match did");
  assert.equal(page.el("search-older-note").textContent,
    `Found 1 more match among 20 older messages fetched from Discord, back to ${searchMoment(page, all[16].timestamp)}.`);
});

test("`#262 search-load-older`: the step back holds still while the matches open under it, behind a line counting what they skip", async () => {
  const { page, all } = await searchPage({ pages: 8, content: (i) => (i === 22 ? "engine" : longMessage(`message ${i}`)) });
  await search(page, "engine");
  await page.settle();
  const area = page.el("scroll-area");
  const chip = page.el("load-older");
  const top = chip.getBoundingClientRect().top;
  await chip.click();
  await settleWalk(page);
  assert.deepStrictEqual(matchedIds(page), [all[22].id]);
  assert.equal(area.scrollTop, 0, "the walk moved the reader off the head of the list");
  assert.equal(chip.getBoundingClientRect().top, top, "the step back moved while the matches arrived");
  // `#250`'s line before the first match counts the unmatched messages between, the walked ones too.
  assert.deepStrictEqual(seamsOf(page), [[all[22].id, "… 14 unmatched messages …", "… 9 unmatched messages …"]]);
  // `#249`'s marks are on what it found.
  await press(page, page.el("search-field"), "Enter");
  assert.ok(rowWithId(page, all[22].id).descendants().some((node) => node.tagName === "mark" || node.tagName === "MARK"),
    "the match found further back is not marked");
});

/** Two columns over `threadHistory`'s channel, the long thread selected on the right. */
async function sideThreadPage(options = {}) {
  const page = newPage();
  const all = threadHistory(page, options);
  page.enableAbort();
  await signIn(page);
  await showDiscord(page, page.messages);
  await twoColumns(page);
  await openerOf(cardRow(page, page.threads[0].id)).click();
  await page.settle();
  assert.equal(page.el("side-pane").hidden, false, "the thread did not open on the right");
  return { page, all };
}

test("`#262 search-load-older`: in two columns the right column's thread walks its own older replies, from its own control", async () => {
  const { page, all } = await sideThreadPage({ content: (i) => (i === 3 ? "the engine reply" : `reply ${i}`) });
  await search(page, "engine");
  await page.settle();
  // Main holds the first message alone, and says so.
  assert.equal(page.el("search-empty").hidden, false);
  assert.match(page.el("search-empty").textContent, / Main shows a thread when a reply held here matches; All may hold more replies\.$/);
  assert.equal(page.el("search-in-all").hidden, true, "Search All was offered in two columns");
  const chip = page.el("side-older");
  assert.equal(chip.hidden, false);
  assert.equal(chip.textContent, `Search replies before ${searchMoment(page, all[21].timestamp)}`);
  await chip.click();
  await settleWalk(page);
  assert.deepStrictEqual(matchedIds(page, "side-log"), [all[3].id], "the thread's walk did not find its older reply");
  assert.match(page.el("side-search-older-note").textContent, /^Found 1 more match among \d+ older messages fetched from /);
  assert.equal(page.el("search-older-note").hidden, true, "the left column said what the right one found");
  // Enter walks the column that holds the keys: the left, given them back by [, which has nothing older
  // to search. (The card's replies button handed them to the right, `#298 thread-card-header`.)
  const reads = page.olderOptions.length;
  await press(page, page.el("side-scroll"), "[");
  await press(page, page.el("search-field"), "Enter");
  await settleWalk(page);
  assert.equal(page.olderOptions.length, reads, "Enter walked a column the keys are not in");
});

test("`#262 search-load-older`: closing the right column's thread, or leaving two columns, ends its walk where it is", async () => {
  for (const end of ["close", "narrow"]) {
    const { page } = await sideThreadPage();
    await search(page, "engine");
    await page.settle();
    page.olderHold = searchHold();
    const hold = page.olderHold;
    void page.el("side-older").click();
    await page.settle();
    assert.equal(page.el("side-older").textContent, "Fetching older messages… · Stop");
    if (end === "close") await page.el("side-close").click();
    else await page.setDesk(false);
    await page.settle();
    assert.equal(page.olderOptions[0].signal.aborted, true, `${end}: the read on the wire went on`);
    hold.open();
    await settleWalk(page);
    assert.equal(page.olderOptions.length, 1, `${end}: the walk read on into a list nobody sees`);
    assert.equal(page.el("side-search-older-note").hidden, true, `${end}: the ended walk wrote its note`);
    assert.equal(page.el("error").hidden, true, `${end}: the ended walk was reported as a failure`);
  }
});

test("`#262 search-load-older`: Hide read steps aside under the search, so a walk finds the read messages it reads", async () => {
  const page = newPage();
  const all = searchHistory(page, { threads: true, pages: 8, content: (i) => (i === 17 ? "engine log" : `message ${i}`) });
  // Everything is read but the newest page.
  page.dealtWith = new Set(all.slice(0, 28).map((m) => m.id));
  await signIn(page);
  await showDiscord(page, page.messages);
  await chooseReadMode(page, "Hide read");
  await search(page, "engine");
  await page.settle();
  assert.match(page.el("load-older").textContent, /^Search before /, "the walk was not offered under Hide read");
  await page.el("load-older").click();
  await settleWalk(page);
  assert.deepStrictEqual(matchedIds(page), [all[17].id], "the walk under Hide read did not find a read match");
  assert.equal(rowWithId(page, all[17].id).getAttribute("data-archived"), "true");
  assert.equal(page.el("search-older-note").textContent,
    `Found 1 more match among 20 older messages fetched from Discord, back to ${searchMoment(page, all[8].timestamp)}.`);
  // Closed, the search gives the list back to Hide read.
  await page.el("search-toggle").click();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), all.slice(28).map((m) => m.id), "Hide read did not come back");
});

test("`#262 search-load-older`: without threads, under Hide read, the walk is not offered and the sentence says how out", async () => {
  const page = newPage();
  searchHistory(page, { pages: 4 });
  await signIn(page);
  await showDiscord(page, page.messages);
  await turnTodoOn(page);
  await search(page, "engine");
  await page.settle();
  assert.doesNotMatch(page.el("load-older").textContent, /^Search/, "a walk was offered over Hide read's own list");
  await press(page, page.el("search-field"), "Enter");
  assert.equal(searchReads(page).length, 0, "Enter walked back over Hide read's own list");
  assert.equal(page.el("search-empty").hidden, false);
  assert.match(page.el("search-empty").textContent, / Hide read is on — press h for Show read to search older messages\.$/);
});

test("`#262 search-load-older`: a change to the search ends the walk, and aborts its read on the wire", async () => {
  const { page } = await searchPage({ pages: 10 });
  page.enableAbort();
  await search(page, "engine");
  await page.settle();
  page.olderHold = searchHold();
  const hold = page.olderHold;
  void page.el("load-older").click();
  await page.settle();
  await page.el("search-field").setValue("engine room");
  await page.settle();
  assert.equal(page.olderOptions[0].signal.aborted, true, "typing left the walk's read on the wire");
  hold.open();
  await settleWalk(page);
  assert.equal(searchReads(page).length, 1, "the walk read on under a search that had changed");
  assert.match(page.el("load-older").textContent, /^Search before /);
  assert.equal(page.el("search-older-note").hidden, true);
});

test("`#262 search-load-older`: under a filter the scroll's own step back gives way to the walk, and one already on the wire is the walk's first page", async () => {
  const { page } = await searchPage({ pages: 10 });
  await search(page, "engine");
  await page.settle();
  // At the head of the list the search emptied, a scroll reads nothing by itself: the walk is bounded,
  // and the reader's, and a page drawn and hidden there must not fire the next step on its own.
  page.el("scroll-area").scrollTop = 0;
  await page.el("scroll-area").dispatch("scroll");
  await page.settle();
  assert.equal(searchReads(page).length, 0, "a scroll under the search read an older page by itself");
  // The help says so: it is the way the owner found older matches before, and it is gone under a filter.
  assert.match(HTML_CODE, /while a search, Links or whose messages is on, scrolling to\s+the top of the list loads nothing by itself/);
  // With nothing typed it steps back as ever; typed while that step is on the wire, a tap takes it as
  // the walk's first page, and the control is not greyed out meanwhile.
  await page.el("search-field").setValue("");
  await page.settle();
  page.olderHold = searchHold();
  const hold = page.olderHold;
  page.el("scroll-area").scrollTop = 0;
  await page.el("scroll-area").dispatch("scroll");
  await page.settle();
  assert.equal(searchReads(page).length, 1, "scrolling to the head took no step back");
  await page.el("search-field").setValue("engine");
  await page.settle();
  assert.match(page.el("load-older").textContent, /^Search before /);
  assert.equal(page.el("load-older").disabled, false, "the step back was greyed out under the scroll's step");
  void page.el("load-older").click();
  await page.settle();
  assert.equal(page.el("load-older").textContent, "Fetching older messages… · Stop");
  hold.open();
  await settleWalk(page);
  assert.equal(searchReads(page).length, SEARCH_OLDER_PAGES, "the scroll's step was not the walk's first page");
});

test("`#262 search-load-older`: a card that shows its thread's summary is found by what the thread's first message says too", async () => {
  const page = newPage();
  const data = threadData();
  const [first] = data.threads;
  first.display_name = "runner-wedged";
  first.summary = "The overnight runner hung and was restarted by hand.";
  page.threadingSupported = true;
  page.threads = data.threads;
  await signIn(page);
  await showDiscord(page, data.messages);
  await twoColumns(page);
  const shown = () => page.el("side-cards").children.filter((li) => !li.hidden && !li.hasClass("search-hidden"))
    .map((li) => li.getAttribute("data-id"));
  await search(page, "first thread root");
  await page.settle();
  assert.deepStrictEqual(shown(), [first.id], "a summed-up thread was not found by its first message's words");
  assert.equal(replyHint(page, first.id), null, "a card found by its first message said it was found in its replies");
  // The summary still finds it, and a card showing its first message is not searched for it twice.
  await page.el("search-field").setValue("overnight runner");
  await page.settle();
  assert.deepStrictEqual(shown(), [first.id]);
  assert.equal(page.el("side-cards").children[1].getAttribute("data-search").split("second thread root").length, 2);
});

test("`#262 search-load-older`: in one column on Main a search nothing matches offers All, which has the replies, and keeps the search", async () => {
  const page = await threadPage("main");
  // `#606 server-side-search`: in Main a thread stands for its replies, so a word only replies say
  // keeps their roots, and All is not offered over them.
  await search(page, "answer");
  await page.settle();
  assert.deepStrictEqual(matchedIds(page), ["201", "203"], "Main did not keep the roots whose replies match");
  assert.equal(page.el("search-in-all").hidden, true, "Search All was offered over matches");
  await page.el("search-field").setValue("kestrel");
  await page.settle();
  assert.deepStrictEqual(matchedIds(page), []);
  assert.equal(page.el("search-empty").hidden, false);
  assert.match(page.el("search-empty").textContent, /^Searched .+ in this browser.+ None match\. Main shows a thread when a reply held here matches; All may hold more replies\.$/);
  const all = page.el("search-in-all");
  assert.equal(all.hidden, false, "Main offered no way to the replies");
  assert.equal(all.textContent, "Search All — thread replies too");
  await all.click();
  await page.settle();
  assert.equal(page.el("thread-select").value, "flat", "Search All did not open All");
  assert.equal(page.el("search-field").value, "kestrel", "Search All dropped the search");
  assert.deepStrictEqual(matchedIds(page), [], "All found what nothing held says");
  await page.el("search-field").setValue("answer");
  await page.settle();
  assert.deepStrictEqual(matchedIds(page), ["202", "204"], "All did not show the replies the search finds");
  assert.equal(all.hidden, true, "Search All stayed up once there were matches");
  // Not offered while something on Main matches, nor with nothing typed.
  await pickThread(page, "main");
  await page.el("search-field").setValue("root");
  await page.settle();
  assert.equal(all.hidden, true, "Search All was offered over matches");
  await page.el("search-field").setValue("");
  await page.settle();
  assert.equal(all.hidden, true, "Search All was offered with nothing typed");
});

test("`#262 search-load-older`: a page that fails ends the walk, said where every failed read is and on the status line", async () => {
  const { page } = await searchPage({ pages: 10 });
  await search(page, "engine");
  await page.settle();
  const serve = page.channelPage;
  let reads = 0;
  page.channelPage = async (path, options) => {
    if (/[?&]before=/.test(String(path)) && ++reads === 2) return json(502, { error: "chat_error", detail: "the provider returned HTTP 502" });
    return serve(path, options);
  };
  await page.el("load-older").click();
  await settleWalk(page);
  assert.equal(reads, 2, "the walk read on after a page failed");
  assert.equal(page.el("error").hidden, false, "the failed page was not reported");
  assert.equal(page.el("status").textContent, "Could not search further back — try again.");
  assert.match(page.el("load-older").textContent, /^Search before /);
  // `#578 search-status-clarity`. And under the control, until the search changes, with the reason.
  assert.equal(page.el("search-older-note").hidden, false);
  assert.match(page.el("search-older-note").textContent, new RegExp("^Could not fetch older messages from Discord: " +
    "the provider returned HTTP 502\\. Before that, 4 older messages fetched from Discord, back to .+\\. " +
    "Press the button again to retry\\.$"));
});

test("`#262 search-load-older`: a finger on the glass holds the walk's next page until it lifts", async () => {
  const { page } = await searchPage({ pages: 10 });
  await search(page, "engine");
  await page.settle();
  const area = page.el("scroll-area");
  await area.dispatch("touchstart", touchAt(300, 100));
  void page.el("load-older").click();
  await settleWalk(page);
  assert.equal(searchReads(page).length, 0, "the walk read a page under a finger");
  assert.ok(page.expireTimers(sourceConstant("PULL_WAIT_MS")) > 0, "the walk is not waiting for the finger");
  await settleWalk(page);
  assert.equal(searchReads(page).length, 0, "the walk read a page under a finger");
  await area.dispatch("touchend");
  page.expireTimers(sourceConstant("PULL_WAIT_MS"));
  await settleWalk(page);
  assert.equal(searchReads(page).length, SEARCH_OLDER_PAGES, "the walk did not go on once the finger lifted");
});

test("`#262 search-load-older`: Search All goes when two columns come, and a press left on it does not make the left column All", async () => {
  const page = await threadPage("main");
  await search(page, "kestrel");
  await page.settle();
  const all = page.el("search-in-all");
  assert.equal(all.hidden, false, "Main offered no way to the replies");
  // The left column was Main already, so nothing searched it again as two columns came.
  await twoColumns(page);
  assert.equal(all.hidden, true, "Search All stayed up in two columns");
  // A press that reaches it anyway, as a tap a moment late would, leaves the left column Main.
  await all.click();
  await page.settle();
  assert.ok(inTwoColumns(page), "the press left two columns");
  assert.equal(page.el("thread-select").value, "columns", "the press made the left column of two All");
  assert.deepStrictEqual(matchedIds(page), [], "the left column of two drew the replies");
  assert.equal(all.hidden, true);
});

test("`#262 search-load-older`: back in one column on Main, a search that still matches nothing offers Search All at once", async () => {
  const page = await threadPage("main");
  await twoColumns(page);
  await search(page, "kestrel");
  await page.settle();
  const all = page.el("search-in-all");
  assert.equal(all.hidden, true, "Search All was offered in two columns");
  assert.match(page.el("search-empty").textContent, / Main shows a thread when a reply held here matches; All may hold more replies\.$/);
  // One column again, in Main, the view last chosen in one: nothing reads or searches it again.
  await page.el("column-toggle").click();
  await page.settle();
  assert.ok(!inTwoColumns(page));
  assert.equal(page.el("thread-select").value, "main");
  assert.equal(all.hidden, false, "Search All waited for another keystroke after two columns went");
  await all.click();
  await page.settle();
  assert.equal(page.el("thread-select").value, "flat", "Search All did not open All");
  assert.deepStrictEqual(matchedIds(page), [], "All found what nothing held says");
  assert.equal(page.el("search-field").value, "kestrel", "Search All dropped the search");
  // ...and the narrow window's way out of two columns does the same.
  const second = await threadPage("main");
  await twoColumns(second);
  await search(second, "kestrel");
  await second.settle();
  await second.setDesk(false);
  assert.equal(second.el("search-in-all").hidden, false, "a window narrowed to one column on Main did not offer Search All");
});

/**
 * `#262 search-load-older`. A channel whose newest page is eight long messages that all say "engine",
 * so the list of matches is taller than the window, and three short older matches two pages back.
 */
async function matchesOnScreen(threads = false) {
  const older = new Set([37, 30, 25]);
  const made = await searchPage({ pages: 6, size: 8, threads,
    content: (i) => (i >= 40 ? longMessage(`engine ${i}`) : older.has(i) ? `engine ${i}` : `message ${i}`) });
  await search(made.page, "engine");
  await made.page.settle();
  return made;
}

test("`#262 search-load-older`: scrolled a little with matches on screen, the step back holds still and the older matches open under it", async () => {
  const { page, all } = await matchesOnScreen();
  const area = page.el("scroll-area");
  const chip = page.el("load-older");
  // The step back is a little way down the list, under the channel's line, and in view.
  const head = chip.getBoundingClientRect().top + area.scrollTop;
  assert.ok(head > 2, "the fixture has nothing above the step back, so this proves nothing");
  area.scrollTop = Math.floor(head / 2);
  await area.dispatch("scroll");
  await page.settle();
  const at = area.scrollTop;
  const top = chip.getBoundingClientRect().top;
  assert.ok(at > 1 && top > 0, "the step back is not in view at a scroll position off the head");
  const below = rowWithId(page, all[40].id).getBoundingClientRect().top;
  assert.ok(below < area.clientHeight, "no match is on screen, so this proves nothing");
  await chip.click();
  await settleWalk(page);
  assert.equal(searchReads(page).length, 2, "the walk did not stop at the page that brought the third match");
  assert.deepStrictEqual(matchedIds(page).slice(0, 3), [25, 30, 37].map((i) => all[i].id));
  assert.equal(chip.getBoundingClientRect().top, top, "the step back moved while the matches opened under it");
  assert.equal(area.scrollTop, at, "the walk scrolled the reader");
  // What it found is directly under it, and the matches that were on screen moved down to make room.
  const note = page.el("search-older-note");
  assert.equal(note.hidden, false);
  assert.ok(Math.abs(note.getBoundingClientRect().top - chip.getBoundingClientRect().bottom) <= 1, "the note is not under the step back");
  assert.ok(rowWithId(page, all[25].id).getBoundingClientRect().top < rowWithId(page, all[40].id).getBoundingClientRect().top);
  assert.ok(rowWithId(page, all[40].id).getBoundingClientRect().top > below, "the older matches did not open under the step back");
});

test("`#262 search-load-older`: scrolling while a walk reads lets the step back go, and keeps the message the reader is on instead", async () => {
  for (const threads of [false, true]) await scrollingFromWalk(threads);
});

/** The test above, on a chat service without threads or with them: each draws an older page its own way. */
async function scrollingFromWalk(threads) {
  const { page, all } = await matchesOnScreen(threads);
  const area = page.el("scroll-area");
  const chip = page.el("load-older");
  area.scrollTop = Math.floor((chip.getBoundingClientRect().top + area.scrollTop) / 2);
  await area.dispatch("scroll");
  await page.settle();
  page.olderHold = searchHold();
  const hold = page.olderHold;
  void chip.click();
  await page.settle();
  assert.equal(chip.textContent, "Fetching older messages… · Stop");
  // The reader scrolls down the matches while the page is read: the step back goes off screen.
  area.scrollTop = 600;
  await area.dispatch("scroll");
  await page.settle();
  const reading = page.el("discord-log").children.find((li) => !li.hasClass("search-hidden") && li.getBoundingClientRect().bottom > 0);
  assert.ok(reading, "no row is on screen");
  const y = reading.getBoundingClientRect().top;
  hold.open();
  await settleWalk(page);
  assert.ok(matchedIds(page).includes(all[25].id), "the walk did not go on after the reader scrolled");
  assert.ok(Math.abs(reading.getBoundingClientRect().top - y) <= 1,
    `threads ${threads}: the message the reader was on moved from ${y}px to ${reading.getBoundingClientRect().top}px`);
  assert.ok(chip.getBoundingClientRect().bottom < 0, `threads ${threads}: the step back was brought back on screen`);
}

test("`#262 search-load-older`: Enter with the step back scrolled off screen brings it to the top of the list first", async () => {
  const { page, all } = await matchesOnScreen();
  const area = page.el("scroll-area");
  const chip = page.el("load-older");
  area.scrollTop = area.scrollHeight;
  await area.dispatch("scroll");
  await page.settle();
  assert.ok(chip.getBoundingClientRect().bottom < 0, "the step back is on screen, so this proves nothing");
  await press(page, page.el("search-field"), "Enter");
  assert.equal(area.scrollTop, 0, "Enter left the step back off screen");
  const top = chip.getBoundingClientRect().top;
  await settleWalk(page);
  assert.deepStrictEqual(matchedIds(page).slice(0, 3), [25, 30, 37].map((i) => all[i].id));
  assert.equal(chip.getBoundingClientRect().top, top, "the step back moved once it was brought into view");
  assert.equal(area.scrollTop, 0);
});

test("`#262 search-load-older`: the sentence over a list the search emptied is written once a walk, when it ends, and not per page", async () => {
  const { page, all } = await searchPage({ pages: 10 });
  await search(page, "engine");
  await page.settle();
  const sentence = page.el("search-empty");
  const before = sentence.textContent;
  assert.match(before, /^Searched 4 messages in this browser/);
  // Every write of the live region, from here on.
  let text = before;
  const writes = [];
  Object.defineProperty(sentence, "textContent", {
    configurable: true,
    get: () => text,
    set: (value) => { text = value; writes.push(value); },
  });
  const first = searchHold();
  page.olderHold = first;
  void page.el("load-older").click();
  await page.settle();
  // The first page lands, and the second is held: the walk is between pages.
  first.open();
  const second = searchHold();
  page.olderHold = second;
  for (let i = 0; i < 8; i += 1) await page.settle();
  assert.equal(searchReads(page).length, 2, "the walk is not on its second page");
  assert.equal(page.el("search-count").textContent, "0 of 8 loaded", "the count did not follow the page");
  assert.equal(sentence.textContent, before, "the sentence was rewritten while the walk read");
  assert.deepStrictEqual(writes, []);
  second.open();
  await settleWalk(page);
  assert.deepStrictEqual(writes,
    [`Searched 24 messages in this browser, a complete history back to ${searchMoment(page, all[16].timestamp)} (no gaps). None match.`],
    "the sentence was not written exactly once, when the walk ended");
});

test("`#262 search-load-older`: a walk in the other column ends this one, and reads only once this one's read is off the wire", async () => {
  const page = newPage();
  // No way to abort a request: the left column's read stays on the wire until it is answered.
  threadHistory(page, { top: 24 });
  await signIn(page);
  await showDiscord(page, page.messages);
  await twoColumns(page);
  await openerOf(cardRow(page, page.threads[0].id)).click();
  await page.settle();
  await search(page, "engine");
  await page.settle();
  const left = page.el("load-older");
  const right = page.el("side-older");
  assert.match(left.textContent, /^Search before /);
  assert.match(right.textContent, /^Search replies before /);
  const view = (path) => new URL(path, "http://fixture.test").searchParams.get("view");
  const hold = searchHold();
  page.olderHold = hold;
  void left.click();
  await page.settle();
  assert.equal(left.textContent, "Fetching older messages… · Stop");
  void right.click();
  for (let i = 0; i < 8; i += 1) await page.settle();
  assert.match(left.textContent, /^Search before /, "the left column's walk went on beside the right's");
  assert.equal(right.textContent, "Fetching older messages… · Stop");
  assert.deepStrictEqual(searchReads(page).map(view), ["main"], "the right column read while the left column's read was on the wire");
  hold.open();
  await settleWalk(page);
  const reads = searchReads(page).map(view);
  assert.equal(reads.filter((v) => v === "main").length, 1, "the ended walk read on");
  assert.equal(reads.filter((v) => v === "thread").length, SEARCH_OLDER_PAGES, "the right column's walk did not read once the left's was off the wire");
  assert.equal(page.el("search-older-note").hidden, true, "the ended walk said what it found");
});

test("`#262 search-load-older`: a reply's jump ends the walk, aborting its read, and goes on to the message it answers", async () => {
  for (const threads of [false, true]) {
    const page = newPage();
    const all = searchHistory(page, { pages: 10, threads,
      content: (i) => (i === 39 ? "engine answer" : i === 30 ? "engine question" : `message ${i}`) });
    all[39].reply_to = all[30].id;
    page.enableAbort();
    await signIn(page);
    await showDiscord(page, page.messages);
    await search(page, "engine");
    await page.settle();
    const hold = searchHold();
    page.olderHold = hold;
    void page.el("load-older").click();
    await page.settle();
    assert.equal(page.el("load-older").textContent, "Fetching older messages… · Stop");
    await tapArrow(page, rowWithId(page, all[39].id));
    assert.equal(page.olderOptions[0].signal.aborted, true, `threads ${threads}: the jump left the walk's read on the wire`);
    assert.doesNotMatch(page.el("load-older").textContent, /^Fetching/, `threads ${threads}: the walk went on under the jump`);
    // The step the walk had on the wire was the jump's first, and its abort no failure of the jump's.
    assert.equal(rowWithId(page, all[30].id).getAttribute("data-landed"), "true", `threads ${threads}: the jump did not arrive`);
    assert.match(statusText(page), /Here is the message it answers/);
    assert.equal(page.el("error").hidden, true, `threads ${threads}: the aborted read was reported as a failure`);
    assert.equal(page.el("search-older-note").hidden, true, `threads ${threads}: the ended walk said what it found`);
  }
});

test("`#262 search-load-older`: a filter's walk back to an old pin ends a walk in the other column", async () => {
  const page = newPage();
  threadHistory(page, { top: 24 });
  const [pin] = page.posts;
  page.pinned.set(pin.id, storedPin(pin));
  page.enableAbort();
  await signIn(page);
  await showDiscord(page, page.messages);
  await twoColumns(page);
  await openerOf(cardRow(page, page.threads[0].id)).click();
  await page.settle();
  // Every post matches, on the left; no reply does, on the right, whose step back offers the walk.
  await search(page, "post");
  await page.settle();
  assert.match(page.el("side-older").textContent, /^Search replies before /);
  // The old pin, selected over the left column's Pinned list.
  await page.el("pinned-filter").click();
  await page.settle();
  await page.settle();
  await tapRow(page, pin.id, "pinned-log");
  // The right column's thread walks back for the search, its read on the wire.
  const hold = searchHold();
  page.olderHold = hold;
  void page.el("side-older").click();
  await page.settle();
  assert.equal(page.el("side-older").textContent, "Fetching older messages… · Stop");
  // Pinned off: the left column walks back to the pin, and one walk at a time is on the page.
  await page.el("pinned-filter").click();
  await page.settle();
  assert.equal(page.olderOptions[0].signal.aborted, true, "the right column's read went on beside the walk back to the pin");
  assert.match(page.el("side-older").textContent, /^Search replies before /, "the right column's walk went on");
  hold.open();
  await settleWalk(page);
  assert.equal(page.el("side-search-older-note").hidden, true, "the ended walk said what it found");
  assert.ok(rowWithId(page, pin.id), "the walk back did not reach the pin");
});

test("`#262 search-load-older`: Main drawn from All reads its own newest page first, which is no older page and not counted as one", async () => {
  const page = newPage();
  pagedThreads(page, 4);
  await signIn(page);
  await showDiscord(page, page.messages);
  await pickThread(page, "main");
  const from = page.timelineCalls.length;
  await search(page, "engine");
  await page.settle();
  await page.el("load-older").click();
  await settleWalk(page);
  const reads = page.timelineCalls.slice(from).map((call) => [viewOf(call), beforeOf(call) !== null]);
  assert.deepStrictEqual(reads, [["main", false], ...Array(SEARCH_OLDER_PAGES).fill(["main", true])],
    "the walk did not read Main's newest page and then five older ones");
  assert.match(page.el("search-empty").textContent,
    /^Searched \d+ messages in this browser, a complete history back to .+ \(no gaps\)\. None match\. Main shows a thread when a reply held here matches; All may hold more replies\.$/);
});

/** A thread's id in `threadsHistory`. */
const historyThread = (t) => `spaces/A/threads/t${t}`;

/** Where `threadsHistory` has message `i` of `replies` replies a thread. */
const historyIndex = (t, r, replies = 6) => t * (replies + 1) + r;

/** The cards the filters let through, by thread id. */
const shownCards = (page) => page.el("side-cards").children.filter((li) => !li.hidden && !li.hasClass("search-hidden"))
  .map((li) => li.getAttribute("data-id"));

/** The older reads of All so far, as `[view, before]`. */
const allReads = (page) => page.pagesServed.map((path) => new URL(path, "http://fixture.test").searchParams)
  .filter((query) => query.get("view") === "flat").map((query) => query.get("before"));

/** The control at the head of the cards that searches their threads' older replies, or null. */
const repliesControl = (page) => page.el("side-search-replies");

/** Three replies saying "engine", each in its own thread, one, two and three pages behind All's newest. */
const ENGINE_REPLIES = new Set([historyIndex(5, 1), historyIndex(4, 3), historyIndex(3, 6)]);
const engineSaid = (t, r) => (ENGINE_REPLIES.has(historyIndex(t, r)) ? `the engine stalled again, ${t}.${r}` : `thread ${t} reply ${r}`);

test("`#262 search-load-older`: one column on All walks back to three older replies, and two columns find them too, from the cards' own walk", async () => {
  // One column, on All, as the owner had it: the walk back finds the three replies.
  const one = await repliesPage({ said: engineSaid }, false);
  await search(one.page, "engine");
  await one.page.settle();
  assert.deepStrictEqual(matchedIds(one.page), [], "the fixture already held a match");
  await one.page.el("load-older").click();
  await settleWalk(one.page);
  assert.deepStrictEqual(matchedIds(one.page), [...ENGINE_REPLIES].sort((a, b) => a - b).map((i) => one.all[i].id),
    "one column on All did not find the three older replies");

  // Two columns: Main on the left holds none of them, however far back it is walked.
  const { page, all } = await repliesPage({ said: engineSaid });
  await search(page, "engine");
  await page.settle();
  assert.deepStrictEqual(matchedIds(page), []);
  assert.match(page.el("search-empty").textContent, / Main shows a thread when a reply held here matches; All may hold more replies\.$/);
  assert.deepStrictEqual(shownCards(page), [], "a card was found before any older reply was read");
  assert.equal(page.el("side-list-state").textContent, "No threads match that search.");
  // The cards offer to search their threads' older replies, from where All reaches back to.
  const control = repliesControl(page);
  assert.ok(control && !control.hidden, "two columns offer no way to search the threads' older replies");
  const floor = all.length - 4;
  assert.equal(control.textContent, `Search replies before ${searchMoment(page, all[floor].timestamp)}`);
  assert.match(control.getAttribute("aria-label"),
    /^Search replies before .+: fetches up to 250 older messages of the whole channel, replies and all, from Discord, and searches them here\.$/);
  await control.click();
  await settleWalk(page);
  // It read All's older pages from All's own cursor, and stopped at the end of the page bringing the third.
  assert.deepStrictEqual(allReads(page).filter((before) => before !== null),
    [floor, floor - 4, floor - 8].map((i) => all[i].id), "the walk did not read All back three pages from its cursor");
  assert.deepStrictEqual(shownCards(page).sort(), [3, 4, 5].map(historyThread), "the cards did not find the three threads");
  for (const t of [3, 4, 5]) assert.equal(replyHint(page, historyThread(t)), "in replies");
  const back = searchMoment(page, all[floor - 12].timestamp);
  assert.equal(page.el("side-replies-note").hidden, false);
  assert.equal(page.el("side-replies-note").textContent, `Found 3 threads in 12 older messages fetched from Discord, back to ${back}.`);
  assert.equal(control.textContent, `Search replies before ${back}`, "the control does not say how far back the next walk starts");
  assert.equal(page.el("side-list-state").hidden, true);
  assert.match(HTML_CODE, /<button id="side-search-replies" class="chip" type="button" hidden>/);
  assert.match(HTML_CODE, /<p id="side-replies-note" class="hint search-older-note" role="status" hidden><\/p>/);
  // A thumb's target, as the step back has; and Help says where it is.
  assert.match(cssBlock("#side-search-replies::after"), /content:\s*""/);
  assert.match(HTML_CODE, /the head of the\s+cards says <em>Search replies before Oct 3<\/em>/);
  // A found thread opens on the right with its matching reply shown and marked.
  await openerOf(cardRow(page, historyThread(3))).click();
  await page.settle();
  assert.deepStrictEqual(matchedIds(page, "side-log"), [all[historyIndex(3, 6)].id], "the found thread did not show its matching reply");
});

test("`#262 search-load-older`: the cards' walk reads at most five pages, and a second press walks on from where it stopped", async () => {
  const { page, all } = await repliesPage({ replies: 5 });
  await search(page, "engine");
  await page.settle();
  await repliesControl(page).click();
  await settleWalk(page);
  const older = () => allReads(page).filter((before) => before !== null);
  assert.equal(older().length, SEARCH_OLDER_PAGES, "the cards' walk was not bounded");
  const back = searchMoment(page, all[all.length - 4 - 20].timestamp);
  assert.equal(page.el("side-replies-note").textContent, `No more threads match in 20 older messages fetched from Discord, back to ${back}.`);
  await repliesControl(page).click();
  await settleWalk(page);
  assert.equal(older().length, SEARCH_OLDER_PAGES + 3, "the second press did not walk on to the start");
  assert.equal(page.el("side-replies-note").textContent, "Reached the start of this channel; no more threads match.");
  assert.equal(repliesControl(page).hidden, true, "the walk was offered past the start of the channel");
  assert.equal(page.el("side-list-state").textContent, "No threads match that search — all of this channel is loaded.");
  // Another word: the note goes with the search it spoke for, and the state still says how far back.
  await page.el("search-field").setValue("nothing like this");
  await page.settle();
  assert.equal(page.el("side-replies-note").hidden, true, "the last walk's note outlived its search");
  assert.equal(page.el("side-list-state").textContent, "No threads match that search — all of this channel is loaded.");
  assert.equal(repliesControl(page).hidden, true);
});

test("`#262 search-load-older`: the cards' walk at the start of the channel says what it found there", async () => {
  const { page } = await repliesPage({ threads: 3, replies: 2, said: (t, r) => (t === 0 && r === 1 ? "engine" : `reply ${t}.${r}`) });
  await search(page, "engine");
  await page.settle();
  await repliesControl(page).click();
  await settleWalk(page);
  assert.deepStrictEqual(shownCards(page), [historyThread(0)]);
  assert.equal(page.el("side-replies-note").textContent, "Reached the start of this channel. Found 1 thread.");
  assert.equal(repliesControl(page).hidden, true);
});

test("`#262 search-load-older`: the cards' walk starts no page its last page's time says would end past fifteen seconds", async () => {
  const { page, all } = await repliesPage({ said: (t, r) => (historyIndex(t, r) === historyIndex(4, 3) ? "engine" : `reply ${t}.${r}`) });
  page.olderTakes = 6000;
  await search(page, "engine");
  await page.settle();
  await repliesControl(page).click();
  await settleWalk(page);
  assert.equal(allReads(page).filter((before) => before !== null).length, 2, "a page that would end past the budget was started");
  assert.equal(page.el("side-replies-note").textContent,
    `Found 1 thread in 8 older messages fetched from Discord in 15 s, back to ${searchMoment(page, all[all.length - 12].timestamp)}.`);
});

test("`#262 search-load-older`: Stop aborts the cards' read on the wire, and a walk stopped with nothing found says so", async () => {
  const { page } = await repliesPage();
  await search(page, "engine");
  await page.settle();
  page.olderHold = searchHold();
  const hold = page.olderHold;
  void repliesControl(page).click();
  await page.settle();
  assert.equal(repliesControl(page).textContent, "Fetching older messages… · Stop");
  // A second tap at once is the same tap again.
  await repliesControl(page).click();
  await page.settle();
  assert.equal(page.olderOptions[0].signal.aborted, false, "a second tap in the walk's first moments stopped it");
  page.setClock(page.clock() + SEARCH_OLDER_TAP_MS);
  await repliesControl(page).click();
  await page.settle();
  assert.equal(page.olderOptions[0].signal.aborted, true, "Stop left the read on the wire");
  hold.open();
  await settleWalk(page);
  assert.equal(page.olderOptions.length, 1, "the walk read on after Stop");
  assert.equal(page.el("side-replies-note").hidden, false, "a walk stopped with nothing found said nothing");
  assert.equal(page.el("side-replies-note").textContent, "Stopped before any older messages arrived.");
  assert.equal(page.el("error").hidden, true, "Stop was reported as a failure");
  assert.match(repliesControl(page).textContent, /^Search replies before /);
});

test("`#262 search-load-older`: a change to the search, a thread opened on the right, or leaving two columns ends the cards' walk where it is", async () => {
  for (const end of ["typed", "thread", "narrow", "closed"]) {
    const { page } = await repliesPage();
    await search(page, "engine");
    await page.settle();
    page.olderHold = searchHold();
    const hold = page.olderHold;
    void repliesControl(page).click();
    await page.settle();
    if (end === "typed") await page.el("search-field").setValue("engine room");
    else if (end === "thread") await openerOf(cardRow(page, historyThread(5))).click();
    else if (end === "narrow") await page.setDesk(false);
    else await page.el("search-toggle").click();
    await page.settle();
    assert.equal(page.olderOptions[0].signal.aborted, true, `${end}: the cards' read on the wire went on`);
    hold.open();
    await settleWalk(page);
    assert.equal(page.olderOptions.length, 1, `${end}: the walk read on`);
    assert.equal(page.el("side-replies-note").hidden, true, `${end}: the ended walk wrote its note`);
    assert.equal(page.el("error").hidden, true, `${end}: the ended walk was reported as a failure`);
  }
});

test("`#262 search-load-older`: one walk at a time: Main's walk ends the cards', and the cards' ends Main's", async () => {
  const { page } = await repliesPage({ threads: 40 });
  await search(page, "engine");
  await page.settle();
  // The cards' walk, then Main's: the cards' read is aborted, and Main's waits for it to be off the wire.
  page.olderHold = searchHold();
  let hold = page.olderHold;
  void repliesControl(page).click();
  await page.settle();
  void page.el("load-older").click();
  await page.settle();
  assert.equal(page.olderOptions[0].signal.aborted, true, "Main's walk left the cards' read on the wire");
  hold.open();
  await settleWalk(page);
  assert.match(repliesControl(page).textContent, /^Search replies before /);
  const reads = page.pagesServed.map((path) => new URL(path, "http://fixture.test").searchParams)
    .filter((query) => query.get("before") !== null).map((query) => query.get("view"));
  assert.equal(reads[0], "flat");
  assert.ok(reads.slice(1).length > 0 && reads.slice(1).every((view) => view === "main"), `Main's walk did not read Main: ${reads}`);
  // Main's walk, then the cards': Main's read is aborted.
  page.olderHold = searchHold();
  hold = page.olderHold;
  const from = page.olderOptions.length;
  void page.el("load-older").click();
  await page.settle();
  void repliesControl(page).click();
  await page.settle();
  assert.equal(page.olderOptions[from].signal.aborted, true, "the cards' walk left Main's read on the wire");
  hold.open();
  await settleWalk(page);
  assert.equal(page.el("search-older-note").hidden, true, "Main's ended walk wrote its note");
  assert.ok(page.el("side-replies-note").textContent !== "", "the cards' walk did not run once Main's had ended");
});

test("`#262 search-load-older`: over the thread list on a phone, Enter in the search field walks the cards' replies, not Main behind them", async () => {
  const { page, all } = await repliesPage({ said: engineSaid }, false);
  await pickThread(page, "threads");
  assert.equal(page.el("screen-main").getAttribute("data-columns"), "list");
  await search(page, "engine");
  await page.settle();
  assert.match(repliesControl(page).textContent, /^Search replies before /);
  const from = page.pagesServed.length;
  await press(page, page.el("search-field"), "Enter");
  await settleWalk(page);
  const views = page.pagesServed.slice(from).map((path) => new URL(path, "http://fixture.test").searchParams)
    .filter((query) => query.get("before") !== null).map((query) => query.get("view"));
  assert.deepStrictEqual(views, ["flat", "flat", "flat"], "Enter over the thread list did not walk All's replies");
  assert.deepStrictEqual(shownCards(page).sort(), [3, 4, 5].map(historyThread));
  assert.match(page.el("side-replies-note").textContent, /^Found 3 threads in 12 older messages fetched from Discord, back to /);
  assert.equal(page.el("search-older-note").hidden, true, "Main, behind the list, was walked");
  void all;
});

test("`#262 search-load-older`: in two columns Enter walks the cards' replies when the keys are in them, or when Main has nothing older", async () => {
  const { page } = await repliesPage({ said: engineSaid });
  await search(page, "engine");
  await page.settle();
  // The keys in the left column, which offers its own walk: Enter walks Main, as before.
  let from = page.pagesServed.length;
  await press(page, page.el("search-field"), "Enter");
  await settleWalk(page);
  const views = () => page.pagesServed.slice(from).map((path) => new URL(path, "http://fixture.test").searchParams)
    .filter((query) => query.get("before") !== null).map((query) => query.get("view"));
  assert.ok(views().length > 0 && views().every((view) => view === "main"), `Enter with the keys in Main did not walk Main: ${views()}`);
  // Main is all loaded now: Enter walks the cards'.
  assert.equal(page.el("load-older").hidden, true, "Main still has older messages, so this proves nothing");
  from = page.pagesServed.length;
  await press(page, page.el("search-field"), "Enter");
  await settleWalk(page);
  assert.deepStrictEqual(views(), ["flat", "flat", "flat"], "Enter did not walk the cards' replies once Main had nothing older");
  assert.deepStrictEqual(shownCards(page).sort(), [3, 4, 5].map(historyThread));
});

test("`#262 search-load-older`: with All never read, the cards' walk reads All's newest page first, which is not counted as one of the five", async () => {
  const store = new Map([["vibe-talk.voice.columns", "two"]]);
  const page = newPage(store);
  threadsHistory(page, { said: engineSaid });
  page.wideDesk = true;
  page.enableAbort();
  await signIn(page);
  await showDiscord(page, page.messages);
  assert.ok(inTwoColumns(page), "the page did not open in two columns");
  // Every read of All so far is the counts' newest page, if any, which gave no cursor to walk back on.
  await search(page, "engine");
  await page.settle();
  const from = page.pagesServed.length;
  await repliesControl(page).click();
  await settleWalk(page);
  const reads = page.pagesServed.slice(from).map((path) => new URL(path, "http://fixture.test").searchParams)
    .filter((query) => query.get("view") === "flat").map((query) => query.get("before") !== null);
  assert.deepStrictEqual(reads, [false, true, true, true], "the walk did not read All's newest page and then three older ones");
  assert.match(page.el("side-replies-note").textContent, /^Found 3 threads in 12 older messages fetched from Discord, back to /);
});

test("`#262 search-load-older`: a thread arriving live mid-walk whose first message matches is shown, but not counted as found by the cards' walk", async () => {
  const { page, all } = await repliesPage({ said: engineSaid });
  const stream = page.stream();
  await search(page, "engine");
  await page.settle();
  page.olderHold = searchHold();
  const hold = page.olderHold;
  void repliesControl(page).click();
  await page.settle();
  const floor = all.length - 4;
  // A new thread, its first message saying the word, on the server and then on the stream, which re-reads Main.
  const live = "spaces/A/threads/live";
  const root = message({ id: "8900000000000099999", content: "the engine is back",
    timestamp: new Date(Date.UTC(2026, 7, 19, 13, 0, 0)).toISOString(),
    thread: { id: live, root_message_id: "8900000000000099999", is_root: true, reply_count: 2, reply_count_exact: true } });
  all.push(root);
  await deliver(page, stream, sseMessage(root));
  await page.settle();
  assert.ok(shownCards(page).includes(live), "the live thread's card is not shown");
  hold.open();
  await settleWalk(page);
  // Three pages, as without it: the live card neither ended the walk at two nor is in the count.
  assert.deepStrictEqual(allReads(page).filter((before) => before !== null), [floor, floor - 4, floor - 8].map((i) => all[i].id),
    "a live match ended the cards' walk early");
  assert.equal(page.el("side-replies-note").textContent,
    `Found 3 threads in 12 older messages fetched from Discord, back to ${searchMoment(page, all[floor - 12].timestamp)}.`);
});

test("`#262 search-load-older`: a matching reply whose thread has no card is said, not passed over in \"Nothing more\"", async () => {
  const page = newPage();
  const all = threadsHistory(page, { threads: 10, replies: 3 });
  // Thread 0 is not in the list of threads, and its first message is older than any page the walk reads; one
  // reply to it, much later, says the word, two pages behind All's newest.
  const t0 = historyThread(0);
  page.threads.splice(page.threads.findIndex((summary) => summary.id === t0), 1);
  const reply = message({ id: "8900000000000099998", content: "the engine stalled in the old thread",
    timestamp: new Date((Date.parse(all[29].timestamp) + Date.parse(all[30].timestamp)) / 2).toISOString(),
    thread: { ...all[1].thread } });
  all.splice(30, 0, reply);
  page.enableAbort();
  await signIn(page);
  await showDiscord(page, page.messages);
  await twoColumns(page);
  await search(page, "engine");
  await page.settle();
  assert.ok(!cardRow(page, t0), "thread 0 has a card, so this proves nothing");
  await repliesControl(page).click();
  await settleWalk(page);
  assert.equal(allReads(page).filter((before) => before !== null).length, SEARCH_OLDER_PAGES);
  assert.ok(page.messages.includes(reply) && allReads(page).includes(all[33].id), "the walk did not read past the reply");
  assert.deepStrictEqual(shownCards(page), []);
  assert.equal(page.el("side-replies-note").textContent,
    `No more threads match in 20 older messages fetched from Discord, back to ${searchMoment(page, all[17].timestamp)}. ` +
    "1 more thread with a matching reply is not among the threads loaded.");
});

test("`#262 search-load-older`: a page of the cards' walk that fails ends it, said where every failed read is and on the status line", async () => {
  const { page } = await repliesPage({ threads: 12 });
  await search(page, "engine");
  await page.settle();
  const serve = page.timeline;
  let reads = 0;
  page.timeline = async (path, options) => {
    if (/[?&]before=/.test(String(path)) && ++reads === 2) return json(502, { error: "chat_error", detail: "the provider returned HTTP 502" });
    return serve(path, options);
  };
  await repliesControl(page).click();
  await settleWalk(page);
  assert.equal(reads, 2, "the walk read on after a page failed");
  assert.equal(page.el("error").hidden, false, "the failed page was not reported");
  assert.equal(page.el("status").textContent, "Could not search further back — try again.");
  assert.match(repliesControl(page).textContent, /^Search replies before /);
  // `#578 search-status-clarity`. And under the control, with the reason.
  assert.match(page.el("side-replies-note").textContent, new RegExp("^Could not fetch older messages from Discord: " +
    "the provider returned HTTP 502\\. Before that, 4 older messages fetched from Discord, back to .+\\. " +
    "Press the button again to retry\\.$"));
});

test("`#262 search-load-older`: Main's walk stopped with nothing more found says so, rather than nothing", async () => {
  const { page, all } = await searchPage({ pages: 10, content: (i) => (i === 38 ? "engine" : `message ${i}`) });
  page.enableAbort();
  await search(page, "engine");
  await page.settle();
  assert.deepStrictEqual(matchedIds(page), [all[38].id]);
  // The walk's second page is held until it is stopped.
  const serve = page.channelPage;
  const hold = searchHold();
  let reads = 0;
  page.channelPage = async (path, options) => {
    if (/[?&]before=/.test(String(path)) && ++reads === 2) {
      await new Promise((resolve, reject) => {
        hold.promise.then(resolve);
        options.signal.addEventListener("abort", () => reject(Object.assign(new Error("aborted"), { name: "AbortError" })));
      });
    }
    return serve(path, options);
  };
  void page.el("load-older").click();
  await page.settle();
  assert.equal(page.el("load-older").textContent, "Fetching… 4 older messages · Stop");
  page.setClock(page.clock() + SEARCH_OLDER_TAP_MS);
  await page.el("load-older").click();
  hold.open();
  await settleWalk(page);
  assert.equal(reads, 2);
  assert.equal(page.el("search-older-note").hidden, false, "a walk stopped with nothing found said nothing");
  assert.equal(page.el("search-older-note").textContent,
    `Stopped. None of the 4 older messages fetched from Discord, back to ${searchMoment(page, all[32].timestamp)}, match.`);
  // Stopped before any page landed, it says that.
  const second = await searchPage({ pages: 10, content: (i) => (i === 38 ? "engine" : `message ${i}`) });
  second.page.enableAbort();
  await search(second.page, "engine");
  await second.page.settle();
  second.page.olderHold = searchHold();
  void second.page.el("load-older").click();
  await second.page.settle();
  second.page.setClock(second.page.clock() + SEARCH_OLDER_TAP_MS);
  await second.page.el("load-older").click();
  await settleWalk(second.page);
  assert.equal(second.page.el("search-older-note").textContent, "Stopped before any older messages arrived.");
});

// --- `#578 search-status-clarity` --------------------------------------------------------------------
//
// The owner, 2026-10-27: "nothing in the search loading view makes sense ... it shows me no hits and a
// button 'Search before October 25'. I press it and it does nothing for a while and then shows THE SAME
// button." A press over a busy channel brought fifty more messages of the same day, the control named the
// day alone, and a walk that found nothing over a list the search had emptied said nothing at all. And
// "Nothing in the 82 messages loaded so far" did not say whether those were a dense history or a sparse
// set, nor "Searching… 1 of 5" one of five what.

test("`#578 search-status-clarity`: every press that brings older messages of the same day moves the control, to the minute", async () => {
  for (const threads of [false, true]) {
    // Forty messages a minute apart: every one of them on the same day.
    const { page, all } = await searchPage({ pages: 10, spacing: 60000, threads });
    await search(page, "engine");
    await page.settle();
    const chip = page.el("load-older");
    const at = (i) => searchMoment(page, all[i].timestamp);
    assert.equal(searchDay(page, all[0].timestamp), searchDay(page, all[39].timestamp), "the premise: one day");
    assert.equal(chip.textContent, `Search before ${at(36)}`);
    await chip.click();
    await settleWalk(page);
    assert.equal(chip.textContent, `Search before ${at(16)}`, `threads ${threads}: the press did not move the control`);
    assert.equal(page.el("search-older-note").textContent,
      `None of the 20 older messages fetched from Discord, back to ${at(16)}, match.`);
    assert.equal(page.el("search-empty").textContent,
      `Searched 24 messages in this browser, a complete history back to ${at(16)} (no gaps). None match.`);
    // The next press goes on from there, to the start.
    await chip.click();
    await settleWalk(page);
    assert.equal(chip.hidden, true);
    assert.match(page.el("search-older-note").textContent, /^Reached the start of /);
  }
});

test("`#578 search-status-clarity`: a step back that brings nothing older says so, and the next press walks on", async () => {
  for (const threads of [false, true]) {
    const { page, all } = await searchPage({ pages: 10, threads });
    // The server's walk below a page it served from its cache found nothing older yet: an empty page, a
    // cursor to go on from. Five of those, then the history as it is.
    const route = threads ? "timeline" : "channelPage";
    const serve = page[route];
    let empty = SEARCH_OLDER_PAGES;
    page[route] = async (path, options) => {
      const before = /[?&]before=([^&]+)/.exec(String(path));
      if (!before || empty === 0) return serve(path, options);
      empty -= 1;
      page.pagesServed.push(String(path));
      const cursor = decodeURIComponent(before[1]);
      return threads
        ? json(200, timelineAnswer({ view: "flat", messages: [], has_more: true, next_before: cursor }))
        : json(200, { channel: CHANNEL, messages: [], returned: 0, limit: 4, has_more: true, next_before: cursor, dismissed: [] });
    };
    await search(page, "engine");
    await page.settle();
    const chip = page.el("load-older");
    const before = chip.textContent;
    await chip.click();
    await settleWalk(page);
    assert.equal(page.el("search-older-note").textContent,
      "Discord sent nothing older this time, though it says older messages exist. Press the button again to keep going.",
      `threads ${threads}: a press that brought nothing said nothing`);
    assert.equal(chip.textContent, before, `threads ${threads}: the control moved over nothing`);
    assert.equal(page.el("search-empty").textContent,
      `Searched 4 messages in this browser, a complete history back to ${searchMoment(page, all[36].timestamp)} (no gaps). None match.`,
      `threads ${threads}: an empty step back moved the stretch`);
    await chip.click();
    await settleWalk(page);
    assert.equal(chip.textContent, `Search before ${searchMoment(page, all[16].timestamp)}`, `threads ${threads}: the next press stood still`);
  }
});

test("`#578 search-status-clarity`: a newest read that reaches none of what is loaded leaves it scattered below, and says how much", async () => {
  for (const threads of [false, true]) {
    const { page, all } = await searchPage({ pages: 10, threads });
    await search(page, "engine");
    await page.settle();
    await page.el("load-older").click();
    await settleWalk(page);
    // Six arrive while the page is away, more than a read holds: its next newest read is the newest four.
    const later = Date.parse(all[all.length - 1].timestamp);
    for (let i = 1; i <= 6; i += 1) {
      all.push(message({ id: String(8700000000000000100n + BigInt(i)), content: `later ${i}`,
        timestamp: new Date(later + i * 60000).toISOString() }));
    }
    await reReadChannel(page);
    await page.settle();
    assert.equal(page.el("search-empty").textContent,
      `Searched 4 messages in this browser, a complete history back to ${searchMoment(page, all[all.length - 4].timestamp)} ` +
      "(no gaps), plus 24 scattered older ones. None match.", `threads ${threads}: the gap was not said`);
  }
});

test("`#578 search-status-clarity`: All says its top-level messages and, of their threads, how many it holds every reply of", async () => {
  // Four threads of one reply each; All's newest page holds three of them whole, and older history.
  const page = newPage();
  const all = threadsHistory(page, { threads: 4, replies: 1, size: 6 });
  await signIn(page);
  await showDiscord(page, page.messages);
  await search(page, "engine");
  await page.settle();
  assert.equal(page.el("search-empty").textContent,
    `Searched 6 messages in this browser: every top-level message back to ${searchMoment(page, all[2].timestamp)} (no gaps), ` +
    "and every reply in their 3 threads. None match.");
  // Bounded, as Slack's is: a page leaving out one thread's replies. All reaches the start, but is not whole.
  const bounded = newPage();
  const held = threadsHistory(bounded, { threads: 3, replies: 2, size: 50 });
  allLeavingOut(bounded, [historyThread(1)]);
  await signIn(bounded);
  await showDiscord(bounded, bounded.messages);
  assert.equal(channelSummaryText(bounded), "lead team — back to the first message; 1 thread here has replies not loaded");
  assert.equal(bounded.el("timeline-notice").textContent,
    "1 thread here has replies All has not loaded; open a thread to read all of it.");
  await search(bounded, "engine");
  await bounded.settle();
  assert.equal(bounded.el("search-empty").textContent,
    `Searched 7 messages in this browser: every top-level message back to the first one (${searchMoment(bounded, held[0].timestamp)}), ` +
    "and every reply in 2 of their 3 threads. None match.");
});

test("`#603 server-side-search`: All's notice of threads longer than it brings is read as its bound, and said in threads", async () => {
  // The server reads every thread on All's page now, and names only threads past fifty replies.
  const notice = "Some threads' replies are not in All: 1 thread here has more than 50 replies, and All brings " +
    "the newest 50 of each; open a thread to read all of it.";
  const bounded = newPage();
  threadsHistory(bounded, { threads: 3, replies: 2, size: 50 });
  allLeavingOut(bounded, [historyThread(1)], notice);
  await signIn(bounded);
  await showDiscord(bounded, bounded.messages);
  assert.equal(channelSummaryText(bounded), "lead team — back to the first message; 1 thread here has replies not loaded");
  assert.equal(bounded.el("timeline-notice").textContent,
    "1 thread here has replies All has not loaded; open a thread to read all of it.");
});

test("`#578 search-status-clarity`: rows saved on this device claim no stretch until a read checks them", async () => {
  const first = newPage();
  const all = searchHistory(first, { threads: true, pages: 3 });
  await signIn(first);
  await showDiscord(first, first.messages);
  let timeline = null;
  const page = reloadPage(first.storage, (p) => {
    searchHistory(p, { threads: true, pages: 3 });
    timeline = gate(p.timeline);
    p.timeline = timeline.respond;
  });
  await page.settle();
  await search(page, "engine");
  await page.settle();
  const back = searchMoment(page, all[8].timestamp);
  assert.equal(page.el("search-empty").textContent,
    `Searched 4 messages saved on this device, back to ${back}, not yet checked for gaps. None match.`);
  timeline.open();
  await page.settle();
  await page.settle();
  assert.equal(page.el("search-empty").textContent,
    `Searched 4 messages in this browser, a complete history back to ${back} (no gaps). None match.`,
    "a read that joined what was saved did not vouch for it");
});

test("`#578 search-status-clarity`: without threads a gap is saved with the rows, and a reload still says what is scattered", async () => {
  const later = (all) => {
    const from = Date.parse(all[39].timestamp);
    for (let i = 1; i <= 6; i += 1) {
      all.push(message({ id: String(8700000000000000100n + BigInt(i)), content: `later ${i}`,
        timestamp: new Date(from + i * 60000).toISOString() }));
    }
  };
  const { page, all } = await searchPage({ pages: 10 });
  await search(page, "engine");
  await page.settle();
  await page.el("load-older").click();
  await settleWalk(page);
  later(all);
  await reReadChannel(page);
  await page.settle();
  const said = `Searched 4 messages in this browser, a complete history back to ${searchMoment(page, all[all.length - 4].timestamp)} ` +
    "(no gaps), plus 24 scattered older ones. None match.";
  assert.equal(page.el("search-empty").textContent, said);
  let reads = null;
  const reloaded = reloadPage(page.storage, (p) => {
    later(searchHistory(p, { pages: 10 }));
    reads = gate(p.channelPage);
    p.channelPage = reads.respond;
  });
  await reloaded.settle();
  await search(reloaded, "engine");
  await reloaded.settle();
  assert.match(reloaded.el("search-empty").textContent, /^Searched 28 messages saved on this device, back to .+, not yet checked for gaps\. None match\.$/,
    "rows drawn from the device claimed a stretch before any read");
  reads.open();
  await reloaded.settle();
  await reloaded.settle();
  assert.equal(reloaded.el("search-empty").textContent, said, "the reload forgot the gap");
});

test("`#578 search-status-clarity`: only the notice that All left replies out is said in threads; a forum's own stays as it came", async () => {
  const page = newPage();
  threadsHistory(page, { threads: 3, replies: 2, size: 50 });
  const forum = "This channel contains threads. Open a thread to post a reply.";
  allLeavingOut(page, [], forum);
  await signIn(page);
  await showDiscord(page, page.messages);
  assert.equal(page.el("timeline-notice").textContent, forum);
  assert.equal(page.el("timeline-notice").hidden, false);
});

// --- `#606 server-side-search`: the search follows the view ----------------------------------------
//
// The owner, 2026-10-27: in Main a thread stands for all its children, so any hit in the thread keeps
// it; in All threads are unpacked inline and every message is a candidate on its own; over the cards a
// thread with any hit is chosen, and unpacking it shows only the hits, with folded lines between them.

test("`#606 server-side-search`: in All every message is a candidate on its own, by the shared language", async () => {
  const page = await threadPage("flat");
  await search(page, "in:thread");
  await page.settle();
  assert.deepStrictEqual(matchedIds(page), ["202", "204"], "in:thread is the replies, and nothing else");
  await page.el("search-field").setValue("root OR announcement");
  await page.settle();
  assert.deepStrictEqual(matchedIds(page), ["200", "201", "203"]);
  await page.el("search-field").setValue("thread -answer");
  await page.settle();
  assert.deepStrictEqual(matchedIds(page), ["201", "203"], "a minus takes the replies out");
  await page.el("search-field").setValue("\"FIRST thread\"");
  await page.settle();
  assert.deepStrictEqual(matchedIds(page), ["201", "202"], "a phrase, whatever its case");
  // NEGATIVE CONTROL: a word nothing says matches nothing.
  await page.el("search-field").setValue("kestrel");
  await page.settle();
  assert.deepStrictEqual(matchedIds(page), []);
});

test("`#606 server-side-search`: in Main a thread stands for its children, so a hit in any reply keeps its root", async () => {
  const page = await threadPage("main");
  await search(page, "in:thread");
  await page.settle();
  assert.deepStrictEqual(matchedIds(page), ["201", "203"], "the roots of the threads whose replies match");
  await page.el("search-field").setValue("second answer");
  await page.settle();
  assert.deepStrictEqual(matchedIds(page), ["203"], "only the thread whose reply says both words");
  await page.el("search-field").setValue("announcement");
  await page.settle();
  assert.deepStrictEqual(matchedIds(page), ["200"], "a message of Main's own matches as itself");
  // NEGATIVE CONTROL: a reply holding one word and its root the other is no message holding both.
  await page.el("search-field").setValue("root answer");
  await page.settle();
  assert.deepStrictEqual(matchedIds(page), [], "the thread's words were matched together, not message by message");
});

test("`#606 server-side-search`: over the cards a thread with any hit is chosen, and unpacked it shows only its hits, with lines that open the rest", async () => {
  const { page } = await unpackPage();
  await page.el("side-unpack").click();
  await page.settle();
  const cards = () => page.el("side-cards").children.filter((li) => li.hasAttribute("data-card") &&
    !li.hidden && !li.hasClass("search-hidden")).map((li) => li.getAttribute("data-id"));
  const shownRows = (threadId) => unpackRows(page, threadId)
    .filter((li) => !li.hidden && !li.hasClass("search-hidden"))
    .flatMap((li) => (li.getAttribute("data-ids") || "").split(" "));
  const line = (li, below) => {
    const seam = below ? li.seamBelowLine : li.seamLine;
    return seam && seam.parentNode === li ? seam.textContent : null;
  };
  await search(page, "read two");
  await page.settle();
  assert.deepStrictEqual(cards(), ["spaces/A/threads/sum", "spaces/A/threads/raw"], "a reply's hit did not choose its card");
  assert.deepStrictEqual(shownRows("spaces/A/threads/sum"), ["702"]);
  assert.deepStrictEqual(shownRows("spaces/A/threads/raw"), ["706"]);
  const sumCard = page.el("side-cards").children.find((li) => li.getAttribute("data-id") === "spaces/A/threads/sum");
  const hit = unpackRows(page, "spaces/A/threads/sum").find((li) => (li.getAttribute("data-ids") || "").split(" ").includes("702"));
  assert.equal(line(sumCard, true), "… 2 unmatched messages …", "no line for the run before the first hit");
  assert.equal(line(hit, true), "… 1 unmatched message …", "no line for the run after the last hit");
  // A tap on the card's line opens the run it stands for, and only that run.
  await sumCard.seamBelowLine.click();
  await page.settle();
  assert.deepStrictEqual(shownRows("spaces/A/threads/sum"), ["700", "701", "702"]);
  assert.equal(line(sumCard, true), null, "the opened run kept its line");
  // A word only one thread's card says chooses that card alone; NEGATIVE CONTROL for the reply rule.
  await page.el("search-field").setValue("jog memory");
  await page.settle();
  assert.deepStrictEqual(cards(), ["spaces/A/threads/sum"]);
  // The search closed: every block whole again, and no line left behind.
  await page.el("search-toggle").click();
  await page.settle();
  assert.deepStrictEqual(shownRows("spaces/A/threads/sum"), ["700", "701", "702", "703"]);
  // Closing the search draws the cards again: every card and row as it now stands.
  for (const li of page.el("side-cards").children) {
    assert.equal(line(li, true), null, `a line was left below ${li.getAttribute("data-id")}`);
    assert.equal(line(li, false), null, `a line was left above ${li.getAttribute("data-id")}`);
  }
});

test("`#606 server-side-search`: the cards' line for the threads after a card and its block's line for the run before its hit each keep their own place", async () => {
  const { page } = await unpackPage();
  await page.el("side-unpack").click();
  await page.settle();
  const line = (li, below) => {
    const seam = below ? li.seamBelowLine : li.seamLine;
    return seam && seam.parentNode === li ? seam.textContent : null;
  };
  const byId = (id) => page.el("side-cards").children.find((li) => li.getAttribute("data-id") === id ||
    (li.getAttribute("data-ids") || "").split(" ").includes(id));
  // Only the summed thread's second read reply says it: the raw thread after it is hidden.
  await search(page, "sum read two");
  await page.settle();
  for (let pass = 0; pass < 2; pass += 1) {
    assert.equal(line(byId("spaces/A/threads/sum"), true), "… 4 unmatched messages in 1 thread …",
      `pass ${pass}: the cards' line below the card was taken`);
    assert.equal(line(byId("702"), false), "… 2 unmatched messages …", `pass ${pass}: the block's run before its hit has no line`);
    assert.equal(line(byId("702"), true), "… 1 unmatched message …");
    // Another pass of the search draws the same lines, not the other's.
    await page.el("search-field").setValue("sum read two ");
    await page.settle();
  }
});

// --- `#605`, `#607`, `#608 server-side-search`: the server's copy, searched --------------------------

const SERVER_SEARCH_DELAY_MS = sourceConstant("SERVER_SEARCH_DELAY_MS");
const SEARCH_FURTHER_POLL_MS = sourceConstant("SEARCH_FURTHER_POLL_MS");

/** A channel with two threads, open in `view`, on a server that keeps a copy (or none, with `cache` false). */
async function serverSearchPage(view = "flat", cache = true) {
  const page = newPage();
  if (cache) page.messageCache = messageCacheStatus();
  const data = threadData();
  page.threadingSupported = true;
  page.threads = data.threads;
  await signIn(page);
  await showDiscord(page, data.messages);
  await pickThread(page, view);
  return page;
}

/** Type `query`, and let the typing pause long enough for the server to be asked and to answer. */
async function searchServer(page, query) {
  await search(page, query);
  await page.settle();
  page.expireTimers(SERVER_SEARCH_DELAY_MS);
  await page.settle();
}

/** One channel's part of a search answer: searched, back to `back`, with these facts. */
const searchedScope = (fields = {}) => ({
  channel: CHANNEL, state: "searched", examined: 120, searched_back_to_ms: Date.parse("2026-08-01T12:00:00Z"),
  held_from_ms: Date.parse("2026-08-01T12:00:00Z"), top_level_only: true, whole: false, expanding: true,
  capped: 0, partial: 0, withheld: 0, ...fields,
});

const searchAnswer = (hits, scopes, extra = {}) =>
  json(200, { hits, scopes, upstream: "none", untrusted_content_notice: "third-party text", ...extra });

test("`#605 server-side-search`: a pause in the typing searches the server's copy below what is loaded, and the line says what was searched", async () => {
  const page = await serverSearchPage();
  const old = message({ id: "150", content: "an old kestrel by the weir", timestamp: "2026-08-02T09:00:00Z" });
  page.serverSearch = async () => searchAnswer([old], [searchedScope()]);
  await search(page, "kestrel");
  await page.settle();
  assert.equal(page.serverSearchCalls.length, 0, "the server was asked before the typing paused");
  page.expireTimers(SERVER_SEARCH_DELAY_MS);
  await page.settle();
  assert.equal(page.serverSearchCalls.length, 1);
  const asked = new URL(page.serverSearchCalls[0], "http://fixture.test");
  assert.equal(asked.pathname, `/api/v1/channels/${CHANNEL.id}/search`);
  assert.equal(asked.searchParams.get("q"), "kestrel");
  assert.ok(Number(asked.searchParams.get("below")) > 0, "not only what lies below the loaded messages");
  assert.equal(page.el("server-search").hidden, false);
  assert.match(page.el("search-scope").textContent,
    /^No upstream search · this page: back to .+ · server: back to .+ \(still expanding\) · 125 messages searched$/);
  const hits = page.el("server-hits").children;
  assert.deepStrictEqual(hits.map((li) => li.getAttribute("data-id")), ["150"]);
  assert.ok(hits[0].descendants().some((n) => n.textContent === "Show in channel"), "a hit has no way to its channel");
  // The same search again asks nothing; a change of search asks again.
  await page.el("search-field").setValue("kestrel ");
  await page.settle();
  page.expireTimers(SERVER_SEARCH_DELAY_MS);
  await page.settle();
  assert.equal(page.serverSearchCalls.length, 1, "the same search was asked twice");
  // Closed, the section goes, and nothing more is asked.
  await page.el("search-toggle").click();
  await page.settle();
  assert.equal(page.el("server-search").hidden, true);
});

test("`#605 server-side-search`: the line says why the server did not search, and a page with no copy on the server asks nothing", async () => {
  const page = await serverSearchPage();
  page.serverSearch = async () => searchAnswer([], [{ ...searchedScope(), state: "not_searched", reason: "read_scope",
    examined: 0, searched_back_to_ms: undefined, held_from_ms: undefined }]);
  await searchServer(page, "kestrel");
  assert.match(page.el("search-scope").textContent, / · server: not searched \(this device holds the read-only token\) · /);
  page.serverSearch = async () => json(502, { error: "chat_error", detail: "the server is restarting" });
  await searchServer(page, "heron");
  assert.match(page.el("search-scope").textContent, / · server: could not be searched \(the server is restarting\) · /);
  // NEGATIVE CONTROL: no copy kept, nothing asked, and the line says so.
  const bare = await serverSearchPage("flat", false);
  await searchServer(bare, "kestrel");
  assert.equal(bare.serverSearchCalls.length, 0);
  assert.match(bare.el("search-scope").textContent, / · server: keeps no copy · /);
});

test("`#607 server-side-search`: hits the page already holds are not repeated, and All channels searches every channel", async () => {
  const page = await serverSearchPage();
  const held = page.messages.find((m) => m.id === "202");
  const elsewhere = { ...message({ id: "990", content: "a kestrel elsewhere", timestamp: "2026-08-03T09:00:00Z" }),
    channel_id: "2220000000000000002" };
  page.serverSearch = async (path) => (path.startsWith("/api/v1/search?")
    ? searchAnswer([elsewhere, held], [searchedScope(), searchedScope({ channel: { ...CHANNEL, id: "2220000000000000002", label: "ops" }, whole: true, expanding: false })])
    : searchAnswer([held], [searchedScope()]));
  await searchServer(page, "answer");
  assert.deepStrictEqual(page.el("server-hits").children.map((li) => li.getAttribute("data-id")), [],
    "a hit the page holds was drawn again");
  await page.el("search-all-channels").setChecked(true);
  await page.settle();
  page.expireTimers(SERVER_SEARCH_DELAY_MS);
  await page.settle();
  const asked = new URL(page.serverSearchCalls.at(-1), "http://fixture.test");
  assert.equal(asked.pathname, "/api/v1/search");
  assert.equal(asked.searchParams.get("below"), null, "every channel's newest, not below this page's floor");
  assert.deepStrictEqual(page.el("server-hits").children.map((li) => li.getAttribute("data-id")), ["990"]);
  assert.match(page.el("search-scope").textContent, / · server: 2 of 2 channels, all back to .+, 1 whole · /);
  // Counted once: the server's search of every channel includes this one's newest.
  assert.match(page.el("search-scope").textContent, / · 240 messages searched$/);
  assert.equal(page.el("search-further").hidden, true, "Search further back is one channel's");
});

test("`#605 server-side-search`: the line never says the server searched further back than it says it did", async () => {
  const page = await serverSearchPage();
  // A budget spent above the stretch the server vouches for: no reach to claim.
  page.serverSearch = async () => searchAnswer([], [searchedScope({ searched_back_to_ms: undefined,
    resume_below_ms: 1_700_000_000_000 })], { resume_below_ms: 1_700_000_000_000 });
  await searchServer(page, "kestrel");
  assert.match(page.el("search-scope").textContent, / · server: its newest part searched so far \(still expanding\) · /);
  // Every row held searched, none of it vouched whole.
  page.serverSearch = async () => searchAnswer([], [searchedScope({ searched_back_to_ms: undefined,
    held_from_ms: undefined, expanding: false })]);
  await searchServer(page, "heron");
  assert.match(page.el("search-scope").textContent, / · server: all it holds searched, with gaps · /);
  // NEGATIVE CONTROL: a reach the server says is said.
  page.serverSearch = async () => searchAnswer([], [searchedScope({ whole: true, expanding: false })]);
  await searchServer(page, "otter");
  assert.match(page.el("search-scope").textContent, / · server: back to .+ \(complete\) · /);
});

test("`#608 server-side-search`: a look that fails keeps watching and says so, and Stop then Search further back starts one watch, not two", async () => {
  const page = await serverSearchPage();
  page.serverSearch = async () => searchAnswer([], [searchedScope()]);
  const state = { channel_id: CHANNEL.id, server_mode: "full", mode: "full", settable: true, messages: 300,
    oldest_ms: Date.parse("2026-08-01T12:00:00Z"), walking: true, complete: false, cycle_pages: 3, queue_position: 0,
    now_ms: Date.now() };
  page.history.set(CHANNEL.id, state);
  await searchServer(page, "kestrel");
  const further = page.el("search-further");
  await further.click();
  await page.settle();
  page.historyFailure = { status: 502, error: "chat_error", detail: "the server is restarting" };
  page.expireTimers(SEARCH_FURTHER_POLL_MS);
  await page.settle();
  assert.match(page.el("search-scope").textContent, /gathering older history: the server is restarting/);
  page.historyFailure = null;
  page.expireTimers(SEARCH_FURTHER_POLL_MS);
  await page.settle();
  assert.match(page.el("search-scope").textContent, /gathering older history back to /, "the watch died after one failure");
  // Stop, and at once Search further back again: one watch, so each period asks once.
  await further.click();
  await further.click();
  await page.settle();
  const looks = () => page.historyCalls.filter((call) => call === `GET ${CHANNEL.id}`).length;
  const before = looks();
  page.expireTimers(SEARCH_FURTHER_POLL_MS);
  await page.settle();
  assert.equal(looks() - before, 1, "two watches ran at once");
});

test("`#605 server-side-search`: More from the server goes on below where its last answer stopped", async () => {
  const page = await serverSearchPage();
  const first = message({ id: "160", content: "kestrel one", timestamp: "2026-08-05T09:00:00Z" });
  const second = message({ id: "140", content: "kestrel two", timestamp: "2026-08-01T09:00:00Z" });
  const resume = Date.parse("2026-08-04T00:00:00Z");
  page.serverSearch = async (path) => (new URL(path, "http://fixture.test").searchParams.get("below") === String(resume)
    ? searchAnswer([second], [searchedScope()])
    : searchAnswer([first], [searchedScope({ resume_below_ms: resume })], { resume_below_ms: resume }));
  await searchServer(page, "kestrel");
  assert.match(page.el("search-scope").textContent, / · server: back to .+ so far \(still expanding\) · /);
  assert.equal(page.el("server-more").hidden, false);
  await page.el("server-more").click();
  await page.settle();
  assert.deepStrictEqual(page.el("server-hits").children.map((li) => li.getAttribute("data-id")), ["160", "140"]);
  assert.equal(page.el("server-more").hidden, true);
  assert.match(page.el("search-scope").textContent, / · 245 messages searched$/);
});

test("`#608 server-side-search`: Search further back asks the server to gather this channel next, watches it grow, searches again, and Stop takes it back", async () => {
  const page = await serverSearchPage();
  let oldest = Date.parse("2026-08-01T12:00:00Z");
  page.serverSearch = async () => searchAnswer([], [searchedScope({ held_from_ms: oldest, searched_back_to_ms: oldest })]);
  const state = () => ({ channel_id: CHANNEL.id, server_mode: "full", mode: "full", settable: true, messages: 300,
    oldest_ms: oldest, reached_ms: oldest, walking: true, complete: false, cycle_pages: 12, pages_per_hour: 60,
    days_per_hour: 2, eta_s: 600, queue_position: 0, requests_per_minute: 20, requests_last_minute: 3,
    now_ms: Date.now() });
  page.history.set(CHANNEL.id, state());
  await searchServer(page, "kestrel");
  const further = page.el("search-further");
  assert.equal(further.hidden, false, "Search further back was not offered over a copy that is not whole");
  await further.click();
  await page.settle();
  assert.deepStrictEqual(page.historyCalls, [`POST ${CHANNEL.id} prioritize`]);
  assert.equal(further.textContent, "Stop gathering older history");
  assert.match(page.el("search-scope").textContent, / · gathering older history back to .+, about 10 min left · /);
  // The walk reaches further back: the page sees it on its next look, and searches the copy again.
  const searched = page.serverSearchCalls.length;
  oldest = Date.parse("2026-07-01T12:00:00Z");
  page.history.set(CHANNEL.id, state());
  page.expireTimers(SEARCH_FURTHER_POLL_MS);
  await page.settle();
  assert.equal(page.historyCalls.at(-1), `GET ${CHANNEL.id}`);
  page.expireTimers(SERVER_SEARCH_DELAY_MS);
  await page.settle();
  assert.equal(page.serverSearchCalls.length, searched + 1, "the copy grew and was not searched again");
  // Stop: the bump is taken back and nothing more is asked.
  await further.click();
  await page.settle();
  assert.equal(page.historyCalls.at(-1), `DELETE ${CHANNEL.id} prioritize`);
  assert.equal(further.textContent, "Search further back");
  const looked = page.historyCalls.length;
  page.expireTimers(SEARCH_FURTHER_POLL_MS);
  await page.settle();
  assert.equal(page.historyCalls.length, looked, "the page went on watching after Stop");
  // NEGATIVE CONTROL: a server that gathers no history says so, and nothing is watched.
  page.historyFailure = { status: 409, error: "history_backfill_off", detail: "the backfill is off" };
  await further.click();
  await page.settle();
  assert.match(page.el("search-scope").textContent, /gathering older history: the server does not gather history for this channel/);
});

test("`#404 threads-unpack-all`: Unpack is a chip in the cards' heading, Pack all means cards alone, u is in the help", async () => {
  assertMarkupContains("side-heading", "side-unpack");
  const tag = /<button\s+id="side-unpack"[^>]*>/.exec(HTML_CODE)[0];
  assert.match(tag, /aria-pressed="false"/);
  assert.match(HTML_CODE, /id="side-unpack"[^>]*>Pack all<\/button>/);
  const { page } = await unpackPage();
  const toggle = page.el("side-unpack");
  assert.equal(toggle.hidden, false, "the toggle is not offered over the cards");
  assert.equal(toggle.getAttribute("aria-pressed"), "false");
  assert.equal(toggle.getAttribute("aria-keyshortcuts"), "U", "the toggle does not name its key");
  assert.deepStrictEqual(unpackRows(page, "spaces/A/threads/sum"), [], "off, messages are already unpacked");
  assert.ok(page.el("keyboard-keys").text().includes("Unpack"), "the ? help does not list the key");
  // `#440 thread-card-pack-toggle`: three states, Pack all, Unpack unread, Unpack all; u and a press go round.
  await page.documentEvent("keydown", { key: "u" });
  await page.settle();
  assert.equal(toggle.getAttribute("aria-pressed"), "mixed", "u did not go to Unpack unread");
  assert.equal(toggle.text(), "Unpack unread");
  assert.equal(page.storage.get(UNPACK_STORAGE), "unread", "the choice was not kept on the device");
  await page.documentEvent("keydown", { key: "u" });
  await page.settle();
  assert.equal(toggle.getAttribute("aria-pressed"), "true", "u did not go to Unpack all");
  assert.equal(toggle.text(), "Unpack all");
  assert.ok(unpackRows(page, "spaces/A/threads/sum").length > 0, "Unpack all left a thread packed");
  await toggle.click();
  await page.settle();
  assert.equal(toggle.text(), "Pack all");
  assert.deepStrictEqual(unpackRows(page, "spaces/A/threads/sum"), [], "Pack all left rows behind");
  assert.equal(page.storage.get(UNPACK_STORAGE), "packed");
});
