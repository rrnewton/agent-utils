// `#2233 vibe-talk-glossary-popup`: the full-screen glossary, opened by the Glossary button of the
// glossary popover (the same button for a WTF? answer and for a tapped defined term).
//
// The owner, 2026-11-25: "when I pop open the wtf window I want a button to jump to a separate glossary
// page or full screen popup like discuss ... When I click an existing defined term I want the pop up
// definition to share the same link to go to glossary. Searchable alphabetical by default."
//
// Every term here is synthetic. The glossary routes are the fixture's (`page.glossaryTerms`), in the
// server's shapes; tests/glossary_lookup.rs holds the real routes through the real router.

import assert from "node:assert/strict";

import {
  CHANNEL, CSS, SCRIPT, assertMarkupContains, brokenModule, focusName, focused, press, quotePage, rowWithId, test,
  untilDeepLink, wordsIn,
} from "./fixture.mjs";

/** The synthetic glossary: reviewed and learned entries, a message source, a document source, an accent and a digit. */
const TERMS = Object.freeze([
  { term: "runner", definition: "A synthetic build worker that takes queued jobs.", origin: "reviewed",
    source: { kind: "message", href: `?c=${CHANNEL.id}&v=main&m=discord:${CHANNEL.id}:1000000000000000002`, label: "the second message" } },
  { term: "Zephyr cache", definition: "A made-up cache that keeps runner results.", origin: "dynamic", source: null },
  { term: "deploy", definition: "Shipping a synthetic build to the test fleet.", origin: "dynamic",
    source: { kind: "document", href: "paste:synthetic-runbook", label: "Synthetic runbook" } },
  { term: "alpha quorum", definition: "Three made-up votes out of five.", origin: "reviewed", source: null },
  { term: "Élan gate", definition: "A synthetic gate named to test accents.", origin: "dynamic", source: null },
  { term: "42 rule", definition: "A synthetic rule about the order of a deploy.", origin: "reviewed", source: null },
]);

const PHONE = { name: "phone 412x915", width: 412, height: 915 };
const DESK = { name: "desktop 1280x800", width: 1280, height: 800 };

/** An event a tap hands its listeners: one target, its default refusable. */
const tap = (target) => ({
  target, defaultPrevented: false, preventDefault() { this.defaultPrevented = true; }, stopPropagation() {},
});

/** The main screen nested as web/voice.html nests it, so what the glossary makes inert can be read. */
function frame(page) {
  const nests = [
    ["frame-body", "screens"],
    ["screens", "screen-main"],
    ["screen-main", "discuss-panel", "glossary-panel", "scroll-area"],
    ["glossary-panel", "glossary-panel-close", "glossary-panel-search", "glossary-panel-state", "glossary-panel-retry",
      "glossary-panel-list"],
    ["glossary-popover", "glossary-popover-term", "glossary-popover-body", "glossary-popover-source", "glossary-popover-open",
      "glossary-popover-close"],
    ["dock", "read-speed"],
  ];
  page.document.body.append(page.el("topbar"), page.el("frame-body"), page.el("selection-chip-row"),
    page.el("glossary-popover"), page.el("dock"));
  for (const [parent, ...kids] of nests) {
    for (const kid of kids) assertMarkupContains(parent, kid);
    page.el(parent).append(...kids.map((kid) => page.el(kid)));
  }
}

/** The quote page's channel, framed, at `size`, with the synthetic glossary served and WTF? able to run. */
async function glossaryPage(size = PHONE, { terms = TERMS, arrange = null, script = SCRIPT } = {}) {
  const page = await quotePage((p) => {
    p.glossaryAvailable = true;
    p.glossaryTerms = terms.map((entry) => ({ ...entry }));
    if (arrange) arrange(p);
  }, script);
  page.document.documentElement.clientWidth = size.width;
  page.viewport.layout = size.height;
  page.viewport.visual = size.height;
  frame(page);
  await page.settle();
  return page;
}

/** Select `words` in the first message and press WTF?, as a finger does; the popover answers. */
async function pressWtf(page, words = "about the runner") {
  await page.selectText(wordsIn(rowWithId(page, "1000000000000000001")), null, words);
  await page.settle();
  await page.el("selection-chip-wtf").dispatch("click", tap(page.el("selection-chip-wtf")));
  await page.settle();
}

/** A known term in a message, as the linker draws one, and a tap on it (the fixture's DOM has no tree walker). */
async function tapDefinedTerm(page, term = "deploy") {
  const row = rowWithId(page, "1000000000000000002");
  const button = page.document.createElement("button");
  button.className = "glossary-term";
  button.textContent = term;
  button.setAttribute("data-glossary-term", term);
  wordsIn(row).append(button);
  button.closest = (selector) => (selector === ".glossary-term" ? button : null);
  const popover = page.el("glossary-popover");
  popover.contains = (node) => { for (let at = node; at; at = at.parentNode) if (at === popover) return true; return false; };
  // The term list is read on the first selection, as on a page.
  await page.selectText(wordsIn(rowWithId(page, "1000000000000000001")), null, "the first");
  await page.settle();
  await page.clearSelection();
  await page.settle();
  await page.documentEvent("click", tap(button));
  await page.settle();
  return button;
}

/** Press the popover's Glossary button. */
async function openGlossary(page) {
  await page.el("glossary-popover-open").dispatch("click", tap(page.el("glossary-popover-open")));
  await page.settle();
}

/** The entries on show, top to bottom, as their terms. */
const shownTerms = (page) => page.el("glossary-panel-list").descendants()
  .filter((node) => node.tagName === "li" && String(node.className).split(/\s+/).includes("glossary-entry"))
  .map((node) => node.descendants().find((kid) => kid.className === "glossary-entry-term").textContent);

/** The letter headings on show, top to bottom. */
const shownHeadings = (page) => page.el("glossary-panel-list").descendants()
  .filter((node) => node.tagName === "h3").map((node) => node.textContent);

/** The entry marked as the one the glossary was opened at, by its term, or null. */
const markedTerm = (page) => {
  const row = page.el("glossary-panel-list").descendants().find((node) => node.getAttribute("aria-current") === "true");
  return row ? row.descendants().find((kid) => kid.className === "glossary-entry-term").textContent : null;
};

/** Whether `node` or anything around it is inert now. */
const inertNow = (node) => {
  for (let at = node; at; at = at.parentNode) if (at.hasAttribute("inert")) return true;
  return false;
};

const ALPHABETICAL = ["42 rule", "alpha quorum", "deploy", "Élan gate", "runner", "Zephyr cache"];

for (const size of [PHONE, DESK]) {
  test(`\`#2233 vibe-talk-glossary-popup\`: the WTF? popover's Glossary button opens the whole glossary full screen, alphabetical, at the term it found; the X closes it and the focus goes back (${size.name})`, async () => {
    const page = await glossaryPage(size);
    await pressWtf(page);
    assert.equal(page.el("glossary-popover").hidden, false, "fixture: WTF? opened no popover");
    assert.equal(page.el("glossary-popover-term").textContent, "runner", "fixture: the lookup did not find the term");
    assert.equal(page.el("glossary-popover-open").hidden, false, "the WTF? popover carries no Glossary button");
    const reads = page.glossaryTermReads;
    await openGlossary(page);
    assert.equal(page.el("glossary-panel").hidden, false, "the Glossary button opened nothing");
    assert.equal(page.el("glossary-popover").hidden, true, "the popover stayed up over the glossary");
    assert.ok(page.glossaryTermReads > reads, "the glossary did not read the terms fresh as it opened");
    assert.deepStrictEqual(shownTerms(page), ALPHABETICAL, "the glossary is not alphabetical");
    assert.deepStrictEqual(shownHeadings(page), ["#", "A", "D", "E", "R", "Z"], "the letter headings are not the entries' first letters");
    assert.equal(markedTerm(page), "runner", "the glossary is not opened at the term the popover found");
    assert.equal(page.el("glossary-panel-count").textContent, "6 terms");
    // Badges from the data: reviewed and learned.
    const badges = page.el("glossary-panel-list").descendants().filter((node) => node.className === "glossary-entry-badge")
      .map((node) => `${node.getAttribute("data-origin")}=${node.textContent}`);
    assert.deepStrictEqual(badges, ["reviewed=Reviewed", "reviewed=Reviewed", "dynamic=Learned", "dynamic=Learned", "reviewed=Reviewed", "dynamic=Learned"]);
    // A desk types at once; a phone keeps its keyboard down, the focus in the popup itself.
    assert.equal(focused(page), size === DESK ? page.el("glossary-panel-search") : page.el("glossary-panel"),
      `the focus is on ${focusName(page)}`);
    await page.el("glossary-panel-close").click();
    await page.settle();
    assert.equal(page.el("glossary-panel").hidden, true, "the X did not close the glossary");
    assert.equal(focused(page), page.el("selection-chip-wtf"), `closing left the focus on ${focusName(page)}, not on WTF?`);
  });

  test(`\`#2233 vibe-talk-glossary-popup\`: a defined term's popup carries the same Glossary button, which opens the glossary at that term; Escape closes it and the focus goes back to the term (${size.name})`, async () => {
    const page = await glossaryPage(size);
    const term = await tapDefinedTerm(page, "deploy");
    assert.equal(page.el("glossary-popover").hidden, false, "fixture: the defined term opened no popup");
    assert.equal(page.el("glossary-popover-term").textContent, "deploy");
    assert.equal(page.el("glossary-popover-open").hidden, false, "the defined term's popup carries no Glossary button");
    await openGlossary(page);
    assert.equal(page.el("glossary-panel").hidden, false);
    assert.equal(markedTerm(page), "deploy", "the glossary is not opened at the tapped term");
    const escape = await press(page, focused(page), "Escape");
    assert.equal(escape.defaulted, false, "Escape in the glossary was left to the browser");
    assert.equal(page.el("glossary-panel").hidden, true, "Escape did not close the glossary");
    assert.equal(focused(page), term, `closing left the focus on ${focusName(page)}, not on the term`);
  });
}

test("`#2233 vibe-talk-glossary-popup`: the search filters as it is typed, terms first, then words in definitions, and says when nothing matches", async () => {
  const page = await glossaryPage(DESK);
  await pressWtf(page, "about the zzz");
  assert.equal(page.el("glossary-popover-open").hidden, false, "a not-found answer carries no Glossary button");
  await openGlossary(page);
  assert.equal(markedTerm(page), null, "a not-found answer opened the glossary at a term");
  const search = page.el("glossary-panel-search");
  await search.setValue("RUNNER");
  assert.deepStrictEqual(shownTerms(page), ["runner", "Zephyr cache"], "the term match is not first, then the definition match");
  assert.deepStrictEqual(shownHeadings(page), ["In definitions"]);
  assert.equal(page.el("glossary-panel-count").textContent, "2 of 6 terms");
  await search.setValue("  alpha   quorum ");
  assert.deepStrictEqual(shownTerms(page), ["alpha quorum"], "runs of spaces and case changed the match");
  await search.setValue("nothing synthetic like this");
  assert.deepStrictEqual(shownTerms(page), []);
  assert.equal(page.el("glossary-panel-state").hidden, false);
  assert.equal(page.el("glossary-panel-state").textContent, "No terms match “nothing synthetic like this”.");
  await search.setValue("");
  assert.deepStrictEqual(shownTerms(page), ALPHABETICAL, "clearing the search did not bring every term back");
  assert.equal(page.el("glossary-panel-state").hidden, true);
});

test("`#2233 vibe-talk-glossary-popup`: an empty glossary says how to learn a term, and a failed read says so with Retry", async () => {
  const page = await glossaryPage(PHONE, { terms: [] });
  await pressWtf(page, "about the runner");
  await openGlossary(page);
  assert.deepStrictEqual(shownTerms(page), []);
  assert.equal(page.el("glossary-panel-state").textContent,
    "No glossary entries yet. Select words in a message and press WTF? to learn one.");
  assert.equal(page.el("glossary-panel-retry").hidden, true);
  await page.el("glossary-panel-close").click();
  page.glossaryTermsStatus = 503;
  page.glossaryTerms = TERMS.map((entry) => ({ ...entry }));
  await pressWtf(page, "about the zzz");
  await openGlossary(page);
  assert.match(page.el("glossary-panel-state").textContent, /^Couldn't load the glossary/);
  assert.equal(page.el("glossary-panel-retry").hidden, false, "a failed read offers no Retry");
  page.glossaryTermsStatus = 200;
  await page.el("glossary-panel-retry").click();
  await page.settle();
  assert.deepStrictEqual(shownTerms(page), ALPHABETICAL, "Retry did not read the glossary again");
  assert.equal(page.el("glossary-panel-retry").hidden, true);
});

test("`#2233 vibe-talk-glossary-popup`: learned entries are text only: markup in a definition stays words, and a javascript: source is no link", async () => {
  const hostile = [
    { term: "img trap", definition: '<img src=x onerror="window.__glossaryPwned=1">a synthetic trap', origin: "dynamic",
      source: { kind: "message", href: "javascript:window.__glossaryPwned=1", label: "<b>click me</b>" } },
    { term: "<script>term</script>", definition: "a synthetic term with markup in its name", origin: "dynamic", source: null },
  ];
  const page = await glossaryPage(DESK, { terms: hostile });
  const createdBefore = page.createdTags.length;
  await pressWtf(page, "about the img trap");
  await openGlossary(page);
  const made = page.createdTags.slice(createdBefore);
  assert.ok(!made.includes("img") && !made.includes("script") && !made.includes("b"), `markup became elements: ${made.join(" ")}`);
  assert.equal(page.templatesParsed, 0, "the glossary parsed markup");
  const list = page.el("glossary-panel-list");
  const definitions = list.descendants().filter((node) => node.className === "glossary-entry-definition").map((node) => node.textContent);
  assert.ok(definitions.includes('<img src=x onerror="window.__glossaryPwned=1">a synthetic trap'), "the definition was not kept as words");
  assert.ok(shownTerms(page).includes("<script>term</script>"), "the term was not kept as words");
  // No link at all for the javascript: source: its label is plain words.
  assert.ok(!list.descendants().some((node) => node.tagName === "a"), "a javascript: source became a link");
  const label = list.descendants().find((node) => node.className === "glossary-entry-source-text");
  assert.equal(label && label.textContent, "<b>click me</b>");
  // And the popover shows no source line for it either.
  await page.el("glossary-panel-close").click();
  await pressWtf(page, "about the img trap");
  assert.equal(page.el("glossary-popover-term").textContent, "img trap", "fixture: the lookup did not find the hostile term");
  assert.equal(page.el("glossary-popover-source").hidden, true, "the popover linked a javascript: source");
  assert.notEqual(page.el("glossary-popover-link").getAttribute("href"), "javascript:window.__glossaryPwned=1");
});

test("`#2233 vibe-talk-glossary-popup`: while the glossary is up nothing under it takes a tap, a key or the focus, and closing gives back exactly what was there: Discuss open, the list where it was", async () => {
  const page = await glossaryPage(PHONE, { arrange: (p) => { p.discussAvailable = true; } });
  await page.el("discuss-open").click();
  await page.settle();
  assert.equal(page.el("discuss-panel").hidden, false, "fixture: Discuss did not open");
  const area = page.el("scroll-area");
  const scrollBefore = area.scrollTop;
  const rowTopBefore = rowWithId(page, "1000000000000000002").getBoundingClientRect().top;
  await pressWtf(page);
  await openGlossary(page);
  assert.equal(page.el("glossary-panel").hidden, false);
  // Inert: the strip, the list (and the composer under it), Discuss, the top bar, the dock with Pace and Read.
  for (const id of ["selection-chip-row", "scroll-area", "discuss-panel", "topbar", "dock", "read-speed"]) {
    assert.equal(inertNow(page.el(id)), true, `#${id} can still be reached under the glossary`);
  }
  assert.equal(inertNow(page.el("glossary-panel")), false, "the glossary made itself inert");
  // Beyond the main screen, and in the flow: dimmed too, so nothing unreachable looks pressable.
  assert.equal(page.el("topbar").hasAttribute("data-under-dim"), true, "the top bar is not dimmed");
  assert.equal(page.el("dock").hasAttribute("data-under-dim"), true, "the dock is not dimmed");
  // The page's keys stand down: Ctrl+/ opens no search, Ctrl+S takes no box, and Tab stays in the glossary.
  let key = await press(page, page.el("glossary-panel"), "/", { ctrlKey: true });
  assert.equal(page.el("search-field").hidden, true, "Ctrl+/ opened the search under the glossary");
  key = await press(page, page.el("glossary-panel"), "s", { ctrlKey: true });
  assert.ok(!String(focused(page)?.id || "").includes("compose"), `Ctrl+S took the focus to ${focusName(page)} under the glossary`);
  // Tab goes round the glossary's own stops: Shift+Tab from its first (the X) to its last (the runner's
  // source link), and Tab from there back to the X.
  key = await press(page, page.el("glossary-panel-close"), "Tab", { shiftKey: true });
  assert.equal(key.defaulted, false, "Shift+Tab from the glossary's first stop went on to the page under it");
  const last = page.el("glossary-panel-list").descendants().filter((node) => node.tagName === "a" || node.tagName === "button").at(-1);
  assert.equal(focused(page), last, `Shift+Tab from the X went to ${focusName(page)}, not the glossary's last stop`);
  key = await press(page, last, "Tab");
  assert.equal(key.defaulted, false, "Tab from the glossary's last stop went on to the page under it");
  assert.equal(focused(page), page.el("glossary-panel-close"), `Tab from the last stop went to ${focusName(page)}`);
  await page.el("glossary-panel-close").click();
  await page.settle();
  for (const id of ["selection-chip-row", "scroll-area", "discuss-panel", "topbar", "dock"]) {
    assert.equal(inertNow(page.el(id)), false, `#${id} stayed inert after the glossary closed`);
    assert.equal(page.el(id).hasAttribute("data-under-dim"), false, `#${id} stayed dimmed after the glossary closed`);
  }
  assert.equal(page.el("discuss-panel").hidden, false, "closing the glossary closed Discuss");
  assert.equal(area.scrollTop, scrollBefore, "the list moved under the glossary");
  assert.equal(rowWithId(page, "1000000000000000002").getBoundingClientRect().top, rowTopBefore, "a row moved under the glossary (dy must be 0)");
  // And the keys are the page's again.
  await press(page, null, "/", { ctrlKey: true });
  assert.equal(page.el("search-field").hidden, false, "fixture: Ctrl+/ opens the search with the glossary closed");
});

test("`#2233 vibe-talk-glossary-popup`: THE CONTROL: with the key dispatch not standing down, Ctrl+/ under the glossary reads as reaching the page", async () => {
  const guard = "    onEverywhereKey(event) {\n      if (!event || event.isComposing || event.keyCode === 229) return;\n";
  const control = await glossaryPage(PHONE, { script: await brokenModule([{ file: "src/view/shell/keydispatch.ts",
    find: `${guard}      if (deps.pageCovered?.()) return;\n`, replace: guard }]) });
  await pressWtf(control);
  await openGlossary(control);
  await press(control, control.el("glossary-panel"), "/", { ctrlKey: true });
  assert.equal(control.el("search-field").hidden, false, "a page whose keys never stand down still read as blocked, so the test cannot see it");
});

test("`#2233 vibe-talk-glossary-popup`: one phone Back closes the glossary alone; Discuss under it stays open", async () => {
  const page = await glossaryPage(PHONE, { arrange: (p) => { p.discussAvailable = true; p.enableHistory(); } });
  await page.el("discuss-open").click();
  await page.settle();
  assert.equal(page.el("discuss-panel").hidden, false, "fixture: Discuss did not open");
  await pressWtf(page);
  await openGlossary(page);
  await page.traverse();
  assert.equal(page.el("glossary-panel").hidden, false);
  await page.browserBack();
  await page.settle();
  assert.equal(page.left, false, "Back left the app");
  assert.equal(page.el("glossary-panel").hidden, true, "Back did not close the glossary");
  assert.equal(page.el("discuss-panel").hidden, false, "one Back closed Discuss as well as the glossary");
});

test("`#2233 vibe-talk-glossary-popup`: a message source closes the glossary and jumps to the message through the page's jump; a document source opens it in Documents", async () => {
  const page = await glossaryPage(PHONE, { arrange: (p) => {
    p.enableHistory();
    p.documents = [{ key: "paste:synthetic-runbook", title: "Synthetic runbook", content: "# Synthetic runbook\n\nMade-up steps.\n", outline: ["Synthetic runbook"] }];
  } });
  await pressWtf(page);
  await openGlossary(page);
  const list = page.el("glossary-panel-list");
  const messageLink = list.descendants().find((node) => node.getAttribute("data-glossary-source") === "message");
  assert.ok(messageLink, "the message source is no link");
  assert.equal(messageLink.tagName, "a");
  assert.equal(messageLink.getAttribute("href"), TERMS[0].source.href);
  const press1 = tap(messageLink);
  await list.dispatch("click", press1);
  await page.settle();
  assert.equal(press1.defaultPrevented, true, "the message source was left to the browser");
  assert.equal(page.el("glossary-panel").hidden, true, "the message source did not close the glossary");
  await untilDeepLink(page, () => rowWithId(page, "1000000000000000002")?.getAttribute("data-landed") === "true");
  assert.equal(rowWithId(page, "1000000000000000002").getAttribute("data-landed"), "true", "the jump did not land on the message");
  // A document source: the reader opens on it.
  await pressWtf(page);
  await openGlossary(page);
  const docLink = page.el("glossary-panel-list").descendants().find((node) => node.getAttribute("data-glossary-source") === "document");
  assert.ok(docLink, "the document source is no button");
  assert.equal(docLink.tagName, "button");
  await page.el("glossary-panel-list").dispatch("click", tap(docLink));
  await page.settle();
  assert.equal(page.el("glossary-panel").hidden, true, "the document source did not close the glossary");
  await untilDeepLink(page, () => !page.el("docs-reader").hidden && page.el("doc-title").text().includes("Synthetic runbook"));
  assert.equal(page.tab(), "docs", `the document source did not open Documents: ${page.requests.slice(-4).join(" | ")}`);
  assert.ok(page.el("doc-title").text().includes("Synthetic runbook"), "Documents did not open the source document");
});

test("`#2233 vibe-talk-glossary-popup`: the Glossary button shares the popover's top row with the X, adding no row (the popover stands no taller over the strip)", () => {
  assertMarkupContains("glossary-popover", "glossary-popover-open");
  // Out of the flow, in the top corner beside the X; the term line keeps clear of both.
  assert.match(CSS, /\.glossary-popover-open \{[^}]*position: absolute;[^}]*top: 0\.35rem;[^}]*right: 2\.5rem;/,
    "the Glossary button is not drawn in the popover's top corner");
  assert.match(CSS, /\.glossary-popover-term \{[^}]*padding-right: 5\.6rem;/, "the term line does not keep clear of the corner buttons");
});

/** Tap the popover's own source link, as a finger does. */
async function tapPopoverSource(page) {
  const link = page.el("glossary-popover-link");
  await link.dispatch("click", { ...tap(link), currentTarget: link });
  await page.settle();
}

const RUNBOOK = { key: "paste:synthetic-runbook", title: "Synthetic runbook", content: "# Synthetic runbook\n\nMade-up steps.\n", outline: ["Synthetic runbook"] };

test("`#2233 vibe-talk-glossary-popup`: the popover's document source opens the document in Documents (it used to load the reader and leave the channel on screen)", async () => {
  const page = await glossaryPage(PHONE, { arrange: (p) => { p.documents = [{ ...RUNBOOK }]; } });
  await tapDefinedTerm(page, "deploy");
  assert.equal(page.el("glossary-popover-source").hidden, false, "fixture: the defined term's popup shows no source");
  assert.equal(page.el("glossary-popover-link").textContent, "Synthetic runbook");
  await tapPopoverSource(page);
  assert.equal(page.el("glossary-popover").hidden, true, "the source tap left the popover up");
  await untilDeepLink(page, () => page.tab() === "docs" && page.el("doc-title").text().includes("Synthetic runbook"));
  assert.equal(page.tab(), "docs", "the document source left the channel on screen");
  assert.ok(page.el("doc-title").text().includes("Synthetic runbook"), "Documents did not open the source document");
});

test("`#2233 vibe-talk-glossary-popup`: THE CONTROL (the code before): without the switch to Documents, the source tap reads as leaving the channel on screen", async () => {
  const control = await glossaryPage(PHONE, {
    arrange: (p) => { p.documents = [{ ...RUNBOOK }]; },
    script: await brokenModule([{ file: "src/app/main.js",
      find: '    chooseMajorView("docs");\n    void documentsView.show(key);\n  },\n  tokenKnown:',
      replace: "    void documentsView.show(key);\n  },\n  tokenKnown:" }]),
  });
  await tapDefinedTerm(control, "deploy");
  await tapPopoverSource(control);
  await untilDeepLink(control, () => control.requests.some((request) => request.includes("/api/v1/documents/paste%3Asynthetic-runbook")));
  assert.ok(control.requests.some((request) => request.includes("/api/v1/documents/paste%3Asynthetic-runbook")), "fixture: the old code did not even read the document");
  assert.equal(control.tab(), "discord", "the old code switched to Documents after all, so the test cannot tell the fix from it");
});

test("`#2273 vibe-talk-cross-view-back-lands-wrong`: the popover's message source jumps in the app (a Back level), never reloading the page on its address", async () => {
  const page = await glossaryPage(PHONE, { arrange: (p) => { p.enableHistory(); } });
  await tapDefinedTerm(page, "runner");
  assert.equal(page.el("glossary-popover-source").hidden, false, "fixture: the defined term's popup shows no source");
  assert.equal(page.el("glossary-popover-link").textContent, "the second message");
  await tapPopoverSource(page);
  assert.equal(page.el("glossary-popover").hidden, true, "the source tap left the popover up");
  await untilDeepLink(page, () => rowWithId(page, "1000000000000000002")?.getAttribute("data-landed") === "true");
  assert.equal(rowWithId(page, "1000000000000000002")?.getAttribute("data-landed"), "true",
    "the message source did not land its message in the app");
});

test("`#2233 vibe-talk-glossary-popup`: a source jump taken with Discuss open beneath leaves Discuss open", async () => {
  const page = await glossaryPage(PHONE, { arrange: (p) => { p.enableHistory(); p.discussAvailable = true; } });
  await page.el("discuss-open").click();
  await page.settle();
  assert.equal(page.el("discuss-panel").hidden, false, "fixture: Discuss did not open");
  await pressWtf(page);
  await openGlossary(page);
  const list = page.el("glossary-panel-list");
  const messageLink = list.descendants().find((node) => node.getAttribute("data-glossary-source") === "message");
  await list.dispatch("click", tap(messageLink));
  await page.settle();
  await untilDeepLink(page, () => rowWithId(page, "1000000000000000002")?.getAttribute("data-landed") === "true");
  assert.equal(page.el("discuss-panel").hidden, false, "the source jump closed Discuss");
});
