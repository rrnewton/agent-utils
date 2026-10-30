// The app page's tests: the frame, the header and control bar, the status line, and what the page says when things fail.
//
// One feature's file of the suite tests/js/voice_page.test.mjs runs, against the fixture in
// ./fixture.mjs. Run with the rest of the suite, or alone: node --test tests/js/page/shell.mjs.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { WEB } from "../../../web/page_sources.mjs";

import { BASE_CSS, CANNED, CONVAI_WRITE, CSS, CSS_CODE, CSS_UNCONDITIONAL, DESKTOP_QUERY, HTML,
  HTML_CODE, MODULE_CODE, PAGE_CODE, PAGE_ELEMENTS, PAGE_IDS, SCRIPT, SCRIPT_CODE,
  SEAM_DETAIL_MAX_WORDS, SEAM_DETAIL_MIN_WORDS, SEARCH_LANGUAGE, assertMarkupContains, barSlice,
  bigCardData, brokenScript, channelSummaryText, compose, cssBlock, cssBlockIn, cssBlockOutside,
  cssRules, cssRulesElsewhere, cssWithout, errorResponse, forwardTimeline, freshness, json,
  longMessage, markupHolds, markupPlace, mediaBody, message, newPage, pagedChannel, pickThread,
  reReadChannel, replyButton, replyInStream, seamDetail, searchLanguage, secondOfSpeech,
  session_lines, showDiscord, signIn, speech, startNeutralCall, startTalking, state, storedTexts,
  test, threadData, threadPage, turnDone, withoutStrings, words } from "./fixture.mjs";

// --- what the page says when things fail --------------------------------------------------------

test("the page starts with no error showing", () => {
  // From the markup: the panel ships hidden, so its appearance later is itself the signal.
  const page = newPage();
  assert.equal(page.el("error").hidden, true, "web/voice.html must ship the panel hidden");
  assert.equal(page.el("error").textContent, "");
});

test("an error can be dismissed without waiting for another request", async () => {
  const page = newPage();
  page.el("api-token").value = "write-token-aaaaaaaaaaaaaaaa";
  page.clientConfig = async () => { throw new TypeError("Failed to fetch"); };
  await page.el("save-token").click();
  await page.settle();
  assert.match(page.el("error").textContent, /Could not reach vibe-talk while loading data/);
  await page.el("dismiss-error").click();
  assert.equal(page.el("error").hidden, true);
  assert.equal(page.el("error").textContent, "");
});

test("a vendor 502 is shown IN THE PAGE, in the vendor's own words", async () => {
  // The live case: the owner's ElevenLabs key lacks `convai_write`. Nobody should need dev tools
  // to learn that.
  const page = newPage();
  await signIn(page);
  page.setFetch(
    errorResponse(502, "elevenlabs_error", `elevenlabs returned HTTP 401: ${CONVAI_WRITE}`)
  );

  await page.el("talk").click();

  const shown = page.el("error");
  assert.equal(shown.hidden, false, "the error panel stayed hidden");
  assert.match(shown.textContent, /missing the permission convai_write/);
  assert.ok(
    shown.textContent.includes(CONVAI_WRITE),
    `the vendor's sentence was not shown verbatim: ${shown.textContent}`
  );
  assert.match(shown.textContent, /502/, "the HTTP status is part of the diagnosis");
  assert.match(shown.textContent, /elevenlabs_error/, "the error code identifies which failure");
  assert.equal(state(page), "error");
});

test("a 503 names the exact setting the operator has not set", async () => {
  const page = newPage();
  await signIn(page);
  page.setFetch(
    errorResponse(
      503,
      "elevenlabs_not_configured",
      "elevenlabs.api_key is not set, so no conversation can be minted"
    )
  );

  await page.el("talk").click();

  assert.match(page.el("error").textContent, /elevenlabs\.api_key/);
  assert.match(page.el("error").textContent, /503/);
});

test("a missing token is reported in the page rather than silently doing nothing", async () => {
  const page = newPage();
  let called = false;
  page.setFetch(async () => {
    called = true;
    throw new Error("unreachable");
  });

  await page.el("talk").click();

  assert.equal(called, false, "nothing should be requested without a token");
  assert.match(page.el("error").textContent, /token/i);
  assert.equal(page.el("error").hidden, false);
});

test("no credential ever reaches the rendered page, even if the server echoes one back", async () => {
  // The server redacts its own secrets, but the page holds one the server never sees. A hostile
  // or merely careless error body that quotes the request back must not become visible text.
  const TOKEN = "write-token-SECRET-do-not-render";
  const page = newPage();
  page.el("api-token").value = TOKEN;
  await page.el("save-token").click();
  await page.settle();
  page.setFetch(
    errorResponse(502, "elevenlabs_error", `upstream rejected Authorization: Bearer ${TOKEN}`)
  );

  await page.el("talk").click();

  const rendered = page.renderedText();
  assert.ok(!rendered.includes(TOKEN), `the API token was rendered into the page: ${rendered}`);
  assert.match(rendered, /\[redacted\]/, "the redaction must be visible, not a silent deletion");
  assert.ok(
    rendered.includes("upstream rejected"),
    "redaction must not swallow the rest of the message"
  );
});

// --- the token, with a button that visibly reacts -----------------------------------------------

test("Save changes the button itself, not only a banner", async () => {
  const page = newPage();
  const button = page.el("save-token");
  assert.equal(
    button.textContent,
    "Save token",
    "the fixture takes the starting label from web/voice.html itself"
  );
  page.el("api-token").value = "write-token-aaaaaaaaaaaaaaaa";

  await button.click();

  assert.equal(button.textContent, "Saved ✓", "the button did not change state on a save");
  assert.equal(button.className, "ok");
  assert.equal(page.storage.get("vibe-talk.token"), "write-token-aaaaaaaaaaaaaaaa");
  assert.match(page.el("token-state").textContent, /token saved/);
});

test("saving nothing is an error, not a success banner", async () => {
  const page = newPage();
  page.el("api-token").value = "   ";

  await page.el("save-token").click();

  assert.equal(page.storage.has("vibe-talk.token"), false, "whitespace was stored as a token");
  assert.equal(page.el("save-token").textContent, "Save token");
  assert.match(page.el("error").textContent, /nothing to save/i);
});

test("a browser that refuses to store the token says so instead of claiming success", async () => {
  const page = newPage();
  page.el("api-token").value = "write-token-aaaaaaaaaaaaaaaa";
  // Private browsing: setItem is accepted and then does nothing.
  page.storage.set = () => page.storage;

  await page.el("save-token").click();

  assert.notEqual(page.el("save-token").textContent, "Saved ✓");
  assert.match(page.el("error").textContent, /NOT saved/);
});

test("Forget clears the stored token and says what is now true", async () => {
  const page = newPage();
  await signIn(page);

  await page.el("forget-token").click();

  assert.equal(page.storage.has("vibe-talk.token"), false);
  assert.equal(page.el("api-token").value, "");
  assert.match(page.el("token-state").textContent, /no token saved/);
});

// --- screens -------------------------------------------------------------------------------------

test("with no token the page IS the sign-in screen, not the interface behind a nag", async () => {
  const page = newPage();
  await page.settle();

  assert.equal(page.screen(), "signin");
  // The controls act on a call this visitor cannot place, so they are not on the screen.
  assert.equal(page.el("control-pane").hidden, true, "the control pane showed before sign-in");
  assert.equal(page.el("view-switch").hidden, true, "there is nothing to switch between yet");
  // The status line used to be kept visible here so a refused token could say so. It is a
  // transient message now (`#63 status-line-placement`) and nothing has happened yet, so firing
  // one at page load would put a toast over a screen whose whole subject is the thing it is
  // asking for. The instruction lives in the sign-in screen's own body instead — and a refusal,
  // when there is one, is the permanent `#error` panel, which shows on whichever screen is up.
  assert.equal(page.el("status-line").hidden, true, "an empty toast greeted a first-time visitor");
  assert.match(
    page.el("token-state").textContent,
    /paste your write-scope token/i,
    "nothing on the sign-in screen tells a first visitor what to do"
  );
});

test("a pending audio suspension honors the next read and a later stop", async () => {
  const page = newPage();
  page.enableStreamedAudio();
  const audio = page.pure("readAloudAudioContext")();
  const pending = [];
  audio.suspend = () => new Promise((resolve) => pending.push(() => {
    audio.state = "suspended";
    resolve();
  }));
  page.pure("quietReadAloudContext")();
  page.pure("readAloudAudioContext")();
  pending.shift()();
  await page.settle();
  assert.equal(audio.state, "running", "the preceding read's suspension silenced the next read");

  page.pure("quietReadAloudContext")();
  page.pure("readAloudAudioContext")();
  page.pure("quietReadAloudContext")();
  for (const finish of pending.splice(0)) finish();
  await page.settle();
  assert.equal(audio.state, "suspended", "an old suspension resumed audio after Stop");
});

test("the sign-in screen is compact and names the server receiving the token", () => {
  const signin = HTML.slice(
    HTML.indexOf('id="screen-signin"'),
    HTML.indexOf('id="screen-main"')
  );
  const hints = [...signin.matchAll(/<p[^>]*class="hint"[^>]*>([\s\S]*?)<\/p>/g)].map((match) =>
    match[1].replace(/<[^>]+>/g, " ").replace(/\s+/g, " ").trim()
  );
  assert.equal(hints.length, 2, "the sign-in form grew explanatory paragraphs again");
  assert.ok(hints.every((text) => text.length < 100), `verbose sign-in text: ${hints.join(" | ")}`);
  const page = newPage();
  assert.equal(page.el("signin-server-url").textContent, "https://vibe.example");
  assert.equal(page.el("settings-server-url").textContent, "https://vibe.example");
});

test("settings names its server host first", () => {
  const settings = HTML.slice(HTML.indexOf('id="screen-settings"'));
  const banner = settings.indexOf('id="settings-server-host"');
  assert.ok(banner !== -1, "settings has no server host banner");
  assert.ok(
    banner < settings.indexOf('id="settings-voice-agent"'),
    "the server host is not the first thing on the settings screen"
  );
  const page = newPage();
  assert.equal(page.el("settings-server-host").text(), "vibe.example");
});

// --- the frame: what makes it an app rather than a page -----------------------------------------

test("the document itself cannot scroll, and cannot rubber-band", () => {
  const body = cssBlock("body");
  assert.match(body, /height:\s*100dvh/, "the frame must use the DYNAMIC viewport unit");
  assert.match(body, /overflow:\s*hidden/, "the document must not scroll");
  assert.match(body, /overscroll-behavior:\s*none/, "the page must not rubber-band");
  assert.match(cssBlock("html"), /overscroll-behavior:\s*none/);
  assert.doesNotMatch(body, /height:\s*100vh\b/, "100vh is the unit that gets the toolbar wrong");
});

test("the transcript is the one thing that scrolls, and the control pane is a grid row", () => {
  const body = cssBlock("body");
  assert.match(
    body,
    /grid-template-rows:\s*auto 1fr auto/,
    "header / body / pane must be a three-row grid"
  );
  const scroll = cssBlock("#scroll-area");
  assert.match(scroll, /overflow-y:\s*auto/, "the transcript area must be the scrolling element");
  assert.match(scroll, /overscroll-behavior:\s*contain/, "a flick must not escape the transcript");
  assert.match(
    scroll,
    /min-height:\s*0/,
    "without min-height:0 the grid row refuses to shrink and the document scrolls again"
  );

  const pane = cssBlock("#control-pane");
  assert.doesNotMatch(
    pane,
    /position:\s*(fixed|absolute)/,
    "the pane holds its place by being a grid row, not by being pinned"
  );
  assert.doesNotMatch(pane, /overflow-y:\s*(auto|scroll)/, "the pane must never scroll");
  assert.match(
    pane,
    /padding-bottom:\s*calc\([^)]*env\(safe-area-inset-bottom\)/,
    "the controls must clear the home indicator"
  );
});

test("the controls are built to be tapped, not clicked", () => {
  const control = cssBlock(".control");
  assert.match(control, /touch-action:\s*manipulation/, "leaves the 300 ms tap delay in place");
  assert.match(control, /-webkit-tap-highlight-color:\s*transparent/, "grey flash on tap");
  assert.match(control, /user-select:\s*none/, "a long press would start selecting the label");
});

test("home-screen identity installs standalone where supported and keeps the browser fallback", () => {
  const manifest = JSON.parse(readFileSync(join(WEB, "manifest.webmanifest"), "utf8"));
  assert.deepStrictEqual(manifest.display_override, ["standalone"]);
  assert.equal(manifest.display, "browser");
  assert.doesNotMatch(HTML_CODE, /apple-mobile-web-app-capable/);
  assert.doesNotMatch(HTML_CODE, /mobile-web-app-capable/);
  // The markup does not declare it, and the prose says why — so the comment must survive too.
  assert.match(HTML, /apple-mobile-web-app-capable/, "the reason it is absent must stay recorded");
});

test("no function in the page is declared twice, because the second one wins in silence", () => {
  // A real trap, not a style rule. `sendClientEvent` and `canSendText` were each declared TWICE in
  // this file — once beside the long note explaining the send path, once beside the Discord relay
  // — with different conditions, and function declarations hoist, so the later pair won every call
  // including the ones under the comment describing the earlier pair. Nothing warned: no build
  // step, no linter, no runtime error, and the tests passed either way because they only ever
  // exercised the winner. It cost `#73 mute-is-invisible` a wrongly placed announcement, put on
  // the socket at a moment the LIVE definition refuses to send at.
  const declared = [...SCRIPT_CODE.matchAll(/^function ([A-Za-z0-9_]+)\(/gm)].map((m) => m[1]);
  const twice = declared.filter((name, i) => declared.indexOf(name) !== i);
  assert.deepStrictEqual(
    [...new Set(twice)],
    [],
    "these functions are declared more than once in web/voice.js; the LAST declaration is the one " +
      "that runs, so every comment on the others describes code nobody calls"
  );
});

test("the page LOADS nothing from anywhere but this server", () => {
  // No framework, no CDN, no build step: the whole reason the frame is CSS.
  //
  // "LOADS", and the word is the change. The rule used to be that no off-origin URL appeared in
  // these files at all, which is a proxy for the real one and stopped being an accurate proxy the
  // moment the page grew its first LINK — a destination the reader taps, which pulls in nothing
  // and executes nothing here. So the invariant is stated directly instead: an off-origin URL may
  // be somewhere to GO, and may never be something this page brings in.
  assert.doesNotMatch(CSS, /@import|url\(\s*["']?https?:/, "web/voice.css reaches off-origin");
  // In the markup, that means it sits on an `<a>` and on nothing else. A `src`, a `<link href>`,
  // a form `action` or an `<iframe>` all fail here, whatever the tag is called.
  const carried = [...HTML_CODE.matchAll(/<([\w-]+)\b[^>]*?(https?:\/\/[^"'\s>]+)/g)];
  assert.deepStrictEqual(
    carried.filter((m) => m[1] !== "a").map((m) => `<${m[1]}> ${m[2]}`),
    [],
    "web/voice.html pulls something in from off-origin"
  );
  // ...and in the script, that it is never handed to anything that goes and gets it. Named
  // explicitly rather than by a blanket ban, so the next off-origin destination is a link and the
  // next off-origin FETCH is still a failure.
  for (const how of [/fetch\(\s*["'`]https?:/, /new Worker\(\s*["'`]https?:/, /import\(\s*["'`]https?:/]) {
    assert.doesNotMatch(PAGE_CODE, how, "web/voice.js goes and gets something off-origin");
  }
  // Provider destinations arrive as data from the backend trait; this file knows no vendor URL.
  assert.deepStrictEqual(
    [...new Set([...PAGE_CODE.matchAll(/https?:\/\/[^"'`\s]+/g)].map((m) => m[0]))],
    [],
    "web/voice.js hardcodes an off-origin destination instead of using backend data"
  );
});

test("the control pane is one dense tile, and the two small controls stack to the height of the big ones", () => {
  // The shape the owner asked for:
  //
  //     +----+----------+----------+
  //     | () |          |          |
  //     +----+ Hang up  |   Talk   |
  //     | [] |          |          |
  //     +----+----------+----------+
  //
  // Checked as GEOMETRY rather than as pixels: both large controls span the same two rows the two
  // small ones occupy one each, so their heights cannot drift apart — there is nothing to keep in
  // sync. The two large columns are `1fr` each, so the pane fills the width rather than sitting in
  // it.
  const pane = cssBlock("#control-pane");
  assert.match(pane, /display:\s*grid/, "the pane is a tile, not a row of floating buttons");
  const columns = /grid-template-columns:\s*([^;]+);/.exec(pane);
  assert.ok(columns, "the pane must state its columns");
  assert.match(
    columns[1],
    /1fr\s+1fr\s*$/,
    `the two large controls must each take a share of the width, not ${columns[1]}`
  );
  assert.match(
    pane,
    /grid-template-rows:\s*1fr 1fr/,
    "two rows: one per small control, and both spanned by each large one"
  );

  assert.match(cssBlock("#speaker"), /grid-row:\s*1/);
  assert.match(cssBlock("#clear-view"), /grid-row:\s*2/);
  assert.match(cssBlock(".control-mini"), /grid-column:\s*1/, "the small controls are the left column");
  for (const selector of [".control-hangup", ".control-talk"]) {
    assert.match(
      cssBlock(selector),
      /grid-row:\s*1 \/ span 2/,
      `${selector} must span both rows, which is what makes the stack line up`
    );
  }
  assert.match(cssBlock(".control-hangup"), /grid-column:\s*2/);
  assert.match(cssBlock(".control-talk"), /grid-column:\s*3/);
});

test("the large controls are rectangular: nothing pins them to a square-ish fixed width", () => {
  // They were 8.5rem wide by 8rem tall — near-square slabs taking a third of a phone screen. The
  // owner asked for less tall and more rectangular. Width is now a share of the pane, so the only
  // way to regress this is to put a fixed width back.
  for (const selector of [".control-talk", ".control-hangup"]) {
    const block = cssBlock(selector);
    assert.doesNotMatch(block, /(^|[^-])width:\s*[\d.]+rem/, `${selector} pins its own width again`);
    assert.doesNotMatch(block, /min-height:\s*[\d.]+rem/, `${selector} pins its own height again`);
  }
  // Their height is two rows of the small control, and that is the only place a number appears.
  const mini = Number(/min-height:\s*([\d.]+)rem/.exec(cssBlock(".control-mini"))[1]);
  assert.ok(mini > 0 && mini * 2 < 6, `two stacked small controls are ${mini * 2}rem, which is not "less tall"`);
  assert.match(cssBlock(".control-talk.live .control-ring"), /animation:\s*talk-pulse/);
});

test("hang up comes before talk in the markup, so talk lands under the right thumb", () => {
  const pane = HTML.slice(HTML.indexOf('id="control-pane"'));
  const hangUp = pane.indexOf('id="hang-up"');
  const talk = pane.indexOf('id="talk"');
  assert.ok(hangUp > -1 && talk > -1, "the pane must hold both controls");
  assert.ok(hangUp < talk, "hang up must come before talk");
  // And the two small ones come before both, which is the left column.
  assert.ok(pane.indexOf('id="speaker"') < hangUp);
  assert.ok(pane.indexOf('id="clear-view"') < hangUp);
});

test("the view control is ONE switch carrying the word for the view you are in", async () => {
  // What this replaces: two text labels, one outlined and one plain, which read as one item having
  // been disabled rather than as the unselected half of a pair.
  assert.equal(PAGE_IDS.has("tab-voice"), false, "the two-label pseudo-tabs are gone");
  assert.equal(PAGE_IDS.has("tab-discord"), false);

  const page = newPage();
  await signIn(page);
  const label = page.el("view-switch-label");
  assert.equal(label.textContent, "Voice", "the switch must name the view you are IN");
  assert.equal(page.el("view-switch").getAttribute("aria-checked"), "false");

  await page.el("view-switch").click();
  await page.settle();

  assert.equal(page.tab(), "discord");
  assert.equal(label.textContent, "Channel", "the word did not follow the switch");
  assert.equal(page.el("view-switch").getAttribute("aria-checked"), "true");

  await page.el("view-switch").click();
  assert.equal(page.tab(), "voice");
  assert.equal(label.textContent, "Voice");
});

test("the switch is a switch: a knob that moves, and a track that changes with it", () => {
  // Three signals for one state — word, knob position, track colour — so it survives being read
  // quickly, at arm's length, by somebody who is not looking for it.
  // RE-SLICED by `#58 control-bar`: the switch and the gear both live in the bar now, and the gear
  // comes FIRST, so the old bounds (from the switch to the gear) run backwards and would silently
  // return an empty string — a test that passes by having nothing to look at.
  const html = barSlice(HTML.indexOf('id="view-switch"'));
  assert.match(html, /role="switch"/, "a switch has to announce itself as one");
  assert.match(html, /class="switch-track"/);
  assert.match(html, /class="switch-knob"/);
  assert.match(cssBlock('.switch[aria-checked="true"] .switch-knob'), /transform:\s*translateX/);
  assert.match(cssBlock('.switch[aria-checked="true"] .switch-track'), /background:\s*var\(--accent\)/);
});

test("settings is a gear, not the word Settings", () => {
  // RE-SLICED by `#58 control-bar`: the gear is in the dock now, so `</header>` no longer closes
  // the region it sits in and the old slice would have been empty.
  const button = barSlice(HTML.indexOf('id="open-settings"'));
  assert.match(button, /<svg/, "the settings control must be drawn, not spelled");
  assert.match(button, /aria-label="Settings"/, "an icon still has to say what it is");
  assert.equal(
    PAGE_ELEMENTS.get("open-settings").text,
    "",
    "the gear must carry no word; that was the thing being replaced"
  );
});

test("the header shows two things at a time, and which two depends on the screen", async () => {
  // It used to hold four controls in two styles and gave no readable model of which were tabs and
  // which were actions. Clear has moved to the control pane, where it is honestly grouped with the
  // other things you DO; the settings screen turns the same row into a title bar with a way back,
  // rather than leaving it empty and putting a Back button in the body.
  //
  // REWRITTEN BY `#58 control-bar`. The switch and the gear are not in this row any anymore, and
  // the id list below says so rather than being relaxed to accommodate them: what is left in the
  // header markup is the two ways back and the title, and the loop underneath — which is the part
  // that was always doing the work — is unchanged and still asserts that at most two of the five
  // are up at once.
  const row = HTML.slice(HTML.indexOf('id="topbar-row"'), HTML.indexOf("</header>"));
  const ids = [...row.matchAll(/<button[^>]*\bid="([^"]+)"/g)].map((m) => m[1]);
  // `#51 reply-view` added a SECOND way back, deliberately rather than by generalising the first:
  // the two go to different places, and scripts/screenshots.py drives #close-settings by name.
  // `#85 voice-desktop-review` added a THIRD for the same reason: Settings returns to the call, Reply
  // returns to the channel, and Help returns to Settings.
  //
  // `#129 message-search` added a fourth, the search glass, in the corner; `#197 floating-search`
  // took it out again, because a glass in this row is what made the header cost a whole row on
  // every main screen. It floats over the list now — see "THE HEADER COSTS NO ROW ON THE MAIN
  // SCREEN". The list says so rather than being relaxed: only the ways back are declared here.
  //
  // `#194 thread-picker-polish` added a fourth way back, for the Threads screen, which returns to
  // the channel it lists — at the leading edge with the others. `#281 view-keys` retired that screen
  // for the thread list, a view of the main screen, and its way back with it. `#410
  // inline-reply-editor` retired the reply screen, Reply opening in the list, and its way back too.
  assert.deepStrictEqual(ids, ["close-settings", "close-help"]);

  const page = newPage();
  const showing = () =>
    [
      "close-settings",
      "close-help",
      "topbar-title",
      "view-switch",
      "open-settings",
    ].filter((id) => !page.el(id).hidden);
  await signIn(page);
  assert.deepStrictEqual(showing(), ["view-switch", "open-settings"]);

  await page.el("open-settings").click();
  assert.deepStrictEqual(showing(), ["close-settings", "topbar-title"]);

  await page.el("close-settings").click();
  assert.deepStrictEqual(showing(), ["view-switch", "open-settings"]);

  // `#410 inline-reply-editor`. Reply is in the list: the header is the main screen's, before and after.
  const [line] = await showDiscord(page, [message({ id: "1", content: "hello" })]);
  await replyButton(line).click();
  assert.ok(replyInStream(page), "the premise is a reply open in the list");
  assert.deepStrictEqual(showing(), ["view-switch", "open-settings"]);

  await page.el("reply-cancel").click();
  assert.deepStrictEqual(showing(), ["view-switch", "open-settings"]);
});

// --- the control bar (#58 control-bar) ------------------------------------------------------------

test("THE BAR'S DEFAULT HOME IS THE DOCK, DIRECTLY ABOVE THE BIG BUTTONS", () => {
  // The owner's words, and "bottom" is the half that is easy to get wrong: not below the big
  // buttons, and not pinned to the viewport floor — DIRECTLY ABOVE them. In the markup that is an
  // ordering fact, and it is asserted here because it is also the default, so it has to hold on a
  // page whose script never ran.
  const dock = HTML.slice(HTML.indexOf('id="dock"'));
  const bar = dock.indexOf('id="control-bar-bottom"');
  const pane = dock.indexOf('id="control-pane"');
  assert.ok(bar > -1, "the bar's default home is not in the dock at all");
  assert.ok(bar < pane, "the control bar is below the big buttons rather than above them");
  assertMarkupContains("control-bar-bottom", "control-bar");
  // ...and it is a grid row of the dock like everything else there, not something positioned over
  // the top of the controls.
  assert.doesNotMatch(cssBlock("#control-bar"), /position:\s*(fixed|absolute)/);
  // The top mount exists and is EMPTY in the markup: two mounts, one bar. A second copy would be a
  // second gear and a second switch to keep in agreement.
  const header = HTML.slice(HTML.indexOf('id="topbar"'), HTML.indexOf("</header>"));
  assert.ok(header.includes('id="control-bar-top"'), "there is nowhere to put the bar at the top");
  assert.doesNotMatch(header, /id="view-switch"/, "a second switch is declared in the header");
  assert.doesNotMatch(header, /id="open-settings"/, "a second gear is declared in the header");
});

test("the gear is LEFT and the switch is RIGHT, which is a right-handed thumb argument", () => {
  // The owner's reasoning, not a symmetry: the gear is the control you least want to hit by
  // accident, so it goes as far as possible from where the thumb rests, and the switch — the one
  // you actually flick — goes under it.
  const bar = barSlice();
  const gear = bar.indexOf('id="open-settings"');
  const pack = bar.indexOf('id="bar-pack"');
  const swtch = bar.indexOf('id="view-switch"');
  assert.ok(gear > -1 && pack > -1 && swtch > -1, "the bar is missing one of its three parts");
  assert.ok(gear < pack, "the gear is not the leftmost thing in the bar");
  assert.ok(pack < swtch, "the switch is not the rightmost thing in the bar");
  // And it is the PACK that pins them to the two ends: it takes the leftover width, so the gear
  // cannot drift right nor the switch left as buttons are added between them.
  assert.match(cssBlock("#bar-pack"), /flex:\s*1 1 auto/, "nothing holds the two ends apart");
});

test("THE BAR IS A CONTAINER THAT PACKS, NOT A TWO-ITEM LAYOUT", () => {
  // The issue is explicit that this is built for a bar that fills up: `#59 text-entry-button` and
  // `#60 canned-prompt-buttons` both add buttons to it, and the failure it is asking to avoid is a
  // layout that has to be rewritten the first time a third button arrives.
  const pack = cssBlock("#bar-pack");
  assert.match(pack, /display:\s*flex/);
  assert.match(pack, /min-width:\s*0/, "a flex item without this refuses to shrink and clips");
  // The pack, stated: extra buttons SCROLL. Wrapping would spend a second row of vertical space —
  // the exact thing this issue exists to save — and clipping would push the switch off the right
  // edge of a 375px phone, where it cannot be tapped at all. The switch is the widest item in the
  // bar, so that is arithmetic rather than a worry.
  assert.match(pack, /overflow-x:\s*auto/, "the bar clips or wraps once it is full");
  assert.doesNotMatch(pack, /flex-wrap:\s*wrap/);
  const track = Number(/width:\s*([\d.]+)rem/.exec(cssBlock(".switch-track"))[1]);
  const word = Number(/min-width:\s*([\d.]+)rem/.exec(cssBlock(".switch-word"))[1]);
  assert.ok(track + word > 5, `the switch is ${track + word}rem wide; the packing claim above is stale`);

  // "Sized like the existing Sound / Clear controls", which is the issue's own phrase. Derived
  // from `.control-mini` rather than restated, so the two cannot drift apart silently.
  const height = (block) => /min-height:\s*([\d.]+)rem/.exec(block)[1];
  assert.equal(
    height(cssBlock(".bar-button")),
    height(cssBlock(".control-mini")),
    "a bar button is not the same size as Sound and Clear"
  );
  // ...and deliberately NOT by being one of them: `.control-mini` carries `grid-column: 1`, which
  // belongs to #control-pane's 3x2 grid.
  assert.doesNotMatch(cssBlock(".bar-button"), /grid-column/, "the bar joined the pane's grid");
  // The tap ergonomics the rest of this page's controls have. A button that is one only in the DOM
  // still feels like a web page.
  for (const property of [/touch-action:\s*manipulation/, /-webkit-tap-highlight-color/, /user-select/]) {
    assert.match(cssBlock(".bar-button"), property, "the bar buttons are not built to be tapped");
  }
});

test("CHOOSING TOP REALLY MOVES THE BAR, AND THE CHOICE SURVIVES A RELOAD", async () => {
  const page = newPage();
  await signIn(page);
  const bar = page.el("control-bar");
  assert.equal(bar.parentNode, page.el("control-bar-bottom"), "the default home is not the dock");
  assert.equal(bar.getAttribute("data-placement"), "bottom");
  assert.equal(page.el("bar-placement").value, "bottom", "the setting does not show where it is");

  page.el("bar-placement").value = "top";
  await page.el("bar-placement").dispatch("change");

  assert.equal(bar.parentNode, page.el("control-bar-top"), "the bar did not move into the header");
  assert.equal(
    page.el("control-bar-bottom").children.includes(bar),
    false,
    "the bar is in BOTH homes at once, so nothing was moved"
  );
  assert.equal(bar.getAttribute("data-placement"), "top");
  assert.match(page.el("bar-placement-state").textContent, /Saved/);

  // A reload: same storage, a brand new execution of the page.
  const again = newPage(page.storage);
  await signIn(again);
  assert.equal(again.el("control-bar").parentNode, again.el("control-bar-top"), "the choice was lost");
  assert.equal(again.el("bar-placement").value, "top", "the setting does not show the stored choice");
});

test("a browser that refuses to store the placement says so instead of claiming saved", async () => {
  // Same shape as the microphone toggles and the token: private browsing accepts setItem and
  // stores nothing, and "Saved" would be a lie found out only after a reload.
  const page = newPage();
  await signIn(page);
  page.storage.set = () => page.storage;

  page.el("bar-placement").value = "top";
  await page.el("bar-placement").dispatch("change");

  assert.doesNotMatch(page.el("bar-placement-state").textContent, /Saved/, "it claimed success");
  assert.match(page.el("bar-placement-state").textContent, /refused to store/);
  // ...and it still moved, because the reader asked for it. Refusing to store is not refusing to act.
  assert.equal(page.el("control-bar").parentNode, page.el("control-bar-top"));
});

test("the bar hides PER MEMBER, so the gear stays reachable before anyone has signed in", async () => {
  // Hiding the whole bar off the main screen would be the easy implementation and would take the
  // gear away from the sign-in screen, where it is reachable today.
  const page = newPage();
  assert.equal(page.screen(), "signin");
  assert.equal(page.el("open-settings").hidden, false, "settings became unreachable when signed out");
  assert.equal(page.el("view-switch").hidden, true, "the view switch is offered with no views");
  assert.equal(page.el("control-bar").hidden, false, "the bar went away and took the gear with it");

  await page.el("open-settings").click();
  assert.equal(page.el("open-settings").hidden, true, "the gear offers the screen you are on");
  assert.equal(
    page.el("control-bar").hidden,
    true,
    "an empty bar still stands in the dock on the settings screen"
  );
});

// --- the channel picker, on the bar (#83 channel-selector-in-bar) --------------------------------
//
// The owner's complaint, in his words: "the crazy thread selector buried at the TOP of the
// scrollback window (useless) is still there. It's worse now because it seems to auto-load past
// history making it even more unreachable."
//
// Two facts, and the second is a regression this project introduced. The picker sat inside
// #scroll-area above the log, so the control for choosing WHAT YOU ARE READING scrolled away as
// soon as you read anything — and `#65 scrollback-paging` then made arriving at the top load
// another page, so scrolling toward the picker prepended history above it and it receded as you
// approached. On a channel of any size it could not be reached by scrolling at all.
//
// The fix is placement, not tuning: out of the scrolling element entirely and onto the bar `#58
// control-bar` built for exactly this. Auto-load is then free to keep working as designed, which
// is why nothing below touches OLDER_TRIGGER_PX.

test("THE CHANNEL PICKER IS ON THE BAR, AND OUT OF THE SCROLLING ELEMENT ENTIRELY", () => {
  assertMarkupContains("bar-pack", "discord-channel");
  // The half that is the issue. "It is on the bar" is satisfied by a page with a picker in both
  // places, and the one in the scrollback is the one the owner keeps finding.
  assert.equal(
    markupHolds("scroll-area", "discord-channel"),
    false,
    "the channel picker is still inside the scrolling element, where prepending history moves it"
  );
  assert.equal(
    (HTML_CODE.match(/id="discord-channel"/g) || []).length,
    1,
    "there are two channel pickers, so the page has two opinions about which channel is up"
  );
  // FIRST in the pack. The pack scrolls sideways when it is full, so the member at its left edge
  // is the one on screen without scrolling anything — and being unreachable is the whole defect.
  const pack = HTML.indexOf('id="bar-pack"');
  const picker = HTML.indexOf('id="discord-channel"');
  for (const later of ["prompts-open", "read-new", "canned-summary", "canned-blockers"]) {
    assert.ok(
      picker < HTML.indexOf(`id="${later}"`),
      `#${later} is packed before the channel picker, so the picker is what scrolls off the bar`
    );
  }
  assert.ok(picker > pack, "the picker is not in the pack at all");
});

test("the picker cannot take the width the pack holds for everything else", () => {
  // style.css gives every `select` `flex: 1`, and a pack member that GROWS takes the leftover
  // width the pack exists to distribute — pushing the buttons `#59` and `#60` put here off the
  // right-hand edge of a 375px phone, which is the clipping `#58 control-bar` was built to avoid.
  const rule = cssBlock(".bar-select");
  assert.match(rule, /flex:\s*0 1 auto/, "the picker grows into the bar");
  assert.match(rule, /max-width:/, "nothing caps how much of the bar a long channel name takes");
  // ...and it shrinks rather than clipping, which is what makes a long channel name cost
  // characters instead of costing the buttons beside it.
  assert.match(rule, /min-width:\s*[\d.]+rem/);
  const shared = cssRules(BASE_CSS, "select").join("\n");
  assert.match(shared, /flex:\s*1/, "the rule above is overriding something that is no longer there");
  // Sixteen pixels, the same iOS rule `#compose-text` states: a control smaller than that zooms
  // the whole frame on focus, and this page is a fixed 100dvh grid that cannot scroll back.
  assert.match(rule, /font-size:\s*16px/, "focusing the picker will zoom the frame on iOS");
});

// --- and does the whole bar FIT? ----------------------------------------------------------------
//
// `#83 channel-selector-in-bar` put a SIXTH control on a strip that already carried five, and the
// pack scrolls sideways — so "it is on the bar" and "it can be reached" stopped being the same
// sentence the moment the sixth arrived. An overfull pack reintroduces the exact defect this issue
// is about at the other end of the strip: the member past the right edge of the pack's visible
// window is as unreachable as the picker used to be, and it goes unnoticed because nothing in this
// fixture lays anything out horizontally.
//
// So the budget is read OUT OF THE STYLESHEET and spent by whatever `renderControlBar` really
// leaves visible. Neither half is restated: change a min-width and the budget moves with it, show
// one more member on a view and the bill goes up. A layout engine would say it better — that is
// scene 27 and scene 30 — but a browser is exactly what this repository does not have on every
// run, and "six controls fit in 375px" is arithmetic before it is rendering.

/** Nothing in either stylesheet sets a root font size, so a rem is the browser's default. */
const REM_PX = 16;

/** The owner's screen, and the width every other 375 in this file means. */
const PHONE_PX = 375;

/**
 * One CSS length, in pixels.
 *
 * `calc(0.6rem + env(safe-area-inset-left))` counts as the rem alone: the inset is zero on a phone
 * without a cutout and POSITIVE on one with, so ignoring it can only make this budget optimistic —
 * never wrong in the direction that would let an overfull bar pass.
 */
function lengthPx(token) {
  const rem = /(-?[\d.]+)rem/.exec(token);
  if (rem) {
    return Number(rem[1]) * REM_PX;
  }
  const px = /(-?[\d.]+)px/.exec(token);
  assert.ok(px || /^\s*0\s*$/.test(token), `no length this test can read in ${JSON.stringify(token)}`);
  return px ? Number(px[1]) : 0;
}

/** What a declaration block gives one property, or undefined. Matched as a property, not grepped. */
function cssValue(block, property) {
  const found = new RegExp(`(?:^|[;{\\s])${property}:\\s*([^;}]+)`).exec(block);
  return found ? found[1].trim() : undefined;
}

/**
 * The left and right padding a rule declares, in pixels, shorthand included.
 *
 * Split at the TOP LEVEL, counting parentheses: this file's insets are
 * `calc(0.6rem + env(safe-area-inset-left))`, and a naive split on the first `)` reads the second
 * value as the fourth and quietly prices the wrong edge.
 */
function paddingXPx(block) {
  const declared = cssValue(block, "padding");
  if (declared === undefined) {
    return 0;
  }
  const parts = [];
  let depth = 0;
  let current = "";
  for (const character of declared) {
    if (character === "(") depth += 1;
    if (character === ")") depth -= 1;
    if (/\s/.test(character) && depth === 0) {
      if (current) parts.push(current);
      current = "";
      continue;
    }
    current += character;
  }
  if (current) parts.push(current);
  assert.ok(
    parts.length >= 1 && parts.length <= 4,
    `padding shorthand with ${parts.length} values in ${JSON.stringify(declared)}`
  );
  const right = parts.length >= 2 ? parts[1] : parts[0];
  const left = parts.length >= 4 ? parts[3] : right;
  return lengthPx(right) + lengthPx(left);
}

/**
 * The narrowest box a rule can be drawn in, in pixels.
 *
 * `style.css` sets `* { box-sizing: border-box }`, so a declared min-width already contains the
 * padding and the border. A rule with NO floor at all is a failure rather than a zero: the whole
 * point of this arithmetic is that every member of the strip has a width nothing can take away.
 */
function minBoxPx(selector) {
  const block = cssBlock(selector);
  const floor = cssValue(block, "min-width") || cssValue(block, "width");
  assert.ok(
    floor !== undefined,
    `${selector} declares neither a min-width nor a width, so nothing can say whether the bar fits`
  );
  return lengthPx(floor);
}

/** The view switch, which has no width of its own: it is as wide as the parts inside it. */
function switchPx() {
  const outer = cssBlock(".switch");
  const border = lengthPx((cssValue(outer, "border") || "0").split(/\s+/)[0]) * 2;
  return (
    minBoxPx(".switch-track") +
    minBoxPx(".switch-word") +
    lengthPx(cssValue(outer, "gap")) +
    paddingXPx(outer) +
    border
  );
}

/**
 * What one member of the strip costs, priced by the CLASS THE MARKUP GIVES IT.
 *
 * The widest priced class wins, because a member wearing two — `.bar-button.heavy` — is drawn by
 * both. A member wearing a class nothing here has priced is a failure and not a free control.
 */
function memberPx(id) {
  const className = PAGE_ELEMENTS.get(id).className;
  const byClass = {
    "bar-button": () => minBoxPx(".bar-button"),
    "bar-select": () => minBoxPx(".bar-select"),
    switch: switchPx,
  };
  const priced = className
    .split(/\s+/)
    .filter(Boolean)
    .filter((name) => byClass[name])
    .map((name) => byClass[name]());
  if (priced.length) {
    return Math.max(...priced);
  }
  assert.equal(
    className,
    "",
    `#${id} is on the bar wearing class ${JSON.stringify(className)}, which this budget cannot ` +
      "price — add it to the table above rather than letting an unpriced control onto the strip"
  );
  return minBoxPx(`#${id}`);
}

/**
 * Children of the bar that take NO width from it, because they are not in its flow.
 *
 * Read out of the stylesheet rather than listed by hand: a member that stops being absolutely
 * positioned starts costing the strip real width, and the budget below has to notice that on the
 * same edit rather than on whichever later one happens to look.
 */
function outOfFlow(id) {
  const declared = cssRules(CSS, `#${id}`).join("\n");
  return /^(absolute|fixed)$/.test((cssValue(declared, "position") || "").trim());
}

/** What the bar costs as it is CURRENTLY drawn: the visible members, plus the gaps between them. */
function barCostPx(page) {
  const gap = lengthPx(cssValue(cssBlock("#control-bar"), "gap"));
  const packGap = lengthPx(cssValue(cssBlock("#bar-pack"), "gap"));
  const shown = [];
  let cost = 0;
  for (const member of page.el("control-bar").children) {
    if (outOfFlow(member.id)) {
      continue;
    }
    if (member.id === "bar-pack") {
      const inside = member.children.filter((kid) => !kid.hidden);
      if (!inside.length) {
        continue;
      }
      shown.push(...inside.map((kid) => kid.id));
      cost += inside.reduce((total, kid) => total + memberPx(kid.id), 0);
      cost += packGap * (inside.length - 1);
      continue;
    }
    if (member.hidden) {
      continue;
    }
    shown.push(member.id);
    cost += memberPx(member.id);
  }
  const boxes = page.el("control-bar").children.filter(
    (member) =>
      !member.hidden &&
      !outOfFlow(member.id) &&
      (member.id !== "bar-pack" || member.children.some((k) => !k.hidden))
  ).length;
  return { cost: cost + gap * Math.max(0, boxes - 1), shown };
}

/** The width the bar really has on a 375px phone: the tighter of its two homes. */
function barBudgetPx() {
  const dock = paddingXPx(cssBlock('#control-bar[data-placement="bottom"]'));
  const header = paddingXPx(cssBlock("#topbar"));
  return PHONE_PX - Math.max(dock, header);
}

test("EVERY MEMBER OF THE BAR SAYS WHICH VIEWS IT BELONGS ON, AND NONE IS LEFT TO A DEFAULT", () => {
  // The rule is a TABLE and this is what makes that claim true rather than a description of one
  // hardcoded comparison: a member added to the pack with no line in `PACK_VIEWS` is hidden
  // everywhere, and it fails here by name rather than quietly appearing on a view nobody chose.
  const table = /const PACK_VIEWS = \{([\s\S]*?)\n\};/.exec(SCRIPT_CODE);
  assert.ok(table, "web/voice.js no longer states which views the bar's members belong on");
  const listed = new Set([...table[1].matchAll(/"([\w-]+)":/g)].map((m) => m[1]));
  // The canned buttons are DERIVED from `CANNED_PROMPTS` rather than named here — that is the
  // whole of `#60`'s "a third one is an entry, not a code path" — so the table is read through a
  // real page instead of by eye: what matters is that every member is decided by it.
  const page = newPage();
  for (const member of page.el("bar-pack").children) {
    const decided =
      listed.has(member.id) ||
      CANNED.some((entry) => entry.button === member.id);
    assert.ok(
      decided,
      `#${member.id} is in the pack but in no line of PACK_VIEWS, so nothing decided which view ` +
        "it belongs on"
    );
  }
});

test("THE BAR FITS ON THE OWNER'S 375px PHONE, ON EVERY VIEW", async () => {
  // The measurement `#83` skipped. Six controls went onto one strip and the only thing that had an
  // opinion about whether they fit was a screenshot scene that runs on the CALL view — where the
  // picker is hidden, so it could not see the case that matters.
  const budget = barBudgetPx();
  assert.ok(budget > 300 && budget < PHONE_PX, `the budget arithmetic is broken: ${budget}px`);
  const page = newPage();
  await signIn(page);

  const report = (where, { cost, shown }) =>
    `${where}: ${shown.join(" + ")} need ${cost.toFixed(1)}px of the ${budget.toFixed(1)}px a ` +
    `${PHONE_PX}px phone leaves, so ${Math.ceil(cost - budget)}px of the bar is off the edge — ` +
    "#bar-pack scrolls, so what is off the edge is a control the owner cannot reach";

  // The call view: the gear, the three controls that act on the conversation, and the switch.
  const call = barCostPx(page);
  assert.ok(call.shown.includes("prompts-open"), "this is not the call view's full bar");
  // The slot the tray gave back, spent. `#126 read-new-selector` went onto the strip on the strength
  // of that arithmetic, so it has to be one of the members this budget is actually pricing — and it
  // is a `.bar-button` for the same reason: a `.bar-select` here does not fit.
  assert.ok(call.shown.includes("read-new"), "the read-new control is not on the call view's bar");
  assert.ok(call.cost <= budget, report("the call view", call));

  // The channel view: the same strip with the picker instead, which is the widest single member of
  // the pack. THIS is the composition `#83` added and nothing measured.
  await showDiscord(page, [message({ content: "hi" })]);
  const channel = barCostPx(page);
  assert.ok(channel.shown.includes("discord-channel"), "this is not the channel view's bar");
  assert.ok(channel.cost <= budget, report("the channel view", channel));

  // `#241 voice-composer`. There is no second mode to price any more: writing to the agent happens
  // in a box in the transcript, and the bar is the same strip while you write as before.

  // ...and the arithmetic is not vacuous: the bar as it stood BEFORE the tray — the picker, the
  // Type toggle (a `.bar-button`, until `#241` took it off the strip), and every canned prompt a
  // member in its own right, all on one view — really does overflow. That is the measurement the
  // tray was built for, and stating it here is what stops a later prompt from being put back on
  // the strip on the grounds that one more cannot hurt.
  const packGap = lengthPx(cssValue(cssBlock("#bar-pack"), "gap"));
  const eachOnTheStrip =
    memberPx("open-settings") +
    memberPx("discord-channel") +
    minBoxPx(".bar-button") +
    CANNED.length * minBoxPx(".bar-button") +
    memberPx("view-switch") +
    packGap * (1 + CANNED.length) +
    lengthPx(cssValue(cssBlock("#control-bar"), "gap")) * 2;
  assert.ok(
    eachOnTheStrip > budget,
    `the picker, the toggle and ${CANNED.length} canned prompts need only ${eachOnTheStrip}px of ` +
      `${budget}px between them, so this test would pass with every control on every view and is ` +
      "measuring nothing"
  );
});

test("WALKING BACK THROUGH THE CHANNEL CANNOT MOVE THE PICKER", async () => {
  // The regression half of `#83`, stated as the thing the reader experiences: take several steps
  // back through the history — each one prepending above the viewport, exactly as `#65
  // scrollback-paging` intends — and the picker must be exactly where it was.
  const page = newPage();
  await signIn(page);
  pagedChannel(page, { steps: 5, size: 4, content: (i) => longMessage(`m${i}`) });
  await showDiscord(page, []);
  const area = page.el("scroll-area");
  const picker = page.el("discord-channel");
  assert.equal(picker.hidden, false, "the picker is not on screen in the channel view");

  area.scrollTop = Math.round((area.scrollHeight - area.clientHeight) * 0.5);
  const summaryBefore = page.el("channel-summary").getBoundingClientRect().top;

  for (let step = 0; step < 3; step += 1) {
    await page.el("load-older").click();
    await page.settle();
  }

  assert.equal(page.el("discord-log").children.length, 16, "the walk did not really go back");
  // The control group the picker used to be part of DID recede: the summary sits at the head of
  // the scrolling content, and three steps of prepended history have pushed it far above the
  // viewport. That is the fate the picker escaped, measured rather than asserted by analogy.
  const summaryAfter = page.el("channel-summary").getBoundingClientRect().top;
  assert.ok(
    summaryAfter < summaryBefore - 100,
    `the head of the scrollback only moved ${summaryBefore - summaryAfter}px, so this test is not ` +
      "walking the channel back far enough to say anything"
  );
  // ...and the picker is not in that box at all, which is WHY it did not move. `scrollBox()` is
  // the fixture's own answer to "which scrolling element lays this out", so this is a claim about
  // where the element really is rather than about a number that happens to be equal.
  assert.equal(picker.scrollBox(), null, "the picker is laid out inside the scrolling element");
  assert.equal(
    area.descendants().includes(picker),
    false,
    "the picker is a descendant of #scroll-area, so prepending history moves it"
  );
  assert.equal(picker.parentNode, page.el("bar-pack"), "the picker left the bar");
  assert.equal(picker.hidden, false, "the walk back took the picker off the screen");
});

test("the picker is offered where there is a channel to pick, and nowhere else", async () => {
  // It is the widest member of the pack and it names a channel the call view is not showing, so
  // it belongs to the VIEW rather than to the screen — one line of `PACK_VIEWS`, not a branch.
  const page = newPage();
  await signIn(page);
  assert.equal(page.tab(), "voice");
  assert.equal(page.el("discord-channel").hidden, true, "a channel picker over the call view");

  await showDiscord(page, [message({ content: "hi" })]);
  assert.equal(page.el("discord-channel").hidden, false, "the picker is missing from its own view");

  // `#241 voice-composer`. A draft in the call's message box changes nothing about the bar: the
  // box is in the call's transcript, so it goes when its view goes, and the picker comes back with
  // the channel exactly as it would have without one.
  await page.el("view-switch").click();
  assert.equal(page.tab(), "voice");
  await compose(page, "half a thought");
  assert.equal(page.el("discord-channel").hidden, true, "a channel picker over the call view");
  assert.equal(page.el("view-switch").hidden, false, "a draft took the switch off the bar");
  await page.el("view-switch").click();
  assert.equal(page.tab(), "discord");
  assert.equal(page.el("pane-voice").hidden, true, "the call's box followed the reader off its view");
  assert.equal(page.el("discord-channel").hidden, false, "the picker is missing from its own view");
  await page.el("view-switch").click();
  assert.equal(page.el("compose-text").value, "half a thought", "switching views lost the draft");
  await page.el("view-switch").click();
  assert.equal(page.el("discord-channel").hidden, false);

  // ...and so does the screen rule.
  await page.el("open-settings").click();
  assert.equal(page.el("discord-channel").hidden, true, "the picker followed you into Settings");
  await page.el("close-settings").click();
  assert.equal(page.el("discord-channel").hidden, false, "coming back lost the picker");

  await page.el("view-switch").click();
  assert.equal(page.tab(), "voice");
  assert.equal(page.el("discord-channel").hidden, true, "the picker stayed after leaving its view");
});

// --- the one status line -------------------------------------------------------------------------

test("THE DOCK HOLDS NOTHING BUT THE CONTROL PANE", () => {
  // It used to hold a permanent status row as well, which cost a strip of a phone screen on every
  // frame for a line that is blank most of the time. `#63 status-line-placement` moved it into the
  // body as a transient overlay.
  //
  // Two facts from before the move that MUST survive it, because each was a fix for something the
  // owner photographed:
  //
  //   * state is reported in ONE place. It used to be two — a word under the header and a sentence
  //     at the foot — which is how the closed state announced itself three times in three
  //     vocabularies.
  //   * it is not on the bottom EDGE. On a phone with rounded corners that is exactly where a line
  //     of text is eaten by the curve; the first word of this line went missing on an iPhone 16.
  //     Above the dock is where it was moved to, and above the dock is where it still floats.
  assert.equal(
    PAGE_IDS.has("conversation-state"),
    false,
    "the second status element is still in the page"
  );
  const dock = HTML.slice(HTML.indexOf('id="dock"'));
  assert.doesNotMatch(dock, /id="status-line"/, "the status row is back in the dock");
  assert.ok(dock.includes('id="control-pane"'), "the dock lost the controls too");
  assertMarkupContains("frame-body", "status-line");
  const header = HTML.slice(HTML.indexOf('id="topbar"'), HTML.indexOf("</header>"));
  assert.doesNotMatch(header, /id="status/, "the header must not report status too");
});

test("the status line floats ABOVE the dock rather than reflowing the transcript", () => {
  // An overlay is the one exception to this file's standing "nothing here is positioned" rule, and
  // the exception is the point: a row that appears and disappears resizes #scroll-area, and
  // resizing the scrolling element moves the transcript under the reader's thumb — the very defect
  // `#47 scrollback-stability` exists to remove.
  const line = cssBlock("#status-line");
  assert.match(line, /position:\s*absolute/, "it is a reflowing row again");
  assert.match(line, /bottom:\s*/, "it is not anchored to the foot of the body area");
  assert.match(cssBlock("#frame-body"), /position:\s*relative/, "it has nothing to float against");
  // ...and it is NOT on the bottom edge of the SCREEN: the dock is still a grid row below it.
  const body = cssBlock("body");
  assert.match(body, /grid-template-rows:\s*auto 1fr auto/);
  assert.doesNotMatch(cssBlock("#dock"), /position:\s*(fixed|absolute)/);
  // A status strip pinned to the viewport bottom is what put the line inside the corner curvature
  // of the owner's phone, which ate its first word. Neither sheet may pin it there again.
  for (const [sheet, text] of [["web/voice.css", CSS], ["web/style.css", BASE_CSS]]) {
    assert.doesNotMatch(cssRules(text, "#status").join("\n"), /position:\s*(fixed|absolute|sticky)/,
      `${sheet} pins #status to the viewport again`);
  }
  // The control pane relied on the status row for separation from the dock's border; with the row
  // gone it has to say so itself.
  assert.match(cssBlock("#control-pane"), /padding:\s*[\d.]+rem/, "the controls sit on the border");

  // WHAT THE OVERLAY COSTS, and the bound on it. A pill floating over the foot of the list covers
  // the last line of a channel message for as long as it is up — that is the trade against the
  // reserved row, and it is only acceptable while the reader can still reach what is under it. So
  // the pill lets taps through and only its dismiss control takes one, the same idiom #scroll-tools
  // already uses for the chips over the same corner. Without this, six seconds of "fetching the
  // channel…" also means six seconds in which the reply button under it does nothing.
  assert.match(
    line,
    /pointer-events:\s*none/,
    "the message pill swallows taps meant for the message it is covering"
  );
  assert.match(
    cssBlock("#dismiss-status"),
    /pointer-events:\s*auto/,
    "letting taps through the pill also disabled the way out of it"
  );
});

test("THE STATUS LINE IS A MESSAGE, NOT A FIXTURE", async () => {
  // The issue, in one test. It ships hidden, it appears when there is something to say, and it
  // takes itself away — and taking itself away is a HIDE, not an erase.
  assert.equal(
    PAGE_ELEMENTS.get("status-line").hidden,
    true,
    "the markup ships it visible, so it holds space before anything has happened"
  );

  const page = newPage();
  await signIn(page);
  page.el("status-line").hidden = true; // signing in says "Ready."; start from nothing showing.

  // WHICH ORDER the page does it in, which is not decoration: an `aria-live` region whose element
  // is `display: none` at the moment its text changes announces nothing at all, so the un-hide has
  // to come first. Recorded here rather than asserted after the fact, because after the fact both
  // orders look identical.
  const status = page.el("status");
  let hiddenWhenWritten = null;
  let written = "";
  Object.defineProperty(status, "textContent", {
    configurable: true,
    get: () => written,
    set: (value) => {
      hiddenWhenWritten = page.el("status-line").hidden;
      written = value;
    },
  });

  await page.el("clear-view").click();
  assert.equal(page.el("status-line").hidden, false, "arming Clear said nothing");
  assert.equal(
    hiddenWhenWritten,
    false,
    "the text was written while the element was still hidden, so a screen reader hears nothing"
  );
  assert.match(page.el("status").textContent, /again/);

  assert.equal(page.expireTimers(6000), 1, "no timer was armed to take the message away");
  assert.equal(page.el("status-line").hidden, true, "the message outstayed its welcome");
  assert.match(
    page.el("status").textContent,
    /again/,
    "the dismissal ERASED the text; it is a hide, so what was last said stays readable"
  );
});

test("the transient message can be dismissed by hand, without waiting", async () => {
  const page = newPage();
  await signIn(page);
  await page.el("clear-view").click();
  assert.equal(page.el("status-line").hidden, false);

  await page.el("dismiss-status").click();

  assert.equal(page.el("status-line").hidden, true, "Dismiss did not dismiss");
  assert.equal(page.expireTimers(6000), 0, "the timer was left running after a manual dismissal");
});

test("A LIVE CALL IS STILL LEGIBLE ONCE THE MESSAGE HAS GONE", async () => {
  // This is what justifies retiring the permanent row. Everything the strip used to hold
  // permanently is carried somewhere durable: the live state by the controls, an error by the
  // panel, the close code by the connection detail, a conversation boundary by a seam.
  const page = newPage();
  await startTalking(page);
  assert.equal(page.el("status-line").hidden, false, "a connected call said nothing at all");

  page.expireTimers(6000);

  assert.equal(page.el("status-line").hidden, true);
  assert.equal(page.el("talk-label").textContent, "Listening", "nothing says the call is live");
  assert.match(page.el("talk").className, /\blive\b/);
  assert.equal(page.el("hang-up").hidden, false, "nothing says there is a call to hang up");
  // ...and a failure is NOT transient: it stays until it is fixed.
  page.sockets[0].onerror({});
  page.sockets[0].onclose({ code: 1006, reason: "" });
  page.expireTimers(6000);
  assert.equal(page.el("error").hidden, false, "the failure panel went away with the toast");
});

test("the channel says what it is showing INLINE, where it scrolls away", async () => {
  // `#63 status-line-placement` names this line specifically: it is a standing fact about what you
  // are looking at, and it was living on a strip that is now a message with a six-second life.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "1" }), message({ id: "2" })]);

  const summary = page.el("channel-summary");
  assert.equal(summary.children.length, 1, "the channel states no summary at all");
  assert.equal(summary.children[0].className, "seam", "it is not drawn as a seam");
  assert.match(channelSummaryText(page), /lead team/, "it does not name the channel");
  // Inside the scrolling element, so it scrolls off — that is the whole point of moving it.
  assertMarkupContains("scroll-area", "channel-summary");
  // And the explanation is one tap inside it, measured exactly like the transcript's two seams.
  const spent = words(seamDetail(summary.children[0]));
  assert.ok(spent >= SEAM_DETAIL_MIN_WORDS, `the channel seam explains nothing: ${spent} words`);
  assert.ok(spent <= SEAM_DETAIL_MAX_WORDS, `${spent} words behind a tap is an essay`);
  // It is not ALSO on the strip: two mechanisms saying the same thing is what this page keeps
  // removing.
  assert.doesNotMatch(page.el("status").textContent, /lead team/);
});

test("the horizontal safe-area insets are applied, not only the bottom one", () => {
  // NOT VERIFIABLE BY SCREENSHOT: no browser automation can make Chromium report a non-zero inset,
  // and a headless browser renders a rectangle with no curved corners at all. This asserts the
  // declaration, which is the only part that can be checked here.
  // `#58 control-bar` added the fifth: in the dock the bar is not inside anything that already
  // applies the insets, so without its own the same curve that ate the first word of the status
  // line would eat the gear.
  for (const selector of [
    "#topbar",
    "#status-line",
    "#control-pane",
    "#scroll-area",
    '#control-bar[data-placement="bottom"]',
  ]) {
    const block = cssBlock(selector);
    assert.match(block, /env\(safe-area-inset-left\)/, `${selector} ignores the left inset`);
    assert.match(block, /env\(safe-area-inset-right\)/, `${selector} ignores the right inset`);
  }
});

test("the controls SHOW their state, not merely hold it", () => {
  // Each of these was a mutation that behaviour tests alone let through: the page can be perfectly
  // correct about what it will do and still look identical while doing it. That is the whole class
  // of defect the owner photographed — a control that is dead and looks alive.

  // The status dot must actually differ between states, or the one status line is just a sentence.
  const dot = (state) => cssBlock(`#status-line[data-state="${state}"] .status-dot`);
  const colour = (block) => /background:\s*([^;]+);/.exec(block)[1].trim();
  // `#54 resume-recovery` made this four rather than three. A suspension that shares a colour with
  // either an ended call or an error is a state the eye cannot tell from the thing it is NOT.
  const shades = new Set([
    colour(cssBlock(".status-dot")),
    colour(dot("live")),
    colour(dot("error")),
    colour(dot("suspended")),
  ]);
  assert.equal(
    shades.size,
    4,
    `idle, live, error and suspended must look different: ${[...shades]}`
  );
  // And the token really has a value in BOTH schemes: a dot that is only defined for dark mode
  // renders as nothing at all in light mode, which is worse than the wrong colour.
  for (const block of [cssBlock(":root"), cssBlockIn("(prefers-color-scheme: light)", ":root")]) {
    assert.match(block, /--paused:\s*#[0-9a-f]{6}/i, "the suspended colour is missing a scheme");
  }

  // An armed Clear that looks like an unarmed one is not a confirmation.
  const armed = cssBlock("#clear-view.armed");
  assert.match(armed, /var\(--warn\)/, "the armed state must be visibly different, not just named");

  // A control that cannot act must not keep the loudest fill on the screen.
  const off = cssBlock(".control[disabled]");
  assert.doesNotMatch(off, /background:\s*var\(--warn\)/, "a dead control kept its warm fill");
  assert.match(off, /background:\s*var\(--panel\)/);
});

// --- the desktop composition ----------------------------------------------------------------
//
// `#55 voice-desktop-app`. The fixture lays nothing out, so it cannot say the desktop composition
// LOOKS right — that is what the two desktop screenshot profiles are for. What it can say is which
// query the rules are in, that the phone is not affected by them, and that the width the reader
// chose is really carried, clamped and stored. Everything below is one of those three.

test("the desktop layout is chosen by CAPABILITY, and no user-agent string decides anything", () => {
  // The requirement, stated as the two questions a layout actually needs answered: is there room
  // for a column with margin beside it, and can the input device put a cursor on a nine-pixel
  // handle. A UA string answers neither — a tablet with a trackpad and a phone in desktop mode
  // both lie to it — and it is the thing that rots, because the strings keep changing.
  assert.ok(
    CSS_CODE.includes(`@media ${DESKTOP_QUERY}`),
    `web/voice.css declares no "@media ${DESKTOP_QUERY}" regime`
  );
  for (const [name, text] of [
    ["web/voice.js", SCRIPT_CODE],
    ["web/src/ modules.js", MODULE_CODE],
    ["web/voice.css", CSS_CODE],
    ["web/voice.html", HTML_CODE],
  ]) {
    assert.doesNotMatch(text, /userAgent|userAgentData/, `${name} sniffs the user agent`);
    // User-facing device-speech setup instructions may name Android. Quoted copy is not a
    // layout decision; a navigator.userAgent read remains forbidden by the assertion above.
    const layoutCode = name.endsWith(".js") ? withoutStrings(text) : text;
    assert.doesNotMatch(
      layoutCode,
      /\b(iPhone|iPad|Android|Macintosh|Windows NT)\b/,
      `${name} names a device or an operating system to decide a layout`
    );
  }
  // And the page does not decide the layout in script either: which regime is in force is the
  // stylesheet's business, so there is nothing here to disagree with it.
  assert.doesNotMatch(PAGE_CODE, /matchMedia/, "web/voice.js grew a second opinion about layout");
});

test("on a desktop both lists are held to a reading column, and the phone is left alone", () => {
  // A line stops being readable somewhere past ninety characters; the captured desktop frame was
  // running to about a hundred and eighty. Both panes, because the channel is read the same way
  // the transcript is.
  for (const pane of ["#pane-voice", "#pane-discord"]) {
    const desktop = cssBlockIn(DESKTOP_QUERY, pane);
    assert.match(desktop, /max-width:\s*var\(--reading-width\)/, `${pane} is not held to a column`);
    assert.match(desktop, /margin-inline:\s*auto/, `${pane} is capped but not centred`);
    // The COLUMN, outside the desktop query, is what must not happen. `#241 voice-composer` gave the
    // call's pane a rule of its own on every screen — a flex column at least as tall as the list, so
    // its message box can sit at the foot — and that is not a column; a width or an inline margin
    // there would be.
    const elsewhere = cssRules(cssWithout(DESKTOP_QUERY), pane).join("\n");
    assert.doesNotMatch(
      elsewhere,
      /(?:^|[;\s])(?:max-width|width|margin-inline|margin-left|margin-right|margin)\s*:/,
      `${pane} is given a width or a margin outside the desktop query, so the phone gets the column too`
    );
  }
  // The dock is never pinned: the frame test above asserts the pane is still a grid row; this is the
  // other half. `#218 desktop-dock` held the DOCK to the column, as its padding; `#406
  // command-bar-one-row` spans it across the window instead, at the owner's word: "keep that bar one
  // row that can go up to full width on desktop". The inset is stated once, as `--dock-inset`, because
  // the switch and the prompts tray are placed by it as well as the dock's padding, and it is the
  // window's less the controls' own inset, not the reading column's.
  const dock = cssBlockIn(DESKTOP_QUERY, "#dock");
  assert.equal(cssValue(dock, "--dock-inset"), "0.6rem", "the dock is held to a column narrower than the window");
  assert.doesNotMatch(dock, /reading-width/, "the dock is still sized by the reading column");
  assert.match(
    cssValue(dock, "padding") || "",
    /var\(--dock-inset\)/,
    "the dock's padding is not its inset"
  );
  assert.doesNotMatch(cssBlock("#dock"), /position:\s*(fixed|absolute)/);
  assert.doesNotMatch(cssBlock("#control-pane"), /position:\s*(fixed|absolute)/);
  // ...and neither half sets a column of its own any more, which would be a second column inside
  // the first and stop the pane sitting beside the bar.
  // (`#406 command-bar-one-row` caps the pane at what the dock leaves beside the bar's floor and the
  // switch, which is no column: the room it may wrap its buttons in, as a last resort.)
  for (const half of ["#control-pane", '#control-bar[data-placement="bottom"]']) {
    assert.doesNotMatch(
      cssBlockIn(DESKTOP_QUERY, half),
      /max-width:(?!\s*calc\(100% - var\(--bar-floor\))/,
      `${half} is held to a column of its own inside the dock's`
    );
  }
  assert.match(cssBlockIn(DESKTOP_QUERY, "#status-line"), /max-width:\s*var\(--reading-width\)/);
  // The width is a token with a default, so a browser that never enters the regime still parses.
  assert.match(cssBlock(":root"), /--reading-width:\s*\d+ch/, "the column has no default width");
});

test("the reading-width handle is a sibling of the scroll area, not a passenger inside it", () => {
  // Inside #scroll-area it would be faded by that element's mask and would scroll away with the
  // content; inside #control-pane it would break the assertion that the pane is a plain grid row.
  // So it is a sibling, positioned against #screen-main, which already carries `position:
  // relative` for the chips.
  assertMarkupContains("screen-main", "width-grip");
  const grip = markupPlace("width-grip");
  const area = markupPlace("scroll-area");
  assert.equal(grip.indent, area.indent, "the handle is nested somewhere it does not belong");
  const pane = HTML.slice(HTML.indexOf('id="control-pane"'));
  assert.ok(!pane.includes('id="width-grip"'), "the handle ended up inside the control pane");
  // Absent entirely outside the desktop regime: there is no column to resize on a phone.
  assert.match(
    cssBlockOutside(DESKTOP_QUERY, "#width-grip"),
    /display:\s*none/,
    "the handle shows where it cannot be used"
  );
  assert.match(cssBlockIn(DESKTOP_QUERY, "#width-grip"), /cursor:\s*ew-resize/);
});

// --- the dock on a desk ---------------------------------------------------------------------
//
// `#218 desktop-dock`. The owner, at his desk: the dock built for a thumb "looks very weird for
// desktop" — Read and Pace HUGE, the channel and view pickers cut short beside empty space, and
// the whole dock spending vertical space the list needs. The fixture lays nothing out, so what it
// can say is that the desktop dock is declared inside the desktop query and only there, at a
// desk's sizes, and that the phone's declarations are still the phone's.
// tests/offline_cache_browser.py measures the result in Chromium at 1280x800 and 1600x1000, and
// measures the phone dock at 412 and 360 against what it was.

/** How tall a control in the desktop dock may be, in px: a mouse's 32px target, short of a slab. */
const DESK_CONTROL_PX = [32, 40];

test("on a desk the dock is ONE ROW of ordinary-sized controls, the pickers giving way before it wraps", () => {
  // `#406 command-bar-one-row`. It wrapped the pane onto a second row when the pickers and it did not
  // fit the column: "Thread list" or a long thread's name did that, beside half a window of nothing.
  // tests/command_bar_browser.py measures the row in Chromium at 1280x800 and 1920x1080.
  const dock = cssBlockIn(DESKTOP_QUERY, "#dock");
  assert.match(dock, /display:\s*flex/, "the dock is still two stacked bands on a desk");
  assert.match(dock, /flex-wrap:\s*nowrap/, "the dock still wraps onto a second row on a desk");
  // The bar takes the room the pane leaves, and gives way down to its floor: the pane keeps its width, a
  // row at the right of the bar, not a tile. Only with the bar at its floor (a large default font after
  // a call, in the narrowest desk) does the pane wrap its own buttons, rather than run under the bar.
  assert.match(cssBlockIn(DESKTOP_QUERY, "#control-bar-bottom"), /flex:\s*1 1 auto/);
  assert.match(cssBlockIn(DESKTOP_QUERY, "#control-bar-bottom"), /min-width:\s*var\(--bar-floor\)/,
    "the bar can be squeezed to nothing under the pane");
  assert.match(cssValue(dock, "--bar-floor") || "", /^[\d.]+rem$/);
  const pane = cssBlockIn(DESKTOP_QUERY, "#control-pane");
  assert.match(pane, /display:\s*flex/, "the pane is still the phone's tile on a desk");
  assert.match(pane, /flex:\s*0 0 auto/, "the pane shrinks, so its buttons are squeezed instead of the pickers");
  assert.match(pane, /max-width:\s*calc\(100% - var\(--bar-floor\) - var\(--dock-switch\)/,
    "the pane can grow over the bar's floor or the switch's room");
  assert.match(pane, /flex-wrap:\s*wrap/, "the pane has no last resort but to run under the bar");
  assert.match(pane, /margin-left:\s*auto/, "the pane is not held at the right of its row");
  // Every control at a desk's size: no smaller than a mouse's target, and nowhere near the
  // phone's 44px squares or the slabs the owner photographed.
  for (const selector of [".bar-button", "#control-pane .control", ".bar-select", ".switch"]) {
    const height = lengthPx(cssValue(cssBlockIn(DESKTOP_QUERY, selector), "min-height") || "");
    assert.ok(
      height >= DESK_CONTROL_PX[0] && height <= DESK_CONTROL_PX[1],
      `${selector} is ${height}px tall on a desk, outside ${DESK_CONTROL_PX.join("-")}px`
    );
  }
  assert.ok(
    lengthPx(cssValue(cssBlockIn(DESKTOP_QUERY, ".bar-button"), "min-width")) >= DESK_CONTROL_PX[0],
    "an icon-only bar button is narrower than a mouse's target"
  );
  // The word BESIDE the icon and at reading size, not the phone's 0.55rem capitals under it.
  assert.match(cssBlockIn(DESKTOP_QUERY, ".bar-button"), /flex-direction:\s*row/);
  assert.match(cssBlockIn(DESKTOP_QUERY, "#control-pane .control"), /grid-template-columns:\s*auto auto/);
  for (const selector of [".mini-label", ".control-label"]) {
    const block = cssBlockIn(DESKTOP_QUERY, selector);
    assert.ok(
      lengthPx(cssValue(block, "font-size")) >= 0.8 * REM_PX,
      `${selector} is a caption on a desk, not a label`
    );
    assert.match(block, /text-transform:\s*none/, `${selector} still shouts in capitals on a desk`);
  }
});

test("on a desk the pickers show their names, and the channel's holds longest", () => {
  const select = cssBlockIn(DESKTOP_QUERY, ".bar-select");
  // Sized to the option SHOWN, so "All" stops reserving the width of the longest thread title.
  assert.match(select, /field-sizing:\s*content/);
  const phoneCap = lengthPx(cssValue(cssBlockOutside(DESKTOP_QUERY, ".bar-select"), "max-width"));
  const deskCap = lengthPx(cssValue(select, "max-width"));
  assert.ok(deskCap > phoneCap, `the pickers are capped at ${deskCap}px on a desk, no wider than a phone's`);
  // A name that does not fit says so.
  assert.match(select, /text-overflow:\s*ellipsis/, "a cut name is clipped mid-letter instead of ending in …");
  // Thirty characters of a 16px face at a generous 0.6em each, plus the padding and the arrow:
  // room for the name the owner's picker cut off at nine rem.
  const channel = cssBlockIn(DESKTOP_QUERY, "#discord-channel");
  const cap = /max-width:\s*min\(([\d.]+rem),\s*100%\s*-([^;]+)\)/.exec(channel);
  assert.ok(cap, "the channel picker is not capped at what the pack leaves it");
  assert.ok(
    lengthPx(cap[1]) >= 30 * 0.6 * REM_PX + 40,
    `the channel picker is capped at ${cap[1]}, short of a thirty-character name`
  );
  // When the row is short the VIEW picker gives way first. The channel's does not shrink at all
  // — any share of the shrinking is a fraction of a pixel the ellipsis turns into a visible "…" —
  // and what it leaves the rest of the pack is at least the view picker's own floor.
  assert.match(channel, /flex-shrink:\s*0/, "the channel's name gives way alongside the view's");
  const floor = lengthPx(cssValue(cssBlockOutside(DESKTOP_QUERY, ".bar-select"), "min-width"));
  const left = cap[2].split(/\s+-\s+/).reduce((total, term) => {
    const [, times, length] = /^\s*(?:([\d.]+)\s*\*\s*)?(.+?)\s*$/.exec(term);
    return total + Number(times || 1) * lengthPx(length);
  }, 0);
  assert.ok(left >= floor, `the channel picker leaves ${left}px of the pack, less than the view picker's ${floor}px`);
  // ...and the desk does not take the sixteen pixels away: an iPad with a trackpad is a desk here.
  assert.doesNotMatch(select, /font-size/, "the desktop pickers shrink their type below 16px");
});

test("the desktop dock leaves the phone's declarations as they were", () => {
  // Every selector the desktop dock introduced exists inside the desktop query and nowhere else,
  // so none of it can reach a phone.
  for (const selector of [
    "#control-bar-bottom",
    "#discord-channel",
    "#control-pane .control",
    "#control-pane .control > svg",
    "#control-pane .control-note",
    "#control-pane:has(#read-aloud:not([hidden])) #speaker",
    "#control-pane button:focus-visible",
    '#control-bar[data-placement="bottom"] #view-switch',
    "#control-bar-bottom:has(#view-switch:not([hidden])) + #control-pane",
    '#control-bar[data-placement="bottom"] .switch-word',
    "#control-pane .control:has(> .control-note:not([hidden])) > svg",
  ]) {
    assert.ok(cssRules(mediaBody(DESKTOP_QUERY), selector).length > 0, `${selector} is gone from the desk`);
    assert.equal(
      cssRulesElsewhere(DESKTOP_QUERY, selector),
      0,
      `${selector} is also declared outside the desktop query, where a phone gets it`
    );
  }
  // And the selectors it overrides still say what they said for everybody: two bands, a 3x2 tile
  // of large buttons, a captioned icon, and a picker capped narrow — the dock the phone has.
  const everybody = (selector) => cssRules(CSS_UNCONDITIONAL, selector).join("\n");
  assert.doesNotMatch(everybody("#dock"), /display:|flex-wrap/, "the phone's dock became a flex row");
  assert.match(everybody("#control-pane"), /display:\s*grid/);
  assert.match(everybody("#control-pane"), /grid-template-columns:\s*3\.5rem 1fr 1fr/);
  assert.match(everybody(".control"), /flex-direction:\s*column/);
  assert.match(everybody(".bar-button"), /flex-direction:\s*column/);
  assert.match(everybody(".mini-label"), /text-transform:\s*uppercase/);
  assert.doesNotMatch(everybody(".bar-select"), /field-sizing/, "the phone's pickers lost their fixed caps");
});

test("the pane's markup runs in the order its buttons are seen, so Tab follows the eye", () => {
  // The phone tile places each button by explicit column, so it never depended on the markup; the
  // desk lays the pane out as a row in markup order. Read used to be declared ahead of the filter
  // and the pace, which on a desk would have put it first — or, moved with `order`, have sent Tab
  // the opposite way to the eye.
  const pane = HTML_CODE.slice(HTML_CODE.indexOf('id="control-pane"'));
  const at = (id) => pane.indexOf(`id="${id}"`);
  for (const order of [
    ["todo-filter", "read-speed", "read-aloud", "talk"],
    ["speaker", "clear-view", "hang-up", "talk"],
  ]) {
    for (let i = 1; i < order.length; i += 1) {
      assert.ok(at(order[i - 1]) > -1 && at(order[i - 1]) < at(order[i]),
        `#${order[i - 1]} is not declared before #${order[i]}`);
    }
  }
  assert.doesNotMatch(
    mediaBody(DESKTOP_QUERY),
    /(?:^|[;{\s])order:/,
    "a desktop rule reorders the dock, so Tab and the eye disagree about what comes next"
  );
  // Sound acts on a call's voice. Without a call the channel view shows Hide read, Pace and Read,
  // and on a desk nothing is left to cover Sound the way the phone tile's shared cell does.
  assert.match(
    cssBlockIn(DESKTOP_QUERY, "#control-pane:has(#read-aloud:not([hidden])) #speaker"),
    /display:\s*none/
  );
});

test("on a desk the view switch has one place, and the pane keeps the room for it", () => {
  // The switch rode along with the row: between the bar and the pane when they shared one, at the end
  // of the first row when they did not — about 265px sideways and a row up between the call and the
  // channel, so a second click where the first had landed met an empty dock. It is set in the
  // dock's lower right corner now — the window's, since `#406 command-bar-one-row` — the same width
  // whatever word it shows.
  const flick = cssBlockIn(DESKTOP_QUERY, '#control-bar[data-placement="bottom"] #view-switch');
  assert.match(flick, /position:\s*absolute/, "the switch is still carried along by the row");
  assert.match(flick, /right:\s*var\(--dock-inset\)/, "the switch is not at the dock's right edge");
  assert.match(flick, /bottom:\s*0\.4rem/, "the switch is not at the foot of the dock");
  assert.equal(
    cssValue(flick, "bottom"),
    cssValue(cssBlockIn(DESKTOP_QUERY, "#dock"), "padding").split(/\s+/)[0],
    "the switch's foot is not the dock's last row"
  );
  assert.match(flick, /width:\s*var\(--dock-switch\)/, "the switch's width follows its word, so its edge moves");
  // Its `ch`-based column measured in the page's type, not in its own 0.9rem: see the rule.
  assert.match(flick, /font-size:\s*inherit/);
  assert.match(cssBlockIn(DESKTOP_QUERY, '#control-bar[data-placement="bottom"] .switch-word'),
    /text-overflow:\s*ellipsis/, "a wider face can push the word out of a fixed-width switch");
  // Positioned against the DOCK: the bar is static on a desk, the dock is not. The tray, which hung
  // off the bar, hangs off the dock across the column.
  assert.match(cssBlockIn(DESKTOP_QUERY, "#dock"), /position:\s*relative/, "the switch has nothing to stand in");
  assert.match(cssBlockIn(DESKTOP_QUERY, '#control-bar[data-placement="bottom"]'), /position:\s*static/);
  const tray = cssBlockIn(DESKTOP_QUERY, '#control-bar[data-placement="bottom"] #prompts-tray');
  assert.match(tray, /left:\s*var\(--dock-inset\)/, "the tray spans the window rather than the column");
  assert.match(tray, /right:\s*var\(--dock-inset\)/, "the tray spans the window rather than the column");
  // The room: the pane's right margin is the switch's width and a gap, and only while the switch is
  // up (text mode takes it down, and the pane does not hold an empty corner for it).
  const room = cssBlockIn(DESKTOP_QUERY, "#control-bar-bottom:has(#view-switch:not([hidden])) + #control-pane");
  assert.match(room, /margin-right:\s*calc\(var\(--dock-switch\) \+ [\d.]+rem\)/, "the pane keeps no room for the switch");
  assert.match(cssValue(cssBlockIn(DESKTOP_QUERY, "#dock"), "--dock-switch") || "", /^[\d.]+rem$/);
  // ...and it is the pane that keeps it, the last thing in the dock and so on its last row.
  const dock = HTML_CODE.slice(HTML_CODE.indexOf('id="dock"'), HTML_CODE.indexOf("</footer>"));
  assert.ok(dock.indexOf('id="control-pane"') > dock.indexOf('id="control-bar-bottom"'));
  // Still no `order`: Tab reaches the switch where the markup has it, at the end of the bar and
  // before the pane, as it does on a phone and in the header. tests/offline_cache_browser.py walks it.
  assert.doesNotMatch(mediaBody(DESKTOP_QUERY), /(?:^|[;{\s])order:/);
  // The pace popover opens above the dock at the column's edge, not over the row above the pane.
  assert.match(cssBlockIn(DESKTOP_QUERY, "#control-pane"), /position:\s*static/);
  assert.match(cssBlockIn(DESKTOP_QUERY, ".popover"), /right:\s*var\(--dock-inset\)/);
});

test("on a desk a pane button's word stands level with its icon", () => {
  // The icon spanned two grid lines in every button so that Talk's note could go under its word; in
  // every other button the second line was empty and still took a pixel or two of the icon's height,
  // standing the word that much above the icon. It spans two only when there is a note.
  assert.match(cssBlockIn(DESKTOP_QUERY, "#control-pane .control > svg"), /grid-row:\s*1;/);
  assert.match(
    cssBlockIn(DESKTOP_QUERY, "#control-pane .control:has(> .control-note:not([hidden])) > svg"),
    /grid-row:\s*1 \/ span 2/
  );
  assert.match(cssBlockIn(DESKTOP_QUERY, "#control-pane .control"), /align-items:\s*center/);
});

test("every control in the desktop dock shows keyboard focus, and the pack has room for the ring", () => {
  const body = mediaBody(DESKTOP_QUERY);
  assert.match(body, /#control-bar :is\(button, select\):focus-visible/, "the bar's controls have no ring");
  const focus = cssRules(body, "#control-pane button:focus-visible").join("\n");
  assert.match(focus, /outline:\s*2px solid var\(--accent\)/, "the pane's buttons have no ring");
  assert.doesNotMatch(body, /outline:\s*(none|0)\b/, "a desktop rule takes a focus ring away");
  // The pack scrolls sideways, and a scrolling box clips anything drawn outside it. Its padding is
  // the room the ring is drawn in; the equal negative margin gives that room back to the row.
  const ring = 2 + lengthPx(cssValue(focus, "outline-offset"));
  const pack = cssBlockIn(DESKTOP_QUERY, "#bar-pack");
  const room = lengthPx(cssValue(pack, "padding"));
  assert.ok(room >= ring, `the pack keeps ${room}px for a ${ring}px ring, so its edge clips it`);
  assert.equal(lengthPx(cssValue(pack, "margin")), -room, "the ring's room moved the bar");
});

test("the width the reader chose survives a reload", async () => {
  const store = new Map();
  const page = newPage(store);
  await signIn(page);
  assert.equal(page.documentElement.style.getPropertyValue("--reading-width"), "72ch");

  page.el("reading-width").value = "54";
  await page.el("reading-width").dispatch("input");
  assert.equal(page.documentElement.style.getPropertyValue("--reading-width"), "54ch");
  assert.match(page.el("reading-width-state").textContent, /Saved/);

  // A RELOAD: same storage, brand new execution of the same script.
  const reloaded = newPage(store);
  assert.equal(
    reloaded.documentElement.style.getPropertyValue("--reading-width"),
    "54ch",
    "the column went back to the default, so the choice was not really kept"
  );
  assert.equal(reloaded.el("reading-width").value, "54", "the slider does not show what is in force");
});

test("a browser that refuses to store the reading width says so instead of claiming saved", async () => {
  // Private browsing accepts setItem and stores nothing. The same trap the token and the
  // microphone settings already read back for, and the same answer.
  const page = newPage();
  await signIn(page);
  page.storage.set = () => page.storage;

  page.el("reading-width").value = "90";
  await page.el("reading-width").dispatch("input");

  const said = page.el("reading-width-state").textContent;
  assert.match(said, /refused to store/, `it claimed a save it did not get: ${said}`);
  assert.doesNotMatch(said, /^Saved/);
  // It still APPLIES, though — refusing to remember it is not a reason to refuse to do it.
  assert.equal(page.documentElement.style.getPropertyValue("--reading-width"), "90ch");
});

test("a stored width outside the range is clamped, not applied", () => {
  // Storage is shared with everything else on this origin, survives a version of the page with
  // different limits, and people edit it by hand. A stored 4000 must become a column, not a window
  // with no margins at all.
  const cases = [
    ["4000", "120ch", "an absurd width was applied verbatim"],
    ["3", "45ch", "a width narrower than a sentence was applied verbatim"],
    ["wider please", "72ch", "a value that is not a number became NaN rather than the default"],
    ["", "72ch", "an empty entry became a zero-width column"],
    ["63.4", "63ch", "a fractional width was not rounded to whole characters"],
  ];
  for (const [stored, expected, complaint] of cases) {
    const page = newPage(new Map([["vibe-talk.voice.width", stored]]));
    assert.equal(
      page.documentElement.style.getPropertyValue("--reading-width"),
      expected,
      `${complaint} (stored ${JSON.stringify(stored)})`
    );
  }
});

test("the handle works without a mouse: arrow keys move it and it says where it is", async () => {
  // It is a `separator` with a tabindex, and a separator with a tabindex is only a control if the
  // keyboard can move it. The aria value is what a screen reader reads out, so it has to follow.
  const page = newPage();
  await signIn(page);
  const grip = page.el("width-grip");
  assert.equal(grip.getAttribute("aria-valuenow"), "72");

  await grip.dispatch("keydown", { key: "ArrowRight" });
  assert.equal(page.documentElement.style.getPropertyValue("--reading-width"), "73ch");
  assert.equal(grip.getAttribute("aria-valuenow"), "73");

  await grip.dispatch("keydown", { key: "PageDown" });
  assert.equal(page.documentElement.style.getPropertyValue("--reading-width"), "63ch");

  await grip.dispatch("keydown", { key: "End" });
  assert.equal(page.documentElement.style.getPropertyValue("--reading-width"), "120ch");
  await grip.dispatch("keydown", { key: "Home" });
  assert.equal(page.documentElement.style.getPropertyValue("--reading-width"), "45ch");

  // A key it does not own is left alone rather than swallowed.
  await grip.dispatch("keydown", { key: "a" });
  assert.equal(page.documentElement.style.getPropertyValue("--reading-width"), "45ch");
  assert.equal(page.storage.get("vibe-talk.voice.width"), "45", "the keyboard change was not kept");
});

test("dragging the handle sets the column from where the pointer actually is", async () => {
  // The fixture has no layout engine, so the geometry has to be STATED: a 1200px body and a
  // 576px column at the default 72 characters, which makes a character eight pixels wide. What is
  // being checked is the arithmetic the page does with those numbers — the column is centred, so
  // the handle's distance from the middle is half the width — not that it looks right, which is
  // what the desktop screenshot profiles are for.
  const page = newPage();
  await signIn(page);
  page.el("screen-main").clientWidth = 1200;
  page.el("pane-voice").clientWidth = 576;
  const grip = page.el("width-grip");

  await grip.dispatch("pointerdown", { pointerId: 1, clientX: 888 });
  await grip.dispatch("pointermove", { pointerId: 1, clientX: 800 });

  // (800 - 600) * 2 / 8 = 50 characters.
  assert.equal(page.documentElement.style.getPropertyValue("--reading-width"), "50ch");
  assert.equal(
    page.storage.has("vibe-talk.voice.width"),
    false,
    "a drag wrote to storage on every frame instead of once at the end"
  );

  await grip.dispatch("pointerup", { pointerId: 1 });
  assert.equal(page.storage.get("vibe-talk.voice.width"), "50", "the drag was not kept");

  // A move with no drag in progress must do nothing: the pointer crosses this handle constantly.
  await grip.dispatch("pointermove", { pointerId: 1, clientX: 1100 });
  assert.equal(page.documentElement.style.getPropertyValue("--reading-width"), "50ch");
});

test("content fades under the header rather than being sliced off square", () => {
  const scroll = cssBlock("#scroll-area");
  assert.match(scroll, /mask-image:\s*linear-gradient/, "the top edge has no fade");
  assert.match(scroll, /-webkit-mask-image:\s*linear-gradient/, "Safari needs the prefixed one");
});

// --- `#406 command-bar-one-row` -------------------------------------------------------------------

test("`#406 command-bar-one-row`: the view picker's tooltip names the open thread WHOLE, which its option cuts", async () => {
  // On a desk the bar no longer wraps: a label too long for it ends in an ellipsis, and the option is
  // cut at 28 characters of name besides. The tooltip is where the whole name is.
  const long = "Release train for the autumn platform update: canary widening, staging migration check, cache headers, retry wrapper fix";
  assert.equal(long.length, 120);
  const page = newPage();
  const data = threadData();
  data.threads[0].title = long;
  data.threads[0].root.content = long;
  page.threadingSupported = true;
  page.threads = data.threads;
  await signIn(page);
  await showDiscord(page, data.messages);
  const picker = page.el("thread-select");
  const general = picker.title;
  assert.doesNotMatch(general, /Release train/, "the tooltip names a thread nobody has opened");
  await pickThread(page, `thread:${data.threads[0].id}`);
  const option = picker.children.find((one) => one.value === picker.value);
  assert.ok(option && option.textContent.endsWith("…"), `the premise is an option cut short: ${option && option.textContent}`);
  assert.equal(picker.title, `${long}\n${general}`, "the tooltip does not name the open thread whole");
  // Back to Main: the tooltip names no thread.
  await pickThread(page, "main");
  assert.equal(picker.title, general, "the tooltip still names a thread after leaving it");
});

// --- a page running an older script than the server serves says so (#529 page-build-stale-notice) --------
//
// The owner, 2026-10-26, of a thread card drawn the old way: "Maybe I'm running a stale version". A page
// left open across a deploy keeps its old script; every timeline read now says which one the server serves.

test("`#529 page-build-stale-notice`: a read saying the server serves another page script shows New version — reload, which reloads only on a press", async () => {
  const page = newPage(new Map(), SCRIPT, (p) => { p.scriptSrc = "https://vibe.example/voice.js?v=aaaaaaaa11111111"; });
  const data = bigCardData();
  page.threadingSupported = true;
  page.threads = data.threads;
  page.messages = data.messages;
  const server = forwardTimeline(page);
  const answer = server.answer;
  let build = "aaaaaaaa11111111";
  server.answer = async (path) => {
    const response = await answer(path);
    return json(response.status, { ...JSON.parse(await response.text()), page_build: build });
  };
  await signIn(page);
  await showDiscord(page, data.messages);
  await page.settle();
  // The same build: nothing shown, and Settings names it.
  assert.equal(page.el("page-update").hidden, true, "the page offered a reload of the build it runs");
  assert.equal(page.el("settings-page-build").textContent, "This page is build aaaaaaaa.");
  // A deploy: the next read says another build.
  build = "bbbbbbbb22222222";
  await reReadChannel(page);
  assert.equal(page.el("page-update").hidden, false, "a page older than the server's did not say so");
  assert.match(page.el("page-update").textContent, /New version/);
  assert.match(page.el("settings-page-build").textContent, /build aaaaaaaa; the server now serves bbbbbbbb/);
  assert.equal(page.reloads || 0, 0, "the page reloaded by itself");
  await page.el("page-update").click();
  assert.equal(page.reloads, 1, "the chip did not reload the page");
  // A page that does not know its own build — loaded without a hashed address — never claims to be old.
  const unknown = newPage(new Map());
  unknown.threadingSupported = true;
  unknown.threads = data.threads;
  unknown.messages = data.messages;
  const other = forwardTimeline(unknown);
  const plain = other.answer;
  other.answer = async (path) => json(200, { ...JSON.parse(await (await plain(path)).text()), page_build: "cccccccc33333333" });
  await signIn(unknown);
  await showDiscord(unknown, data.messages);
  await unknown.settle();
  assert.equal(unknown.el("page-update").hidden, true, "a page with no build of its own offered a reload");
});

test("`#624 voice-page-modularity`: a page knows its build from the page build's hashed script name as it does from an older ?v= address, and from nothing else", () => {
  const buildOf = (src, script = SCRIPT) => newPage(new Map(), script, (p) => { p.scriptSrc = src; }).pure("PAGE_BUILD");
  const hash = "0123456789abcdef0123456789abcdef";
  assert.equal(buildOf(`https://vibe.example/assets/voice.${hash}.js`), hash);
  assert.equal(buildOf(`https://vibe.example/assets/voice.${hash}.js?x=1`), hash);
  assert.equal(buildOf("https://vibe.example/voice.js?v=aaaaaaaa11111111"), "aaaaaaaa11111111");
  for (const src of [`https://vibe.example/assets/voice.${hash.slice(1)}.js`, `https://vibe.example/assets/voice.${hash}0.js`,
    `https://vibe.example/assets/other.${hash}.js`, `https://vibe.example/assets/voice.${hash}.js.map`,
    `https://vibe.example/assets/voice.${hash.toUpperCase()}.js`, "https://vibe.example/voice.js"]) {
    assert.equal(buildOf(src), "", src);
  }
  // THE CONTROL: the reader as it was, ?v= alone, does not know a page loaded by the hashed name.
  const before = brokenScript("/\\/assets\\/voice\\.([0-9a-f]{32})\\.js(?:[?#]|$)/.exec(src) || ", "");
  assert.equal(buildOf(`https://vibe.example/assets/voice.${hash}.js`, before), "");
});

/**
 * `#521 server-side-search`. The search language's conformance corpus, which src/search_query.rs is
 * held to as well: the page's twin must fold, parse, match and plan upstream queries exactly as it
 * says. Values cross out of the page's realm through JSON, so a tree is compared as data.
 */
const SEARCH_CORPUS = JSON.parse(readFileSync(join(WEB, "..", "tests", "fixtures", "search_query.json"), "utf8"));

// The search language module and searchLanguage() are loaded near PRELUDE, above every test.

function searchCorpusDisagreements(language) {
  const fold = language.searchFold;
  const parse = language.searchQueryParse;
  const tree = language.searchQueryTree;
  const matches = language.searchQueryMatches;
  const subject = language.searchSubject;
  const plan = language.searchUpstreamPlan;
  const data = (value) => JSON.parse(JSON.stringify(value ?? null));
  const wrong = [];
  for (const { in: input, out } of SEARCH_CORPUS.fold) {
    if (fold(input) !== out) wrong.push(`fold ${JSON.stringify(input)} gave ${JSON.stringify(fold(input))}`);
  }
  for (const { query, tree: expected } of SEARCH_CORPUS.parse) {
    const got = data(tree(parse(query)));
    if (JSON.stringify(got) !== JSON.stringify(expected)) wrong.push(`parse ${JSON.stringify(query)} gave ${JSON.stringify(got)}`);
  }
  for (const { query, message, matches: expected, tz_offset_minutes: offset = 0 } of SEARCH_CORPUS.match) {
    const folded = subject({ author: message.author ?? "", authorId: message.author_id ?? "", text: message.text ?? "",
      links: message.links ?? [], createdMs: Date.parse(message.created), inThread: message.in_thread ?? false });
    const dayStart = (day) => Date.UTC(day.year, day.month - 1, day.day) - offset * 60_000;
    if (matches(parse(query), folded, dayStart) !== expected) wrong.push(`match ${JSON.stringify(query)} on ${JSON.stringify(message.text)}`);
  }
  for (const { query, queries, scan } of SEARCH_CORPUS.upstream) {
    const got = data(plan(parse(query)));
    if (JSON.stringify(got) !== JSON.stringify({ queries, scan })) wrong.push(`upstream ${JSON.stringify(query)} gave ${JSON.stringify(got)}`);
  }
  return wrong;
}

test("`#521 server-side-search`: the page's search language folds, parses, matches and plans exactly as the shared corpus says", () => {
  const counted = ["fold", "parse", "match", "upstream"].map((kind) => SEARCH_CORPUS[kind].length);
  assert.ok(counted.every((n) => n >= 20), `the corpus shrank: ${counted}`);
  assert.deepEqual(searchCorpusDisagreements(SEARCH_LANGUAGE), []);
  // `#624 voice-page-modularity`: the page runs the same functions, by the prelude, under the names
  // its legacy script has always called them by.
  const page = newPage();
  for (const name of ["searchQueryParse", "searchQueryTree", "searchQueryMatches", "searchSubject", "searchPositiveTerms"]) {
    assert.equal(typeof page.preludeBinding(name), "function", name);
  }
  const node = page.preludeBinding("searchQueryParse")("from:Ada (a OR b) -c");
  assert.deepEqual(JSON.parse(JSON.stringify(page.preludeBinding("searchQueryTree")(node))),
    JSON.parse(JSON.stringify(SEARCH_LANGUAGE.searchQueryTree(SEARCH_LANGUAGE.searchQueryParse("from:Ada (a OR b) -c")))));
});

test("`#521 server-side-search`: the corpus catches a page that lost one fold entry or bound OR loosely (negative controls)", async () => {
  // Without final sigma folded, ΟΣ (lower-cased to ος) no longer finds ΟΣΑ (οσα).
  const LANGUAGE = "src/core/search/language.ts";
  const sigma = await searchLanguage([{ file: LANGUAGE, find: "[\"ς\", \"σ\"], ", replace: "" }]);
  assert.ok(searchCorpusDisagreements(sigma).some((line) => line.includes("ΟΔΟΣ")));
  // OR taking whatever follows it as a whole conjunction disagrees with Gmail's binding of its neighbours.
  const looseOr = await searchLanguage([{ file: LANGUAGE,
    find: "    while (tokens[at] === \"OR\") {\n      at += 1;\n      const node = unary();",
    replace: "    while (tokens[at] === \"OR\") {\n      at += 1;\n      const node = and(false);" }]);
  assert.ok(searchCorpusDisagreements(looseOr).some((line) => line.startsWith("parse \"a OR b c\"")));
});

// --- `#617 cold-load-first-paint` ------------------------------------------------------------------

/** Answer every channel read from the server's own copy, as of `ageMs` ago, until put back. */
function answeredFromCopy(page, ageMs) {
  const standard = page.timeline;
  const asOf = new Date(page.clock() - ageMs).toISOString();
  page.timeline = async (path, options) => {
    const response = await standard(path, options);
    const { delta: _delta, ...newest } = JSON.parse(await response.text());
    return json(200, { ...newest, as_of: asOf, refresh_failed: "The chat service did not answer in time." });
  };
  return standard;
}

test("`#617 cold-load-first-paint`: a page from the server's own copy a minute old says nothing failed", async () => {
  const page = await threadPage("main");
  answeredFromCopy(page, 60_000);
  await reReadChannel(page);
  assert.notEqual(freshness(page).getAttribute("data-state"), "failed");
  assert.doesNotMatch(freshness(page).textContent, /failed/i, freshness(page).textContent);
});

test("`#617 cold-load-first-paint`: after a page from the server's own copy the next read comes in seconds, not at the next poll", async () => {
  const page = await threadPage("main");
  const standard = answeredFromCopy(page, 5 * 60_000);
  await reReadChannel(page);
  assert.equal(freshness(page).getAttribute("data-state"), "failed");
  page.timeline = standard;
  const reads = page.timelineCalls.length;
  assert.ok(page.expireTimers(3000) > 0, "no read was armed within three seconds");
  await page.settle();
  assert.ok(page.timelineCalls.length > reads, "the soon read was not sent");
  assert.equal(freshness(page).getAttribute("data-state"), "fresh", freshness(page).textContent);
});

test("`#649 voice-agent-tools`: an answer's text that arrives after its turn_complete takes the place of 'no text received' and is filed as the agent's words", async () => {
  const page = newPage();
  const socket = await startNeutralCall(page);
  speech(socket, "user", 1, "summarize overnight", true);
  socket.onmessage({ data: secondOfSpeech(page) });
  turnDone(socket, 1);
  await page.settle();
  const noted = () => session_lines(page).filter((li) => li.getAttribute("data-no-text") === "");
  assert.equal(noted().length, 1);
  const rows = session_lines(page).length;
  // Seen live: the bridge's text for a turn came after that turn's turn_complete.
  speech(socket, "assistant", 1, "The runner came back at nine.", true);
  await page.settle();
  assert.equal(noted().length, 0, "the note stayed beside the words it said were missing");
  assert.equal(session_lines(page).length, rows, "the late text took a row of its own");
  assert.match(session_lines(page).at(-1).text(), /The runner came back at nine\.$/);
  // Filed once the next turn is under way, as the agent's.
  socket.onmessage({ data: secondOfSpeech(page) });
  speech(socket, "assistant", 2, "Anything else?", true);
  turnDone(socket, 2);
  await page.settle();
  assert.ok(storedTexts(page).includes("agent: The runner came back at nine."), storedTexts(page).join(" | "));
  assert.deepEqual(page.healthPosts.map((r) => r.cause), ["audio_without_text", "late_text"]);
});

test("`#649 voice-agent-tools`: Hang up while an answer is arriving listens a moment for its text, shows it in its row and files it", async () => {
  const page = newPage();
  const socket = await startNeutralCall(page);
  speech(socket, "user", 1, "what happened overnight", true);
  speech(socket, "assistant", 2, "Reading the messages.", true);
  socket.onmessage({ data: secondOfSpeech(page) });
  await page.el("hang-up").click();
  await page.settle();
  assert.equal(state(page), "ended");
  assert.notEqual(socket.readyState, 3, "closed before the answer's text could arrive");
  assert.ok(socket.sent.some((frame) => typeof frame === "string" && frame.includes('"quit"')), "no quit was sent");
  socket.onmessage({ data: JSON.stringify({ type: "transcript", role: "assistant", turn: 2, text: "Reading the messages. The build is green." }) });
  socket.onclose({ code: 1000, reason: "" });
  await page.settle();
  assert.equal(socket.readyState, 3);
  const shown = session_lines(page).filter((li) => li.text().includes("The build is green."));
  assert.equal(shown.length, 1, "the late text was not shown, or shown twice");
  assert.match(shown[0].text(), /Reading the messages\. The build is green\. \(text arrived after you hung up\)$/);
  assert.equal(
    session_lines(page).filter((li) => /Reading the messages\.( \(cut off\))?$/.test(li.text())).length,
    0,
    "the cut row stayed beside the whole answer"
  );
  await page.settle();
  assert.ok(
    storedTexts(page).includes("agent: Reading the messages. The build is green. (text arrived after you hung up)"),
    storedTexts(page).join(" | ")
  );
});

test("`#649 voice-agent-tools`: Hang up with no answer owed still closes the call's socket at once", async () => {
  const page = newPage();
  const socket = await startNeutralCall(page);
  speech(socket, "user", 1, "thanks", true);
  speech(socket, "assistant", 1, "You're welcome.", true);
  turnDone(socket, 1);
  await page.settle();
  await page.el("hang-up").click();
  assert.equal(socket.readyState, 3);
});
