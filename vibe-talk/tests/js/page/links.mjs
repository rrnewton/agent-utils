// The app page's tests: the links in a message and under Links, and opening them in the app.
//
// One feature's file of the suite tests/js/voice_page.test.mjs runs, against the fixture in
// ./fixture.mjs. Run with the rest of the suite, or alone: node --test tests/js/page/links.mjs.

import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

import { CHANNEL, CSS, GLYPHS, HOLD_MS, HTML_CODE, REPO, SCRIPT, SCRIPT_CODE, SETTINGS,
  assertMarkupContains, assistantSays, atBottomOf, backFacts, backlog, brokenModule, brokenScript,
  cssBlock, cssRules, doneButton, inReadingMode, linkPairs, linksOf, longMessage, message, newPage,
  pagedTimeline, passTime, reReadChannel, readAloudMessages, reloadWith, reopenedChannel, rowMore,
  rowMoreButton, rowMoreMenu, rowState, rowWithId, search, showDiscord, showLinks, signIn,
  sourceConstant, startTalking, statusText, storedPin, tapKind, tapPin, test, threadData,
  turnTodoOn, unfiltered, untilDeepLink, upstreamReadButton } from "./fixture.mjs";

// --- `#543 in-app-message-links` ----------------------------------------------------------------------
//
// The server marks a link to a chat message with what it names (src/message_links.rs, tested there);
// these walks hand the page bodies as the server writes them and check what it does with the mark: a
// link to a channel this server reads is drawn as the app's and jumps, every jump is one history entry,
// and Back puts the reader back where they were, one jump at a time.

const IN_APP_LINKS = "vibe-talk.voice.in-app-links";
const MSG_LINK_ATTRS = 'target="_blank" rel="noopener noreferrer nofollow"';

/** A message whose body is one link to `href`, marked `mark` as the server marks one, its text `text`. */
function linkMessage(id, href, mark, text = href) {
  const escaped = href.replace(/&/g, "&amp;");
  return message({
    id, content: `see ${href}`,
    content_html: `<p>see <a href="${escaped}" data-msg="${mark}" ${MSG_LINK_ATTRS}>${text.replace(/&/g, "&amp;")}</a></p>\n`,
  });
}

/** The mark of a link to `message` in `channel`, a reply in `thread` when there is one. */
const msgMark = (channel, message, thread = "") =>
  ["discord", channel, message, thread].map((part) => encodeURIComponent(part)).join(":");

/** The first link drawn in `row`. */
const linkIn = (row) => row.descendants().find((node) => node.tagName === "a");

/** A tap on `anchor` as a finger makes it: what the page did with the browser's own act is returned. */
async function tapLink(page, anchor, extra = {}) {
  const event = { button: 0, prevented: false, preventDefault() { this.prevented = true; }, stopPropagation() {}, ...extra };
  await anchor.dispatch("click", event);
  await page.settle();
  return event;
}

const REPLY_HREF = "https://discord.com/channels/100000000000000009/1110000000000000001/204";
const ROOT_HREF = "https://discord.com/channels/100000000000000009/1110000000000000001/200";

/**
 * `threadData`'s channel, tall: a message linking to the second thread's answer (`204`), a dozen long
 * fillers after it, so the reader is scrolled up to the link, and the answer itself linking back to the
 * channel's first message (`200`) for a second jump.
 */
function linkedThreadData() {
  const data = threadData();
  const another = data.messages[4].thread;
  data.messages[4] = linkMessage("204", ROOT_HREF, msgMark(CHANNEL.id, "200"));
  data.messages[4].thread = another;
  const link = linkMessage("300", REPLY_HREF, msgMark(CHANNEL.id, "204", "spaces/A/threads/another"));
  const fillers = Array.from({ length: 12 }, (_unused, i) =>
    message({ id: `${310 + i}`, content: longMessage(`filler ${i}`) }));
  return { messages: [...data.messages, link, ...fillers], threads: data.threads };
}

/** A page on `linkedThreadData`'s channel that keeps a session history, scrolled up to the link. */
async function linkedPage({ script = SCRIPT, stored = new Map() } = {}) {
  const page = newPage(stored, script, (p) => p.enableHistory());
  const data = linkedThreadData();
  page.threadingSupported = true;
  page.threads = data.threads;
  await signIn(page);
  await showDiscord(page, data.messages);
  await page.settle();
  const area = page.el("scroll-area");
  const row = rowWithId(page, "300");
  assert.ok(row, "fixture: the message with the link is not drawn");
  area.scrollTop += row.getBoundingClientRect().top - area.getBoundingClientRect().top - 40;
  await area.dispatch("scroll");
  await page.settle();
  return page;
}

test("`#543 in-app-message-links`: a marked link to a channel this app reads is drawn as the app's, named by where it goes; one to another channel, an unmarked one and every one with the switch off are left as written", async () => {
  const page = newPage();
  await signIn(page);
  const elsewhere = "https://discord.com/channels/1/1999999999999999999/5";
  const rows = await showDiscord(page, [
    linkMessage("8000000000000000100", REPLY_HREF, msgMark(CHANNEL.id, "204", "spaces/A/threads/another")),
    linkMessage("8000000000000000101", ROOT_HREF, msgMark(CHANNEL.id, "200"), "the plan"),
    linkMessage("8000000000000000102", elsewhere, msgMark("1999999999999999999", "5")),
    message({ id: "8000000000000000103", content: "x",
      content_html: `<p><a href="https://example.com/a" ${MSG_LINK_ATTRS}>https://example.com/a</a></p>\n` }),
  ]);
  const [reply, named, other, plain] = rows.map(linkIn);
  assert.ok(reply.hasAttribute("data-in-app"), "a link to a message here is not the app's");
  assert.equal(reply.text(), "#lead team › reply", "a link whose text was its address is not named by where it goes");
  assert.equal(reply.getAttribute("href"), REPLY_HREF, "the address under the link changed");
  assert.match(reply.getAttribute("title"), /^Opens here — https:\/\/discord\.com\//);
  assert.ok(named.hasAttribute("data-in-app"));
  assert.equal(named.text(), "the plan", "a link its writer named lost the name");
  assert.ok(!other.hasAttribute("data-in-app"), "a link to a channel this app does not read was taken");
  assert.equal(other.text(), elsewhere);
  assert.ok(!plain.hasAttribute("data-in-app"));
  // A tap on the one to another channel is the browser's: nothing prevented.
  assert.equal((await tapLink(page, other)).prevented, false, "a link to a channel not read was kept from leaving");
  // A new tab asked for is the browser's too.
  assert.equal((await tapLink(page, reply, { ctrlKey: true })).prevented, false, "Ctrl+click was taken from the browser");

  // The switch, ON by default, and off: every link is as written again, and its tap leaves.
  assert.equal(page.el("in-app-links").checked, true, "message links are not kept in the app by default");
  await page.el("in-app-links").setChecked(false);
  assert.equal(page.storage.get(IN_APP_LINKS), "0", "the choice was not kept");
  const off = linkIn(rowWithId(page, "8000000000000000100"));
  assert.ok(!off.hasAttribute("data-in-app"), "with the switch off a link was still the app's");
  assert.equal(off.text(), REPLY_HREF);
  assert.equal((await tapLink(page, off)).prevented, false, "with the switch off a tap was still kept in the app");
  assert.ok(SETTINGS.SYNCED_SETTING_KEYS.includes(page.pure("IN_APP_LINKS_KEY")), "the switch is not one of the synced settings");
  // The pure reading of a mark: four escaped parts, an absent one empty; nothing else is one.
  assert.deepStrictEqual(GLYPHS.messageLinkRef("gchat:spaces%2FA:spaces%2FA%2Fmessages%2FT.M:"),
    { service: "gchat", channel: "spaces/A", message: "spaces/A/messages/T.M", thread: null });
  assert.equal(GLYPHS.messageLinkRef("slack:C1:2"), null);
  assert.equal(GLYPHS.messageLinkRef(":C1::"), null);
});

test("`#543 in-app-message-links`: a tap jumps to a reply — its thread opens over All and the message is landed — as ONE history entry, and Back puts the reader back in All on the message their eye was on, at the same height", async () => {
  const page = await linkedPage();
  const area = page.el("scroll-area");
  const before = { length: page.history.length, facts: backFacts(page), top: rowWithId(page, "300").getBoundingClientRect().top,
    scroll: area.scrollTop };
  assert.equal(page.el("thread-select").value, "flat", "fixture: the channel did not open in All");
  assert.ok(before.scroll > 0, "fixture: the reader is not scrolled up the list");
  const tap = await tapLink(page, linkIn(rowWithId(page, "300")));
  assert.equal(tap.prevented, true, "the tap was left to the browser, which would leave the app");
  await untilDeepLink(page, () => rowWithId(page, "204")?.getAttribute("data-landed") === "true");
  assert.equal(page.el("thread-heading").hidden, false, "the jump did not open the reply's thread");
  assert.equal(page.el("thread-select").value, "thread:spaces/A/threads/another");
  assert.equal(rowWithId(page, "204").getAttribute("data-landed"), "true", "the linked reply was not landed on");
  assert.equal(page.history.length - before.length, 1, "the jump did not push exactly one entry");
  assert.deepStrictEqual(backFacts(page), { depth: 1, tag: "jump" }, "the jump is not one level over the channel");

  await page.browserBack();
  await untilDeepLink(page, () => page.el("thread-heading").hidden && statusText(page) === "Back where you were.");
  assert.equal(page.left, false, "Back after a jump left the app");
  assert.equal(page.el("thread-heading").hidden, true, "Back did not close the thread the jump opened");
  assert.equal(page.el("thread-select").value, "flat", "Back did not return to All");
  assert.deepStrictEqual(backFacts(page), { depth: 0, tag: "top" });
  const back = rowWithId(page, "300");
  assert.ok(back, "the message with the link is not drawn after Back");
  assert.ok(Math.abs(back.getBoundingClientRect().top - before.top) <= 1,
    `Back put the reader's message at ${back.getBoundingClientRect().top}, not ${before.top}`);
  // At the top, Back is the browser's as ever: the jump left nothing behind to spend a press on.
  await page.browserBack();
  assert.equal(page.left, true, "a Back at the top after a jump and its return did not leave the app");

  // NEGATIVE CONTROL: with no way back from a jump, Back lands and leaves the thread open.
  const deaf = await linkedPage({ script: brokenScript('  else if (tag === "jump") returnFromLinkJump();\n', "\n") });
  await tapLink(deaf, linkIn(rowWithId(deaf, "300")));
  await untilDeepLink(deaf, () => rowWithId(deaf, "204")?.getAttribute("data-landed") === "true");
  await deaf.browserBack();
  await deaf.settle();
  assert.equal(deaf.el("thread-heading").hidden, false, "the control without a return closed the thread anyway");
});

test("`#543 in-app-message-links`: JUMPS STACK — a link in the landed reply jumps on to a root in Main's view, two entries deep; Back returns to the reply in its thread, and Back again to All, one jump at a time", async () => {
  const page = await linkedPage();
  const start = rowWithId(page, "300").getBoundingClientRect().top;
  await tapLink(page, linkIn(rowWithId(page, "300")));
  await untilDeepLink(page, () => rowWithId(page, "204")?.getAttribute("data-landed") === "true");
  // The second link, in the reply the first landed on: to the channel's first message, a root.
  await tapLink(page, linkIn(rowWithId(page, "204")));
  await untilDeepLink(page, () => page.el("thread-heading").hidden && rowWithId(page, "200")?.getAttribute("data-landed") === "true");
  assert.equal(page.el("thread-heading").hidden, true, "a jump to a root stayed in the thread, which cannot show it");
  assert.equal(page.el("thread-select").value, "flat", "a jump to a root from a thread opened from All did not go back to All");
  assert.deepStrictEqual(backFacts(page), { depth: 2, tag: "jump" }, "the second jump is not a level over the first");

  await page.browserBack();
  await untilDeepLink(page, () => !page.el("thread-heading").hidden && rowWithId(page, "204"));
  assert.equal(page.el("thread-select").value, "thread:spaces/A/threads/another", "the first Back did not return to the reply's thread");
  assert.deepStrictEqual(backFacts(page), { depth: 1, tag: "jump" });
  await page.browserBack();
  await untilDeepLink(page, () => page.el("thread-heading").hidden && statusText(page) === "Back where you were.");
  assert.equal(page.el("thread-select").value, "flat", "the second Back did not return to All");
  assert.deepStrictEqual(backFacts(page), { depth: 0, tag: "top" });
  assert.ok(Math.abs(rowWithId(page, "300").getBoundingClientRect().top - start) <= 1,
    "the second Back did not put the reader back on the message they started from");
  assert.equal(page.left, false);
});

test("`#543 in-app-message-links`: what the reader opens after landing is a level over the jump, and the jump's own thread closed by its ← Back still leaves the jump for the gesture", async () => {
  const page = await linkedPage();
  await tapLink(page, linkIn(rowWithId(page, "300")));
  await untilDeepLink(page, () => rowWithId(page, "204")?.getAttribute("data-landed") === "true");
  // Settings over the landed reply: its own entry, closed first.
  await page.el("open-settings").click();
  await page.settle();
  assert.deepStrictEqual(backFacts(page), { depth: 2, tag: "settings" });
  await page.browserBack();
  assert.equal(page.screen(), "main", "the gesture did not close Settings first");
  assert.equal(page.el("thread-heading").hidden, false, "closing Settings undid the jump too");
  assert.deepStrictEqual(backFacts(page), { depth: 1, tag: "jump" });
  // The thread the jump opened, left by its own ← Back: the jump still stands, and the gesture undoes it.
  await page.el("thread-back").click();
  await page.settle();
  await page.traverse();
  assert.deepStrictEqual(backFacts(page), { depth: 1, tag: "jump" }, "leaving the jump's thread took the jump's entry with it");
  await page.browserBack();
  await untilDeepLink(page, () => statusText(page) === "Back where you were.");
  assert.equal(page.left, false);
  assert.equal(page.el("thread-select").value, "flat");
  assert.deepStrictEqual(backFacts(page), { depth: 0, tag: "top" });
});

test("`#543 in-app-message-links`: a Back while the jump still walks back for its message ends the walk — nothing lands over the place put back", async () => {
  const page = newPage(new Map(), SCRIPT, (p) => p.enableHistory());
  const all = pagedTimeline(page, { steps: 4, size: 4 });
  const newest = all[all.length - 1];
  const href = `https://discord.com/channels/100000000000000009/${CHANNEL.id}/${all[1].id}`;
  newest.content_html = `<p>see <a href="${href}" data-msg="${msgMark(CHANNEL.id, all[1].id)}" ${MSG_LINK_ATTRS}>${href}</a></p>\n`;
  // Every step back is held until the test lets it go, so the reader's Back comes mid-walk.
  const serve = page.timeline;
  let release = () => {};
  const held = new Promise((resolve) => { release = resolve; });
  page.timeline = async (path, options) => {
    if (/[?&]before=/.test(path)) await held;
    return serve(path, options);
  };
  await signIn(page);
  await showDiscord(page, []);
  await page.settle();
  await linkIn(rowWithId(page, newest.id)).dispatch("click", { button: 0, preventDefault() {}, stopPropagation() {} });
  await passTime(page, 60);
  assert.deepStrictEqual(backFacts(page), { depth: 1, tag: "jump" }, "fixture: the jump is not under way");
  await page.browserBack();
  await untilDeepLink(page, () => statusText(page) === "Back where you were.");
  assert.deepStrictEqual(backFacts(page), { depth: 0, tag: "top" });
  release();
  for (let spins = 0; spins < 20; spins += 1) {
    await passTime(page, 60);
    await page.settle();
  }
  assert.notEqual(rowWithId(page, all[1].id)?.getAttribute("data-landed"), "true",
    "the walk the reader's Back ended landed on the linked message anyway");
  assert.doesNotMatch(page.el("status").textContent, /Here is the linked message/);
});

test("mark read through here targets the newest part without archiving the row", async () => {
  const page = newPage();
  page.upstreamReadMarkSupported = true;
  await signIn(page);
  const messages = [
    message({
      id: "8000000000000000100",
      content: "first half",
      timestamp: "2026-08-19T04:31:00.000Z",
    }),
    message({
      id: "8000000000000000101",
      content: "second half",
      timestamp: "2026-08-19T04:31:01.000Z",
    }),
  ];
  const rows = await showDiscord(page, messages);
  assert.equal(rows.length, 1, "the fixture did not produce one combined row");
  const control = upstreamReadButton(rows[0]);
  assert.equal(control.hidden, false, "the advertised provider action stayed hidden");
  // Named for what it changes: the chat service's own marker, for the whole space. Short on screen
  // under the group's "Mark read" caption, and whole to a screen reader (`#206 pin-message`).
  assert.equal(control.textContent, "In Discord only · whole space");
  assert.equal(control.getAttribute("aria-label"), "Mark read in Discord only · whole space");
  assert.match(control.getAttribute("title"), /Discord's own read marker for the whole space/);
  // It lives under the row's ⋯ menu, closed until asked for, so it costs the row no width.
  assert.equal(rowMore(rows[0]).hidden, false, "the ⋯ menu holding the action is not offered");
  assert.equal(rowMoreMenu(rows[0]).hidden, true, "the ⋯ menu was open before it was asked for");
  await rowMoreButton(rows[0]).click();
  assert.equal(rowMoreMenu(rows[0]).hidden, false, "⋯ did not open the menu");
  assert.equal(rowMoreButton(rows[0]).getAttribute("aria-expanded"), "true");

  await control.click();
  await page.settle();

  assert.deepStrictEqual(
    page.upstreamReadCalls,
    [{ message_id: "8000000000000000101" }],
    "the provider cursor stopped before the end of the combined row"
  );
  assert.deepStrictEqual(page.dismissCalls, [], "marking upstream silently archived the row here");
  assert.equal(rowMoreMenu(rows[0]).hidden, true, "the ⋯ menu stayed open after its action ran");
  assert.equal(rowState(page, 0).archived, "false");
  assert.match(page.el("status").textContent, /source chat provider/);
});

test("the head of the to-do list says how much is left OF how much, not how big the channel is", async () => {
  // `#62 message-count-accuracy` again, one level in: "3 messages" over a filtered list reads as
  // the size of the channel, which is the confidently-wrong number that issue exists to stop.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog(4));
  await turnTodoOn(page);
  await doneButton(page.el("discord-log").children[0]).click();
  await page.settle();

  const head = page.el("channel-summary").text();
  assert.match(head, /3 of 4/, `the head of the list does not say 3 of 4: ${head}`);
});

// --- the links in what is loaded ------------------------------------------------------------------
//
// `#211 link-filter`. The owner, on his phone: "I want another little icon next to it with a
// hyperlink symbol which filters just for hyperlinks ... filter down to only the messages containing
// hyperlinks and then filter the string of the message text for only the hyperlinks themselves
// formatted one per line and turn it into real clickable hyperlinks so that clicking them opens it
// in a new tab." And: "if we find markdown formatted hyperlinks that are complete and correct, then
// let's format those properly as a real hyperlink using the name given in markdown rather than the
// raw URL." What is pinned down here: the toggle and where it lives, which addresses are read out of
// a message and what each is called, the view that shows them, how it combines with the search text
// and with Pinned, what it says when it has nothing to show, and that a tap on a link opens the link
// and does nothing else to the row.

const rowLinkList = (li) => li.children.find((node) => node.className === "row-links");

/** The channel rows the filters are letting through, by the text of their first message. */
const linkRowTexts = (page, id = "discord-log") =>
  unfiltered(page, id).map((li) => li.messages.map((m) => m.content).join(" / "));

test("the Links filter sits right beside Pinned, only while the bar is open, and says what it does", async () => {
  // Markup: field, count, Links, Pinned, the glass — Links IMMEDIATELY before Pinned.
  assertMarkupContains("search-float", "links-filter");
  const bar = HTML_CODE.slice(HTML_CODE.indexOf('id="search-float"'));
  const at = (id) => bar.indexOf(`id="${id}"`);
  assert.ok(at("search-count") < at("links-filter") && at("links-filter") < at("pinned-filter"),
    "the Links filter is not between the count and Pinned");
  assert.equal(bar.slice(bar.indexOf("</button>", at("links-filter")) + "</button>".length, at("pinned-filter")).trim(),
    "<button", "something stands between Links and Pinned");
  // An icon in the house style, the same stroke, cap and join as the pin and the glass.
  const svgOf = (id) => {
    const from = bar.indexOf("<svg", at(id));
    return bar.slice(from, bar.indexOf(">", from));
  };
  for (const attribute of ['class="icon"', 'stroke-width="1.9"', 'stroke-linecap="round"', 'aria-hidden="true"']) {
    assert.ok(svgOf("links-filter").includes(attribute), `the link icon does not share ${attribute} with the pin`);
  }
  const markup = bar.slice(at("links-filter"), bar.indexOf(">", at("links-filter")));
  assert.match(markup, /aria-label="Show only links"/);
  assert.match(markup, /aria-pressed="false"/);
  assert.match(markup, /class="icon-button"/);
  // The same square as Pinned, sized and drawn by the same rules.
  for (const selector of ["#links-filter", "#links-filter::before", "#links-filter .icon"]) {
    assert.deepStrictEqual(cssRules(CSS, selector), cssRules(CSS, selector.replace("links", "pinned")).slice(0, 1),
      `${selector} is not drawn by the rule that draws Pinned`);
  }
  assert.match(cssBlock("#links-filter"), /width: var\(--toggle-hit\)/);
  assert.match(cssBlock("#links-filter[aria-pressed=\"true\"]"), /color: var\(--accent\)/, "Links on does not say so");

  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog(2));
  const filter = page.el("links-filter");
  assert.equal(filter.hidden, true, "the filter is offered before the search bar is open");
  await page.el("search-toggle").click();
  assert.equal(filter.hidden, false, "the open bar has no Links filter");
  assert.equal(page.el("pinned-filter").hidden, false);
  await filter.click();
  await page.settle();
  assert.equal(filter.getAttribute("aria-pressed"), "true", "the filter does not say it is on");
  // Over the call view too — it filters what the search filters — where Pinned is not offered.
  await page.el("view-switch").click();
  await page.settle();
  assert.equal(filter.hidden, false, "the Links filter is not offered over the call view");
  assert.equal(page.el("pinned-filter").hidden, true);
  assert.equal(filter.getAttribute("aria-pressed"), "true", "switching views turned the filter off");
  await page.el("view-switch").click();
  await page.settle();

  // Closing the bar turns it off with the text, and so does Escape: no filter is ever left in force
  // behind a control that is not on screen.
  await page.el("search-toggle").click();
  await page.settle();
  assert.equal(filter.getAttribute("aria-pressed"), "false", "the filter outlived the bar");
  assert.equal(filter.hidden, true);
  await showLinks(page);
  assert.equal(filter.getAttribute("aria-pressed"), "true");
  await page.el("search-field").dispatch("keydown", { key: "Escape" });
  await page.settle();
  assert.equal(filter.getAttribute("aria-pressed"), "false", "Escape left the filter on");
  assert.equal(unfiltered(page, "discord-log").length, 2, "a row stayed hidden after the filter went");
});

test("every form of link is found, named as written or by its address, and only http(s) becomes one", async () => {
  const page = newPage();
  await signIn(page);
  const cases = [
    ["markdown", "the [Docs](https://example.com/docs) for it", [["Docs", "https://example.com/docs"]]],
    ["angle with a name", "plan: <https://example.org/plan|the plan>", [["the plan", "https://example.org/plan"]]],
    ["angle", "plan: <https://example.org/bare>", [["https://example.org/bare", "https://example.org/bare"]]],
    ["bare, full stop after", "see https://example.com/c.", [["https://example.com/c", "https://example.com/c"]]],
    ["trailing punctuation and quotes",
      "(https://example.com/d), and 'https://example.com/e'! Or https://example.com/f? \"https://example.com/g\";",
      [["https://example.com/d", "https://example.com/d"], ["https://example.com/e", "https://example.com/e"],
        ["https://example.com/f", "https://example.com/f"], ["https://example.com/g", "https://example.com/g"]]],
    ["parentheses that pair are the address's",
      "https://en.wikipedia.org/wiki/Mercury_(planet) and [Venus](https://en.wikipedia.org/wiki/Venus_(planet))",
      [["https://en.wikipedia.org/wiki/Mercury_(planet)", "https://en.wikipedia.org/wiki/Mercury_(planet)"],
        ["Venus", "https://en.wikipedia.org/wiki/Venus_(planet)"]]],
    ["a query and a fragment", "https://example.com/s?q=a&b=c#top,",
      [["https://example.com/s?q=a&b=c#top", "https://example.com/s?q=a&b=c#top"]]],
    ["a title after the address", "[Guide](https://example.com/guide \"the guide\")", [["Guide", "https://example.com/guide"]]],
    ["the same address twice, named the second time",
      "https://example.com/h, again https://example.com/h and [the same page](https://example.com/h) then https://example.org/i",
      [["the same page", "https://example.com/h"], ["https://example.org/i", "https://example.org/i"]]],
    ["an unclosed name", "[Docs(https://example.com/m1)", [["https://example.com/m1", "https://example.com/m1"]]],
    ["an empty name", "[](https://example.com/m2)", [["https://example.com/m2", "https://example.com/m2"]]],
    ["an unclosed target", "[Docs](https://example.com/m3", [["https://example.com/m3", "https://example.com/m3"]]],
    ["no opening bracket", "Docs](https://example.com/m4)", [["https://example.com/m4", "https://example.com/m4"]]],
    ["a space before the target", "[Docs] (https://example.com/m5)", [["https://example.com/m5", "https://example.com/m5"]]],
    ["a script target hiding an address", "[x](javascript:alert(1)//https://example.com/n)",
      [["https://example.com/n", "https://example.com/n"]]],
  ];
  const none = [
    "[run](javascript:alert(1)) and [data](data:text/html,<b>x</b>) and [ftp](ftp://example.com/x)",
    "javascript:alert(2) data:text/html,x www.example.com/x mailto:someone@example.com",
    "xhttps://example.com/glued <mailto:someone@example.com|mail> http:// and https://",
  ];
  const messages = [
    ...cases.map(([, content], i) => message({ id: `81000000000000000${String(i).padStart(2, "0")}`, content })),
    ...none.map((content, i) => message({ id: `82000000000000000${String(i).padStart(2, "0")}`, content })),
  ];
  const rows = await showDiscord(page, messages);
  await showLinks(page);
  for (const [i, [name, , expected]] of cases.entries()) {
    assert.deepStrictEqual(linkPairs(rows[i]), expected, `${name}: ${cases[i][1]}`);
    for (const anchor of linksOf(rows[i])) {
      assert.equal(anchor.tagName, "a", `${name}: a link that is not a real link`);
      assert.equal(anchor.getAttribute("target"), "_blank", `${name}: a link that does not open a new tab`);
      assert.equal(anchor.getAttribute("rel"), "noopener noreferrer", `${name}: the new tab can reach back into this one`);
      assert.equal(anchor.getAttribute("title"), anchor.getAttribute("href"), `${name}: the address is not in the title`);
      assert.match(anchor.getAttribute("href"), /^https?:\/\//);
    }
  }
  for (const [i, content] of none.entries()) {
    const li = rows[cases.length + i];
    assert.deepStrictEqual(linkPairs(li), [], `a link was made out of: ${content}`);
    assert.equal(li.hasClass("search-hidden"), true, `a row with no http(s) link is shown: ${content}`);
  }
  // Nothing anywhere on the page became an href that is not http(s).
  for (const node of page.el("discord-log").descendants()) {
    const href = node.getAttribute("href");
    if (href !== null) assert.match(href, /^https?:\/\//, `an href of ${href}`);
  }
});

test("reading the links out of a message takes time that grows with its length, not with its square", async () => {
  // `#211 link-filter`, from review. Taking a bare address's unpaired closers off its end counted
  // the brackets of the whole address again for each one taken, and a bare address runs to the end
  // of its message: one address followed by 40,000 `)` — the longest message Slack allows, and
  // anyone in the channel can post it — took 15s in Node, on the page's one thread, and again on
  // every redraw of the row while Links is on. Two scans read a long way for an address that was
  // never going to pass: a run of `https:///` was read to its end once per repeat (1.9s), and a run
  // of `[a](` read the longest target for every bracket (0.6s). And a Markdown target that does start
  // as an address is read a character at a time, up to LINK_TARGET_MAX for every bracket (0.2s).
  const page = newPage();
  await signIn(page);
  const contents = [
    `see https://example.com/paren${")".repeat(40000)}`,
    `see https://example.org/square${"]".repeat(40000)}`,
    `see https://example.com/(both)${"(".repeat(100)}${")".repeat(39900)}`,
    "https:///".repeat(4444),
    "[a](".repeat(10000),
    "[a](https://example.com/(<".repeat(1500),
  ];
  const rows = await showDiscord(page, contents.map((content, i) => message({ id: String(9100 + i), content })));
  const started = performance.now();
  await showLinks(page);
  const took = performance.now() - started;
  // Still read right, at the edge of each: the closers with no partner go, the ones with one stay.
  assert.deepStrictEqual(linkPairs(rows[0]).map(([, href]) => href), ["https://example.com/paren"]);
  assert.deepStrictEqual(linkPairs(rows[1]).map(([, href]) => href), ["https://example.org/square"]);
  assert.deepStrictEqual(linkPairs(rows[2]).map(([, href]) => href),
    [`https://example.com/(both)${"(".repeat(100)}${")".repeat(100)}`]);
  assert.deepStrictEqual(linkPairs(rows[3]), [], "an address with no host became a link");
  assert.deepStrictEqual(linkPairs(rows[4]), [], "a bracket with no address became a link");
  assert.deepStrictEqual(linkPairs(rows[5]).map(([, href]) => href), ["https://example.com/("]);
  // Generous, because this host may be loaded: all six take tens of milliseconds here, and the
  // first one alone took fifteen seconds.
  assert.ok(took < 1500, `reading the links out of six 40,000-character messages took ${Math.round(took)}ms`);
});

test("the Links view shows only the rows that hold a link, each with ONLY its links, one per line", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [
    message({ id: "8300000000000000001", author: "alice", author_is_bot: false, content: "no links here at all" }),
    message({ id: "8300000000000000002", content: "the build log is at https://example.com/log/7 and the diff at [the diff](https://example.org/d/7). Thoughts?" }),
    message({ id: "8300000000000000003", content: "another message without a link" }),
    message({ id: "8300000000000000004", content: "<https://example.com/dash|dashboard>" }),
  ]);
  assert.equal(page.el("search-count").textContent, "", "a count stands with no filter on");
  await showLinks(page);
  assert.deepStrictEqual(unfiltered(page, "discord-log").map((li) => li.getAttribute("data-id")),
    ["8300000000000000002", "8300000000000000004"], "the rows without a link are still shown");
  assert.equal(page.el("search-count").textContent, "2 of 4 loaded", "the count does not say what the filter left");
  const row = rows[1];
  assert.equal(row.getAttribute("data-links-view"), "true", "the row does not say it is showing its links");
  // ONLY the links: the list of them stands right after the text, which the stylesheet hides, and
  // holds the links and nothing else, one element per link.
  const list = rowLinkList(row);
  assert.ok(list, "the row grew no list of links");
  const kids = row.children.map((node) => node.className.split(" ")[0]);
  assert.equal(kids[kids.indexOf("body") + 1], "row-links", "the links are not where the text was");
  assert.deepStrictEqual(list.children.map((node) => node.className), ["row-link", "row-link"]);
  assert.deepStrictEqual(linkPairs(row), [["https://example.com/log/7", "https://example.com/log/7"],
    ["the diff", "https://example.org/d/7"]]);
  assert.equal(list.text(), "https://example.com/log/7the diff", "the list holds more than the links");
  for (const selector of ['.messages li[data-links-view="true"] > .body', '.messages li[data-links-view="true"] > .summary',
    '.messages li[data-links-view="true"] .fold']) {
    assert.match(cssBlock(selector), /display:\s*none !important/, `${selector} is still drawn beside the links`);
  }
  // One per line, and an address too long for the screen is cut short rather than wrapped or run
  // off the side; the href is whole regardless.
  assert.match(cssBlock(".row-links"), /flex-direction:\s*column/);
  const link = cssBlock(".row-link");
  assert.match(link, /display:\s*block/);
  assert.match(link, /text-overflow:\s*ellipsis/);
  assert.match(link, /white-space:\s*nowrap/);
  assert.match(link, /min-height:\s*2\.75rem/, "a link is not a target a thumb can hit on its own");
  // The row keeps who wrote it and when, so the reader knows where each link came from.
  const meta = row.children.find((node) => node.className === "meta");
  assert.ok(meta && meta.text().includes("ci-bot") && /\d{1,2}:\d{2}/.test(meta.text()), "the row lost its author or its time");

  // Off, every row is back as it was, with no links left in it.
  await page.el("links-filter").click();
  await page.settle();
  assert.equal(unfiltered(page, "discord-log").length, 4);
  for (const li of rows) {
    assert.equal(li.getAttribute("data-links-view"), null, "a row still says it is showing its links");
    assert.equal(rowLinkList(li), undefined, "a row kept its list of links after the filter went");
  }
  assert.equal(page.el("search-count").textContent, "");
});

test("a combined row shows the links of every message in it, once each", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [
    message({ id: "8400000000000000001", content: "first half, see https://example.com/one and", timestamp: "2026-08-19T04:31:00.000Z" }),
    message({ id: "8400000000000000002", content: "the second half: [two](https://example.com/two), https://example.com/one", timestamp: "2026-08-19T04:31:01.000Z" }),
  ]);
  assert.equal(rows.length, 1, "the fixture did not produce one combined row");
  await showLinks(page);
  assert.deepStrictEqual(linkPairs(rows[0]), [["https://example.com/one", "https://example.com/one"],
    ["two", "https://example.com/two"]], "a combined row did not show both halves' links, once each");
  // `#262 search-load-older`. Counted in messages: the one row holds two.
  assert.equal(page.el("search-count").textContent, "2 of 2 loaded");
});

test("Links is ANDed with the search text and with Pinned, and the count speaks for all of them", async () => {
  const page = newPage();
  const messages = [
    message({ id: "8500000000000000001", content: "deploy notes: https://example.com/deploy" }),
    message({ id: "8500000000000000002", content: "lunch menu https://example.org/menu" }),
    message({ id: "8500000000000000003", content: "deploy finished, no link" }),
    message({ id: "8500000000000000004", content: "an old pinned link https://example.com/pinned" }),
  ];
  page.pinned.set(messages[3].id, storedPin(messages[3], 1));
  page.pinned.set(messages[2].id, storedPin(messages[2], 2));
  page.servePinsRevision = true;
  page.pinsRevision = 2;
  await signIn(page);
  await showDiscord(page, messages);
  await showLinks(page);
  assert.equal(page.el("search-count").textContent, "3 of 4 loaded");
  // The text narrows the rows with links to the ones about the deploy — matched against the
  // message, not against its address.
  await page.el("search-field").setValue("deploy");
  assert.deepStrictEqual(linkRowTexts(page), ["deploy notes: https://example.com/deploy"],
    "the search text and Links are not both required");
  assert.equal(page.el("search-count").textContent, "1 of 4 loaded");
  await page.el("search-field").setValue("finished");
  assert.deepStrictEqual(linkRowTexts(page), [], "a row with no link matched the search and was shown");
  assert.equal(page.el("search-empty").hidden, false);
  assert.equal(page.el("search-empty").textContent, "None of the loaded messages with a link match that search.");
  await page.el("search-field").setValue("");

  // Pinned and Links: the pins that hold a link.
  await page.el("pinned-filter").click();
  await page.settle();
  await page.settle();
  assert.deepStrictEqual(linkRowTexts(page, "pinned-log"), ["an old pinned link https://example.com/pinned"],
    "Pinned and Links are not both required");
  assert.equal(page.el("search-count").textContent, "1 of 2 pinned");
  assert.deepStrictEqual(linkPairs(unfiltered(page, "pinned-log")[0]),
    [["https://example.com/pinned", "https://example.com/pinned"]]);
  // ...and when none of the pins holds one, it says that, not that there are no pins.
  await tapPin(page, page.el("pinned-log").children.find((li) => li.getAttribute("data-id") === messages[3].id));
  assert.equal(page.el("search-count").textContent, "0 of 1 pinned");
  assert.equal(page.el("search-empty").hidden, false);
  assert.equal(page.el("search-empty").textContent, "None of this channel's pinned messages has a link.");

  // Closing the bar takes both filters off and puts every row back.
  await page.el("search-toggle").click();
  await page.settle();
  assert.equal(page.el("links-filter").getAttribute("aria-pressed"), "false");
  assert.equal(page.el("pinned-filter").getAttribute("aria-pressed"), "false");
  assert.equal(unfiltered(page, "discord-log").length, 4, "a filter outlived the bar");
  assert.equal(page.el("search-empty").hidden, true);
});

test("an empty Links view says why, in words that say the list is only what is loaded", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog(3));
  await showLinks(page);
  assert.deepStrictEqual(unfiltered(page, "discord-log"), []);
  assert.equal(page.el("search-count").textContent, "0 of 3 loaded");
  assert.equal(page.el("search-empty").hidden, false, "a blank list and no word about why");
  assert.equal(page.el("search-empty").textContent, "No links in the messages loaded so far.");
  // A link arriving in a re-read is drawn as a link, and the sentence goes.
  page.messages = [...backlog(3), message({ id: "8600000000000000001", content: "new: https://example.com/new" })];
  await reReadChannel(page);
  await page.settle();
  assert.deepStrictEqual(linkRowTexts(page), ["new: https://example.com/new"], "a redrawn list came back unfiltered");
  assert.deepStrictEqual(linkPairs(unfiltered(page, "discord-log")[0]), [["https://example.com/new", "https://example.com/new"]]);
  assert.equal(page.el("search-empty").hidden, true, "the empty sentence stayed over a link");

  // Over a list that was empty to begin with, the pane says why in its own words, as it does for a search.
  const empty = newPage();
  await signIn(empty);
  await empty.settle();
  await showLinks(empty);
  assert.equal(empty.el("search-empty").hidden, true, "the filter took the blame for an empty page");
  assert.equal(empty.el("empty-state").hidden, false);
});

test("over the call view Links reads the turns, and a turn arriving under it arrives filtered", async () => {
  const page = newPage();
  await signIn(page);
  await startTalking(page);
  assistantSays(page, "the runbook is at https://example.com/runbook.");
  assistantSays(page, "nothing to click in this one");
  await page.settle();
  await showLinks(page);
  assert.deepStrictEqual(unfiltered(page, "transcript").map((li) => linkPairs(li)),
    [[["https://example.com/runbook", "https://example.com/runbook"]]]);
  assert.equal(page.el("search-count").textContent, "1 of 2 loaded");
  assistantSays(page, "and the dashboard: [dashboard](https://example.org/dash)");
  assistantSays(page, "still nothing to click");
  await page.settle();
  assert.deepStrictEqual(unfiltered(page, "transcript").map((li) => linkPairs(li)), [
    [["https://example.com/runbook", "https://example.com/runbook"]],
    [["dashboard", "https://example.org/dash"]],
  ], "a turn arrived unfiltered, or without its link drawn");
});

test("a tap on a link opens it and does nothing else: no fold, no read aloud, no hold", async () => {
  const page = newPage();
  await signIn(page);
  const long = `${"words ".repeat(80)}https://example.com/long-read`;
  const rows = await inReadingMode(page, [
    message({ id: "8700000000000000001", content: long }),
    message({ id: "8700000000000000002", content: "short, with https://example.com/short" }),
  ]);
  await showLinks(page);
  const li = rows[0];
  const anchor = linksOf(li)[0];
  assert.ok(anchor, "the fixture drew no link to tap");
  const folded = li.getAttribute("data-collapsed");
  // The browser hands the row the click, with the link as its target.
  await li.dispatch("click", { target: anchor });
  await page.settle();
  assert.deepEqual(readAloudMessages(page), [], "a tap on a link started reading the message aloud");
  assert.equal(li.getAttribute("data-collapsed"), folded, "a tap on a link folded the row");
  await rows[1].dispatch("click", { target: linksOf(rows[1])[0] });
  await page.settle();
  assert.deepEqual(readAloudMessages(page), [], "a tap on a short row's link read it aloud");
  // A press that starts on a link is the phone's own long press; the row's details do not open.
  await li.dispatch("pointerdown", { pointerId: 1, pointerType: "touch", clientX: 10, clientY: 10, target: anchor });
  assert.equal(page.expireTimers(HOLD_MS), 0, "a press on a link armed the row's details");
  await li.dispatch("pointerup", { pointerId: 1, pointerType: "touch", clientX: 10, clientY: 10, target: anchor });
  // Elsewhere on the row a hold still shows the details, and a tap in reading mode still reads it.
  const meta = li.children.find((node) => node.className === "meta");
  await li.dispatch("pointerdown", { pointerId: 2, pointerType: "touch", clientX: 10, clientY: 10, target: meta });
  assert.equal(page.expireTimers(HOLD_MS), 1, "a press on the row's meta line no longer holds");
  await page.settle();
  await li.dispatch("click", { target: meta });
  await page.settle();
  await li.dispatch("click", { target: meta });
  await page.settle();
  assert.deepEqual(readAloudMessages(page), ["8700000000000000001"], "the row itself can no longer be read aloud");
});

test("under Links a tap on the row folds nothing, and the fold chips and summaries stand aside", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [
    message({ id: "8800000000000000001", content: `${"x".repeat(400)} https://example.com/x` }),
  ]);
  assert.equal(page.el("expand-all").hidden, false, "the fixture has nothing to expand");
  await showLinks(page);
  assert.equal(page.el("expand-all").hidden, true, "Expand all offers to open text that is not on screen");
  assert.equal(page.el("collapse-all").hidden, true);
  const li = rows[0];
  const folded = li.getAttribute("data-collapsed");
  await li.dispatch("click", { target: li.children.find((node) => node.className === "meta") });
  assert.equal(li.getAttribute("data-collapsed"), folded, "a tap folded text the row is not showing");
  await page.el("links-filter").click();
  await page.settle();
  assert.equal(page.el("expand-all").hidden, false, "the fold chips did not come back with the text");
});

test("turning Links on keeps the reader's place — the newest line, or the message at the top — and off puts them back where they were", async () => {
  const page = newPage();
  await signIn(page);
  const area = page.el("scroll-area");
  await showDiscord(page, Array.from({ length: 12 }, (_unused, i) =>
    message({ id: String(i + 1), content: i % 3 === 0 ? `${longMessage(`m${i}`)} https://example.com/${i}` : longMessage(`m${i}`) })));
  // On the newest line: Links keeps the reader on it, at the newest link, and off on the newest message.
  await page.el("search-toggle").click();
  await page.settle();
  assert.ok(atBottomOf(area), "the channel did not open at its newest line");
  await page.el("links-filter").click();
  await page.settle();
  assert.ok(atBottomOf(area), "a reader on the newest line was not kept at the newest link");
  await page.el("links-filter").click();
  await page.settle();
  assert.ok(atBottomOf(area), "a reader at the newest link was not put back on the newest line");
  // Up the history, with a message holding a link at the top: it stays where it is on the screen.
  area.scrollTop = 0;
  const top = area.scrollTop;
  const at = rowWithId(page, "1").getBoundingClientRect().top;
  await page.el("links-filter").click();
  await page.settle();
  assert.equal(rowWithId(page, "1").getBoundingClientRect().top, at, "the message at the top moved when Links came on");
  await page.el("links-filter").click();
  await page.settle();
  assert.equal(area.scrollTop, top, "turning the filter off lost the reader's place");
});

/** The kinds, in the order their buttons stand under Links. */
const KINDS = ["pr", "commit", "action", "issue", "file", "other"];
/** The buttons under Links, in the order they stand. */
const KIND_CHIPS = KINDS.map((kind) => `link-kind-${kind}`);
/** What `kindsPressed` reads with every kind on. */
const ALL_ON = KINDS.map(() => "true");
const kindsPressed = (page) => KIND_CHIPS.map((id) => page.el(id).getAttribute("aria-pressed"));
const kindsCounted = (page) => KIND_CHIPS.map((id) => page.el(`${id}-count`).textContent);
const HIDDEN_KINDS_KEY = "vibe-talk.voice.hidden-link-kinds";

/** The rows the filters let through, each as its id and the addresses it shows. */
const shownHrefs = (page, id = "discord-log") => Object.fromEntries(
  unfiltered(page, id).map((li) => [li.getAttribute("data-id"), linkPairs(li).map(([, href]) => href)]));

test("a link's kind is read from its address alone, and every address is of exactly one of the six", async () => {
  const kindOf = GLYPHS.linkKind;
  const cases = {
    pr: [
      `${REPO}/pull/41`, `${REPO}/pull/41/`, `${REPO}/pull/41/files`, `${REPO}/pull/41/checks`,
      // The pull request's own list of commits is the pull request; one commit in it is not (below).
      `${REPO}/pull/41/commits`,
      `${REPO}/pull/41#discussion_r1234567`, `${REPO}/pull/41?w=1`, `${REPO}/pull/41/files#diff-0a1b2c`,
      `${REPO}/pull/41.diff`, `${REPO}/pulls`, `${REPO}/pulls?q=is%3Aopen`,
      // The host in any case, with or without www, over http too.
      "https://www.github.com/example/project/pull/7", "https://GitHub.com/example/project/pull/7",
      "HTTP://WWW.GITHUB.COM/example/project/pull/7", "http://github.com/example/project/pull/7",
    ],
    commit: [
      `${REPO}/commit/abc1234`, `${REPO}/commit/0123456789abcdef0123456789abcdef01234567`,
      `${REPO}/commit/ABC1234DEF`, `${REPO}/commit/abc1234.patch`, `${REPO}/commit/abc1234/`,
      `${REPO}/commit/abc1234#diff-9f8e7d`, `${REPO}/commit/abc1234?diff=split`,
      `${REPO}/commits`, `${REPO}/commits/main`, `${REPO}/commits/abc1234`,
      // THE EDGE: one commit seen from its pull request is a commit, not the pull request.
      `${REPO}/pull/41/commits/abc1234def`, `${REPO}/pull/41/commits/abc1234def?diff=unified#r1`,
      `${REPO}/compare/main...feature`, `${REPO}/compare/v1.0...v1.1`,
      "https://www.github.com/example/project/commit/abc1234",
    ],
    action: [
      `${REPO}/actions`, `${REPO}/actions/`, `${REPO}/actions/runs/123456`,
      `${REPO}/actions/runs/123456/job/789`, `${REPO}/actions/workflows/ci.yml`,
      `${REPO}/actions/runs/123456?pr=41#step:4:12`,
      // A check run, and the checks of a commit's suite.
      `${REPO}/runs/987654`, `${REPO}/runs/987654?check_suite_focus=true`, `${REPO}/checks`, `${REPO}/checks?sha=abc1234`,
      "https://GITHUB.com/example/project/actions/runs/5",
    ],
    // `#266 link-filter-kinds`. An issue, a comment on one, a new one, and the lists of them.
    issue: [
      `${REPO}/issues/7`, `${REPO}/issues/7/`, `${REPO}/issues/7#issuecomment-1234567`,
      `${REPO}/issues`, `${REPO}/issues?q=is%3Aopen+label%3Abug`, `${REPO}/issues/new`,
      `${REPO}/issues/new?template=bug.md`,
      // A keyword in the query is not the path: this is the issue.
      `${REPO}/issues/7?ref=pull/41`,
      "https://www.github.com/example/project/issues/7", "HTTP://GITHUB.COM/example/project/issues/7",
    ],
    // ...and a file or a folder of the code, at a branch, a tag or a commit, as GitHub shows it and as
    // it serves it raw.
    file: [
      `${REPO}/blob/main/README.md`, `${REPO}/blob/main/src/lib.rs#L10-L20`,
      `${REPO}/blob/abc1234def/docs/design.md?plain=1`,
      // A keyword in the fragment is not the path either.
      `${REPO}/blob/main/notes.md#pull/41`,
      `${REPO}/tree/main`, `${REPO}/tree/main/docs`, `${REPO}/tree/v1.0/`,
      `${REPO}/raw/main/data.csv`, `${REPO}/blame/main/src/lib.rs#L4`,
      "https://www.github.com/example/project/blob/main/README.md",
      "https://raw.githubusercontent.com/example/project/main/README.md",
      "https://RAW.githubusercontent.com/example/project/abc1234/src/lib.rs",
    ],
    // THE CATCH-ALL: every address no kind above claims. These are the documents.
    other: [
      // GitHub, but none of the five.
      `${REPO}/wiki/Home`, `${REPO}/releases/tag/v1.0`, `${REPO}/releases`, `${REPO}/discussions/3`,
      `${REPO}/labels`, REPO, `${REPO}/`,
      "https://github.com/example", "https://github.com/", "https://github.com",
      // A keyword where an owner or a repository stands is that owner or repository.
      "https://github.com/pulls", "https://github.com/issues", "https://github.com/actions/checkout",
      "https://github.com/example/actions", "https://github.com/example/issues",
      // Not what the path says it is: too short, not hex, no number, no commit, no ref, or a capital.
      `${REPO}/commit/abc12`, `${REPO}/commit/xyz1234`, `${REPO}/commit`, `${REPO}/pull/new/feature`,
      `${REPO}/pull`, `${REPO}/runs/latest`, `${REPO}/Pull/41`, `${REPO}/Issues/7`, `${REPO}/blob`,
      `${REPO}/tree/`, `${REPO}/Blob/main/README.md`,
      // A raw file host's address that is not a file, and a host that merely begins with it.
      "https://raw.githubusercontent.com/example/project", "https://raw.githubusercontent.com/",
      "https://raw.githubusercontent.com.example.com/example/project/main/README.md",
      // Other hosts, however much they look like it.
      "https://example.com/example/project/pull/41", "https://github.com.example.com/example/project/pull/41",
      "https://notgithub.com/example/project/pull/41", "https://api.github.com/repos/example/project/pulls/41",
      "https://docs.github.com/en/actions", "https://pages.example.com/project/",
      "https://gitlab.example.com/example/project/-/merge_requests/4",
      "https://gitlab.example.com/example/project/-/issues/4",
      `https://example.com/redirect?to=${REPO}/pull/41`, "https://example.com/docs/design.html",
      // And what is not an address at all still has a kind: nothing is left out of the six.
      "", "not an address",
    ],
  };
  const kinds = Array.from(newPage().pure("LINK_KINDS"), ({ kind }) => kind);
  assert.deepStrictEqual(kinds, KINDS, "the kinds are not the six, in the order the owner named them");
  assert.deepStrictEqual(Object.keys(cases), KINDS);
  // Each address is listed once, so none is expected of two kinds...
  const listed = Object.values(cases).flat();
  assert.equal(new Set(listed).size, listed.length, "an address is listed under two kinds");
  // ...and each is read as exactly the kind it is listed under: one of the six, never none.
  for (const [kind, addresses] of Object.entries(cases)) {
    assert.ok(addresses.length >= 8, `${kind} is tried on too few forms`);
    for (const address of addresses) {
      const read = kindOf(address);
      assert.ok(kinds.includes(read), `${address} is of no kind: ${read}`);
      assert.equal(read, kind, `${address} is not read as ${kind}`);
    }
  }

  // THE CONTROL: with the pull request's commits read as the pull request, this tells them apart.
  // The classifier is a module (`#624 voice-page-modularity`): its controls patch its source.
  const LINKS = "src/foundation/links.ts";
  const control = (await moduleForTests("src/foundation/index.ts", { overlays: [{ file: LINKS,
    find: '      return second === "commits" && COMMIT_ID.test(third) ? "commit" : "pr";', replace: '      return "pr";' }] })).linkKind;
  assert.equal(control(`${REPO}/pull/41/commits/abc1234def`), "pr", "the test cannot see the pull request's commit");
  // ...and with the catch-all taken away, an address no kind claims is of none, which this sees.
  const unclaimed = (await moduleForTests("src/foundation/index.ts", { overlays: [{ file: LINKS,
    find: '  if (!found) return "other";', replace: '  if (!found) return null;' }] })).linkKind;
  assert.equal(unclaimed("https://example.com/docs/design.html"), null, "the test cannot see a link left out of every kind");
});

test("PRs, Commits, Actions, Issues, Files and Other come up under Links, all on, only while it is on, and go with it", async () => {
  // Markup: a group of six toggles in the bar, after the glass, so it stands between none of the
  // filters on the bar's line; web/voice.css hangs it under Links.
  assertMarkupContains("search-float", "link-kinds");
  const bar = HTML_CODE.slice(HTML_CODE.indexOf('id="search-float"'));
  const at = (id) => bar.indexOf(`id="${id}"`);
  assert.ok(at("search-toggle") < at("link-kinds"), "the kinds stand between the filters on the bar");
  const group = bar.slice(at("link-kinds"), bar.indexOf("</div>", at("link-kinds")));
  assert.match(group, /role="group"/);
  // Other is of every host, so the group is not only GitHub's. `#266 link-filter-kinds`.
  assert.match(group, /aria-label="Kinds of link to show"/);
  assert.match(group, /hidden>/, "the kinds are served on screen");
  // `#620 link-url-chip`: URL before them, which is no kind (its own test, below).
  const [url, ...buttons] = [...group.matchAll(/<button ([^>]*)>([^<]*)</g)];
  assert.equal(url[2], "URL");
  assert.deepStrictEqual(buttons.map((b) => b[2]), ["PRs", "Commits", "Actions", "Issues", "Files", "Other"],
    "the kinds are not named as the owner named them, in his order");
  assert.deepStrictEqual(buttons.map((b) => /id="([^"]+)"/.exec(b[1])[1]), KIND_CHIPS);
  for (const [, attributes] of buttons) {
    assert.match(attributes, /class="link-kind"/);
    assert.match(attributes, /type="button"/);
    assert.match(attributes, /aria-pressed="true"/, "a kind is served off");
    assert.match(attributes, /title="Show links to /);
  }
  // Each a whole 44px-tall target at least as wide, hung below the bar and under Links — over the
  // call view too, where Pinned is not on the bar — and on and off drawn differently, off struck through.
  const kind = cssBlock(".link-kind");
  assert.match(kind, /min-width: var\(--toggle-hit\)/);
  assert.match(kind, /height: var\(--toggle-hit\)/);
  const row = cssBlock("#link-kinds");
  assert.match(row, /position: absolute/);
  assert.match(row, /top: calc\(100% \+ 1px \+ var\(--kinds-gap\)\)/, "the kinds are not below the bar");
  assert.match(row, /right: var\(--links-mid\)/);
  assert.match(row, /transform: translateX\(min\(50%, var\(--links-mid\)\)\)/, "the kinds are not centred under Links, or not kept to the bar's end");
  assert.match(row, /--links-mid: calc\(2px \+ var\(--search-disc\) \+ var\(--glass-clear\) \+ var\(--bar-gap\) \+ 1\.5 \* var\(--toggle-hit\)\)/);
  // `#266 link-filter-kinds`. Six are wider than a phone: never wider than the bar, the row scrolls
  // sideways inside it, no button squeezed narrower than its words, and with no scrollbar to make it
  // taller than the room the list leaves it. A focus ring is drawn inside the button the row clips.
  assert.match(row, /max-width: 100%/, "the kinds can be wider than the bar, and so than a phone");
  assert.match(row, /overflow-x: auto/, "the kinds wider than the bar cannot be scrolled to");
  assert.match(row, /scrollbar-width: none/);
  assert.match(cssBlock("#link-kinds::-webkit-scrollbar"), /display: none/);
  assert.match(kind, /flex: 0 0 auto/, "a kind can be squeezed narrower than its words");
  assert.match(cssBlock(".link-kind:focus-visible"), /outline-offset: -2px/, "the focus ring is drawn where the row clips it");
  assert.match(cssBlock("#pinned-filter[hidden] ~ #link-kinds"),
    /--links-mid: calc\(2px \+ var\(--search-disc\) \+ var\(--glass-clear\) \+ var\(--bar-gap\) \+ 0\.5 \* var\(--toggle-hit\)\)/,
    "over the call view the kinds are placed as if Pinned were beside Links");
  assert.match(cssBlock("#screen-main[data-searching] #search-float"), /padding: 3px 2px 3px 3px;/,
    "the bar's right padding is not the 2px --links-mid counts from");
  assert.match(cssBlock('.link-kind[aria-pressed="true"]::before'), /border-color: var\(--accent\)/);
  assert.match(cssBlock('.link-kind[aria-pressed="false"]'), /text-decoration: line-through/, "off is told apart by colour alone");
  // ...and the list STARTS below them, its top edge at their foot and the room at its head given back,
  // so that no row, scrolled, is drawn under them: `#620 link-url-chip`, where the owner saw a line's
  // letters between the pills when the list only made room at its head.
  const below = cssBlock("#screen-main[data-searching][data-link-kinds] #scroll-area");
  assert.match(below, /--hang-foot: calc\(var\(--float-line\) \+ var\(--search-bar\) \/ 2 \+ var\(--kinds-gap\) \+ var\(--toggle-hit\)\)/,
    "the list's top is not the kinds' foot");
  assert.match(below, /margin-top: var\(--hang-foot\)/, "the list does not start below the kinds, so rows scroll under them");
  assert.match(below, /padding-top: 0\.35rem/, "the room at the list's head is not given back, so every row moves");
  assert.match(below, /mask-image: linear-gradient\(to bottom, transparent 0, #000 0\.35rem\)/,
    "the fade under the header reaches into the first row");

  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog(2));
  const kinds = page.el("link-kinds");
  const room = () => page.el("screen-main").hasAttribute("data-link-kinds");
  assert.equal(kinds.hidden, true);
  await page.el("search-toggle").click();
  assert.equal(kinds.hidden, true, "the kinds are up before Links is on");
  await page.el("links-filter").click();
  await page.settle();
  assert.equal(kinds.hidden, false, "Links came on without its kinds");
  assert.equal(room(), true, "the list made no room for the kinds");
  assert.deepStrictEqual(kindsPressed(page), ALL_ON, "with nothing kept, a kind starts off");
  // Off with Links.
  await page.el("links-filter").click();
  await page.settle();
  assert.equal(kinds.hidden, true, "the kinds outlived Links");
  assert.equal(room(), false, "the list kept room for kinds that are gone");
  // Over the call view, with Links.
  await page.el("links-filter").click();
  await page.el("view-switch").click();
  await page.settle();
  assert.equal(kinds.hidden, false, "the kinds are not over the call view, where Links is");
  await page.el("view-switch").click();
  await page.settle();
  // Off with the bar, by the glass and by Escape.
  await page.el("search-toggle").click();
  await page.settle();
  assert.equal(kinds.hidden, true, "the kinds outlived the bar");
  assert.equal(room(), false);
  await showLinks(page);
  assert.equal(kinds.hidden, false);
  await page.el("search-field").dispatch("keydown", { key: "Escape" });
  await page.settle();
  assert.equal(kinds.hidden, true, "Escape left the kinds up");
  // And not on a screen that has no bar: Settings, with Links on behind it.
  await showLinks(page);
  await page.el("open-settings").click();
  assert.equal(kinds.hidden, true, "the kinds float over Settings");
  await page.el("close-settings").click();
  assert.equal(kinds.hidden, page.el("links-filter").hidden, "the kinds and Links disagree on the way back");
});

test("a kind turned off takes its links out of each row, and a row with none left goes", async () => {
  const contents = [
    `review please: ${REPO}/pull/41 and the design note https://example.com/docs/design.html`,
    `CI failed at ${REPO}/actions/runs/123456/job/789 on [abc1234](${REPO}/commit/abc1234def)`,
    "the guide: https://example.org/guide",
    "nothing to open here",
    // Named in Markdown, read by its ADDRESS: "the fix" is a pull request, "PR 41" a document.
    `[the fix](${REPO}/pull/42/files) and [PR 41](https://example.com/notes/pr-41)`,
    `filed as ${REPO}/issues/7`,
  ];
  const run = async (script) => {
    const page = newPage(new Map(), script);
    await signIn(page);
    await showDiscord(page, contents.map((content, i) => message({ id: `86000000000000000${i}`, content })));
    await showLinks(page);
    return page;
  };
  const page = await run(SCRIPT);
  const all = {
    "860000000000000000": [`${REPO}/pull/41`, "https://example.com/docs/design.html"],
    "860000000000000001": [`${REPO}/actions/runs/123456/job/789`, `${REPO}/commit/abc1234def`],
    "860000000000000002": ["https://example.org/guide"],
    "860000000000000004": [`${REPO}/pull/42/files`, "https://example.com/notes/pr-41"],
    "860000000000000005": [`${REPO}/issues/7`],
  };
  assert.deepStrictEqual(shownHrefs(page), all, "with every kind on, a link is missing");
  assert.equal(page.el("search-count").textContent, "5 of 6 loaded");
  // Two pull requests, a commit, a run, the issue, no file, and three documents: Other's.
  assert.deepStrictEqual(kindsCounted(page), ["2", "1", "1", "1", "", "3"], "a kind's button does not say how many links it holds");

  // PRs off: out of both rows that hold one, and each row keeps the rest — named as it was.
  await tapKind(page, "pr");
  assert.deepStrictEqual(kindsPressed(page), ["false", "true", "true", "true", "true", "true"]);
  assert.deepStrictEqual(shownHrefs(page), {
    ...all,
    "860000000000000000": ["https://example.com/docs/design.html"],
    "860000000000000004": ["https://example.com/notes/pr-41"],
  }, "PRs off did not take the pull requests, and only them, out of their rows");
  const named = unfiltered(page, "discord-log").find((li) => li.getAttribute("data-id") === "860000000000000004");
  assert.deepStrictEqual(linkPairs(named), [["PR 41", "https://example.com/notes/pr-41"]],
    "a link was hidden for its NAME, or lost it");
  assert.equal(page.el("search-count").textContent, "5 of 6 loaded", "a row with links left went");
  // The number on a button is what it holds whether it is on or off, so a reader can see what
  // turning it back on would bring.
  assert.deepStrictEqual(kindsCounted(page), ["2", "1", "1", "1", "", "3"]);

  // Actions off too: the row of a run and a commit keeps the commit.
  await tapKind(page, "action");
  assert.deepStrictEqual(shownHrefs(page)["860000000000000001"], [`${REPO}/commit/abc1234def`]);
  // Commits off as well: that row has nothing left, and goes; the documents and the issue stay.
  await tapKind(page, "commit");
  assert.deepStrictEqual(shownHrefs(page), {
    "860000000000000000": ["https://example.com/docs/design.html"],
    "860000000000000002": ["https://example.org/guide"],
    "860000000000000004": ["https://example.com/notes/pr-41"],
    "860000000000000005": [`${REPO}/issues/7`],
  }, "a row whose links are all hidden is still shown, or a document went with them");
  assert.equal(page.el("search-count").textContent, "4 of 6 loaded", "the count speaks for rows the kinds took away");
  assert.equal(page.el("search-empty").hidden, true);
  const gone = page.el("discord-log").children.find((li) => li.getAttribute("data-id") === "860000000000000001");
  assert.equal(gone.hasClass("search-hidden"), true);
  assert.equal(gone.getAttribute("data-links-view"), null, "a row with nothing to show still says it shows its links");

  // Every kind back on: every link is back, where it was.
  for (const kind of ["pr", "commit", "action"]) await tapKind(page, kind);
  assert.deepStrictEqual(shownHrefs(page), all, "turning the kinds back on did not bring every link back");
  // ...and Links off gives every row its text back, whatever the kinds were.
  await tapKind(page, "pr");
  await page.el("links-filter").click();
  await page.settle();
  assert.equal(unfiltered(page, "discord-log").length, 6);
  assert.ok(page.el("discord-log").children.every((li) => rowLinkList(li) === undefined), "a row kept its links");

  // THE CONTROL: a list drawn without asking which kinds are hidden fails the first step above.
  const control = await run(brokenScript(
    "    const shown = drawn.links.filter((link) => !hiddenLinkKinds.has(link.kind));", "    const shown = drawn.links;"));
  await tapKind(control, "pr");
  assert.deepStrictEqual(shownHrefs(control)["860000000000000000"], [`${REPO}/pull/41`, "https://example.com/docs/design.html"],
    "the test cannot tell a hidden pull request from a shown one");
});

test("Issues, Files and Other take every link the first three do not: the six numbers add up to every link, and each kind alone shows its own", async () => {
  // `#266 link-filter-kinds`. The owner: "together that should be all of them", and "Other is the
  // catch all". Every link of this channel by the kind it is, and the messages that hold them.
  const byKind = {
    pr: [`${REPO}/pull/41`, `${REPO}/pull/42/files`],
    commit: [`${REPO}/commit/abc1234def`, `${REPO}/compare/main...feature`],
    action: [`${REPO}/actions/runs/123456`],
    issue: [`${REPO}/issues/7`, `${REPO}/issues/8#issuecomment-99`, `${REPO}/issues/new`],
    file: [`${REPO}/blob/main/README.md`, `${REPO}/tree/main/docs`, "https://raw.githubusercontent.com/example/project/main/data.csv"],
    other: ["https://example.com/docs/design.html", `${REPO}/wiki/Home`, `${REPO}/releases/tag/v1.0`, "https://example.org/guide"],
  };
  const contents = [
    `review ${REPO}/pull/41 against the design https://example.com/docs/design.html`,
    `filed ${REPO}/issues/7 and ${REPO}/issues/8#issuecomment-99 and see [the readme](${REPO}/blob/main/README.md)`,
    `landed [abc1234](${REPO}/commit/abc1234def) in ${REPO}/compare/main...feature by ${REPO}/actions/runs/123456 today`,
    `the docs are in ${REPO}/tree/main/docs and the data at https://raw.githubusercontent.com/example/project/main/data.csv now`,
    `notes in ${REPO}/wiki/Home and ${REPO}/releases/tag/v1.0 and https://example.org/guide today`,
    `open one at ${REPO}/issues/new once [the fix](${REPO}/pull/42/files) lands`,
    "nothing to open here",
  ];
  const run = async (script) => {
    const page = newPage(new Map(), script);
    await signIn(page);
    await showDiscord(page, contents.map((content, i) => message({ id: `86100000000000000${i}`, content })));
    await showLinks(page);
    return page;
  };
  const sorted = (list) => [...list].sort();
  const shownLinks = (page) => Object.values(shownHrefs(page)).flat();
  const added = (counts) => counts.reduce((sum, count) => sum + Number(count || 0), 0);
  /** Turn on exactly the kinds in `on`, by taps on the buttons that differ. */
  const showOnly = async (page, on) => {
    for (const kind of KINDS) {
      if ((page.el(`link-kind-${kind}`).getAttribute("aria-pressed") === "true") !== on.includes(kind)) await tapKind(page, kind);
    }
  };

  const page = await run(SCRIPT);
  const every = Object.values(byKind).flat();
  assert.deepStrictEqual(sorted(shownLinks(page)), sorted(every), "with every kind on, a link is missing or doubled");
  // Each kind's number is its own links, and together they are every link on screen.
  assert.deepStrictEqual(kindsCounted(page), KINDS.map((kind) => String(byKind[kind].length)));
  assert.equal(added(kindsCounted(page)), shownLinks(page).length, "the kinds' numbers do not add up to the links shown");
  assert.equal(page.el("search-count").textContent, "6 of 7 loaded");

  // Each kind alone shows exactly its own links, and the six between them show every link once.
  const seen = [];
  for (const kind of KINDS) {
    await showOnly(page, [kind]);
    assert.deepStrictEqual(sorted(shownLinks(page)), sorted(byKind[kind]), `${kind} alone does not show its own links, and only them`);
    seen.push(...shownLinks(page));
    // The numbers are what each kind holds whether it is on or off.
    assert.deepStrictEqual(kindsCounted(page), KINDS.map((one) => String(byKind[one].length)));
  }
  assert.deepStrictEqual(sorted(seen), sorted(every), "a link is in two kinds, or in none");

  // OTHER IS THE CATCH-ALL: with the other five off, what is left is the documents, every one.
  await showOnly(page, ["other"]);
  assert.deepStrictEqual(shownHrefs(page), {
    "861000000000000000": ["https://example.com/docs/design.html"],
    "861000000000000004": [`${REPO}/wiki/Home`, `${REPO}/releases/tag/v1.0`, "https://example.org/guide"],
  }, "Other does not show the leftovers");
  assert.equal(page.el("search-count").textContent, "2 of 7 loaded");
  // Every kind off: nothing left, and the sentence names all six — Other as a plural like the rest.
  await tapKind(page, "other");
  assert.equal(page.el("search-count").textContent, "0 of 7 loaded");
  assert.equal(page.el("search-empty").textContent,
    "No links left once PRs, Commits, Actions, Issues, Files and Other links are hidden — turn them back on under the Links button.");
  // Every kind but Other: the row of documents alone goes, and a row with a document and more keeps the rest.
  await showOnly(page, ["pr", "commit", "action", "issue", "file"]);
  assert.equal(page.el("search-empty").hidden, true);
  assert.equal(page.el("search-count").textContent, "5 of 7 loaded");
  assert.deepStrictEqual(shownHrefs(page)["861000000000000000"], [`${REPO}/pull/41`]);

  // A kind with no link in view: its button is a plain word, turning it off takes nothing away, and
  // an emptied list does not name it.
  const sparse = newPage();
  await signIn(sparse);
  await showDiscord(sparse, [
    message({ id: "8610000000000000101", content: `review ${REPO}/pull/41` }),
    message({ id: "8610000000000000102", content: "the design https://example.com/docs/design.html" }),
  ]);
  await showLinks(sparse);
  assert.deepStrictEqual(kindsCounted(sparse), ["1", "", "", "", "", "1"]);
  const before = shownHrefs(sparse);
  await tapKind(sparse, "file");
  assert.equal(sparse.el("link-kind-file").getAttribute("aria-pressed"), "false");
  assert.deepStrictEqual(shownHrefs(sparse), before, "a kind with no link took something away");
  assert.equal(sparse.el("search-count").textContent, "2 of 2 loaded");
  await tapKind(sparse, "pr");
  await tapKind(sparse, "other");
  assert.equal(sparse.el("search-empty").textContent,
    "No links left once PRs and Other links are hidden — turn them back on under the Links button.",
    "the sentence named a kind that hid nothing, or not the ones that did");
  // Other back on: the leftover is back, and only it.
  await tapKind(sparse, "other");
  assert.deepStrictEqual(shownHrefs(sparse), { "8610000000000000102": ["https://example.com/docs/design.html"] });
  assert.equal(sparse.el("search-empty").hidden, true);

  // THE CONTROL: with the catch-all gone, a link on another host is of no kind — no number counts it,
  // and no button hides it — which this test sees.
  const control = await run(await brokenModule([
    { file: "src/foundation/links.ts", find: '  if (!found) return "other";', replace: "  if (!found) return null;" }]));
  assert.notEqual(added(kindsCounted(control)), shownLinks(control).length, "the test cannot see a link no kind counts");
  await showOnly(control, []);
  assert.notEqual(control.el("search-count").textContent, "0 of 7 loaded", "the test cannot see a link no button hides");
});

test("with every link of a hidden kind, the list says which kinds hid them and how to bring them back", async () => {
  const page = newPage();
  await signIn(page);
  const loaded = [
    message({ id: "8700000000000000101", content: `deploy: ${REPO}/pull/41` }),
    message({ id: "8700000000000000102", content: `deploy: ${REPO}/commit/abc1234` }),
    message({ id: "8700000000000000103", content: `lunch: ${REPO}/pull/43` }),
    message({ id: "8700000000000000104", content: "no link" }),
  ];
  await showDiscord(page, loaded);
  await showLinks(page);
  await tapKind(page, "pr");
  assert.equal(page.el("search-count").textContent, "1 of 4 loaded");
  assert.equal(page.el("search-empty").hidden, true);
  // Text that matches only rows whose links are hidden: the search found them, the kinds hid them.
  await page.el("search-field").setValue("lunch");
  assert.equal(page.el("search-count").textContent, "0 of 4 loaded");
  assert.equal(page.el("search-empty").hidden, false, "a blank list and no word about why");
  assert.equal(page.el("search-empty").textContent, "None of the loaded messages with a link match that search once PRs are hidden.");
  // The numbers on the buttons follow the text: only what it matched.
  assert.deepStrictEqual(kindsCounted(page), ["1", "", "", "", "", ""], "the buttons count links the text did not match");
  // Text that matches no row with a link at all says so, as it did before the kinds.
  await page.el("search-field").setValue("no link");
  assert.equal(page.el("search-empty").textContent, "None of the loaded messages with a link match that search.");
  await page.el("search-field").setValue("");
  assert.deepStrictEqual(kindsCounted(page), ["2", "1", "", "", "", ""]);

  // Every link's kind off: no links left, which kinds did it, and where the way back is.
  await tapKind(page, "commit");
  assert.equal(page.el("search-count").textContent, "0 of 4 loaded");
  assert.equal(page.el("search-empty").hidden, false);
  const twoKinds = "No links left once PRs and Commits are hidden — turn them back on under the Links button.";
  assert.equal(page.el("search-empty").textContent, twoKinds);
  // Actions off as well took nothing away — none is loaded — so the sentence does not blame them.
  await tapKind(page, "action");
  assert.equal(page.el("search-empty").textContent, twoKinds, "a kind that hid nothing was named");
  // A run arriving in a re-read arrives hidden, and now all three did it.
  page.messages = [...loaded, message({ id: "8700000000000000105", content: `a run: ${REPO}/actions/runs/5` })];
  await reReadChannel(page);
  await page.settle();
  assert.equal(page.el("search-count").textContent, "0 of 5 loaded", "a run arrived past a hidden kind");
  assert.equal(page.el("search-empty").textContent,
    "No links left once PRs, Commits and Actions are hidden — turn them back on under the Links button.");
  // One back on, and the rows it holds are back, the sentence gone.
  await tapKind(page, "pr");
  assert.deepStrictEqual(Object.keys(shownHrefs(page)), ["8700000000000000101", "8700000000000000103"]);
  assert.equal(page.el("search-empty").hidden, true, "the empty sentence stayed over links");
  // With no link loaded at all, it is still that, and not the kinds.
  const bare = newPage();
  await signIn(bare);
  await showDiscord(bare, backlog(2));
  await showLinks(bare);
  await tapKind(bare, "pr");
  assert.equal(bare.el("search-empty").textContent, "No links in the messages loaded so far.");
  // A message's links past what a button prints say so with a plus.
  const busy = newPage();
  await signIn(busy);
  await showDiscord(busy, [message({ content: Array.from({ length: 120 }, (_unused, n) => `${REPO}/pull/${n + 1}`).join(" ") })]);
  await showLinks(busy);
  assert.deepStrictEqual(kindsCounted(busy), [`${sourceConstant("LINK_KIND_COUNT_MAX")}+`, "", "", "", "", ""]);
});

test("the kinds are ANDed with the text and with Pinned, and over Pinned the empty list names them too", async () => {
  const page = newPage();
  const messages = [
    message({ id: "8800000000000000201", content: `deploy notes https://example.com/deploy and ${REPO}/pull/41` }),
    message({ id: "8800000000000000202", content: `deploy run ${REPO}/actions/runs/9` }),
    message({ id: "8800000000000000203", content: `pinned run ${REPO}/actions/runs/10` }),
    message({ id: "8800000000000000204", content: "pinned doc https://example.org/pinned" }),
  ];
  page.pinned.set(messages[2].id, storedPin(messages[2], 1));
  page.pinned.set(messages[3].id, storedPin(messages[3], 2));
  page.servePinsRevision = true;
  page.pinsRevision = 2;
  await signIn(page);
  await showDiscord(page, messages);
  await showLinks(page);
  await tapKind(page, "action");
  // Text AND Links AND the kinds: the deploy messages, less the one whose link is a run.
  await page.el("search-field").setValue("deploy");
  assert.deepStrictEqual(shownHrefs(page), { "8800000000000000201": ["https://example.com/deploy", `${REPO}/pull/41`] });
  assert.equal(page.el("search-count").textContent, "1 of 4 loaded");
  await page.el("search-field").setValue("");

  // Pinned AND Links AND the kinds: the pins with a link left.
  await page.el("pinned-filter").click();
  await page.settle();
  await page.settle();
  assert.deepStrictEqual(shownHrefs(page, "pinned-log"), { "8800000000000000204": ["https://example.org/pinned"] },
    "Pinned, Links and the kinds are not all required");
  assert.equal(page.el("search-count").textContent, "1 of 2 pinned");
  assert.equal(page.el("link-kinds").hidden, false, "the kinds went with Pinned");
  // A pin whose link is the hidden kind: the sentence says that, not that no pin has a link.
  await tapPin(page, page.el("pinned-log").children.find((li) => li.getAttribute("data-id") === messages[3].id));
  assert.equal(page.el("search-count").textContent, "0 of 1 pinned");
  assert.equal(page.el("search-empty").hidden, false);
  assert.equal(page.el("search-empty").textContent,
    "None of this channel's pinned messages has a link left once Actions are hidden — turn them back on under the Links button.");
  await page.el("search-field").setValue("pinned");
  assert.equal(page.el("search-empty").textContent,
    "None of this channel's pinned messages has a link left once Actions are hidden — turn them back on under the Links button.");
  await tapKind(page, "action");
  assert.deepStrictEqual(shownHrefs(page, "pinned-log"), { "8800000000000000203": [`${REPO}/actions/runs/10`] });
  assert.equal(page.el("search-empty").hidden, true);
});

test("the kinds turned off are kept on this device, and with nothing kept every kind is on", async () => {
  const page = newPage();
  await signIn(page);
  const contents = [`a pull request ${REPO}/pull/41`, `a commit ${REPO}/commit/abc1234`, "a document https://example.com/doc"];
  const channel = () => contents.map((content, i) => message({ id: `890000000000000000${i}`, content }));
  await showDiscord(page, channel());
  await showLinks(page);
  assert.equal(page.storage.has(HIDDEN_KINDS_KEY), false, "a choice was kept that nobody made");
  await tapKind(page, "pr");
  await tapKind(page, "commit");
  assert.equal(page.storage.get(HIDDEN_KINDS_KEY), '["pr","commit"]', "the choice was not kept");
  await tapKind(page, "commit");
  assert.equal(page.storage.get(HIDDEN_KINDS_KEY), '["pr"]');

  // A reload: the bar is closed, as every reload leaves it; Links on again finds PRs still off.
  const again = reloadWith(page.storage, channel());
  await signIn(again);
  await reopenedChannel(again);
  assert.equal(again.el("link-kinds").hidden, true, "the kinds came back without Links");
  await showLinks(again);
  assert.deepStrictEqual(kindsPressed(again), ["false", "true", "true", "true", "true", "true"], "PRs came back on by itself");
  assert.deepStrictEqual(Object.keys(shownHrefs(again)), ["8900000000000000001", "8900000000000000002"],
    "the kept choice did not hide the pull request");

  // What a device may hold instead: damaged, of another shape, or naming a kind this page does not
  // know. Each reads as on, kind by kind, and nothing in it is trusted to hide anything else. A choice
  // kept before Issues, Files and Other existed still holds, and they start on. `#266
  // link-filter-kinds`.
  for (const [held, pressed] of [
    ["not json", ALL_ON],
    ['{"pr":true}', ALL_ON],
    ['["pr","commit","action"]', ["false", "false", "false", "true", "true", "true"]],
    ['["action","issues","wiki",7,"pr","other"]', ["false", "true", "false", "true", "true", "false"]],
  ]) {
    const store = new Map(page.storage);
    store.set(HIDDEN_KINDS_KEY, held);
    const kept = reloadWith(store, channel());
    await signIn(kept);
    await reopenedChannel(kept);
    await showLinks(kept);
    assert.deepStrictEqual(kindsPressed(kept), pressed, `a device holding ${held} reads as ${kindsPressed(kept)}`);
  }

  // THE CONTROL: with nothing written, there is nothing for the reload above to find.
  const forgetful = newPage(new Map(), brokenScript("    deviceStorage().setItem(LINK_KINDS_KEY,", "    void (LINK_KINDS_KEY,"));
  await signIn(forgetful);
  await showDiscord(forgetful, [message({ content: `${REPO}/pull/41` })]);
  await showLinks(forgetful);
  await tapKind(forgetful, "pr");
  assert.equal(forgetful.storage.has(HIDDEN_KINDS_KEY), false, "the test cannot tell a kept choice from none");
});

// `#620 link-url-chip`. The owner, under Links with Files on and every link reading "REPO:README.md":
// "When we're in URL search mode I would like to have a little chip named URL alongside the other
// filters for files and so on. When I click that it switches back to raw URLs instead of the render
// hyperlink and that would at least let me see where these files live." And: "the URL button is a
// proper filter composed with a type search term in the search bar" — the text still finds a message
// by the address in it, whichever way its links are written.

const LINK_RAW_KEY_NAME = "vibe-talk.voice.link-raw-urls";
/** The README the server labels by its folder, as src/link_labels.rs does. */
const README = `${REPO}/blob/main/vibe-talk/README.md`;

/**
 * What a link says. Under URL its text is the address in parts with a place to break between them,
 * which this fake DOM keeps as children; otherwise its own text.
 */
const linkSays = (a) => (a.children.length > 0
  ? a.children.filter((node) => node.tagName === "#text").map((node) => node.textContent).join("")
  : a.textContent);
const linkWords = (page) => unfiltered(page, "discord-log").flatMap((li) => linksOf(li).map(linkSays));

/** A channel with a labelled README, a named document, a bare address and a message with none. */
const urlChannel = () => [
  message({ id: "8910000000000000001", content: `The notes: ${README}`,
    content_html: `<p>The notes: <a href="${README}" title="${README}" target="_blank" rel="noopener noreferrer nofollow">vibe-talk/README.md</a></p>\n` }),
  message({ id: "8910000000000000002", content: "Background: [the guide](https://docs.example.com/guide)" }),
  message({ id: "8910000000000000003", content: `The fix: ${REPO}/pull/41` }),
  message({ id: "8910000000000000004", content: "No link here." }),
];
const LABELS = ["vibe-talk/README.md", "the guide", `${REPO}/pull/41`];
const ADDRESSES = [README, "https://docs.example.com/guide", `${REPO}/pull/41`];

test("URL writes each link under Links as its whole address and back as its name, and is no kind", async () => {
  // First in the row, before the kinds; off, and never struck through, because off hides nothing.
  const group = HTML_CODE.slice(HTML_CODE.indexOf('id="link-kinds"'));
  const first = /<button ([^>]*)>([^<]*)</.exec(group);
  assert.equal(first[2], "URL", "URL is not the first chip under Links");
  assert.match(first[1], /id="link-raw"/);
  assert.match(first[1], /type="button"/);
  assert.match(first[1], /aria-pressed="false"/, "URL is served on");
  assert.match(first[1], /title="Write each link as its full address"/);
  assert.match(cssBlock('#link-raw[aria-pressed="false"]'), /text-decoration: none/, "URL off is struck through, as a hidden kind is");
  // An address is wrapped, never cut short: the reader pressed URL to see all of it.
  const raw = cssBlock(".row-links[data-raw] .row-link");
  assert.match(raw, /white-space: normal/, "an address under URL is kept on one line and cut short");
  assert.match(raw, /overflow-wrap: anywhere/, "a part of an address too long for a line runs off the side");

  const page = newPage();
  await signIn(page);
  await showDiscord(page, urlChannel());
  await showLinks(page);
  const chip = page.el("link-raw");
  assert.equal(chip.getAttribute("aria-pressed"), "false");
  assert.deepStrictEqual(linkWords(page), LABELS, "off, the links are not written as their names");
  assert.equal(page.storage.has(LINK_RAW_KEY_NAME), false, "a choice was kept that nobody made");

  await chip.click();
  await page.settle();
  assert.equal(chip.getAttribute("aria-pressed"), "true");
  assert.deepStrictEqual(linkWords(page), ADDRESSES, "on, the links are not written as their addresses");
  // Still links, to the same places, the address in the title; the same rows, nothing filtered.
  assert.deepStrictEqual(unfiltered(page, "discord-log").flatMap((li) => linksOf(li).map((a) => [a.getAttribute("href"), a.getAttribute("title")])),
    ADDRESSES.map((href) => [href, href]));
  // Broken after its slashes, and only between parts of the address, so it wraps where a reader looks.
  const readme = unfiltered(page, "discord-log").flatMap(linksOf)[0];
  const parts = readme.children.filter((node) => node.tagName === "#text").map((node) => node.textContent);
  assert.deepStrictEqual(parts, ["https:/", "/", "github.com/", "example/", "project/", "blob/", "main/", "vibe-talk/", "README.md"]);
  assert.ok(readme.children.every((node, i) => (i % 2 === 0 ? node.tagName === "#text" : node.tagName === "wbr")),
    "the address is not its parts with a break between each");
  assert.equal(page.storage.get(LINK_RAW_KEY_NAME), "on", "the choice was not kept");

  // The kinds still choose which: Files off takes the README away, written either way.
  await tapKind(page, "file");
  assert.deepStrictEqual(linkWords(page), ADDRESSES.slice(1));
  await tapKind(page, "file");

  // The text finds a message by a word only its address holds — "blob" is in no label and no other
  // message — and URL only says how its link is written.
  await search(page, "blob");
  await page.settle();
  assert.deepStrictEqual(linkWords(page), [README], "the text did not find the README by its address under URL");
  await chip.click();
  await page.settle();
  assert.deepStrictEqual(linkWords(page), ["vibe-talk/README.md"], "URL off under the text changed which links are shown");
  assert.equal(page.storage.has(LINK_RAW_KEY_NAME), false, "URL off was not forgotten");
  await chip.click();
  await page.settle();

  // A reload: Links on again finds URL still on.
  const again = reloadWith(page.storage, urlChannel());
  await signIn(again);
  await reopenedChannel(again);
  await showLinks(again);
  assert.equal(again.el("link-raw").getAttribute("aria-pressed"), "true", "URL did not outlast a reload");
  assert.deepStrictEqual(linkWords(again), ADDRESSES);
  // Only the word the page writes turns it on: anything else a device holds reads as off.
  for (const held of ["true", "1", "ON", ""]) {
    const store = new Map(page.storage);
    store.set(LINK_RAW_KEY_NAME, held);
    const kept = reloadWith(store, urlChannel());
    await signIn(kept);
    await reopenedChannel(kept);
    await showLinks(kept);
    assert.deepStrictEqual(linkWords(kept), LABELS, `a device holding ${JSON.stringify(held)} writes addresses`);
  }
  // Kept with the owner's other settings, so every device writes them the same way.
  assert.ok(SETTINGS.SYNCED_SETTING_KEYS.includes(page.pure("LINK_RAW_KEY")), "URL is not one of the synced settings");
  assert.equal(page.pure("LINK_RAW_KEY"), LINK_RAW_KEY_NAME);

  // THE CONTROL: with the address never written, this sees the names stay.
  const blind = newPage(new Map(), brokenScript("    if (raw) rawAddressInto(anchor, link.href);\n    else anchor.textContent = link.name;",
    "    anchor.textContent = link.name;"));
  await signIn(blind);
  await showDiscord(blind, urlChannel());
  await showLinks(blind);
  await blind.el("link-raw").click();
  await blind.settle();
  assert.deepStrictEqual(linkWords(blind), LABELS, "the test cannot tell an address from a name");
});

test("URL turned on by another device rewrites the links under Links here", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, urlChannel());
  await showLinks(page);
  assert.deepStrictEqual(linkWords(page), LABELS);
  page.storage.set(LINK_RAW_KEY_NAME, "on");
  page.pure("applySyncedSetting")(LINK_RAW_KEY_NAME);
  await page.settle();
  assert.equal(page.el("link-raw").getAttribute("aria-pressed"), "true", "the chip does not say what another device chose");
  assert.deepStrictEqual(linkWords(page), ADDRESSES, "the links were not rewritten for another device's choice");
});

test("a kind turned off or on keeps a reader at the newest line there, and asks the server for nothing", async () => {
  const page = newPage();
  await signIn(page);
  const area = page.el("scroll-area");
  // Short messages, so no row is folded to a fixed height and every link drawn or taken away moves
  // the foot of the list: half of them a pull request.
  await showDiscord(page, Array.from({ length: 30 }, (_unused, i) =>
    message({ id: String(i + 1), content: `note ${i}: ${i % 2 ? `${REPO}/pull/${i}` : `https://example.com/${i}`}` })));
  await showLinks(page);
  assert.ok(area.scrollHeight > 3 * area.clientHeight, "the fixture is not tall enough to leave a reader short of its foot");
  assert.ok(atBottomOf(area), "Links did not put the reader at the newest link");
  const reads = page.pageReads;
  await tapKind(page, "pr");
  assert.ok(atBottomOf(area), "turning PRs off left the reader short of the newest link");
  // The list is shorter now. A browser puts a reader who was at its foot at the new foot by itself;
  // this fixture keeps an offset only as it is written, so it is written as a browser would leave it.
  area.scrollTop = area.scrollHeight;
  await tapKind(page, "pr");
  // The list grew under the reader by fifteen links, and the page took them to its new foot.
  assert.ok(atBottomOf(area), "turning PRs back on left the reader short of the newest link");
  assert.equal(page.pageReads, reads, "a kind cost a round trip to the server");
});

// `#212 link-filter`, from review. A list the filters empty says why at its head (`#search-empty`), but
// its rows' going does not make it short: the channel's composer stays below every list, so with the
// bar's room above it the list still overflowed a phone, and a reader at the newest line — where Links
// and the kinds leave one — had the sentence scrolled up under the kinds of link it names, or off the
// screen with the keyboard up. tests/offline_cache_browser.py measures that; this pins down where the
// page puts the reader. Hidden rows keep their height in this fixture, which stands in for the composer.

test("a list the filters empty is shown from its head, where it says why, and filled again opens at its newest", async () => {
  const run = async (script) => {
    const page = newPage(new Map(), script);
    await signIn(page);
    await showDiscord(page, Array.from({ length: 30 }, (_unused, i) =>
      message({ id: String(i + 1), content: `note ${i}: ${REPO}/pull/${i + 1}` })));
    await showLinks(page);
    // The composer below every list keeps it overflowing a phone with all its rows gone, which is the
    // whole reason the sentence needs the reader taken to it. A tall line under the box stands in for
    // it here, where a filtered-out row takes no room and a short list could not be scrolled at all.
    page.el("channel-compose-state").textContent = "a line under the message box. ".repeat(40);
    await page.el("scroll-area").dispatch("scroll");
    page.el("scroll-area").scrollTop = page.el("scroll-area").scrollHeight;
    return page;
  };
  const page = await run(SCRIPT);
  const area = page.el("scroll-area");
  const sentence = page.el("search-empty");
  const sentenceInView = () => {
    const box = sentence.getBoundingClientRect();
    return !sentence.hidden && box.top >= 0 && box.bottom <= area.clientHeight;
  };
  assert.ok(atBottomOf(area) && area.scrollTop > 2 * area.clientHeight,
    "the fixture does not leave a reader at the newest line a screen or more below the head of the list");

  // Every link is a pull request: PRs off empties the list, and the reader is taken to the sentence.
  await tapKind(page, "pr");
  assert.equal(sentence.textContent, "No links left once PRs are hidden — turn them back on under the Links button.");
  assert.equal(area.scrollTop, 0, "the reader was left at the foot of a list that says why it is empty at its head");
  assert.ok(sentenceInView(), "the sentence is not on screen");
  // The scroll that put the reader there, as a browser delivers it: no jump is offered past the sentence.
  await area.dispatch("scroll");
  assert.equal(page.el("jump-newest").hidden, true, "the jump offers a newest message in a list with none shown");
  // PRs on from there: the links are back, and the list opens at its newest, not its oldest.
  await tapKind(page, "pr");
  assert.equal(sentence.hidden, true);
  assert.ok(atBottomOf(area), "the links came back with the reader at the oldest of them");

  // The same for the search text: matching nothing, the sentence; matching again, the newest line.
  await page.el("search-field").setValue("no such words");
  assert.equal(sentence.textContent, "None of the loaded messages with a link match that search.");
  assert.equal(area.scrollTop, 0, "text that matches nothing left the reader short of the sentence");
  await page.el("search-field").setValue("note");
  assert.equal(sentence.hidden, true);
  assert.ok(atBottomOf(area), "text that matches again left the reader at the oldest match");

  // Only after the reader's own act: a poll redrawing the emptied list leaves a reader who went down
  // to the composer at the composer.
  await tapKind(page, "pr");
  area.scrollTop = area.scrollHeight;
  await reReadChannel(page);
  assert.equal(sentence.hidden, false);
  assert.ok(atBottomOf(area), "a poll took the reader from the composer back up to the sentence");

  // THE CONTROLS: each half of `placeForFilter`, and the jump, taken out, and the step above that
  // needs it fails.
  const unplaced = await run(brokenScript(
    "  if (listEmptied()) {\n    area.scrollTop = 0;", "  if (listEmptied()) {\n    void 0;"));
  await tapKind(unplaced, "pr");
  assert.ok(unplaced.el("scroll-area").scrollTop > 0, "the test cannot see a reader left below the sentence");
  const jumping = await run(brokenScript(
    "    shown = !listEmptied() && (inScrollback[currentView] || jumpNewestWanted[currentView] !== false);",
    "    shown = inScrollback[currentView] || jumpNewestWanted[currentView] !== false;"));
  await tapKind(jumping, "pr");
  await jumping.el("scroll-area").dispatch("scroll");
  assert.equal(jumping.el("jump-newest").hidden, false, "the test cannot see a jump offered over an emptied list");
  const unfilled = await run(brokenScript("    if (anchor || wasEmptied) scrollToNewest();", "    if (false) scrollToNewest();"));
  await tapKind(unfilled, "pr");
  await tapKind(unfilled, "pr");
  assert.ok(!atBottomOf(unfilled.el("scroll-area")), "the test cannot see links come back above the reader");
});

test("Links over a channel with no link shows the sentence, and off puts the reader back where they were", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, Array.from({ length: 30 }, (_unused, i) =>
    message({ id: String(i + 1), content: `note ${i}: nothing to open in this one` })));
  const area = page.el("scroll-area");
  assert.ok(area.scrollHeight > 3 * area.clientHeight, "the fixture is not tall enough to leave a reader mid-list");
  await page.el("search-toggle").click();
  await page.settle();
  area.scrollTop = Math.round((area.scrollHeight - area.clientHeight) / 2);
  const top = area.scrollTop;
  await page.el("links-filter").click();
  await page.settle();
  assert.equal(page.el("search-empty").textContent, "No links in the messages loaded so far.");
  assert.equal(area.scrollTop, 0, "Links on left the reader at the newest line, below the sentence");
  // Off, the list is the one it was, and the reader's own place stands: not the newest line an
  // emptied list is filled again at.
  await page.el("links-filter").click();
  await page.settle();
  assert.equal(page.el("search-empty").hidden, true);
  assert.equal(area.scrollTop, top, "Links off lost the reader's place");
});

// `#216 compose-hint`. The owner: the "Message the main channel" line read as one more message, in
// the same type as the messages. There is no heading line now: the box says where the message goes in
// its placeholder and names itself with the same words, in the main channel and in a thread alike.
test("the channel composer says where the message goes in its placeholder, not in a heading line", () => {
  assert.doesNotMatch(HTML_CODE, /id="channel-compose-label"/, "the composer's heading line came back");
  assert.match(HTML_CODE, /id="channel-compose-text"[^>]*placeholder="Message the main channel…"[^>]*aria-label="Message the main channel"/);
  assert.match(SCRIPT_CODE, /const destination = selection\(\)\.threadId \? "Reply in this thread" : "Message the main channel";/);
  assert.match(SCRIPT_CODE, /el\("channel-compose-text"\)\.placeholder = `\$\{destination\}…`;/);
  assert.doesNotMatch(SCRIPT_CODE, /Write a message…|Write a thread reply…/, "the old, less informative hint came back");
});
