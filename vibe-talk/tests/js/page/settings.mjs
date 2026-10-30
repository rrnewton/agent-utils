// The app page's tests: settings and help, pairing, the microphone, and who is who in a channel.
//
// One feature's file of the suite tests/js/voice_page.test.mjs runs, against the fixture in
// ./fixture.mjs. Run with the rest of the suite, or alone: node --test tests/js/page/settings.mjs.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { WEB } from "../../../web/page_sources.mjs";

import { CHANNEL, CSS_CODE, CSS_UNCONDITIONAL, DESKTOP_QUERY, HELP_SLUGS, HTML, HTML_CODE,
  OTHER_CHANNEL, PAGE_ELEMENTS, SCRIPT, SCRIPT_CODE, SEND_UNDO_MS, SETTINGS, SPACE_A, TEAM_A,
  TEAM_B, assertMarkupContains, brokenScript, browsingPage, cardRow, channelBoxCode, chooseService,
  clearingPage, cssBlock, cssBlockIn, cssRules, cssWithoutIn, dismissedItems, dismissedOpenerOf,
  failedSend, focusName, focused, groupsIn, helpEntry, json, keyboardPage, literal, markupHolds,
  mediaBodyOf, message, newPage, nodeIn, openBrowser, openHelp, openReplyOn, openerOf,
  outgoingDismiss, pickerChoices, press, putBackOf, readChannel, reloadWith, reopenedChannel,
  replacedV1Call, row, savedDismissed, savedOutgoing, scriptFolds, serveByChannel, servicesPage,
  settingsChannel, settingsFoldState, settingsGroup, settingsHeading, settingsHeadings,
  settingsOptions, settingsSection, showDiscord, signIn, sourceConstant, sourceString, startTalking,
  startV1Call, test, twoColumnServer, twoServices, whoOf, withEarlierConversation, withLiveChannel } from "./fixture.mjs";

// --- `#372 settings-header-layout` ----------------------------------------------------------------
//
// The owner, 2026-10-19, of Settings on a desktop window: "the machine URL doesn't need to be in a big
// font!! Expand all certainly doesn't need its own line." The host was the loudest line on the screen,
// in large bold type wrapping in a card of its own, and Expand all and Open help each stood alone on a
// row. tests/desktop_layout_browser.py measures the same in Chromium.

/** What is wrong with the server host's type against `#372`, as sentences; empty when nothing is. */
function serverHostProblems(css) {
  const problems = [];
  const rule = cssRules(css, "#screen-settings .settings-server-host").join("\n");
  const name = cssRules(css, "#screen-settings .settings-server-name").join("\n");
  const size = /font-size:\s*([\d.]+)rem/.exec(rule);
  if (!size || Number(size[1]) > 0.95) problems.push(`the host is drawn at ${size ? `${size[1]}rem` : "an unstated size"}, larger than the text around it`);
  if (/font-weight:\s*(?:[5-9]00|bold)/.test(rule) || !/font-weight:\s*(?:400|normal)/.test(rule)) problems.push("the host is not in ordinary weight");
  if (!/white-space:\s*nowrap/.test(rule) || /overflow-wrap:\s*anywhere/.test(rule)) problems.push("the host wraps rather than keeping to one line");
  if (!/overflow:\s*hidden/.test(rule)) problems.push("the host's line lets what does not fit run out of it");
  // Blocks (a flex row's items) would put a line break before the port in what is copied.
  if (/display:\s*(?:flex|grid|block)/.test(rule + name)) problems.push("the host's parts are blocks, and its copy breaks before the port");
  if (!/display:\s*inline-block/.test(name) || !/overflow:\s*hidden/.test(name) || !/text-overflow:\s*ellipsis/.test(name)) {
    problems.push("a host name too long for its line is not cut with an ellipsis");
  }
  if (!/max-width:\s*calc\(100% - var\(--port-width/.test(name)) problems.push("the port can be cut, and it is what tells two instances on one host apart");
  if (/user-select:\s*none/.test(rule + name)) problems.push("the host cannot be selected to copy");
  return problems;
}

test("`#372 settings-header-layout`: the server host is quiet: small, ordinary weight, one line cut with an ellipsis, whole in its tooltip", () => {
  assert.deepStrictEqual(serverHostProblems(CSS_CODE), []);
  const page = newPage();
  const host = page.el("settings-server-host");
  assert.equal(host.title, "vibe.example", "the host's tooltip does not hold it whole");
  assert.equal(host.text(), host.title);
  assert.deepStrictEqual(host.children.map((part) => [part.className, part.textContent]),
    [["settings-server-name", "vibe.example"], ["settings-server-port", ""]]);
  // The negative controls: the host as it was, loud and wrapping; and a port the ellipsis can take.
  const loud = CSS_CODE.replace(/(#screen-settings \.settings-server-host \{)[^}]*\}/,
    "$1 font-size: 1.45rem; font-weight: 700; overflow-wrap: anywhere; }")
    .replace(/(#screen-settings \.settings-server-name \{)[^}]*\}/, "$1 }");
  assert.notEqual(loud, CSS_CODE, "the negative control changed nothing");
  assert.deepStrictEqual(serverHostProblems(loud), [
    "the host is drawn at 1.45rem, larger than the text around it",
    "the host is not in ordinary weight",
    "the host wraps rather than keeping to one line",
    "the host's line lets what does not fit run out of it",
    "a host name too long for its line is not cut with an ellipsis",
    "the port can be cut, and it is what tells two instances on one host apart",
  ]);
  const flex = CSS_CODE.replace(/(#screen-settings \.settings-server-host \{)/, "$1\n  display: flex;");
  assert.deepStrictEqual(serverHostProblems(flex), ["the host's parts are blocks, and its copy breaks before the port"]);
});

test("`#372 settings-header-layout`: the host's port is its own part, an IPv6 address's colons kept in the name", () => {
  for (const [host, name, port] of [["vibe.example:8443", "vibe.example", ":8443"], ["[::1]:8443", "[::1]", ":8443"],
    ["[::1]", "[::1]", ""], ["vibe.example", "vibe.example", ""]]) {
    const page = newPage(new Map(), SCRIPT.replace("function deploymentHost() {", `function deploymentHost() {\n  return ${JSON.stringify(host)};`));
    const parts = page.el("settings-server-host").children.map((part) => part.textContent);
    assert.deepStrictEqual(parts, [name, port], `${host} split as ${JSON.stringify(parts)}`);
    assert.equal(page.el("settings-server-host").title, host);
    assert.equal(page.el("settings-server-host").style.getPropertyValue("--port-width"), `${port.length}ch`);
  }
});

/**
 * What is wrong with the head of Settings against `#372`, as sentences; empty when nothing is: the
 * server line, Expand all and Open help in one row, in that order, before the sections, and neither
 * button standing alone on a line of its own.
 */
function settingsHeadProblems(html, css) {
  const problems = [];
  const code = html.replace(/<!--[\s\S]*?-->/g, " ");
  const settings = code.slice(code.indexOf('id="screen-settings"'), code.indexOf('id="screen-help"'));
  const opens = settings.indexOf('<div class="settings-head">');
  const panes = settings.indexOf('<div class="settings-panes"');
  if (opens < 0 || panes < 0 || opens > panes) return ["there is no head row above the sections"];
  const head = settings.slice(opens, panes);
  const at = (id) => head.indexOf(`id="${id}"`);
  for (const [id, name] of [["settings-server-host", "the server's host"], ["fold-settings-all", "Expand all"], ["open-help", "Open help"]]) {
    if (at(id) < 0) problems.push(`${name} is not in the head row`);
  }
  if (!(at("settings-server-host") < at("fold-settings-all") && at("fold-settings-all") < at("open-help"))) {
    problems.push("the head row is not the server, Expand all, then Open help");
  }
  for (const [id, name] of [["fold-settings-all", "Expand all"], ["open-help", "Open help"]]) {
    if (new RegExp(`<p\\b[^>]*>\\s*<button id="${id}"`).test(settings)) problems.push(`${name} stands alone on a line of its own`);
  }
  if (/tabindex="[1-9]/.test(settings)) problems.push("a positive tabindex reorders the focus");
  const row = cssRules(css, ".settings-head").join("\n");
  if (!/display:\s*flex/.test(row)) problems.push("the head is not laid out as a row");
  if (!/flex-wrap:\s*wrap/.test(row)) problems.push("the head cannot wrap on a phone, so it squeezes or overflows");
  if (/flex-direction:\s*\w+-reverse|(?:^|[\s;{])order:/.test(row + cssRules(css, ".settings-head-actions").join("\n"))) {
    problems.push("a rule reorders the head away from the reading order");
  }
  return problems;
}

test("`#372 settings-header-layout`: the server line, Expand all and Open help share one row above the sections, in reading order", () => {
  assert.deepStrictEqual(settingsHeadProblems(HTML, CSS_CODE), []);
});

test("`#372 settings-header-layout`: the check refuses Expand all or Open help on a row of its own, and a reordered head (negative controls)", () => {
  const help = '<button id="open-help" class="secondary" type="button">Open help</button>';
  const expand = '<button id="fold-settings-all" class="secondary" type="button">Expand all</button>';
  assert.ok(HTML.includes(help) && HTML.includes(expand), "the buttons' markup moved; update the negative controls");
  // Open help back at the foot of the screen, in a paragraph of its own, as it was.
  const foot = HTML.replace(help, "").replace(/(\n {10}<\/div>\n {8}<\/section>\n\n {8}<!-- -+ help screen -->)/,
    `<p class="hint">\n  ${help}\n</p>$1`);
  assert.ok(foot.includes(`<p class="hint">\n  ${help}`), "the negative control changed nothing");
  assert.deepStrictEqual(settingsHeadProblems(foot, CSS_CODE), [
    "Open help is not in the head row",
    "the head row is not the server, Expand all, then Open help",
    "Open help stands alone on a line of its own",
  ]);
  // Expand all on a line of its own under the server, as it was.
  const alone = HTML.replace(expand, `</div><p class="settings-fold-all">${expand}</p><div>`);
  assert.deepStrictEqual(settingsHeadProblems(alone, CSS_CODE), ["Expand all stands alone on a line of its own"]);
  const swapped = HTML.replace(expand, "\u0000").replace(help, expand).replace("\u0000", help);
  assert.deepStrictEqual(settingsHeadProblems(swapped, CSS_CODE), ["the head row is not the server, Expand all, then Open help"]);
  const reversed = CSS_CODE.replace(/(\.settings-head \{)/, "$1\n  flex-direction: row-reverse;");
  assert.deepStrictEqual(settingsHeadProblems(HTML, reversed), ["a rule reorders the head away from the reading order"]);
  const rigid = CSS_CODE.replace(/(\.settings-head \{[^}]*?)flex-wrap:\s*wrap;/, "$1");
  assert.notEqual(rigid, CSS_CODE, "the negative control changed nothing");
  assert.deepStrictEqual(settingsHeadProblems(HTML, rigid), ["the head cannot wrap on a phone, so it squeezes or overflows"]);
});

test("a token the server accepts leads to the main interface", async () => {
  const page = newPage();
  await signIn(page);

  assert.equal(page.screen(), "main");
  assert.equal(page.el("control-pane").hidden, false, "the controls must be reachable");
  assert.equal(page.el("view-switch").hidden, false);
});

test("settings and help name the selected chat backend, with a neutral fallback", async () => {
  for (const name of ["Google Chat", "Discord", "", "   ", "<img src=x>"]) {
    const page = newPage();
    page.chatProviderName = name;
    await signIn(page);
    const expected = typeof name === "string" ? name.trim() || "Chat" : "Chat";
    for (const id of ["chat-provider-settings", "chat-provider-help"]) {
      assert.equal(page.el(id).textContent, expected);
    }
    assert.ok(!page.createdTags.includes("img"), "a backend name was interpreted as markup");
  }
  // Include hidden help/settings prose: users can open it after the initial page has loaded.
  // Internal DOM identifiers and implementation comments do not name a platform to the reader.
  assert.doesNotMatch(HTML_CODE.replace(/<[^>]+>/g, " "), /discord/i);
});

test("the title is the name the server gives its app, and stays as served when it gives none", async () => {
  // `#343 pwa-app-name`. The server writes the name into the page it serves; client-config
  // carries it too, and the page takes it from there once signed in.
  const named = newPage();
  named.document.title = "vibe-talk";
  named.appName = "  vibe-talk at work  ";
  await signIn(named);
  assert.equal(named.document.title, "vibe-talk at work");

  for (const appName of [undefined, "", "   "]) {
    const page = newPage();
    page.document.title = "as served";
    page.appName = appName;
    await signIn(page);
    assert.equal(page.document.title, "as served", `app_name ${JSON.stringify(appName)}`);
  }
});

test("a token the server REFUSES sends you back to sign in, and says which thing was wrong", async () => {
  const page = newPage();
  page.clientConfig = async () => json(401, { error: "unauthorized", detail: "unknown token" });
  page.el("api-token").value = "write-token-aaaaaaaaaaaaaaaa";

  await page.el("save-token").click();
  await page.settle();

  assert.equal(page.screen(), "signin");
  assert.match(page.el("error").textContent, /refused that token/);
  assert.match(page.el("error").textContent, /unknown token/);
});

// --- `#369 pairing-link` -------------------------------------------------------------------------

const PAIR_CODE = "Pq3x_Zr-7AbCdEfGhIjKlMnOpQrStUvWxYz012345678";

/** A page opened at a pairing link, recording what the address was when the code came out of it. */
function pairingPage(store = new Map(), arrangeMore = null) {
  const seen = {};
  const page = newPage(store, SCRIPT, (p) => {
    p.enableHistory();
    p.setAddress({ pathname: "/", search: "?from=chat", hash: `#pair=${PAIR_CODE}` });
    const replace = p.history.replaceState.bind(p.history);
    p.history.replaceState = (state, unused, address) => {
      if (seen.requestsBefore === undefined) {
        seen.requestsBefore = p.requests.length;
        seen.address = address;
      }
      replace(state, unused, address);
    };
    if (arrangeMore) arrangeMore(p);
  });
  return { page, seen };
}

/** Tap Sign this device in, and let the page finish. */
async function confirmPairing(page) {
  assert.equal(page.el("pair-offer").hidden, false, "no offer to pair");
  await page.el("pair-confirm").click();
  await page.settle();
}

test("a pairing link's code leaves the address before the page makes any request", async () => {
  const { page, seen } = pairingPage();
  await page.settle();
  assert.equal(seen.requestsBefore, 0, "the page made a request while the code was still in the address");
  assert.equal(seen.address, "/?from=chat", "the fragment, and only the fragment, is gone");
  await confirmPairing(page);
  assert.ok(!page.history.addresses.some((address) => address.includes(PAIR_CODE)), "the code went into history");
  assert.ok(!page.requests.some((request) => request.includes(PAIR_CODE)), "the code went into a URL");
});

test("opening a pairing link spends nothing until the person taps Sign this device in", async () => {
  const { page } = pairingPage();
  await page.settle();
  assert.equal(page.screen(), "signin");
  assert.equal(page.el("pair-offer").hidden, false, "the page did not offer to pair");
  assert.match(page.el("pair-offer-text").textContent, /signs this browser in/);
  assert.deepStrictEqual(page.pairRedeems, [], "the page redeemed the code with nobody's tap: a link scanner would spend it");
});

test("a pairing link signs the browser in with the token it redeems for, as Save does", async () => {
  const store = new Map();
  const { page } = pairingPage(store, (p) => {
    p.pairRedeem = async () => json(200, { token: "write-token-from-pairing-00" });
  });
  await page.settle();
  await confirmPairing(page);
  assert.deepStrictEqual(page.pairRedeems.map((one) => one.body), [{ code: PAIR_CODE }]);
  assert.ok(!("Authorization" in page.pairRedeems[0].headers), "the redemption carried a bearer token");
  assert.equal(store.get("vibe-talk.token"), "write-token-from-pairing-00");
  assert.equal(page.screen(), "main", "pairing did not continue signed in");
  assert.equal(page.el("pair-offer").hidden, true, "the spent offer stayed up");
  assert.ok(page.requests.indexOf("POST /api/v1/pair/redeem") < page.requests.findIndex((r) => r.startsWith("GET /api/v1/client-config")),
    "the page asked for its configuration before it had the token");
});

test("an expired or used pairing link says so plainly, and to ask for a new one", async () => {
  const store = new Map();
  const { page } = pairingPage(store);
  await page.settle();
  await confirmPairing(page);
  assert.equal(page.screen(), "signin");
  assert.equal(store.get("vibe-talk.token"), undefined, "a refused link stored something");
  assert.match(page.el("error").textContent, /expired or was already used\. Ask for a new one/);
  assert.match(page.el("error").textContent, /someone else may have/);
  assert.equal(page.el("pair-offer").hidden, true, "a spent link was offered again");
});

test("a pairing link refused for too many attempts says to wait, and a stale link leaves a signed-in phone signed in", async () => {
  const store = new Map([["vibe-talk.token", "write-token-aaaaaaaaaaaaaaaa"]]);
  const { page } = pairingPage(store, (p) => {
    p.pairRedeem = async () => json(429, { error: "too_many_attempts", detail: "wait" });
  });
  await page.settle();
  await confirmPairing(page);
  assert.equal(page.screen(), "main", "a failed link signed the phone out");
  assert.equal(store.get("vibe-talk.token"), "write-token-aaaaaaaaaaaaaaaa");
  assert.match(page.el("error").textContent, /Too many pairing attempts.*Wait a minute/);
});

test("a second pairing link opened in a tab already showing the app is offered too", async () => {
  const store = new Map();
  const page = newPage(store, SCRIPT, (p) => p.enableHistory());
  await page.settle();
  assert.equal(page.el("pair-offer").hidden, true);
  page.pairRedeem = async () => json(200, { token: "write-token-from-pairing-00" });
  const asked = page.requests.length;
  await page.navigateToFragment(`#pair=${PAIR_CODE}`);
  assert.equal(page.el("pair-offer").hidden, false, "the new link was not offered");
  assert.ok(page.history.addresses.length > 0 && !page.history.addresses.some((a) => a.includes(PAIR_CODE)),
    "the fragment was not taken out of the address");
  assert.equal(page.requests.length, asked, "a fragment change made a request before the tap");
  await confirmPairing(page);
  assert.equal(store.get("vibe-talk.token"), "write-token-from-pairing-00");
  assert.equal(page.screen(), "main");
});

test("both pairing buttons are disabled while a redemption is in flight", async () => {
  let answer = null;
  const { page } = pairingPage(new Map(), (p) => {
    p.pairRedeem = () => new Promise((resolve) => { answer = resolve; });
  });
  await page.settle();
  page.el("pair-confirm").click();
  await page.settle();
  assert.equal(page.el("pair-confirm").disabled, true);
  assert.equal(page.el("pair-dismiss").disabled, true, "Not now could hide an offer about to sign in");
  answer(json(200, { token: "write-token-from-pairing-00" }));
  await page.settle();
  assert.equal(page.screen(), "main");
});

test("a newer pairing link opened while a redemption is answering is offered once that one fails", async () => {
  let answer = null;
  const { page } = pairingPage(new Map(), (p) => {
    p.pairRedeem = () => new Promise((resolve) => { answer = resolve; });
  });
  await page.settle();
  page.el("pair-confirm").click();
  await page.settle();
  await page.navigateToFragment("#pair=NewerCodeNewerCodeNewerCodeNewerCodeNewerCo");
  assert.equal(page.el("pair-confirm").disabled, true, "a newer link re-enabled a button mid-request");
  answer(json(410, { error: "pairing_link_invalid", detail: "gone" }));
  await page.settle();
  assert.equal(page.el("pair-offer").hidden, false, "the newer, unspent link vanished");
  page.pairRedeem = async () => json(200, { token: "write-token-from-pairing-00" });
  await confirmPairing(page);
  assert.deepStrictEqual(page.pairRedeems.map((one) => one.body.code).slice(-1),
    ["NewerCodeNewerCodeNewerCodeNewerCodeNewerCo"]);
  assert.equal(page.screen(), "main");
});

test("Not now leaves the pairing link unspent and a signed-in phone signed in", async () => {
  const store = new Map([["vibe-talk.token", "write-token-aaaaaaaaaaaaaaaa"]]);
  const { page } = pairingPage(store);
  await page.settle();
  await page.el("pair-dismiss").click();
  await page.settle();
  assert.deepStrictEqual(page.pairRedeems, []);
  assert.equal(page.el("pair-offer").hidden, true);
  assert.equal(page.screen(), "main");
});

test("a READ token is told it is the wrong SCOPE, not that it was refused", async () => {
  // The expensive case. /api/v1/client-config takes a read-scope token, so a read token signs in
  // and only fails where something writes — and "refused" would send the owner looking for a typo
  // in a token that is not wrong. 403 and 401 are different sentences here for that reason.
  const page = newPage();
  page.clientConfig = async () =>
    json(403, { error: "forbidden", detail: "this token may read but not post" });
  page.el("api-token").value = "read-token-aaaaaaaaaaaaaaaaa";

  await page.el("save-token").click();
  await page.settle();

  assert.equal(page.screen(), "signin");
  assert.match(page.el("error").textContent, /read but not post/);
  assert.match(page.el("error").textContent, /write-scope/i);
  assert.match(page.el("error").textContent, /VIBE_TALK_WRITE_TOKEN/);
  // ...and it must NOT be the sentence a 401 gets, which is the whole point of separating them.
  assert.doesNotMatch(page.el("error").textContent, /refused that token/);
  // BOTH halves. The panel and the status strip are written from two different expressions, and
  // asserting only the panel left the strip free to keep saying "refused" — which a mutation of
  // exactly that expression proved, by passing this test while the strip was collapsed back.
  // `#status` is the text; `#status-line` is the strip around it, and reading the strip returns
  // an empty string that matches nothing and fails for the wrong reason.
  assert.match(page.el("status").textContent, /READ token/);
  assert.doesNotMatch(page.el("status").textContent, /refused/);
});

test("a browser signed out BY THE RENAME is told that, rather than looking like a broken deploy", async () => {
  // Same origin, token still sitting under the pre-rename key. Without this the first load after
  // a rename is a sign-in screen on a URL that worked yesterday, with nothing saying why.
  const store = new Map([["gent-talk.token", "write-token-aaaaaaaaaaaaaaaa"]]);
  const page = newPage(store);
  await page.settle();

  assert.equal(page.screen(), "signin");
  assert.match(page.el("token-state").textContent, /renamed/i);
  assert.match(page.el("token-state").textContent, /paste your write-scope token/i);
  // Read, never adopted: a token the owner cannot see must not be reused on their behalf.
  assert.equal(store.get("vibe-talk.token"), undefined, "the old token was silently adopted");
});

test("a first-time visitor is NOT told about a rename that has nothing to do with them", async () => {
  // The negative half. A sentence that fires for everyone explains nothing to anyone.
  const page = newPage();
  await page.settle();

  assert.match(page.el("token-state").textContent, /paste your write-scope token/i);
  assert.doesNotMatch(page.el("token-state").textContent, /renamed/i);
});

test("a server that is merely DOWN does not get mistaken for a bad token", async () => {
  // Bouncing to a sign-in form on a 503 would blame the owner's token for the server's outage,
  // and they would paste the token again and again while nothing changed.
  const page = newPage();
  page.clientConfig = async () => json(503, { error: "unavailable", detail: "still starting" });
  page.el("api-token").value = "write-token-aaaaaaaaaaaaaaaa";

  await page.el("save-token").click();
  await page.settle();

  assert.equal(page.screen(), "main", "a server outage must not look like a sign-in failure");
  assert.match(page.el("error").textContent, /did not answer/);
  assert.equal(page.storage.get("vibe-talk.token"), "write-token-aaaaaaaaaaaaaaaa");
});

test("settings is reachable from the main screen and comes back to it", async () => {
  const page = newPage();
  await signIn(page);

  await page.el("open-settings").click();
  assert.equal(page.screen(), "settings");
  assert.equal(page.el("control-pane").hidden, true, "the pane must not float over settings");

  await page.el("close-settings").click();
  assert.equal(page.screen(), "main");
  assert.equal(page.el("control-pane").hidden, false);
});

test("connection details appear once in a dismissable banner, and stay in settings", async () => {
  const page = newPage();
  assert.equal(
    PAGE_ELEMENTS.get("connection-banner").hidden,
    true,
    "the banner must ship hidden; it is a thing that ARRIVES"
  );

  await startTalking(page);

  const banner = page.el("connection-banner");
  assert.equal(banner.hidden, false, "the connection details never appeared");
  assert.match(page.el("connection-detail").textContent, /Example voice provider/);
  assert.match(page.el("settings-detail").textContent, /Example voice provider/);
  assert.match(page.el("settings-detail").textContent, /wss:\/\/example\.invalid\/convai/);
  assert.doesNotMatch(page.el("settings-detail").textContent, /sig=/, "a signed query reached the screen");

  await page.el("dismiss-banner").click();

  assert.equal(banner.hidden, true, "Dismiss did not dismiss");
  // Dismissing loses nothing: the same text is still in settings, which is the whole reason the
  // banner is allowed to go away by itself.
  assert.match(page.el("settings-detail").textContent, /Example voice provider/);
});

// --- settings and help ---------------------------------------------------------------------------
//
// `#85 voice-desktop-review`. Settings had become a document with controls buried in it: every knob
// carried two or three paragraphs, so reaching the switch you wanted meant reading past everything
// you already knew. The prose is not deleted — it is one tap away, and these assert that the tap
// exists, because a split without it is just a filing cabinet.

test("EVERY SETTINGS GROUP THAT EXPLAINS ITSELF HAS A HELP ENTRY, AND A WAY TO REACH IT", () => {
  // The generalising assertion, and the one that keeps the next group honest: pinning the entries
  // that happen to exist today leaves the next one unlinked, which is exactly where the split
  // silently stops being a split.
  // A set, as it was when `#332 settings-voice-agent-section` reached one entry from beside two settings.
  const wanted = [...new Set([...HTML.matchAll(/data-help="([a-z-]+)"/g)].map((m) => m[1]))];
  assert.ok(wanted.length >= 8, `only ${wanted.length} help links found — the scan is broken`);
  for (const slug of wanted) {
    assert.ok(
      HTML.includes(`id="help-${slug}"`),
      `a settings group links to "${slug}" and no help entry answers to it`
    );
    assert.ok(
      HTML.includes(`id="help-link-${slug}"`),
      `the "${slug}" link has no id, so nothing can wire it`
    );
  }
  // The script's own list has to agree with the markup in BOTH directions, or a group ships with a
  // ? that does nothing.
  const listed = /const HELP_TOPICS = \[([\s\S]*?)\]/.exec(SCRIPT_CODE);
  assert.ok(listed, "web/voice.js no longer declares HELP_TOPICS");
  const topics = [...listed[1].matchAll(/"([a-z-]+)"/g)].map((m) => m[1]);
  assert.deepStrictEqual(
    topics.slice().sort(),
    wanted.slice().sort(),
    "HELP_TOPICS and the markup's help links disagree"
  );
  // ...and nothing the other way either: an entry nobody links to is unreachable except by
  // scrolling, which is the state this issue exists to remove.
  // `help-link-<slug>` is the BUTTON, not an entry; the negative lookahead keeps the two apart.
  const entries = [...HTML.matchAll(/id="help-(?!link-)([a-z-]+)"/g)].map((m) => m[1]);
  const orphans = entries.filter((slug) => !wanted.includes(slug) && slug !== "controls" && slug !== "channel");
  assert.deepStrictEqual(orphans, [], `nothing links to the help entries ${orphans.join(", ")}`);
  // `#332 settings-voice-agent-section`. EVERY `?` is wired by its id, not merely one per entry: its
  // own `help-link-<slug>` through HELP_TOPICS. `#347 settings-prose` took away the one `?` that was
  // wired otherwise, beside a label, so a `?` by any other id would be a button that does nothing.
  assert.deepStrictEqual(helpLinkProblems(HTML, SCRIPT_CODE), []);
});

/**
 * What is wrong with the `?` buttons of a page, as sentences: one in the markup whose id the script
 * does not wire. `#332 settings-voice-agent-section`; since `#347 settings-prose` every one is
 * `help-link-<slug>` with its slug in HELP_TOPICS, the round ones and the one written as words. A
 * function of the two texts, so its negative control can hand it a page with a `?` unwired.
 */
function helpLinkProblems(html, script) {
  const topics = [.../const HELP_TOPICS = \[([\s\S]*?)\]/.exec(script)[1].matchAll(/"([a-z-]+)"/g)].map((m) => m[1]);
  const links = [...html.matchAll(/<button class="help-link(?: help-link-text)?" id="([a-z-]+)" type="button" data-help="([a-z-]+)"/g)]
    .map((m) => [m[1], m[2]]);
  const problems = [];
  if (links.length !== [...html.matchAll(/data-help="/g)].length) {
    problems.push("a ? is not written as <button class=\"help-link\" id=… type=\"button\" data-help=…>, so it was not checked");
  }
  for (const [id, slug] of links) {
    if (id !== `help-link-${slug}` || !topics.includes(slug)) {
      problems.push(`#${id} opens "${slug}" and nothing in web/voice.js wires it`);
    }
  }
  return problems;
}

test("`#332 settings-voice-agent-section`: the ? check finds a ? nothing wires (negative control)", () => {
  // Without this the check above could pass on any page at all. The second `?` for How messages are
  // said that `#347 settings-prose` took away, put back beside its label: nothing wires it any more.
  const back = HTML.replace('<select id="relay-to-agent"',
    '<button class="help-link" id="help-link-speech-prep-relay" type="button" data-help="speech-prep">?</button><select id="relay-to-agent"');
  assert.notEqual(back, HTML, "the negative control changed nothing");
  assert.deepStrictEqual(helpLinkProblems(back, SCRIPT_CODE),
    ['#help-link-speech-prep-relay opens "speech-prep" and nothing in web/voice.js wires it']);
  // ...and an entry taken out of the script's list, which leaves its own ? unwired.
  const unlisted = SCRIPT_CODE.replace(/(const HELP_TOPICS = \[[^\]]*?)  "combining",\n/, "$1");
  assert.notEqual(unlisted, SCRIPT_CODE, "the negative control changed nothing");
  assert.deepStrictEqual(helpLinkProblems(HTML, unlisted), ['#help-link-combining opens "combining" and nothing in web/voice.js wires it']);
});

test("the settings screen is CONTROLS, not paragraphs", () => {
  // The measurable form of the owner's complaint. Not a style rule — a budget: no single hint on
  // the settings screen may be an essay, because that is what pushed the switches off the screen.
  const settings = HTML.slice(
    HTML.indexOf('id="screen-settings"'),
    HTML.indexOf('id="screen-help"')
  );
  const hints = [...settings.matchAll(/<p class="hint">([\s\S]*?)<\/p>/g)].map((m) =>
    m[1].replace(/<[^>]+>/g, " ").replace(/\s+/g, " ").trim()
  );
  const essays = hints.filter((text) => text.length > 160);
  assert.deepStrictEqual(
    essays.map((text) => `${text.slice(0, 60)}…`),
    [],
    "a paragraph that long belongs on the help screen"
  );
  // And the long ones really are on the other screen rather than having been deleted.
  const help = HTML.slice(HTML.indexOf('id="screen-help"'));
  assert.ok(
    [...help.matchAll(/<p[^>]*class="hint"[^>]*>([\s\S]*?)<\/p>/g)].some(
      (m) => m[1].replace(/\s+/g, " ").length > 400
    ),
    "the paragraphs were removed rather than moved"
  );
});

test("CONNECTION STATUS IS LAST, because it is a readout and not a setting", () => {
  const settings = HTML.slice(
    HTML.indexOf('id="screen-settings"'),
    HTML.indexOf('id="screen-help"')
  );
  const headings = settingsHeadings(settings, 3);
  assert.ok(headings.length > 3, "the settings screen lost its groups");
  assert.equal(
    headings[headings.length - 1],
    "Connection status",
    "the readout is still standing where a reader arrives looking for a switch"
  );
  assert.notEqual(headings[0], "Connection status");
  // Renamed as well as moved: "Connection" reads as somewhere you configure one.
  assert.ok(!headings.includes("Connection"), "the old bare heading is still there");
  const sections = settingsHeadings(settings, 2);
  assert.ok(new RegExp(settingsHeading(3, "Connection status")).test(settingsSection(sections[sections.length - 1])),
    "the readout is not in the last section");
});

test("a ? opens Help, and the way back goes to Settings rather than out of it", async () => {
  const page = newPage();
  await signIn(page);
  await page.el("open-settings").click();
  assert.equal(page.screen(), "settings");

  await page.el("help-link-resuming").click();

  assert.equal(page.screen(), "help", "the ? did not open Help");
  assert.equal(page.el("topbar-title").textContent, "Help");
  assert.equal(page.el("close-help").hidden, false, "Help has no way back");
  assert.equal(page.el("close-settings").hidden, true, "two ways back are showing at once");

  await page.el("close-help").click();
  assert.equal(page.screen(), "settings", "leaving Help did not return to Settings");

  // ...and Settings still leaves to where the reader actually was, rather than back into Help.
  await page.el("close-settings").click();
  assert.equal(page.screen(), "main");
});

test("EVERY HELP ENTRY IS A FOLDED DISCLOSURE, its heading the summary, under one Expand all (#246)", () => {
  assert.ok(HELP_SLUGS.length >= 15, `the Help screen has only ${HELP_SLUGS.length} entries`);
  for (const slug of HELP_SLUGS) {
    const tag = new RegExp(`<([a-z]+) id="help-${slug}"([^>]*)>\\s*<summary><h2>[^<]+</h2></summary>`).exec(HTML_CODE);
    assert.ok(tag, `help-${slug} is not a disclosure whose summary is its heading`);
    assert.equal(tag[1], "details", `help-${slug} is a <${tag[1]}>, which does not fold natively`);
    assert.doesNotMatch(tag[2], /\bopen\b/, `help-${slug} is served open`);
  }
  // Above the first entry, inside the Help screen's column.
  const button = HTML_CODE.indexOf('id="fold-help"');
  assert.ok(button > HTML_CODE.indexOf('id="screen-help"') && button < HTML_CODE.indexOf(`id="help-${HELP_SLUGS[0]}"`),
    "Expand all is not at the top of Help");
  assert.equal(PAGE_ELEMENTS.get("fold-help").tag, "button");
  assert.match(cssBlock(".help-entry > summary"), /min-height:\s*2\.75rem/, "a folded heading is a small target");
});

test("A ? OPENS ITS OWN ENTRY AND FOLDS THE REST, every time; Open help opens none (#246)", async () => {
  const page = newPage();
  await signIn(page);
  await page.el("open-settings").click();
  // Every `?` by its own id, the round ones and the one written as words (`#347 settings-prose`).
  const links = [...HTML_CODE.matchAll(/<button class="help-link(?: help-link-text)?" id="([a-z-]+)" type="button" data-help="([a-z-]+)"/g)]
    .map((m) => [m[1], m[2]]);
  assert.ok(links.length >= 10, "the settings screen links to too few help entries to test");
  // `#347 settings-prose`: the one written as words, How messages are pre-processed for speech.
  assert.ok(links.some(([link]) => link === "help-link-speech-prep"), "the scan missed the link written as words");
  for (const [link, topic] of links) {
    if (page.screen() !== "settings") await page.el("close-help").click();
    const scrolled = page.el(`help-${topic}`).scrolledIntoView;
    await page.el(link).click();
    assert.equal(page.screen(), "help", `#${link} did not open Help`);
    assert.deepStrictEqual(openHelp(page), [topic], `#${link} opened ${openHelp(page).join(", ") || "nothing"}`);
    assert.equal(page.el(`help-${topic}`).scrolledIntoView, scrolled + 1, `#${link} did not go to its entry`);
    assert.equal(page.el("fold-help").textContent, "Expand all");
  }
  // The reader opens a second entry by hand, leaves, and comes back through another ?: only that one.
  page.el("help-storage").open = true;
  await page.el("help-storage").dispatch("toggle");
  await page.el("close-help").click();
  await page.el("help-link-microphone").click();
  assert.deepStrictEqual(openHelp(page), ["microphone"], "entries opened on an earlier visit stayed open");
  await page.el("close-help").click();
  await page.el("open-help").click();
  assert.deepStrictEqual(openHelp(page), [], "Open help, which is about nothing in particular, opened an entry");
});

test("? AND Ctrl+? OPEN ONLY THE KEYBOARD ENTRY (#246)", async () => {
  // `#344 search-shortcut`. Ctrl+? (Cmd+?) is Shift+/ with the chord; and on a Mac a browser may say
  // "/" for Cmd+Shift+/, the Shift held all the same, which is Help too.
  for (const [key, fields] of [["?", { shiftKey: true }], ["?", { ctrlKey: true, shiftKey: true, code: "Slash" }],
    ["/", { metaKey: true, shiftKey: true, code: "Slash" }]]) {
    const page = await keyboardPage();
    await press(page, page.el("scroll-area"), key, fields);
    assert.equal(page.screen(), "help", `${JSON.stringify(fields)} ${key} did not open Help`);
    assert.deepStrictEqual(openHelp(page), ["keyboard"], `${key} opened ${openHelp(page).join(", ") || "nothing"}`);
    assert.equal(page.el("search-field").hidden, true, `${JSON.stringify(fields)} ${key} opened the search`);
  }
});

test("EXPAND ALL OPENS EVERY ENTRY AND THEN FOLDS THEM ALL, its word following the entries (#246)", async () => {
  const page = newPage();
  await signIn(page);
  await page.el("open-settings").click();
  await page.el("open-help").click();
  const all = page.el("fold-help");
  assert.equal(all.textContent, "Expand all");
  await all.click();
  // Every entry on the screen: this server has no message cache, so its entry is hidden and stays
  // out of it (`#208 persistent-cache`).
  const shown = HELP_SLUGS.filter((slug) => !page.el(`help-${slug}`).hidden);
  assert.deepStrictEqual(shown, HELP_SLUGS.filter((slug) => slug !== "message-cache"));
  assert.deepStrictEqual(openHelp(page), shown, "Expand all left entries folded");
  assert.equal(all.textContent, "Fold all");
  // One folded by hand (or by the browser's find opening one, which fires the same event).
  page.el("help-keyboard").open = false;
  await page.el("help-keyboard").dispatch("toggle");
  assert.equal(all.textContent, "Expand all", "with an entry folded the button still offers to fold");
  page.el("help-keyboard").open = true;
  await page.el("help-keyboard").dispatch("toggle");
  assert.equal(all.textContent, "Fold all");
  await all.click();
  assert.deepStrictEqual(openHelp(page), [], "Fold all left entries open");
  assert.equal(all.textContent, "Expand all");
});

test("both documents are held to a column, so a checkbox is not a metre from its label", () => {
  // The reading-column block at the foot of the stylesheet deliberately exempted these two — "they
  // are forms rather than reading, and giving them a column is a separate judgement nobody has
  // made yet". The owner has now made it.
  const column = cssBlock(".settings-column");
  assert.match(column, /max-width/, "the settings column is not bounded at all");
  assert.match(column, /margin-inline:\s*auto/, "a bounded column that is not centred");
  // NOT the reader's transcript width: that is a choice about the transcript, and dragging it has
  // no business reflowing a form.
  assert.doesNotMatch(
    column,
    /var\(--reading-width\)/,
    "the form's width now follows the transcript handle"
  );
  for (const id of ["screen-settings", "screen-help"]) {
    assert.ok(
      HTML.slice(HTML.indexOf(`id="${id}"`)).indexOf('class="settings-column"') > 0,
      `#${id} does not put its content in a column`
    );
  }
});

function crowd() {
  return [
    message({ id: "1000000000000000001", author: "alice", author_id: "10", author_is_bot: false }),
    message({ id: "1000000000000000002", author: "MyDiscordBot", author_id: "20", author_is_bot: true }),
    message({ id: "1000000000000000003", author: "alice", author_id: "10", author_is_bot: false }),
  ];
}

test("`#488 slack-app-agents`: who is who is chosen once for every device: another device's choice redraws the rows here", async () => {
  // The owner, 2026-10-21, on Slack: his coding agent's messages, posted through a chat bridge's app,
  // were drawn as another bot. The choice in Settings fixes that, and now syncs as his other settings do.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [
    ...crowd(),
    message({ id: "1000000000000000004", author: "chat-bridge", author_id: "30", author_is_bot: true }),
  ]);
  assert.equal(whoOf(page, 3), "bot", "the premise: with two bots, neither is guessed");
  assert.ok(SETTINGS.SYNCED_SETTING_KEYS.includes("vibe-talk.voice.identities"), "who is who is not synced");
  // What the sync does with a choice another device made: keep it, then apply it.
  page.pure("deviceStorage()").setItem("vibe-talk.voice.identities", JSON.stringify({ 30: "coder" }));
  page.pure("applySyncedSetting")("vibe-talk.voice.identities");
  await page.settle();
  assert.equal(whoOf(page, 3), "coder", "another device's choice did not redraw the row");
  assert.equal(whoOf(page, 1), "bot", "a choice about one account moved another");
  // A value this page cannot read is no choice: every account back to its guess.
  page.pure("deviceStorage()").setItem("vibe-talk.voice.identities", "{not json");
  page.pure("applySyncedSetting")("vibe-talk.voice.identities");
  await page.settle();
  assert.equal(whoOf(page, 3), "bot");
});

test("`#488 slack-app-agents`: the first sync merges who is who with the server's, the server's winning an account both name", async () => {
  // From review: each device had choices of its own before they synced; taking the server's whole
  // would lose this device's.
  const page = newPage(new Map([["vibe-talk.voice.identities", JSON.stringify({ 30: "bot", 40: "me" })]]));
  await signIn(page);
  // The server's choice as another device's frame brings it, before this device has agreed anything.
  page.pure("onSettingsFrame")({ data: JSON.stringify({ settings: [{ key: "vibe-talk.voice.identities",
    value: JSON.stringify({ 30: "coder", 50: "human" }), updated_ms: 5 }] }) });
  await page.settle();
  const kept = JSON.parse(page.pure("deviceStorage()").getItem("vibe-talk.voice.identities"));
  assert.deepEqual({ ...kept }, { 30: "coder", 40: "me", 50: "human" });
  // And what is agreed is still the server's value, so the merge goes back to it on the next pass.
  assert.equal(page.pure("settingsSync()").agreed("vibe-talk.voice.identities").value, JSON.stringify({ 30: "coder", 50: "human" }));
  assert.equal(SETTINGS.mergedIdentities("[1]", "{}"), null, "a value that is not a map was merged");
});

test("A LONE BOT IS GUESSED TO BE THE CODING AGENT, AND PEOPLE ARE GUESSED TO BE PEOPLE", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, crowd());

  assert.equal(whoOf(page, 0), "human");
  assert.equal(whoOf(page, 1), "coder", "the only bot in the channel is not the coding agent");
  assert.equal(whoOf(page, 2), "human");
});

test("...and with TWO bots the guess refuses rather than picking one", async () => {
  // The heuristic is "the only bot that is not us". With two candidates it is a coin toss, and a
  // coin toss that dresses one of them as the principal speaker is worse than saying nothing.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [
    ...crowd(),
    message({ id: "1000000000000000004", author: "some-other-bot", author_id: "30", author_is_bot: true }),
  ]);

  assert.equal(whoOf(page, 1), "bot", "it still claims to know which bot is the coding agent");
  assert.equal(whoOf(page, 3), "bot");
});

// `#323 slack-system-bots`. A Slack workspace's welcome bot, alone in a channel with the owner and
// another person: the lone-bot guess drew it as the coding agent, nameless and in the agent's tile.
function welcomed() {
  return [
    message({ id: "1000000000000000001", author: "Welcome bot", author_id: "B0WELCOME1", author_is_bot: true }),
    message({ id: "1000000000000000002", author: "alice", author_id: "10", author_is_bot: false }),
  ];
}

const authorLine = (li) => li.descendants().find((node) => node.hasClass("msg-author"));

test("A SLACK SYSTEM BOT IS ANOTHER BOT, DRAWN WITH ITS NAME AND (bot), NEVER THE CODING AGENT", async () => {
  const page = newPage();
  page.codingAgentsNamedOnly = true;
  await signIn(page);
  await showDiscord(page, welcomed());

  assert.equal(whoOf(page, 0), "bot", "the lone system bot was taken to be the coding agent");
  const named = authorLine(row(page, 0));
  assert.equal(named.hidden, false, "the system bot's name is hidden, as the coding agent's is");
  assert.equal(named.textContent, "Welcome bot (bot)", "the system bot is not named and labelled");
  assert.equal(whoOf(page, 1), "human");
});

test("...the control: where the server allows the guess, a lone bot is still the coding agent", async () => {
  // Without it the test above passes on a page that never draws a coding agent at all.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, welcomed());

  assert.equal(whoOf(page, 0), "coder", "the lone-bot guess is gone where nothing turned it off");
  assert.equal(authorLine(row(page, 0)).hidden, true, "the coding agent's name is drawn twice");
});

test("A CONFIGURED CODING AGENT IS THE CODING AGENT BESIDE A SYSTEM BOT, AND SETTINGS STILL DECIDES", async () => {
  const store = new Map();
  const page = newPage(store);
  page.codingAgentIds = ["20"];
  page.codingAgentsNamedOnly = true;
  await signIn(page);
  await showDiscord(page, [
    ...welcomed(),
    message({ id: "1000000000000000003", author: "MyDiscordBot", author_id: "20", author_is_bot: true }),
  ]);

  assert.equal(whoOf(page, 0), "bot", "the system bot was drawn as a coding agent");
  assert.equal(whoOf(page, 2), "coder", "the configured coding agent beside a second bot was not drawn as one");

  // The reader's own choice beats the configuration, both ways.
  await page.el("open-settings").click();
  const choose = async (id, bucket) => {
    const select = page.el("identity-list").descendants()
      .find((node) => node.getAttribute("data-author-id") === id);
    assert.ok(select, `Settings offers no row for ${id}`);
    select.value = bucket;
    await select.dispatch("change");
  };
  await choose("B0WELCOME1", "coder");
  await choose("20", "bot");
  assert.equal(whoOf(page, 0), "coder", "a choice in Settings did not beat the configuration");
  assert.equal(whoOf(page, 2), "bot", "a choice in Settings did not beat the configuration");
});

test("EACH PROVIDER'S CHANNELS FOLLOW THEIR OWN PROVIDER'S CODING-AGENT RULE", async () => {
  const page = newPage();
  const team = { id: "C0194ABCDE", label: "team", writable: true, alias: null, added: false, provider: "slack" };
  const space = { id: "1110000000000000301", label: "chat space", writable: true, alias: null, added: false, provider: "gchat" };
  const describe = (key, name, more) => ({ key, name, threading_supported: false, live_delivery: "poll",
    live_poll_seconds: 30, channel_registration_supported: false, channel_discovery_supported: false,
    upstream_read_mark_supported: false, self_author_id: null, owner_author_id: null, ...more });
  page.channels = [team, space];
  page.providers = [describe("slack", "Slack", { coding_agents_named_only: true }), describe("gchat", "Google Chat", {})];
  page.messages = welcomed();
  // Each channel holds the same two messages, as its own.
  page.channelPage = async (path) => {
    const channel = String(path).includes(encodeURIComponent(space.id)) ? space : team;
    return json(200, {
      channel, messages: page.messages.map((m) => ({ ...m, channel_id: channel.id })), dismissed: [],
    });
  };
  await signIn(page);
  for (const [channel, lone] of [[team, "bot"], [space, "coder"], [team, "bot"]]) {
    if (page.el("discord-channel").value !== channel.id) {
      page.el("discord-channel").value = channel.id;
      await page.el("discord-channel").dispatch("change");
      await page.settle();
    } else {
      await showDiscord(page, page.messages);
    }
    assert.equal(whoOf(page, 0), lone, `${channel.label}: the lone bot followed another provider's rule`);
  }
});

test("REPLYING TEACHES THE PAGE WHICH ACCOUNT IS ITS OWN, AND THAT ACCOUNT IS 'ME'", async () => {
  // Learned for free and by construction: the reply comes back as the Message Discord recorded, so
  // its author IS this bridge. No /users/@me call, no name matching, nothing configured.
  const store = new Map();
  const page = newPage(store);
  await signIn(page);
  await openReplyOn(page, crowd(), 0);
  page.el("reply-text").value = "answering";
  await page.el("reply-send").click();
  await page.settle();

  const rows = page.el("discord-log").children;
  const posted = rows[rows.length - 1];
  assert.equal(posted.getAttribute("data-who"), "me", "our own reply is not shown as ours");
  assert.equal(store.get("vibe-talk.voice.self-id"), "1000000000000000009");

  // ...and it is remembered, so the next session already knows without another reply.
  const again = reloadWith(store, [
    message({ id: "1000000000000000050", author: "My Voice Bot", author_id: "1000000000000000009", author_is_bot: true }),
  ]);
  await signIn(again);
  await reopenedChannel(again);
  assert.equal(
    whoOf(again, 0),
    "me",
    "a bot posting on the owner's behalf is not shown as the owner"
  );
});

test("the reader's own choice beats every guess, and survives a reload", async () => {
  const store = new Map();
  const page = newPage(store);
  await signIn(page);
  await showDiscord(page, crowd());
  assert.equal(whoOf(page, 1), "coder", "the guess under test never happened");

  await page.el("open-settings").click();
  const select = page
    .el("identity-list")
    .descendants()
    .find((node) => node.getAttribute("data-author-id") === "20");
  assert.ok(select, "Settings offers no row for an author that has spoken");
  assert.equal(select.getAttribute("data-guessed"), "true", "a guess is not marked as one");
  select.value = "bot";
  await select.dispatch("change");

  assert.equal(whoOf(page, 1), "bot", "the channel was not redrawn from the choice");
  assert.equal(select.getAttribute("data-guessed"), "false", "it still reads as a guess");

  const again = reloadWith(store, crowd());
  await signIn(again);
  await reopenedChannel(again);
  assert.equal(whoOf(again, 1), "bot", "the choice did not survive a reload");
});

test("the channel's two principals are drawn as the TRANSCRIPT's two speakers, not a second palette", () => {
  // The same person saying things in two views must not be two colours. This is the assertion that
  // keeps the two idioms one idiom.
  const mine = cssBlock(".messages li.mine");
  const theirs = cssBlock(".messages li.theirs");
  const me = cssBlock('#discord-log li.discord-message[data-who="me"]');
  const coder = cssBlock('#discord-log li.discord-message[data-who="coder"]');

  for (const [property, transcript, channel] of [
    ["background", mine, me],
    ["background", theirs, coder],
  ]) {
    const want = new RegExp(`${property}:\\s*var\\((--[a-z-]+)\\)`).exec(transcript);
    assert.ok(want, `the transcript rule does not set ${property} from a custom property`);
    assert.match(
      channel,
      new RegExp(`${property}:\\s*var\\(${want[1]}\\)`),
      `the channel uses a different ${property} from the transcript for the same speaker`
    );
  }
  // And the side, which is the other half of the signal.
  assert.match(me, /margin-left/, "the owner's own messages are not offset like the transcript's");
  assert.match(coder, /margin-right/, "the coding agent's messages are not offset the other way");
});

test("an author label is never treated as markup, even in Settings", async () => {
  // The one place outside the message list where a display name is rendered. A name is written by
  // whoever owns the account, and this panel exists because it can be anything at all.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [
    message({
      id: "1000000000000000001",
      author: "<img src=x onerror=alert(1)>",
      author_id: "77",
      author_is_bot: false,
    }),
  ]);
  await page.el("open-settings").click();

  assert.deepStrictEqual(
    page.createdTags.filter((tag) => tag === "img"),
    [],
    "a display name became an element"
  );
  const name = page
    .el("identity-list")
    .descendants()
    .find((node) => node.className === "identity-name");
  assert.match(name.text(), /<img src=x/, "the name was not rendered as the text it is");
});

/** The display names the identity panel is currently listing, in the order it lists them. */
const listedNames = (page) =>
  page
    .el("identity-list")
    .descendants()
    .filter((node) => node.className === "identity-name")
    .map((node) => node.text());

/**
 * What is wrong with Who is who's place against `#402 who-is-who-channel-box`, as sentences; empty when
 * nothing is. A function of the markup, so the negative control can hand it the list where `#349` put it.
 */
function whoIsWhoPlaceProblems(html) {
  const found = [];
  const code = html.replace(/<!--[\s\S]*?-->/g, " ");
  const box = channelBoxCode(code);
  const at = box.indexOf('id="identity-list"');
  if (at < 0) found.push("the channel box has no Who is who list");
  if (at >= 0 && !(box.indexOf('id="upstream-clear-state"') < at)) found.push("Who is who is not under Clear unread");
  if (!new RegExp(settingsHeading(3, "Who is who")).test(box)) found.push("the list in the box has no Who is who heading");
  if (!/data-help="identities"/.test(box)) found.push("Who is who has no way to the entry that explains the guesses");
  if (/Who is who|id="identity-list"/.test(settingsSection("Messages", code))) found.push("Who is who is still in Messages");
  if ((code.match(/id="identity-list"/g) || []).length !== 1) found.push("there is not exactly one list");
  return found;
}

test("`#402 who-is-who-channel-box`: WHO IS WHO IS AT THE FOOT OF THE CHANNEL BOX, out of Messages, and names its channel", async () => {
  // The owner, 2026-10-21: "it should stay under the channel section along with everything that varies
  // depending on selecting the channel in the drop down". `#349 channel-settings-all` had moved it to
  // Messages; it is back where it was before that, under Clear unread.
  assert.deepStrictEqual(whoIsWhoPlaceProblems(HTML), []);
  // The negative control: the list where `#349` put it, a group of Messages before Recently dismissed.
  const box = /<!--\s*WHO IS WHO:[\s\S]*?-->\s*<div id="identity-group">[\s\S]*?<p id="identity-state"[^>]*><\/p>\s*<\/div>\s*/.exec(HTML);
  assert.ok(box, "the negative control cannot find the list in the box");
  const group = '<section class="settings-group" id="identity-group"><div class="group-head">' +
    '<h3 title="Everyone who has spoken in the channel you are reading.">Who is who</h3>' +
    '<button class="help-link" id="help-link-identities" type="button" data-help="identities">?</button></div>' +
    '<div id="identity-list"></div><p id="identity-state" class="hint" aria-live="polite"></p></section>';
  const back = HTML.replace(box[0], "").replace('<section class="settings-group" id="dismissed-group">',
    `${group}<section class="settings-group" id="dismissed-group">`);
  assert.deepStrictEqual(whoIsWhoPlaceProblems(back), ["the channel box has no Who is who list",
    "the list in the box has no Who is who heading", "Who is who has no way to the entry that explains the guesses",
    "Who is who is still in Messages"]);
  const page = newPage();
  await signIn(page);
  await showDiscord(page, crowd());
  await page.el("open-settings").click();
  assert.match(page.el("identity-list").children[0].textContent, /^In lead team:$/,
    "the list does not say which channel it is about");
});

test("`#402 who-is-who-channel-box`: the list follows the channel box's picker, and the channel being read does not move it", async () => {
  const page = newPage();
  twoCasts(page);
  await signIn(page);
  await showDiscord(page, []);
  await readChannel(page, OTHER_CHANNEL.id);
  await readChannel(page, CHANNEL.id);
  await page.el("open-settings").click();
  assert.equal(page.el("settings-channel").value, CHANNEL.id);
  assert.deepStrictEqual(listedNames(page), ["alice"], "the list is not scoped to one channel");
  // The picker is the list's: another channel there lists that channel's cast, and navigates nowhere.
  await settingsChannel(page, OTHER_CHANNEL.id);
  assert.deepStrictEqual(listedNames(page), ["mallory"], "the box's picker did not move the list");
  assert.match(page.el("identity-list").children[0].textContent, /^In build noise:$/);
  assert.equal(page.el("discord-channel").value, CHANNEL.id, "a Settings redraw navigated");
  await settingsChannel(page, CHANNEL.id);
  assert.deepStrictEqual(listedNames(page), ["alice"]);
  // Reading another channel and coming back to Settings leaves the list on the box's channel.
  await page.el("close-settings").click();
  await readChannel(page, OTHER_CHANNEL.id);
  await page.el("open-settings").click();
  assert.equal(page.el("settings-channel").value, CHANNEL.id);
  assert.deepStrictEqual(listedNames(page), ["alice"], "the channel being read moved the list off the box's channel");
  // The negative control: the list as `#349` drew it, following the channel being read. The box's picker
  // then leaves it where it was, which is how this test would fail.
  const before = newPage(new Map(), brokenScript('  const host = el("identity-list");\n  const channelId = el("settings-channel").value;',
    '  const host = el("identity-list");\n  const channelId = selectedChannelId();'));
  twoCasts(before);
  await signIn(before);
  await showDiscord(before, []);
  await readChannel(before, OTHER_CHANNEL.id);
  await readChannel(before, CHANNEL.id);
  await before.el("open-settings").click();
  await settingsChannel(before, OTHER_CHANNEL.id);
  assert.deepStrictEqual(listedNames(before), ["alice"], "the negative control follows the box's picker anyway");
});

test("`#402 who-is-who-channel-box`: on All channels the list asks for one channel, and a channel chosen lists its cast again", async () => {
  const page = await allChannelsPage({}, twoCasts);
  await page.el("close-settings").click();
  await readChannel(page, OTHER_CHANNEL.id);
  await readChannel(page, CHANNEL.id);
  await page.el("open-settings").click();
  assert.deepStrictEqual(listedNames(page), ["alice"]);
  await settingsChannel(page, "*");
  assert.equal(page.el("identity-list").text(), "Pick one channel to see who is who.",
    "All channels does not ask for one channel");
  assert.deepStrictEqual(page.el("identity-list").descendants().filter((node) => node.tagName === "select"), [],
    "All channels offers a row to change");
  await settingsChannel(page, OTHER_CHANNEL.id);
  assert.deepStrictEqual(listedNames(page), ["mallory"], "a channel chosen after All channels lists no cast");
  // A choice made in one channel's list, then All channels: the line about it goes with the list.
  await chooseIdentity(page, "coder");
  assert.match(page.el("identity-state").textContent, /^Saved\./);
  await settingsChannel(page, "*");
  assert.equal(page.el("identity-state").textContent, "", "the line about a choice outlived the list it was about");
  // `#355 provider-grouped-channels`: a service's All channels is All channels too.
  const services = await servicesPage((one) => {
    one.channels = [{ ...TEAM_A }, { ...TEAM_B }, { ...SPACE_A }];
  });
  await services.el("open-settings").click();
  await services.settle();
  assert.equal(pickerChoices(services)[0][1], "All Slack channels");
  await settingsChannel(services, "*");
  assert.equal(services.el("identity-list").text(), "Pick one channel to see who is who.");
  // The negative control: a list with no word for All channels draws one for a channel that is not one.
  const blind = newPage(new Map(), brokenScript("  if (channelId === ALL_CHANNELS) {\n", "  if (false) {\n"));
  blind.chatProviderName = "Google Chat";
  blind.upstreamClearSupported = true;
  twoCasts(blind);
  await signIn(blind);
  await blind.el("open-settings").click();
  await blind.settle();
  await settingsChannel(blind, "*");
  assert.notEqual(blind.el("identity-list").text(), "Pick one channel to see who is who.",
    "the negative control asked for one channel anyway, so the check above proves nothing");
});

test("`#402 who-is-who-channel-box`: a channel added from Browse moves the box, and who is who names it", async () => {
  const addFromBrowse = async (script = SCRIPT) => {
    const page = newPage(new Map(), script);
    page.channelRegistrationSupported = true;
    page.channelDiscoverySupported = true;
    page.directory = browsingPage().directory;
    await openBrowser(page);
    const row = page.el("channel-directory-list").children[1];
    await row.children.find((kid) => kid.tagName === "button").click();
    await page.settle();
    return page;
  };
  const page = await addFromBrowse();
  assert.equal(String(page.el("settings-channel").value), String(page.registeredChannelId));
  assert.match(page.el("identity-list").text(), /^Nobody yet in Release Train\./, "who is who is still drawn for the last channel");
  // The negative control: the add without the box redrawn leaves who is who on the channel before.
  const stale = await addFromBrowse(brokenScript("  if (entry.channel_id) pointSettingsAt(entry.channel_id);\n" +
    "  // The box onto the channel just added, as `addChannel` does: with one chat service `pointSettingsAt`\n" +
    "  // only sets the picker, which fires nothing, and who is who names its channel (`#402\n" +
    "  // who-is-who-channel-box`), so a box left drawn for the last channel says so in words.\n" +
    "  renderChannelBox();\n", "  if (entry.channel_id) pointSettingsAt(entry.channel_id);\n"));
  assert.doesNotMatch(stale.el("identity-list").text(), /Release Train/,
    "the negative control named the new channel anyway, so the check above proves nothing");
});

test("`#402 who-is-who-channel-box`: a service with no channel on the list draws no Who is who over nothing", async () => {
  const emptyService = async (script = SCRIPT) => {
    const reference = await servicesPage();
    const page = newPage(new Map(), script);
    twoServices(page);
    page.providers = reference.providers;
    page.channels = [{ ...TEAM_A }];
    await signIn(page);
    await page.el("open-settings").click();
    await page.settle();
    const states = [page.el("identity-group").hidden];
    await chooseService(page, "gchat");
    states.push(page.el("identity-group").hidden);
    await chooseService(page, "slack");
    states.push(page.el("identity-group").hidden);
    return { page, states };
  };
  const { page, states } = await emptyService();
  await chooseService(page, "gchat");
  assert.deepStrictEqual(settingsOptions(page), []);
  assert.deepStrictEqual(states, [false, true, false], "Who is who is headed over an empty list, or did not come back");
  // The negative control: a list that never hides its group.
  const blind = await emptyService(brokenScript('  el("identity-group").hidden = none;\n', ""));
  assert.deepStrictEqual(blind.states, [false, false, false],
    "the negative control hid the group anyway, so the check above proves nothing");
});

/** Two channels, each with its own one-person cast: alice in CHANNEL, mallory in OTHER_CHANNEL. */
function twoCasts(page) {
  page.channels = [{ ...CHANNEL }, { ...OTHER_CHANNEL }];
  serveByChannel(page, {
    [CHANNEL.id]: [message({ id: "3000000000000000001", channel_id: CHANNEL.id, author: "alice", author_id: "10", author_is_bot: false })],
    [OTHER_CHANNEL.id]: [message({ id: "3000000000000000002", channel_id: OTHER_CHANNEL.id, author: "mallory", author_id: "30", author_is_bot: false })],
  });
}

/** Choose `bucket` for the first account in the list of who is who, as the reader does. */
async function chooseIdentity(page, bucket) {
  const select = page.el("identity-list").descendants().find((node) => node.tagName === "select");
  select.value = bucket;
  await select.dispatch("change");
}

test("`#351 settings-polish`: who is who's line about a choice goes when the box's channel changes, and stays while it does not", async () => {
  const page = newPage();
  twoCasts(page);
  await signIn(page);
  await showDiscord(page, []);
  await readChannel(page, OTHER_CHANNEL.id);
  await readChannel(page, CHANNEL.id);
  await page.el("open-settings").click();
  await chooseIdentity(page, "coder");
  assert.match(page.el("identity-state").textContent, /^Saved\./);
  // The same channel, Settings left and opened again: still the report about the list on screen.
  await page.el("close-settings").click();
  await page.el("open-settings").click();
  assert.match(page.el("identity-state").textContent, /^Saved\./, "the line went with the list unchanged");
  // `#402 who-is-who-channel-box`. Another channel read changes nothing here: the list is the box's.
  await page.el("close-settings").click();
  await readChannel(page, OTHER_CHANNEL.id);
  await page.el("open-settings").click();
  assert.match(page.el("identity-state").textContent, /^Saved\./, "reading another channel cleared a line about the box's list");
  // Another channel in the box: its list, and no report about the last one's.
  await settingsChannel(page, OTHER_CHANNEL.id);
  assert.deepStrictEqual(listedNames(page), ["mallory"]);
  assert.equal(page.el("identity-state").textContent, "", "the line still reports a choice made in another channel's list");
  // The box's channel removed from under the list: the picker moves on, the list with it, and the line
  // about a choice made in the removed one's goes.
  await chooseIdentity(page, "bot");
  assert.match(page.el("identity-state").textContent, /^Saved\./);
  await page.el("remove-channel").click();
  await page.settle();
  assert.equal(page.el("settings-channel").value, CHANNEL.id);
  assert.deepStrictEqual(listedNames(page), ["alice"], "the list stayed on the channel removed from under it");
  assert.equal(page.el("identity-state").textContent, "", "the line outlived the list it was about");
  // The negative control: a list that never clears the line.
  const blind = newPage(new Map(), brokenScript('    el("identity-state").textContent = "";\n    identityListChannel = String(channelId);\n',
    "    identityListChannel = String(channelId);\n"));
  twoCasts(blind);
  await signIn(blind);
  await showDiscord(blind, []);
  await readChannel(blind, OTHER_CHANNEL.id);
  await readChannel(blind, CHANNEL.id);
  await blind.el("open-settings").click();
  await chooseIdentity(blind, "coder");
  await settingsChannel(blind, OTHER_CHANNEL.id);
  assert.match(blind.el("identity-state").textContent, /^Saved\./, "the negative control cleared the line anyway, so the check above proves nothing");
});

test("`#351 settings-polish`: with two columns open, who is who lists the authors either column learns in the box's channel", async () => {
  // Both columns read one channel: the bar's picker, which is not one of the elements the right column
  // stands in for (COLUMN_IDS), so the right column cannot learn authors of another channel. This pins
  // that: with a thread drawn in the right column, its authors are listed under the line naming the channel.
  assert.ok(!/^\s*"discord-channel":/m.test(SCRIPT_CODE.slice(SCRIPT_CODE.indexOf("const COLUMN_IDS = {"),
    SCRIPT_CODE.indexOf("};", SCRIPT_CODE.indexOf("const COLUMN_IDS = {")))), "the right column has a channel picker of its own");
  const { page, data } = await twoColumnServer((page, data) => {
    const answer = data.messages.find((one) => one.id === "204");
    Object.assign(answer, { author: "zed", author_id: "4000000000000000044", author_is_bot: false });
  });
  await openerOf(cardRow(page, data.threads[1].id)).click();
  await page.settle();
  assert.equal(page.el("side-pane").hidden, false, "the thread is not open in the right column");
  await page.el("open-settings").click();
  assert.ok(listedNames(page).includes("zed"), `an author of the right column's thread is not listed: ${listedNames(page)}`);
  assert.match(page.el("identity-list").children[0].textContent, /^In lead team:$/);
});

test("a channel nobody has spoken in yet says WHICH channel is empty rather than showing a blank panel", async () => {
  const page = newPage();
  page.channels = [{ ...CHANNEL }, { ...OTHER_CHANNEL }];
  serveByChannel(page, { [CHANNEL.id]: [], [OTHER_CHANNEL.id]: [] });
  await signIn(page);
  await page.el("open-settings").click();
  await settingsChannel(page, OTHER_CHANNEL.id);
  assert.match(page.el("identity-list").text(), /^Nobody yet in build noise\./, "it does not say what is empty");
});

test("A BOT THAT ONLY SPEAKS SOMEWHERE ELSE CANNOT SPOIL THE GUESS HERE", async () => {
  // The bug the scoping fixes, stated as the owner would hit it: one coding agent in the channel
  // he is reading, some unrelated bot in a channel he opened once, and the guess goes quiet.
  const page = newPage();
  page.channels = [{ ...CHANNEL }, { ...OTHER_CHANNEL }];
  serveByChannel(page, {
    [CHANNEL.id]: [
      message({ id: "3000000000000000011", channel_id: CHANNEL.id, author: "alice", author_id: "10", author_is_bot: false }),
      message({ id: "3000000000000000012", channel_id: CHANNEL.id, author: "MyDiscordBot", author_id: "20", author_is_bot: true }),
    ],
    [OTHER_CHANNEL.id]: [
      message({ id: "3000000000000000013", channel_id: OTHER_CHANNEL.id, author: "some-other-bot", author_id: "30", author_is_bot: true }),
    ],
  });
  await signIn(page);
  await showDiscord(page, []);
  await readChannel(page, OTHER_CHANNEL.id);
  assert.equal(whoOf(page, 0), "coder", "the lone bot of the channel being read is not the agent");
  await readChannel(page, CHANNEL.id);

  assert.equal(
    whoOf(page, 1),
    "coder",
    "a bot from another channel was counted against this channel's only candidate"
  );
});

test("...but a CHOICE is about the account, so it holds in every channel that account speaks in", async () => {
  // The deliberate asymmetry. A census is about a room; an account is the same account wherever it
  // speaks, and making the owner name his coding agent once per channel would be the worse failure.
  const page = newPage();
  page.channels = [{ ...CHANNEL }, { ...OTHER_CHANNEL }];
  const bot = { author: "MyDiscordBot", author_id: "20", author_is_bot: true };
  serveByChannel(page, {
    [CHANNEL.id]: [message({ id: "3000000000000000021", channel_id: CHANNEL.id, ...bot })],
    [OTHER_CHANNEL.id]: [message({ id: "3000000000000000022", channel_id: OTHER_CHANNEL.id, ...bot })],
  });
  await signIn(page);
  await showDiscord(page, []);
  await readChannel(page, OTHER_CHANNEL.id);
  await readChannel(page, CHANNEL.id);
  assert.equal(whoOf(page, 0), "coder", "the guess under test never happened");

  await page.el("open-settings").click();
  const select = page
    .el("identity-list")
    .descendants()
    .find((node) => node.getAttribute("data-author-id") === "20");
  select.value = "bot";
  await select.dispatch("change");
  assert.match(
    page.el("identity-state").textContent,
    /every channel/,
    "the panel does not say how far the choice it just saved reaches"
  );

  await page.el("close-settings").click();
  await readChannel(page, OTHER_CHANNEL.id);
  assert.equal(whoOf(page, 0), "bot", "the choice stopped at the channel it was made in");
});

test("A LONG OPTION CANNOT SQUEEZE A NAME INTO A ONE-CHARACTER COLUMN", () => {
  // Photographed on the owner's iPhone: four identity rows looking perfectly ordinary and a fifth
  // whose display name ran down the screen one letter wide. Not a mystery — arithmetic. The row was
  // `1fr auto`, a <select> is as wide as the option it is SHOWING, and only that fifth row was
  // showing the longest of the four labels. Its intrinsic track took the panel, the flexible track
  // fell to its automatic minimum, and `overflow-wrap: anywhere` had put that minimum at one
  // character. Both halves are asserted, because either one alone brings the column back.
  assert.match(
    SCRIPT_CODE,
    /Me \(including my voice bot\)/,
    "the option whose width provoked this is gone; check the arithmetic below still holds"
  );

  const row = cssRules(CSS_UNCONDITIONAL, ".identity-row").join("\n");
  const tracks = /grid-template-columns:\s*([^;]+)/.exec(row);
  assert.ok(tracks, "the identity row is no longer a grid and this test cannot speak");
  assert.doesNotMatch(
    tracks[1],
    /auto/,
    "on a phone the select still gets a track sized to its own text"
  );
  const name = cssRules(CSS_UNCONDITIONAL, ".identity-name").join("\n");
  assert.doesNotMatch(
    name,
    /overflow-wrap:\s*anywhere/,
    "`anywhere` drops the name's min-content to one character, which is the defect"
  );
  assert.match(name, /overflow-wrap:\s*break-word/, "a long name has to wrap somehow");

  // ...and the two-track layout is back where there is room for it, and only there.
  assert.match(
    cssBlockIn(DESKTOP_QUERY, ".identity-row"),
    /grid-template-columns:\s*minmax\(6rem,\s*1fr\)\s+auto/,
    "the desk does not get the name beside the thing it is being called"
  );
});

// --- `#271 settings-groups` -----------------------------------------------------------------------
//
// The owner: "organize the settings page a little more logically so it doesn't look like so many
// individual granular options" — a Voice agent group holding at least resuming and live messages, a
// Messages group, and the switch that lets the agent send without approval somewhere he can find it.

/** Every Settings section's title, in the order the markup gives them. */
const SETTINGS_SECTIONS = settingsHeadings(HTML.slice(HTML.indexOf('id="screen-settings"'), HTML.indexOf('id="screen-help"')), 2);

test("`#271 settings-groups`: Settings is a handful of sections, and every group is inside one", () => {
  assert.deepStrictEqual(
    SETTINGS_SECTIONS,
    ["Server", "Voice agent", "Messages", "Channels", "Screen and keys", "Storage and connection"],
    "the sections are not the six the screen is arranged around"
  );
  const settings = HTML.slice(HTML.indexOf('id="screen-settings"'), HTML.indexOf('id="screen-help"'));
  const groups = [...settings.matchAll(/<section class="settings-group"/g)].length;
  const inSections = SETTINGS_SECTIONS.reduce(
    (count, title) => count + [...settingsSection(title).matchAll(/<section class="settings-group"/g)].length, 0);
  assert.equal(inSections, groups, "a settings group stands outside every section");
  assert.ok(groups >= 20, `only ${groups} groups were found; one was lost in the move`);
  // A section's title is the screen's only h2; a group's heading is one step down.
  assert.equal([...settings.matchAll(/<h2\b/g)].length, SETTINGS_SECTIONS.length, "a group kept an h2");
  for (const title of SETTINGS_SECTIONS) {
    assert.match(settingsSection(title), /^<section class="settings-section" id="(settings-[a-z-]+)" aria-labelledby="\1-title">/,
      `the "${title}" section is not named by its heading`);
  }
});

test("`#271 settings-groups`: the Voice agent section holds sending, the instructions, resuming and live messages", () => {
  const voice = groupsIn("Voice agent");
  for (const group of ["Sending", "Instructions", "Resuming", "Arriving messages during a call",
    "Reading full messages in channel view", "Canned prompts", "Microphone"]) {
    assert.ok(voice.includes(group), `"${group}" is not in the Voice agent section: ${voice.join(", ")}`);
  }
  // FIRST, where the owner looks: the switch he could not find.
  assert.equal(voice[0], "Sending", `the section opens on "${voice[0]}", not on whether the agent asks first`);
  const sending = settingsGroup("Sending");
  assert.match(sending, /id="voice-ask-before-send"/);
  assert.match(sending, /<p id="voice-send-untick" class="hint">Untick it to let the agent send as soon as you ask, with no read-back\.<\/p>/,
    "nothing beside the switch says it is the way to let the agent send without asking");
  assert.equal(voice[1], "Instructions", "the instructions are not beside the switch about sending");
});

test("`#271 settings-groups`: the Messages section holds the message options, and nothing about the agent", () => {
  const messages = groupsIn("Messages");
  // `#332 settings-voice-agent-section` took the message-reading setting to Voice agent and How
  // messages are said to Help, so Messages is the marking and drawing of messages, and only that.
  // `#349 channel-settings-all` moved Who is who here, out of the per-channel box, and `#402
  // who-is-who-channel-box` put it back there.
  // `#386 hide-empty-messages` put Empty messages beside Mark-read automatically, the owner's place for it.
  // `#457 settings-server-side` moved Collapsed message lines here from Screen and keys:
  // it decides how much of a message is shown, so the server keeps it for every device,
  // and Screen and keys is the one per-device section. `#543 in-app-message-links` put Message links
  // after Rich headers: whether a link to a message opens it here is a choice about drawing messages.
  assert.deepStrictEqual(messages, ["Your own messages", "Mark-read automatically", "Empty messages", "Combining messages",
    "Rich headers", "Message links", "Summaries", "Collapsed message lines", "Thread card lines", "Recently dismissed"]);
  for (const agentGroup of ["Sending", "Instructions", "Resuming", "Arriving messages during a call",
    "Reading full messages in channel view"]) {
    assert.ok(!messages.includes(agentGroup), `"${agentGroup}" is in Messages`);
  }
  // `#481 slack-app-deep-link` put Opening links here, beside the bar that offers the links:
  // whether the Slack app is on this device is the device's own business, so it is one of the
  // per-device choices and is never synced.
  assert.deepStrictEqual(groupsIn("Screen and keys"),
    ["Text size", "Reading width", "Control bar", "Opening links", "Typing", "Keyboard"]);
  // `#457 settings-server-side`: the one per-device section says so beside its title.
  assert.match(HTML, /<h2 id="settings-screen-title"><button class="fold-toggle" id="fold-settings-screen"[^>]*>Screen and keys<\/button><span class="settings-scope-note">\(client side\)<\/span><\/h2>/,
    "Screen and keys does not say (client side) beside its title");
  assert.deepStrictEqual(groupsIn("Storage and connection"),
    ["Stored conversations", "Saved messages on this server", "Token", "Connection status"]);
  // The per-channel box is its own section, with its `?` beside the section's title.
  const channels = settingsSection("Channels");
  assert.match(channels, /<h2 id="settings-channels-title"[^>]*>(?:<button class="fold-toggle"[^>]*>)?Channels(?:<\/button>)?<\/h2>\s*<button class="help-link" id="help-link-channel-alias"/);
  assert.match(channels, /<section class="settings-group" id="channel-settings"/);
});

test("`#271 settings-groups`: the section is the card, and a group inside it is a row under a rule", () => {
  const section = cssBlock(".settings-section");
  assert.match(section, /border:\s*1px solid var\(--edge\)/, "a section is not drawn as a card");
  assert.match(section, /background:\s*var\(--panel\)/);
  // `#346 settings-collapsible`: the groups are in the section's fold body.
  const row = cssBlock(".settings-section > .fold-body > .settings-group");
  assert.match(row, /border:\s*none/, "a group inside a section still draws a card of its own");
  assert.match(row, /border-top:\s*1px solid var\(--edge\)/, "nothing says where one group ends and the next begins");
  assert.match(row, /background:\s*none/);
  const size = (block) => Number(/font-size:\s*([\d.]+)rem/.exec(block)[1]);
  assert.ok(
    size(cssBlock("#screen-settings .settings-section > h2")) > size(cssBlock("#screen-settings .settings-group > .group-head > h3")),
    "a section's title is no louder than the groups inside it"
  );
});

// --- `#332 settings-voice-agent-section` ----------------------------------------------------------
//
// The owner, 2026-10-15, building on `#271 settings-groups`: Resuming, Live messages and the
// message-reading setting together in the Voice agent section; "Message audio" renamed "Reading
// full messages in channel view" and moved there; How messages are said is information, not a
// setting, so its prose leaves Settings and is reached from beside the settings it explains; and
// each state line short, the long explanations in Help.
//
// `#347 settings-prose` (2026-10-18) renamed Live messages for its control, "Arriving messages during
// a call", and made How messages are said one short link under Reading full messages in channel
// view, "How messages are pre-processed for speech", in place of a `?` beside each of the two.

/** The three settings that read a message out, in the order the Voice agent section holds them. */
const READING_GROUPS = ["Resuming", "Arriving messages during a call", "Reading full messages in channel view"];

/**
 * What is wrong with a page's Settings against `#332`, as sentences; empty when nothing is. A function
 * of the markup, so each negative control below can hand it the arrangement from before and watch it
 * be refused.
 */
function voiceAgentSectionProblems(html) {
  const problems = [];
  const code = html.replace(/<!--[\s\S]*?-->/g, " ");
  const settings = code.slice(code.indexOf('id="screen-settings"'), code.indexOf('id="screen-help"'));
  const voice = groupsIn("Voice agent", code);
  const at = voice.indexOf(READING_GROUPS[0]);
  if (at < 0 || READING_GROUPS.some((group, i) => voice[at + i] !== group)) {
    problems.push(`Voice agent does not hold ${READING_GROUPS.join(", ")} together: ${voice.join(", ")}`);
  }
  for (const gone of ["Message audio", "How messages are said"]) {
    if (settings.includes(gone)) problems.push(`Settings still says "${gone}"`);
  }
  const reading = new RegExp(settingsHeading(3, READING_GROUPS[2])).test(settings) ? settingsGroup(READING_GROUPS[2], code) : "";
  if (!/<select id="settings-audio-source">/.test(reading)) {
    problems.push("the message-reading picker is not in Reading full messages in channel view");
  }
  if (!/<button class="help-link help-link-text" id="help-link-speech-prep" type="button" data-help="speech-prep"[^>]*>How messages are pre-processed for speech<\/button>/.test(reading)) {
    problems.push("Reading full messages in channel view has no link to How messages are pre-processed for speech");
  }
  if (settings.includes('id="speech-prep-state"')) {
    problems.push("which arm of the rewrite this server is in is still on Settings");
  }
  const help = code.slice(code.indexOf('id="help-speech-prep"'));
  if (!help.slice(0, help.indexOf("</details>")).includes('<p id="speech-prep-state" class="hint">')) {
    problems.push("How messages are said in Help does not say which arm this server is in");
  }
  return problems;
}

test("`#332 settings-voice-agent-section`: Voice agent holds Resuming, Arriving messages during a call and Reading full messages in channel view, together", () => {
  assert.deepStrictEqual(voiceAgentSectionProblems(HTML), []);
  // Still after Sending and Instructions, which `#271` put first, where the owner looks.
  assert.deepStrictEqual(groupsIn("Voice agent").slice(0, 5), ["Sending", "Instructions", ...READING_GROUPS]);
  // The picker is the one it always was: the bar's switch keeps it in step by these ids.
  const reading = settingsGroup(READING_GROUPS[2]);
  assert.match(reading, /<label for="settings-audio-source">Read messages with<\/label>/);
  assert.match(reading, /<option id="settings-agent-audio" value="agent">Agent audio<\/option>/);
  assert.match(reading, /<option value="device">Device voice<\/option>/);
});

test("`#332 settings-voice-agent-section`: the check refuses the Settings from before, one change at a time (negative controls)", () => {
  // The whole arrangement from before: the picker in Messages as Message audio, beside a group that
  // holds only which arm of the rewrite this server is in.
  const reading = `${settingsGroup(READING_GROUPS[2])}</section>`;
  const prose = '<section class="settings-group"><div class="group-head"><h3>How messages are said</h3>' +
    '<button class="help-link" id="help-link-speech-prep" type="button" data-help="speech-prep">?</button></div>' +
    '<p id="speech-prep-state" class="hint" aria-live="polite"></p></section>';
  const before = HTML
    .replace(reading, "")
    .replace('<p id="speech-prep-state" class="hint"></p>', "")
    .replace(/(<h2 id="settings-messages-title"[^>]*>(?:<button class="fold-toggle"[^>]*>)?Messages(?:<\/button>)?<\/h2>)/,
      `$1${prose}${reading.replace(READING_GROUPS[2], "Message audio").replace(/<p class="help-text-row">[\s\S]*?<\/p>/, "")}`);
  assert.notEqual(before, HTML, "the negative control changed nothing");
  assert.deepStrictEqual(voiceAgentSectionProblems(before), [
    `Voice agent does not hold ${READING_GROUPS.join(", ")} together: Sending, Instructions, Resuming, Arriving messages during a call, Canned prompts, Microphone, Provider`,
    'Settings still says "Message audio"',
    'Settings still says "How messages are said"',
    "the message-reading picker is not in Reading full messages in channel view",
    "Reading full messages in channel view has no link to How messages are pre-processed for speech",
    "which arm of the rewrite this server is in is still on Settings",
    "How messages are said in Help does not say which arm this server is in",
  ]);
  // Each change alone is caught by name: the old name, kept in place.
  const renamed = HTML.replace(new RegExp(`(<h3\\b[^>]*>(?:<button class="fold-toggle"[^>]*>)?)${READING_GROUPS[2]}<`), "$1Message audio<");
  assert.deepStrictEqual(voiceAgentSectionProblems(renamed), [
    `Voice agent does not hold ${READING_GROUPS.join(", ")} together: Sending, Instructions, Resuming, Arriving messages during a call, Message audio, Canned prompts, Microphone, Provider`,
    'Settings still says "Message audio"',
    "the message-reading picker is not in Reading full messages in channel view",
    "Reading full messages in channel view has no link to How messages are pre-processed for speech",
  ]);
  // The group in Voice agent but not beside the other two.
  const apart = HTML.replace(reading, "").replace('<section class="settings-group" id="agent-settings"',
    `${reading}\n<section class="settings-group" id="agent-settings"`);
  assert.deepStrictEqual(voiceAgentSectionProblems(apart), [
    `Voice agent does not hold ${READING_GROUPS.join(", ")} together: Sending, Instructions, Resuming, Arriving messages during a call, Canned prompts, Microphone, ${READING_GROUPS[2]}, Provider`,
  ]);
  // The link taken away, and nothing else.
  const unlinked = HTML.replace(/<p class="help-text-row">\s*<button[^>]*>How messages are pre-processed for speech<\/button>\s*<\/p>/, "");
  assert.notEqual(unlinked, HTML, "the negative control changed nothing");
  assert.deepStrictEqual(voiceAgentSectionProblems(unlinked),
    ["Reading full messages in channel view has no link to How messages are pre-processed for speech"]);
});

/**
 * The Tab stops from Resuming through Reading full messages in channel view, in order. `#346
 * settings-collapsible` made each group's heading the button that folds it, a stop before its `?`.
 */
const SETTINGS_TAB_STOPS = ["fold-resuming", "help-link-resuming", "resume-toggle", "fold-relay", "help-link-live-messages",
  "relay-to-agent", "fold-reading-full", "settings-audio-source", "settings-wit-voice", "help-link-speech-prep"];

/**
 * Where the markup breaks `stops` as consecutive Tab stops on Settings, as sentences. The focus order
 * is the markup's — nothing on Settings carries a positive tabindex, which would reorder it — so it is
 * read from there; tests/keyboard_browser.py presses the real Tab key in Chromium.
 */
function tabStopProblems(html, stops) {
  const code = html.replace(/<!--[\s\S]*?-->/g, " ");
  const settings = code.slice(code.indexOf('id="screen-settings"'), code.indexOf('id="screen-help"'));
  if (/tabindex="[1-9]/.test(settings)) return ["a positive tabindex reorders the settings screen"];
  const starts = stops.map((id) => settings.lastIndexOf("<", settings.indexOf(`id="${id}"`)));
  const missing = stops.filter((id) => !settings.includes(`id="${id}"`));
  if (missing.length > 0) return [`not on Settings: ${missing.join(", ")}`];
  const problems = [];
  for (let i = 1; i < stops.length; i += 1) {
    if (starts[i] < starts[i - 1]) {
      problems.push(`#${stops[i]} comes before #${stops[i - 1]}`);
      continue;
    }
    // Consecutive, not merely in order: nothing focusable between the two.
    const between = settings.slice(settings.indexOf(">", starts[i - 1]) + 1, starts[i]);
    const extra = /<(button|select|input|textarea|summary|a)\b[^>]*>/.exec(between);
    if (extra) problems.push(`${extra[0]} stands between #${stops[i - 1]} and #${stops[i]}`);
  }
  return problems;
}

test("`#332 settings-voice-agent-section`: Tab goes through the three in the order they are read, each ? before its control", () => {
  assert.deepStrictEqual(tabStopProblems(HTML, SETTINGS_TAB_STOPS), []);
  // Negative controls. The link put above the picker it is about:
  const link = /<p class="help-text-row">\s*<button[^>]*>How messages are pre-processed for speech<\/button>\s*<\/p>/.exec(HTML)[0];
  const above = HTML.replace(link, "").replace('<label for="settings-audio-source">', `${link}<label for="settings-audio-source">`);
  assert.notEqual(above, HTML, "the negative control changed nothing");
  assert.deepStrictEqual(tabStopProblems(above, SETTINGS_TAB_STOPS).map((problem) => problem.replace(/^(<[a-z]+ [^ ]+ [^ ]+ id="[^"]+")[^>]*>/, "$1>")), [
    '<button class="help-link help-link-text" id="help-link-speech-prep"> stands between #fold-reading-full and #settings-audio-source',
    "#help-link-speech-prep comes before #settings-wit-voice",
  ]);
  // ...and Canned prompts put back between Arriving messages and the reading picker.
  const canned = `${settingsGroup("Canned prompts")}</section>`;
  const between = HTML.replace(canned, "").replace('<section class="settings-group" id="reading-full-group">',
    `${canned}<section class="settings-group" id="reading-full-group">`);
  const found = tabStopProblems(between, SETTINGS_TAB_STOPS);
  assert.equal(found.length, 1, found.join("; "));
  assert.match(found[0], /^<button class="fold-toggle" id="fold-canned-prompts"[^>]*> stands between #relay-to-agent and #fold-reading-full$/);
});

/** The longest a state line under one of the three may be: two lines on a phone, one at a desk. */
const STATE_LINE_BUDGET = 80;

/**
 * Every state the three settings' lines can be in, each as a label and a way to reach it on a page
 * built from `script`. The line is read back from the element named.
 */
const STATE_LINES = [
  ["resuming off on the server", "resume-state", async (page) => {
    page.replayEnabled = false;
    await withEarlierConversation(page, { resume: true });
  }],
  ["resuming off here", "resume-state", (page) => withEarlierConversation(page, { resume: false })],
  ["resuming on, nothing to resume yet", "resume-state", async (page) => {
    await signIn(page);
    await page.el("resume-toggle").setChecked(true);
  }],
  ["resuming on", "resume-state", (page) => withEarlierConversation(page, { resume: true })],
  ["resumed", "resume-state", async (page) => {
    await withEarlierConversation(page, { resume: true });
    (await startTalking(page)).close();
    await page.settle();
  }],
  ["resumed in part", "resume-state", async (page) => {
    page.replayMaxTurns = 1;
    await withEarlierConversation(page, { resume: true });
    (await startTalking(page)).close();
    await page.settle();
  }],
  ["could not load", "resume-state", async (page) => {
    page.replayStatus = 503;
    await withEarlierConversation(page, { resume: true });
    (await startTalking(page)).close();
    await page.settle();
  }],
  ["all over the budget", "resume-state", async (page) => {
    page.replayMaxTurns = 0;
    await withEarlierConversation(page, { resume: true });
    (await startTalking(page)).close();
    await page.settle();
  }],
  ["nothing to replay", "resume-state", async (page) => {
    page.storedTurns.set("conv_empty", [{ speaker: "note", text: "the call ended", at_ms: 1 }]);
    await signIn(page);
    await page.settle();
    await page.el("resume-toggle").setChecked(true);
    (await startTalking(page)).close();
    await page.settle();
  }],
  // `#331 resume-on-websocket`: what a `vibe-talk-v1` bridge did with the record, and the reason
  // beside a disabled switch, which is its own line.
  ["resumed, and the bridge took it", "resume-state", async (page) => {
    await withEarlierConversation(page, { resume: true });
    const socket = await startV1Call(page, ["replay"]);
    socket.onmessage({ data: JSON.stringify({ type: "replay_ack", accepted: true }) });
    socket.close();
    await page.settle();
  }],
  ["the bridge refused it", "resume-state", async (page) => {
    await withEarlierConversation(page, { resume: true });
    const socket = await startV1Call(page, ["replay"]);
    socket.onmessage({ data: JSON.stringify({ type: "replay_ack", accepted: false }) });
    socket.close();
    await page.settle();
  }],
  ["its session took no recap", "resume-state", async (page) => {
    await withEarlierConversation(page, { resume: true });
    (await startV1Call(page, undefined)).close();
    await page.settle();
  }],
  ["ended before its session started", "resume-state", async (page) => {
    await withEarlierConversation(page, { resume: true });
    (await startV1Call(page, ["replay"], { started: false })).close();
    await page.settle();
  }],
  ["its session was replaced", "resume-state", async (page) => {
    (await replacedV1Call(page)).close();
    await page.settle();
  }],
  ["resuming unavailable on the server", "resume-state", async (page) => {
    page.replayEnabled = false;
    page.replayUnavailable = "bridge_unconfirmed";
    await signIn(page);
  }],
  ["switch: off on the server", "resume-unavailable", async (page) => {
    page.replayEnabled = false;
    await signIn(page);
  }],
  ["switch: the bridge declined", "resume-unavailable", async (page) => {
    page.replayEnabled = false;
    page.replayUnavailable = "bridge_declined";
    await signIn(page);
  }],
  ["switch: the bridge has not answered", "resume-unavailable", async (page) => {
    page.replayEnabled = false;
    page.replayUnavailable = "bridge_unconfirmed";
    await signIn(page);
  }],
  ["live off on the server", "live-state", async (page) => {
    page.livePollSeconds = 0;
    page.liveDelivery = "off";
    await signIn(page);
  }],
  ["live, polling", "live-state", (page) => withLiveChannel(page, [])],
  ["live, polling, not connected", "live-state", async (page) => {
    (await withLiveChannel(page, [])).drop();
    await page.settle();
  }],
  ["live by push", "live-state", async (page) => {
    page.livePollSeconds = 0;
    page.liveDelivery = "push";
    await signIn(page);
  }],
  ["live by push, not connected", "live-state", async (page) => {
    page.livePollSeconds = 0;
    page.liveDelivery = "push";
    await signIn(page);
    page.stream().drop();
    await page.settle();
  }],
];

/** The line one state leaves in its element, on a page running `script`. */
async function stateLine([, id, reach], script = SCRIPT) {
  const page = newPage(new Map(), script);
  await reach(page);
  return page.el(id).textContent;
}

test("`#332 settings-voice-agent-section`: every state line under the three is one short sentence, and each state its own", async () => {
  const seen = new Map();
  for (const state of STATE_LINES) {
    const line = await stateLine(state);
    assert.ok(line.length > 0, `${state[0]}: the line is empty`);
    assert.ok(line.length <= STATE_LINE_BUDGET,
      `${state[0]}: ${line.length} characters, over the ${STATE_LINE_BUDGET} a state line may take — the rest belongs in Help: ${line}`);
    assert.ok(!seen.has(line), `"${state[0]}" and "${seen.get(line)}" say the same thing: ${line}`);
    seen.set(line, state[0]);
  }
  assert.equal(seen.size, STATE_LINES.length);
});

test("`#332 settings-voice-agent-section`: the budget refuses a state line written as the old paragraph (negative control)", async () => {
  const long = brokenScript('"Resuming is OFF on this server: every call starts fresh."',
    '"Resuming is OFF on this server. Every new call starts the agent from nothing, whatever this switch says."');
  const line = await stateLine(STATE_LINES[0], long);
  assert.ok(line.length > STATE_LINE_BUDGET,
    `the negative control's paragraph is ${line.length} characters, inside the budget, so the check above proves nothing`);
});

test("`#332 settings-voice-agent-section`: what the short lines no longer say is in the ? beside them", () => {
  // Shortened, not deleted: each clause a line dropped is in the entry its group's ? opens.
  const resuming = helpEntry("resuming");
  assert.match(resuming, /<code>replay\.max_chars<\/code> or <code>replay\.max_turns<\/code>/,
    "the two settings that resume a conversation too long for the budget are named nowhere");
  assert.match(resuming, /<em>Off on this server<\/em> means the operator has not turned it on/);
  assert.match(resuming, /That last one is not an empty conversation/);
  const live = helpEntry("live-messages");
  assert.match(live, /cannot say whether the adapter itself is healthy/);
  assert.match(live, /re-reads on its own\s+timer while you are looking at it, and nothing reaches the agent between your\s+questions/);
  const said = helpEntry("speech-prep");
  for (const words of ["Arriving messages during a\\s+call", "Reading full messages in channel view", "<em>Agent audio<\\/em>",
    "<em>Device voice<\\/em>", "a car's speakers carry"]) {
    assert.match(said, new RegExp(words), `How messages are pre-processed for speech does not explain ${words}`);
  }
});

// --- `#347 settings-prose` ------------------------------------------------------------------------
//
// The owner, 2026-10-18 ("settings complaints"): remove implementation-detail text, such as "Live
// updates are configured for adapter push, and this page is connected to the server stream...";
// "Arriving messages during a call" becomes the section title, with only its drop-down; rename "Read
// automatically" to "Mark-read automatically"; one-line descriptions ("Same author, within five
// seconds...", "Turn it off to see each message's first line...") become the hover tooltip of the
// (?) and of the subsection title, and clicking (?) opens the full help; "How messages are said"
// becomes a short link "How messages are pre-processed for speech" into help; rewrite or remove "Car
// audio: use Agent audio.".

/** The settings screen's markup, comments out: what a reader can be shown. */
function settingsCode(html) {
  const code = html.replace(/<!--[\s\S]*?-->/g, " ");
  return code.slice(code.indexOf('id="screen-settings"'), code.indexOf('id="screen-help"'));
}

/**
 * The one-line descriptions that stood under their headings, by the heading each describes: each is
 * that heading's tooltip now, and its `?`'s. The owner's two examples are the last two.
 */
const MOVED_DESCRIPTIONS = new Map([
  ["Resuming", "Off by default: it re-sends earlier conversation content to the voice vendor."],
  ["Arriving messages during a call",
    "Briefly, by default. Both speaking modes send channel text to the voice vendor and take conversation time."],
  ["Canned prompts", "What the two prompt buttons send. Empty a field to restore its default."],
  ["Microphone", "Applied to your next call, not the one in progress."],
  ["Your own messages", "Whether your messages start read or unread."],
  ["Combining messages", "Same author, within five seconds. Turn it off to see the raw rows."],
  ["Rich headers", "Turn it off to see each message's first line as it was written."],
]);

/** ...and the two sections that had a line of their own under their title. */
const MOVED_SECTION_DESCRIPTIONS = new Map([
  ["Voice agent", "What the agent may send and is told, what it hears, and how messages are read out."],
  ["Messages", "How messages are marked and shown."],
]);

/** A tag's `title`, or null. */
const titleOf = (tag) => (/\btitle="([^"]*)"/.exec(tag) || [])[1] ?? null;

/**
 * What is wrong with a page's Settings against `#347`, as sentences; empty when nothing is. A function
 * of the markup, so each negative control can hand it a page with one of the old things put back.
 */
function settingsProseProblems(html) {
  const problems = [];
  const settings = settingsCode(html);
  const help = html.replace(/<!--[\s\S]*?-->/g, " ").slice(html.replace(/<!--[\s\S]*?-->/g, " ").indexOf('id="screen-help"'));
  // How live updates are delivered is a readout: Connection status's, and in no setting's group.
  const relayAt = new RegExp(settingsHeading(3, "Arriving messages during a call")).exec(settings);
  const relay = relayAt ? settingsGroup("Arriving messages during a call", settings) : "";
  const status = new RegExp(settingsHeading(3, "Connection status")).test(settings) ? settingsGroup("Connection status", settings) : "";
  if (!status.includes('id="live-state"')) problems.push("how live updates reach this page is not in Connection status");
  if (settingsSection("Voice agent", settings).includes('id="live-state"')) {
    problems.push("how live updates reach this page is still in the Voice agent section");
  }
  if (/adapter push|server stream|server's stream/i.test(settings)) {
    problems.push("Settings still describes how live updates are delivered in words of its own");
  }
  // Arriving messages during a call is the title, with only its drop-down.
  if (!relayAt) {
    problems.push('no group is titled "Arriving messages during a call"');
  } else {
    const heading = /<h3\b[^>]*>/.exec(relay)[0];
    const fields = [...relay.matchAll(/<(select|input|textarea|label|p)\b[^>]*>/g)].map((m) => m[0]);
    if (fields.length !== 1 || !/^<select id="relay-to-agent"/.test(fields[0])) {
      problems.push(`Arriving messages during a call holds more than its drop-down: ${fields.join(" ")}`);
    }
    const named = (/\bid="([^"]+)"/.exec(heading) || [])[1];
    if (!named || !new RegExp(`<select id="relay-to-agent" aria-labelledby="${named}"`).test(relay)) {
      problems.push("the drop-down is not named by the title above it");
    }
    if ([...relay.matchAll(/<button\b(?! class="fold-toggle")/g)].length !== 1 || !/data-help="live-messages"/.test(relay)) {
      problems.push("Arriving messages during a call has some other button than its own ?");
    }
  }
  // The owner's name for the list.
  if (/>Read automatically</.test(settings) || !new RegExp(settingsHeading(3, "Mark-read automatically")).test(settings)) {
    problems.push('the list is still called "Read automatically" on Settings');
  }
  if (!/<summary><h2>Mark-read automatically<\/h2><\/summary>/.test(help)) {
    problems.push('Help does not call the list "Mark-read automatically"');
  }
  // Every ? says what its heading says, and the heading says it too.
  for (const head of settings.matchAll(/<(h[23])\b([^>]*)>(?:<button class="fold-toggle"[^>]*>)?([^<]+)(?:<\/button>)?<\/\1>\s*<button class="help-link" id="([a-z-]+)"[^>]*>\?<\/button>/g)) {
    const [, , attributes, words, link] = head;
    const button = new RegExp(`<button class="help-link" id="${link}"[^>]*>`).exec(head[0])[0];
    const said = titleOf(attributes);
    if (!said || said !== titleOf(button)) {
      problems.push(`"${words}" and its ? do not carry the same one-line description: ${said} / ${titleOf(button)}`);
    } else if (!/^[A-Z].{10,150}\.$/.test(said)) {
      problems.push(`"${words}" has a tooltip that is not a one-line sentence: ${said}`);
    }
  }
  for (const [heading, line] of [...MOVED_DESCRIPTIONS, ...MOVED_SECTION_DESCRIPTIONS]) {
    if (settings.includes(`<p class="hint">${line}</p>`)) problems.push(`"${line}" is still a line under ${heading}`);
    const level = MOVED_SECTION_DESCRIPTIONS.has(heading) ? 2 : 3;
    const tag = new RegExp(settingsHeading(level, literal(heading))).exec(settings);
    if (!tag || titleOf(tag[0]) !== line) problems.push(`${heading}'s tooltip is not "${line}"`);
  }
  // How messages are said is a short link into Help; Car audio is gone, and what it meant is said.
  if (!/<button class="help-link help-link-text" id="help-link-speech-prep"[^>]*>How messages are pre-processed for speech<\/button>/.test(settings)) {
    problems.push('there is no link "How messages are pre-processed for speech"');
  }
  if (/Car audio/.test(settings)) problems.push('"Car audio: use Agent audio." is still on Settings');
  const reading = new RegExp(settingsHeading(3, "Reading full messages in channel view")).exec(settings);
  if (!reading || !/a car's speakers/.test(titleOf(reading[0]) || "")) {
    problems.push("nothing says which voice a car's speakers carry");
  }
  return problems;
}

test("`#347 settings-prose`: Settings drops its implementation prose, and its one-line descriptions are tooltips", () => {
  assert.deepStrictEqual(settingsProseProblems(HTML), []);
});

test("`#347 settings-prose`: the check refuses each old thing put back, one at a time (negative controls)", () => {
  const relay = `${settingsGroup("Arriving messages during a call")}</section>`;
  const cases = [
    // The live-updates line back at the head of the group, and named as it was.
    ["the live line", HTML.replace(/\s*<p id="live-state" class="[^"]*" aria-live="polite"><\/p>/, "")
      .replace('<select id="relay-to-agent"', '<p id="live-state" class="hint" aria-live="polite"></p><select id="relay-to-agent"'), [
      "how live updates reach this page is not in Connection status",
      "how live updates reach this page is still in the Voice agent section",
      "Arriving messages during a call holds more than its drop-down: " +
        '<p id="live-state" class="hint" aria-live="polite"> <select id="relay-to-agent" aria-labelledby="relay-to-agent-title">',
    ]],
    // The #332 group: Live messages, its label with a ? after it, and the line under the picker.
    ["the old group", HTML.replace(relay, relay
      .replace(/(<h3\b[^>]*>(?:<button class="fold-toggle"[^>]*>)?)Arriving messages during a call</, "$1Live messages<")
      .replace('<select id="relay-to-agent" aria-labelledby="relay-to-agent-title">',
        '<div class="label-help"><label for="relay-to-agent">Arriving messages during a call</label>' +
        '<button class="help-link" id="help-link-speech-prep-relay" type="button" data-help="speech-prep">?</button></div>' +
        '<select id="relay-to-agent">')), [
      'no group is titled "Arriving messages during a call"',
      `Arriving messages during a call's tooltip is not "${MOVED_DESCRIPTIONS.get("Arriving messages during a call")}"`,
    ]],
    ["the old name", HTML.replace(/(<h3\b[^>]*>(?:<button class="fold-toggle"[^>]*>)?)Mark-read automatically</, "$1Read automatically<"), [
      'the list is still called "Read automatically" on Settings',
    ]],
    ["a line back under its heading", HTML.replace(/(Show messages sent seconds apart as one\s*<\/label>)/,
      `$1<p class="hint">${MOVED_DESCRIPTIONS.get("Combining messages")}</p>`), [
      `"${MOVED_DESCRIPTIONS.get("Combining messages")}" is still a line under Combining messages`,
    ]],
    ["a ? that says something else", HTML.replace(/(id="help-link-combining"[^>]*title=")[^"]*"/, '$1What this does"'), [
      `"Combining messages" and its ? do not carry the same one-line description: ${MOVED_DESCRIPTIONS.get("Combining messages")} / What this does`,
    ]],
    ["Car audio back", HTML.replace('<p class="help-text-row">', '<p class="hint">Car audio: use Agent audio.</p><p class="help-text-row">'), [
      '"Car audio: use Agent audio." is still on Settings',
    ]],
  ];
  for (const [what, page, wanted] of cases) {
    assert.notEqual(page, HTML, `${what}: the negative control changed nothing`);
    assert.deepStrictEqual(settingsProseProblems(page), wanted, what);
  }
});

test("`#347 settings-prose`: How messages are pre-processed for speech opens Help at the entry of that name", async () => {
  const page = newPage();
  page.speechPrepEnabled = false;
  await signIn(page);
  await page.el("open-settings").click();
  await page.el("help-link-speech-prep").click();
  assert.equal(page.screen(), "help", "the link did not open Help");
  assert.deepStrictEqual(openHelp(page), ["speech-prep"], `the link opened ${openHelp(page).join(", ") || "nothing"}`);
  assert.match(helpEntry("speech-prep"), /^id="help-speech-prep" class="help-entry">\s*<summary><h2>How messages are pre-processed for speech<\/h2><\/summary>/,
    "the entry the link opens is not called what the link says");
  assert.equal(page.el("speech-prep-state").textContent,
    "Off on this server: messages are read out exactly as they were typed.",
    "the entry does not open on which arm this server is in");
  // The negative control: the link with its wiring taken out must then do nothing, or the check above
  // proves only that something opens Help.
  const unwired = newPage(new Map(), brokenScript('  "speech-prep",\n', ""));
  await signIn(unwired);
  await unwired.el("open-settings").click();
  await unwired.el("help-link-speech-prep").click();
  assert.equal(unwired.screen(), "settings", "the negative control opened Help anyway, so the check above proves nothing");
});

test("`#347 settings-prose`: how live updates arrive is still said, in Connection status, for every state", async () => {
  // The line moved; it was not deleted. Connection status holds it, and each state still says itself.
  assert.ok(markupHolds("settings-storage", "live-state"), "the line is not in Storage and connection");
  const group = settingsGroup("Connection status");
  assert.ok(group.indexOf('id="settings-detail"') < group.indexOf('id="live-state"'), "the line is not under the readout");
  const polling = newPage();
  await withLiveChannel(polling, []);
  assert.match(polling.el("live-state").textContent, /re-reads the channel every \d+ seconds/);
  const pushed = newPage();
  pushed.livePollSeconds = 0;
  pushed.liveDelivery = "push";
  await signIn(pushed);
  assert.match(pushed.el("live-state").textContent, /adapter push/);
});

// --- `#349 channel-settings-all` ------------------------------------------------------------------
//
// The owner, 2026-10-18 ("settings complaints"): the channel drop-down gains "All channels"; Clear
// unread can be set for all at once; show "Mixed" when channels differ; drop the who-is-who part from
// that pane. Who is who moved to Messages; `#402 who-is-who-channel-box` put it back in the box, which
// the tests beside `renderIdentityRows` cover.

/** Settings on a server with two channels whose chat service can clear unread, each in its `modes`. */
async function allChannelsPage(modes = {}, arrange = (_page) => {}) {
  return clearingPage((page) => {
    page.channels = [{ ...CHANNEL }, { ...OTHER_CHANNEL }];
    for (const [id, mode] of Object.entries(modes)) {
      page.upstreamClear.set(id, { provider_name: "Google Chat", supported: true, mode, failures: 0, pending: false });
    }
    arrange(page);
  });
}

test("`#349 channel-settings-all`: the picker opens with All channels where there are two or more, and only there", async () => {
  const page = await allChannelsPage();
  assert.deepStrictEqual(pickerChoices(page),
    [["*", "All channels"], [CHANNEL.id, "lead team"], [OTHER_CHANNEL.id, "build noise"]]);
  // Settings still opens on a channel, as it did: All is a choice, not where the box starts.
  assert.equal(page.el("settings-channel").value, CHANNEL.id);
  // The reading picker is about one channel, always.
  assert.ok(!page.el("discord-channel").children.some((option) => option.value === "*"), "the channel view offers All channels");
  // One channel: all of them is that one, and the choice would be a second name for it.
  const one = await clearingPage();
  assert.deepStrictEqual(pickerChoices(one), [[CHANNEL.id, "lead team"]]);
});

test("`#351 settings-polish`: All channels is offered only where a channel can take Clear unread with this token", async () => {
  const offersAll = (page) => pickerChoices(page).some(([value]) => value === "*");
  // A chat service that cannot clear unread: two channels, and nothing All channels could set.
  const unable = newPage();
  unable.channels = [{ ...CHANNEL }, { ...OTHER_CHANNEL }];
  await signIn(unable);
  await unable.el("open-settings").click();
  await unable.settle();
  assert.deepStrictEqual(pickerChoices(unable), [[CHANNEL.id, "lead team"], [OTHER_CHANNEL.id, "build noise"]],
    "All channels is offered where no channel's chat service can clear unread");
  // Providers, none of whose channels can.
  const provider = (key, name) => ({
    key, name, threading_supported: false, live_delivery: "push", live_poll_seconds: 0,
    channel_registration_supported: false, channel_discovery_supported: false, upstream_read_mark_supported: false,
    self_author_id: null, owner_author_id: null,
  });
  const neither = await clearingPage((page) => {
    page.channels = [{ id: "C0123ABCDE", label: "slack team", writable: true, provider: "slack" },
      { id: "C0123ABCDF", label: "other team", writable: true, provider: "slack" }];
    page.providers = [provider("slack", "Slack")];
  });
  assert.equal(offersAll(neither), false, "All channels is offered where no provider can clear unread");
  // A read token, where the chat service can: it may change no channel's setting.
  const reader = await allChannelsPage({}, (page) => { page.tokenScope = "read"; });
  assert.equal(offersAll(reader), false, "All channels is offered to a read token");
  // ...and the same channels with a write token offer it, read from the first answer the server gives.
  const writer = await allChannelsPage();
  assert.equal(offersAll(writer), true, "All channels is not offered where both channels can clear unread");
  // Signed in again with a read token while on All channels: it goes, and the box is about one channel.
  await settingsChannel(writer, "*");
  writer.tokenScope = "read";
  writer.el("api-token").value = "read-token-bbbbbbbbbbbbbbbbbbbb";
  await writer.el("save-token").click();
  await writer.settle();
  if (writer.screen() !== "settings") await writer.el("open-settings").click();
  await writer.settle();
  assert.equal(offersAll(writer), false, "All channels outlived the write token");
  assert.equal(writer.el("settings-channel").value, CHANNEL.id);
  assert.equal(writer.el("rename-channel").hidden, false, "the box stayed on All channels' acts");
  assert.match(writer.el("channel-facts").textContent, /"lead team"/);
  // The negative control: a picker that offers it wherever there are two channels.
  const blind = newPage(new Map(), brokenScript(" && upstreamClearAllChannels().length > 0;", ";"));
  blind.channels = [{ ...CHANNEL }, { ...OTHER_CHANNEL }];
  await signIn(blind);
  await blind.el("open-settings").click();
  await blind.settle();
  assert.equal(offersAll(blind), true, "the negative control left All channels out anyway, so the check above proves nothing");
});

/**
 * `#351 settings-polish`, from its review. What a description of All channels leaves out about when the
 * picker offers it, as sentences: two channels or more, at least one of which can take Clear unread with
 * the token in use, so not to the read token, nor where no channel's chat service can clear unread.
 */
function allChannelsOfferProblems(text) {
  const flat = text.replace(/<[^>]+>/g, "").replace(/\s+/g, " ");
  const problems = [];
  if (!/two or more channels/.test(flat)) problems.push("two channels or more is not said");
  if (!/at least one of (them|which) can take Clear unread with (this|your|the) token/.test(flat)) {
    problems.push("a channel that can take Clear unread with the token is not said to be needed");
  }
  if (!/(With|never to) the read token/.test(flat)) problems.push("the read token is not said to go without it");
  if (!/where no channel's chat service can clear unread/.test(flat)) {
    problems.push("a chat service that cannot clear unread is not said to go without it");
  }
  return problems;
}

test("`#351 settings-polish`: Help, the README and the markup say when All channels is offered, as the picker does", async () => {
  // The page first: two channels, offered with the write token on a service that can clear unread, and
  // not with the read token. The test above covers each case; this one holds the words to them.
  assert.equal(pickerChoices(await allChannelsPage())[0][0], "*", "All channels is not offered at all");
  assert.notEqual(pickerChoices(await allChannelsPage({}, (page) => { page.tokenScope = "read"; }))[0][0], "*");
  const paragraph = (html) => {
    const at = html.indexOf("<strong>All channels</strong>");
    assert.ok(at > 0, "Help no longer describes All channels");
    return html.slice(html.lastIndexOf("<p", at), html.indexOf("</p>", at));
  };
  const help = paragraph(helpEntry("channel-alias"));
  assert.deepStrictEqual(allChannelsOfferProblems(help), [], "Help's Channels entry");
  const readme = readFileSync(join(WEB, "..", "README.md"), "utf8");
  const bullet = readme.slice(readme.indexOf("* **All channels at once**"), readme.indexOf("\n* ", readme.indexOf("* **All channels at once**") + 1));
  assert.ok(bullet.length > 0, "the README no longer has its All channels bullet");
  assert.deepStrictEqual(allChannelsOfferProblems(bullet), [], "the README's All channels bullet");
  const comment = HTML.slice(HTML.indexOf("`#349 channel-settings-all`. The picker's first choice"), HTML.indexOf("-->", HTML.indexOf("`#349 channel-settings-all`. The picker's first choice")));
  assert.ok(comment.length > 0, "the markup no longer says what the picker's first choice is");
  assert.deepStrictEqual(allChannelsOfferProblems(comment), [], "the markup comment over the channel box");
  // The negative control: the entry as it was, offering it wherever there are two channels.
  const old = "<p class=\"hint\"><strong>All channels</strong>, the picker's first choice when there are two or more, " +
    "is for setting <strong>Clear unread</strong> on every channel at once.</p>";
  assert.deepStrictEqual(allChannelsOfferProblems(old), [
    "two channels or more is not said",
    "a channel that can take Clear unread with the token is not said to be needed",
    "the read token is not said to go without it",
    "a chat service that cannot clear unread is not said to go without it",
  ]);
});

test("`#349 channel-settings-all`: with All channels, Rename and Remove step aside, Add and Browse stay, and the line says how many", async () => {
  const page = await allChannelsPage({}, (page) => { page.channelDiscoverySupported = true; });
  await settingsChannel(page, "*");
  assert.equal(page.el("channel-facts").textContent, "All 2 channels on your list.");
  assert.equal(page.el("rename-channel").hidden, true, "Rename is offered for all channels at once");
  assert.equal(page.el("remove-channel").hidden, true, "Remove is offered for all channels at once");
  assert.equal(page.el("open-add-channel").hidden, false, "Add a channel went with them");
  // Back on one channel, its acts are back.
  await settingsChannel(page, OTHER_CHANNEL.id);
  assert.equal(page.el("rename-channel").hidden, false);
  assert.equal(page.el("remove-channel").hidden, false);
  assert.match(page.el("channel-facts").textContent, /"build noise"/);
});

test("`#349 channel-settings-all`: Clear unread for All channels shows their common mode, and Save sets it on every one", async () => {
  const page = await allChannelsPage();
  page.upstreamClearCalls.length = 0;
  await settingsChannel(page, "*");
  assert.deepStrictEqual(page.upstreamClearCalls, [`GET ${CHANNEL.id}`, `GET ${OTHER_CHANNEL.id}`],
    "the box did not read every channel it would set");
  assert.equal(page.el("upstream-clear").hidden, false);
  assert.equal(page.el("upstream-clear-mode").value, "off");
  assert.equal(page.el("upstream-clear-mixed").hidden, true, "Mixed offered when every channel is alike");
  assert.equal(page.el("upstream-clear-state").textContent, "Off in all 2 channels.");
  assert.equal(page.el("upstream-clear-save").disabled, true, "Save offered with nothing to change");
  assert.equal(page.el("upstream-clear-push").hidden, true, "a one-channel push offered for all channels");
  assert.equal(page.el("upstream-clear-scope").textContent, "each of these channels",
    "the warning still speaks of one channel");
  page.el("upstream-clear-mode").value = "on";
  await page.el("upstream-clear-mode").dispatch("change");
  assert.equal(page.el("upstream-clear-save").disabled, false);
  await page.el("upstream-clear-save").click();
  await page.settle();
  assert.deepStrictEqual(page.upstreamClearCalls.slice(2),
    [`PUT ${CHANNEL.id} {"mode":"on"}`, `PUT ${OTHER_CHANNEL.id} {"mode":"on"}`]);
  assert.equal(page.el("upstream-clear-state").textContent, "Saved. On in all 2 channels.");
  assert.equal(page.el("upstream-clear-mode").value, "on");
  assert.equal(page.el("upstream-clear-save").disabled, true);
  // One channel again: its own box, its own words, from what the server stored.
  await settingsChannel(page, OTHER_CHANNEL.id);
  assert.equal(page.el("upstream-clear-scope").textContent, "this channel");
  assert.equal(page.el("upstream-clear-mode").value, "on");
  assert.equal(page.el("upstream-clear-push").hidden, false, "Mark read now is not back for one channel");
});

test("`#349 channel-settings-all`: channels set differently show Mixed, which cannot be saved, until a mode is chosen", async () => {
  const page = await allChannelsPage({ [CHANNEL.id]: "on", [OTHER_CHANNEL.id]: "off" });
  await settingsChannel(page, "*");
  assert.equal(page.el("upstream-clear-mode").value, "mixed");
  assert.equal(page.el("upstream-clear-mixed").hidden, false, "Mixed is not offered when it is true");
  assert.equal(PAGE_ELEMENTS.get("upstream-clear-mixed").tag, "option");
  assert.match(HTML, /<option id="upstream-clear-mixed" value="mixed" disabled hidden>Mixed<\/option>/,
    "Mixed is a mode the reader could choose and save");
  assert.equal(page.el("upstream-clear-state").textContent, "Mixed: On in 1, Off in 1.");
  assert.equal(page.el("upstream-clear-save").disabled, true, "Mixed can be saved");
  page.el("upstream-clear-mode").value = "off";
  await page.el("upstream-clear-mode").dispatch("change");
  await page.el("upstream-clear-save").click();
  await page.settle();
  assert.deepStrictEqual(page.upstreamClearCalls.filter((call) => call.startsWith("PUT")),
    [`PUT ${CHANNEL.id} {"mode":"off"}`, `PUT ${OTHER_CHANNEL.id} {"mode":"off"}`]);
  assert.equal(page.el("upstream-clear-mode").value, "off");
  assert.equal(page.el("upstream-clear-mixed").hidden, true);
  assert.equal(page.el("upstream-clear-state").textContent, "Saved. Off in all 2 channels.");
});

test("`#349 channel-settings-all`: a channel whose service cannot clear unread is left out and said to be, and a refusal is named", async () => {
  const space = { id: "1110000000000000201", label: "chat space", writable: true, provider: "gchat" };
  const team = { id: "C0123ABCDE", label: "slack team", writable: true, provider: "slack" };
  const other = { id: "1110000000000000202", label: "second space", writable: true, provider: "gchat" };
  const provider = (key, name, clear) => ({
    key, name, threading_supported: false, live_delivery: "push", live_poll_seconds: 0,
    channel_registration_supported: false, channel_discovery_supported: false,
    upstream_read_mark_supported: clear, upstream_clear_supported: clear || undefined,
    self_author_id: null, owner_author_id: null,
  });
  const page = await clearingPage((page) => {
    page.channels = [team, space, other];
    page.providers = [provider("gchat", "Google Chat", true), provider("slack", "Slack", false)];
  });
  page.upstreamClearCalls.length = 0;
  await settingsChannel(page, "*");
  assert.deepStrictEqual(page.upstreamClearCalls, [`GET ${space.id}`, `GET ${other.id}`], "the box asked about a channel that cannot");
  assert.equal(page.el("upstream-clear-provider").textContent, "Google Chat");
  // `#355 provider-grouped-channels`. With two services All channels is the chosen service's, Google
  // Chat's here (the service of the channel being read), so the Slack channel, which cannot clear
  // unread, is not among the channels it reaches, and is neither asked about nor counted as left out.
  assert.equal(page.el("upstream-clear-state").textContent, "Off in all 2 channels.");
  // The server refuses every write now: nothing is said to be saved that was not.
  page.upstreamClearFailure = { status: 503, error: "store_unavailable", detail: "the store is not writable" };
  page.el("upstream-clear-mode").value = "dry_run";
  await page.el("upstream-clear-mode").dispatch("change");
  await page.el("upstream-clear-save").click();
  await page.settle();
  assert.equal(page.el("upstream-clear-state").textContent,
    "Saved in 0 of 2 channels; not in chat space (the store is not writable), second space (the store is not writable).");
  assert.equal(page.el("upstream-clear-mode").value, "off", "the choice still shows a mode the server refused");
});

test("`#349 channel-settings-all`: the Save check is not satisfied by a page that sets only the first channel (negative control)", async () => {
  // Without this, the test above could pass on a Save that stopped after one channel.
  const broken = newPage(new Map(), brokenScript(
    "  for (const id of held.ids) {\n    try {\n      const view = await apiDecoded(\"UpstreamClearState\", `/api/v1/channels/${encodeURIComponent(id)}/upstream-clear`,\n        { method: \"PUT\"",
    "  for (const id of held.ids.slice(0, 1)) {\n    try {\n      const view = await apiDecoded(\"UpstreamClearState\", `/api/v1/channels/${encodeURIComponent(id)}/upstream-clear`,\n        { method: \"PUT\""));
  broken.chatProviderName = "Google Chat";
  broken.upstreamClearSupported = true;
  broken.channels = [{ ...CHANNEL }, { ...OTHER_CHANNEL }];
  await signIn(broken);
  await broken.el("open-settings").click();
  await broken.settle();
  await settingsChannel(broken, "*");
  broken.el("upstream-clear-mode").value = "on";
  await broken.el("upstream-clear-mode").dispatch("change");
  await broken.el("upstream-clear-save").click();
  await broken.settle();
  assert.notDeepStrictEqual(broken.upstreamClearCalls.filter((call) => call.startsWith("PUT")),
    [`PUT ${CHANNEL.id} {"mode":"on"}`, `PUT ${OTHER_CHANNEL.id} {"mode":"on"}`],
    "the negative control set every channel anyway");
  assert.equal(broken.el("upstream-clear-state").textContent.startsWith("Saved. On in all 2 channels."), false,
    "the negative control's status line reads as the real one's");
});

// --- `#348 settings-dynamic-lists` ----------------------------------------------------------------
//
// The owner, 2026-10-18 ("settings complaints"): "Recently dismissed" becomes a fixed-height,
// user-resizable, scrollable list styled as data (not a control), with shorter previews; clicking one
// opens a pop-out with the full text and a copy button. The connection-status list gets the same data
// styling (for example a dark terminal-like panel).

/**
 * What is wrong with the two readouts against `#348`, as sentences; empty when nothing is. A function of
 * the markup and the stylesheet, so each negative control can hand it the ones from before.
 */
function dataListProblems(html, css) {
  const problems = [];
  const code = html.replace(/<!--[\s\S]*?-->/g, " ");
  const rule = (selector) => cssRules(css, selector).join("\n");
  const panel = rule(".data-panel");
  if (!/font-family:\s*ui-monospace/.test(panel) || !/background:\s*var\(--data-bg\)/.test(panel)) {
    problems.push("there is no dark, monospaced data panel");
  }
  const bg = /--data-bg:\s*#([0-9a-f]{6})/i.exec(panel);
  if (!bg || parseInt(bg[1].slice(0, 2), 16) > 0x30) problems.push("the data panel is not dark");
  if (!/<ol id="dismissed-sends" class="dismissed-sends data-panel"/.test(code)) {
    problems.push("Recently dismissed is not drawn as a data panel");
  }
  const list = rule(".dismissed-sends");
  if (!/(?:^|\s)height:\s*\d/.test(list)) problems.push("Recently dismissed has no fixed height");
  if (!/resize:\s*vertical/.test(list)) problems.push("Recently dismissed cannot be resized");
  if (!/overflow:\s*auto/.test(list)) problems.push("Recently dismissed does not scroll inside itself");
  const preview = rule(".dismissed-send-text");
  if (!/white-space:\s*nowrap/.test(preview) || !/text-overflow:\s*ellipsis/.test(preview) || /line-clamp/.test(preview)) {
    problems.push("a row's preview is not one line");
  }
  if (!/<div id="dismissed-send-popout"[^>]*role="dialog"[^>]*hidden>/.test(code) ||
      !/<button id="dismissed-send-copy"[^>]*>Copy<\/button>/.test(code)) {
    problems.push("there is no pop-out with a Copy button");
  }
  const connection = (/<div id="connection-panel" class="([^"]*)"[^>]*>([\s\S]*?)<\/div>\s*<\/section>/.exec(code) || []);
  if (!/\bdata-panel\b/.test(connection[1] || "") || !/id="settings-detail"/.test(connection[2] || "") ||
      !/id="live-state"/.test(connection[2] || "")) {
    problems.push("Connection status is not drawn in the data panel");
  }
  return problems;
}

test("`#348 settings-dynamic-lists`: Recently dismissed and Connection status are dark data panels; the list is fixed, resizable and one line a row", () => {
  assert.deepStrictEqual(dataListProblems(HTML, CSS_CODE), []);
});

test("`#348 settings-dynamic-lists`: the check refuses the readouts from before (negative controls)", () => {
  // The list as it was: cards with three lines of text, no height, nothing to resize.
  const oldCss = CSS_CODE
    .replace(/\.dismissed-sends\s*\{[^}]*\}/, ".dismissed-sends { display: grid; gap: 0.5rem; list-style: none; }")
    .replace(/\.dismissed-send-text\s*\{[^}]*\}/, ".dismissed-send-text { white-space: pre-wrap; -webkit-line-clamp: 3; line-clamp: 3; overflow: hidden; }");
  assert.notEqual(oldCss, CSS_CODE, "the negative control changed nothing");
  assert.deepStrictEqual(dataListProblems(HTML, oldCss), [
    "Recently dismissed has no fixed height",
    "Recently dismissed cannot be resized",
    "Recently dismissed does not scroll inside itself",
    "a row's preview is not one line",
  ]);
  // The markup as it was: a plain list, no pop-out, and the connection detail in an ordinary panel.
  const oldHtml = HTML
    .replace('class="dismissed-sends data-panel"', 'class="dismissed-sends"')
    .replace(/<div id="dismissed-send-popout"[\s\S]*?<p id="dismissed-send-popout-state"[^>]*><\/p>\s*<\/div>/, "")
    .replace('<div id="connection-panel" class="data-panel connection-panel"', '<div id="connection-panel" class="panel muted"');
  assert.deepStrictEqual(dataListProblems(oldHtml, CSS_CODE), [
    "Recently dismissed is not drawn as a data panel",
    "there is no pop-out with a Copy button",
    "Connection status is not drawn in the data panel",
  ]);
  // ...and a light panel is not a terminal's.
  assert.deepStrictEqual(dataListProblems(HTML, CSS_CODE.replace(/--data-bg:\s*#[0-9a-f]{6}/i, "--data-bg: #f6f7f9")),
    ["the data panel is not dark"]);
});

/** A page with one long send dismissed from the outbox, on Settings. */
async function dismissedPage(text) {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, []);
  const row = await failedSend(page, text);
  await outgoingDismiss(row).click();
  page.expireTimers(SEND_UNDO_MS);
  await page.el("open-settings").click();
  return page;
}

test("`#348 settings-dynamic-lists`: a row is one button, and it opens the whole message with Copy, Put back and Close", async () => {
  const long = `first line of a long dismissed message\n${"and it goes on ".repeat(30)}to the very end`;
  const page = await dismissedPage(long);
  const item = dismissedItems(page)[0];
  // The row: one button, no Put back of its own, the whole text still in it for find and a screen reader.
  const buttons = item.descendants().filter((node) => node.tagName === "button");
  assert.deepStrictEqual(buttons.map((node) => node.className), ["dismissed-send-open"], "the row is not one button");
  assert.match(item.text(), /to the very end/, "the row holds only part of the text");
  assert.equal(page.el("dismissed-send-popout").hidden, true, "the pop-out is up before anything was opened");
  await dismissedOpenerOf(item).click();
  assert.equal(page.el("dismissed-send-popout").hidden, false);
  assert.equal(page.el("dismissed-send-popout-text").textContent, long, "the pop-out does not hold the whole message");
  assert.match(page.el("dismissed-send-popout-meta").textContent, /^lead team · dismissed just now$/);
  assert.equal(page.document.activeElement, page.el("dismissed-send-copy"), "the pop-out did not take the focus");
  // Copy: the whole message, said so.
  await page.el("dismissed-send-copy").click();
  assert.equal(page.clipboard, long, "Copy did not put the whole message on the clipboard");
  assert.equal(page.el("dismissed-send-popout-state").textContent, "Copied the whole message.");
  // Every way of copying refused: said, never a silent no-op the reader mistakes for a copy.
  page.clipboardRefuses = true;
  page.execCopyRefuses = true;
  page.clipboard = "something else";
  await page.el("dismissed-send-copy").click();
  assert.equal(page.clipboard, "something else");
  assert.equal(page.el("dismissed-send-popout-state").textContent, sourceString("COPY_REFUSED"));
  // Close: the focus back on the row that opened it.
  await page.el("dismissed-send-close").click();
  assert.equal(page.el("dismissed-send-popout").hidden, true);
  assert.equal(page.document.activeElement, dismissedOpenerOf(dismissedItems(page)[0]), "the focus was not given back to the row");
  // Put back, from the pop-out: back in the outbox, gone from the list, and the pop-out with it.
  await dismissedOpenerOf(dismissedItems(page)[0]).click();
  await page.el("dismissed-send-restore").click();
  await page.settle();
  assert.equal(page.el("dismissed-send-popout").hidden, true, "the pop-out outlived the message it showed");
  assert.equal(dismissedItems(page).length, 0);
  assert.deepEqual(savedDismissed(page), []);
  assert.equal(savedOutgoing(page)[0].text, long, "Put back did not return it to the outbox");
});

test("`#348 settings-dynamic-lists`: Escape closes the pop-out before Settings, and leaving Settings closes it", async () => {
  const page = await dismissedPage("a message to read whole");
  await dismissedOpenerOf(dismissedItems(page)[0]).click();
  await press(page, page.el("dismissed-send-copy"), "Escape");
  assert.equal(page.el("dismissed-send-popout").hidden, true, "Escape left the pop-out up");
  assert.equal(page.screen(), "settings", "Escape left Settings with the pop-out up");
  await press(page, dismissedOpenerOf(dismissedItems(page)[0]), "Escape");
  assert.equal(page.screen(), "main", "the next Escape did not leave Settings");
  // Open, then leave Settings by its Back: the pop-out goes with Settings.
  await page.el("open-settings").click();
  await dismissedOpenerOf(dismissedItems(page)[0]).click();
  await page.el("close-settings").click();
  assert.equal(page.el("dismissed-send-popout").hidden, true, "the pop-out stayed up over the channel");
  // The negative control: a page whose Escape does not know the pop-out leaves Settings at once.
  const blind = newPage(new Map(), brokenScript('  if (closeOpenMenu("menu:dismissed")) return true;\n', ""));
  await signIn(blind);
  await showDiscord(blind, []);
  await outgoingDismiss(await failedSend(blind, "another message")).click();
  await blind.el("open-settings").click();
  await dismissedOpenerOf(dismissedItems(blind)[0]).click();
  await press(blind, blind.el("dismissed-send-copy"), "Escape");
  assert.equal(blind.screen(), "main", "the negative control's Escape did not leave Settings, so the check above proves nothing");
});

test("`#348 settings-dynamic-lists` x `#346 settings-collapsible`: folding the group or section that holds the pop-out closes it", async () => {
  for (const fold of ["dismissed", "settings-messages", "settings-all"]) {
    const page = await dismissedPage("a message to read whole");
    // Everything open, as a reader who opened the list to reach it has it.
    await page.el("fold-settings-all").click();
    await dismissedOpenerOf(dismissedItems(page)[0]).click();
    assert.equal(page.el("dismissed-send-popout").hidden, false);
    await page.el(`fold-${fold}`).click();
    assert.equal(page.el("dismissed-send-popout").hidden, true, `folding ${fold} left the pop-out open out of sight`);
    // ...so the next Escape is Settings', not spent on a pop-out nobody can see.
    await press(page, page.el(`fold-${fold}`), "Escape");
    assert.equal(page.screen(), "main", `after folding ${fold}, Escape closed something unseen first`);
  }
  // A fold elsewhere leaves it open.
  const page = await dismissedPage("a message to read whole");
  await page.el("fold-settings-all").click();
  await dismissedOpenerOf(dismissedItems(page)[0]).click();
  await page.el("fold-typing").click();
  assert.equal(page.el("dismissed-send-popout").hidden, false, "folding another group closed the pop-out");
  // The negative control: a page whose folds do not know the pop-out leaves it open behind the fold.
  const blind = newPage(new Map(), brokenScript(
    "    if (!foldOpen(slug) && FOLDS_HOLDING_DISMISSED.includes(slug)) closeDismissedSend(false);\n", ""));
  await signIn(blind);
  await showDiscord(blind, []);
  await outgoingDismiss(await failedSend(blind, "another message")).click();
  await blind.el("open-settings").click();
  await blind.el("fold-settings-all").click();
  await dismissedOpenerOf(dismissedItems(blind)[0]).click();
  await blind.el("fold-dismissed").click();
  assert.equal(blind.el("dismissed-send-popout").hidden, false, "the negative control closed it anyway, so the check above proves nothing");
});

// --- `#351 settings-polish`: the pop-out over Recently dismissed ---------------------------------
//
// Found in a review of `#348 settings-dynamic-lists`: Put back from the pop-out dropped the focus to
// nowhere, the pop-out dimmed the screen like a modal without being one, and each row was named by
// its whole message.

/** A page on Settings with each of `texts` sent, refused and dismissed: the list has them newest first. */
async function dismissedPageOf(texts, script = SCRIPT) {
  const page = newPage(new Map(), script);
  await signIn(page);
  await showDiscord(page, []);
  for (const text of texts) await outgoingDismiss(await failedSend(page, text)).click();
  page.expireTimers(SEND_UNDO_MS);
  await page.el("open-settings").click();
  return page;
}

/** The text of each row of Recently dismissed, top to bottom. */
const dismissedTexts = (page) =>
  dismissedItems(page).map((item) => item.descendants().find((node) => node.hasClass("dismissed-send-text")).textContent);

/** The pop-out's Tab stops, in order. */
const DISMISSED_STOPS = ["dismissed-send-popout-text", "dismissed-send-copy", "dismissed-send-restore", "dismissed-send-close"];

/** Whether `node` is inert: it, or an element around it, carries `inert`. */
function inertNow(node) {
  for (let at = node; at; at = at.parentNode) if (at.hasAttribute("inert")) return true;
  return false;
}

/** The page's own elements carrying `inert` now, by id. */
const inertIds = (page) => [...page.elements.values()].filter((node) => node.hasAttribute("inert")).map((node) => node.id);

test("`#351 settings-polish`: Put back from the pop-out gives the focus to the row that took its place, else the list, else its state line", async () => {
  const page = await dismissedPageOf(["first", "second", "third"]);
  assert.deepStrictEqual(dismissedTexts(page), ["third", "second", "first"]);
  // The top row: the one under it takes its place, and the focus.
  await (await putBackOf(page, dismissedItems(page)[0])).click();
  await page.settle();
  assert.deepStrictEqual(dismissedTexts(page), ["second", "first"]);
  assert.equal(focused(page), dismissedOpenerOf(dismissedItems(page)[0]), `after the top row went, the focus is on ${focusName(page)}`);
  // The last row: nothing takes its place, so the list.
  await (await putBackOf(page, dismissedItems(page)[1])).click();
  await page.settle();
  assert.deepStrictEqual(dismissedTexts(page), ["second"]);
  assert.equal(focused(page), page.el("dismissed-sends"), `after the last row went, the focus is on ${focusName(page)}`);
  // The only row: the list goes with it, so the line that now says it is empty.
  await (await putBackOf(page, dismissedItems(page)[0])).click();
  await page.settle();
  assert.equal(page.el("dismissed-sends").hidden, true);
  assert.equal(focused(page), page.el("dismissed-sends-state"), `after the only row went, the focus is on ${focusName(page)}`);
  assert.match(page.el("dismissed-sends-state").textContent, /^Nothing dismissed/);
  // Each of the two can hold the focus from script, and neither is a Tab stop.
  for (const id of ["dismissed-sends", "dismissed-sends-state"]) {
    assert.match(new RegExp(`<[a-z]+ id="${id}"[^>]*>`).exec(HTML_CODE)[0], /\btabindex="-1"/, `#${id} cannot hold the focus`);
  }
  // The negative control: a page that leaves the focus in the pop-out it has just hidden, on nothing
  // drawn, which a browser hands to the body.
  const blind = await dismissedPageOf(["first", "second"], brokenScript("  focusDismissedAt(at);\n", ""));
  await (await putBackOf(blind, dismissedItems(blind)[0])).click();
  await blind.settle();
  assert.equal(blind.el("dismissed-send-popout").hidden, true);
  assert.equal(nodeIn(blind.el("dismissed-send-popout"), focused(blind)), true,
    `the negative control moved the focus anyway, to ${focusName(blind)}, so the check above proves nothing`);
});

test("`#351 settings-polish`: the pop-out is a modal: marked so, the page behind it inert while it is up, and Tab kept inside it", async () => {
  assert.match(HTML_CODE, /<div id="dismissed-send-popout"[^>]*role="dialog"[^>]*aria-modal="true"/, "the pop-out is not marked modal");
  const page = await dismissedPageOf(["a message to read whole"]);
  await page.el("fold-settings-all").click();
  assert.deepStrictEqual(inertIds(page), [], "something is inert before the pop-out is up");
  await dismissedOpenerOf(dismissedItems(page)[0]).click();
  assert.equal(page.el("dismissed-send-popout").hidden, false);
  // Behind it: its own group's fold, list, line and Forget, the groups beside it and the sections beside
  // its section, and the row that opened it, inside the list.
  for (const id of ["fold-dismissed", "dismissed-sends", "dismissed-sends-state", "forget-dismissed-sends", "fold-mark-own",
    "identity-group", "fold-settings-messages", "settings-channels", "fold-settings-voice-agent", "fold-storage"]) {
    assert.equal(inertNow(page.el(id)), true, `#${id} can still be reached behind the pop-out`);
  }
  assert.equal(inertNow(dismissedOpenerOf(dismissedItems(page)[0])), true, "the row behind the pop-out can still be reached");
  // Not the pop-out, nor anything in it.
  for (const id of ["dismissed-send-popout", ...DISMISSED_STOPS, "dismissed-send-popout-meta", "dismissed-send-popout-state"]) {
    assert.equal(inertNow(page.el(id)), false, `#${id}, in the pop-out, is inert`);
  }
  // Tab goes round its own stops: Close to the message, and Shift+Tab back; from the page, as after a
  // tap on the dim, to its first stop, or its last; between two of its stops it is the browser's.
  let key = await press(page, page.el("dismissed-send-close"), "Tab");
  assert.equal(focused(page), page.el("dismissed-send-popout-text"), `Tab from Close went to ${focusName(page)}`);
  assert.equal(key.defaultPrevented, true, "the browser's Tab still ran after the pop-out moved the focus");
  await press(page, page.el("dismissed-send-popout-text"), "Tab", { shiftKey: true });
  assert.equal(focused(page), page.el("dismissed-send-close"), `Shift+Tab from the message went to ${focusName(page)}`);
  await press(page, null, "Tab");
  assert.equal(focused(page), page.el("dismissed-send-popout-text"), `Tab from the page went to ${focusName(page)}`);
  await press(page, null, "Tab", { shiftKey: true });
  assert.equal(focused(page), page.el("dismissed-send-close"), `Shift+Tab from the page went to ${focusName(page)}`);
  key = await press(page, page.el("dismissed-send-copy"), "Tab");
  assert.equal(key.defaultPrevented, false, "Tab from Copy to Put back was taken from the browser");
  key = await press(page, page.el("dismissed-send-close"), "Tab", { ctrlKey: true });
  assert.equal(key.defaultPrevented, false, "Ctrl+Tab, the browser's next tab, was taken");
  // Close gives the page back, and the focus to the row.
  await page.el("dismissed-send-close").click();
  assert.deepStrictEqual(inertIds(page), [], "the page stayed inert after Close");
  assert.equal(focused(page), dismissedOpenerOf(dismissedItems(page)[0]));
  // ...as every other way it goes does: Escape, folding its group, leaving Settings and Put back.
  await dismissedOpenerOf(dismissedItems(page)[0]).click();
  await press(page, page.el("dismissed-send-copy"), "Escape");
  assert.equal(page.screen(), "settings");
  assert.deepStrictEqual(inertIds(page), [], "the page stayed inert after Escape");
  await dismissedOpenerOf(dismissedItems(page)[0]).click();
  await page.el("fold-dismissed").click();
  assert.deepStrictEqual(inertIds(page), [], "the page stayed inert after its group was folded");
  await page.el("fold-dismissed").click();
  await dismissedOpenerOf(dismissedItems(page)[0]).click();
  await page.el("close-settings").click();
  assert.deepStrictEqual(inertIds(page), [], "the page stayed inert after Settings was left");
  await page.el("open-settings").click();
  // An element already inert is left inert: it is not the pop-out's to give back.
  page.el("settings-screen").setAttribute("inert", "");
  await dismissedOpenerOf(dismissedItems(page)[0]).click();
  await page.el("dismissed-send-restore").click();
  await page.settle();
  assert.deepStrictEqual(inertIds(page), ["settings-screen"], "Put back gave back more, or less, than the pop-out took");
  // The negative controls: a pop-out that makes nothing inert, and one that does not keep Tab.
  const open = await dismissedPageOf(["a message"], brokenScript("inert: inertAround(popout) }", "inert: [] }"));
  await dismissedOpenerOf(dismissedItems(open)[0]).click();
  assert.equal(inertNow(open.el("fold-dismissed")), false, "the negative control made the page inert anyway, so the check above proves nothing");
  const loose = await dismissedPageOf(["a message"], brokenScript('document.addEventListener("keydown", trapDismissedTab);\n', ""));
  await dismissedOpenerOf(dismissedItems(loose)[0]).click();
  await press(loose, loose.el("dismissed-send-close"), "Tab");
  assert.equal(focused(loose), loose.el("dismissed-send-close"), "the negative control kept Tab anyway, so the check above proves nothing");
});

// `#351 settings-polish`, from its review. The dim is the pop-out's shadow, and the scrolling
// #screen-settings clips it, so the top bar stood above it bright, with its Back inert: a tap on the
// way out a phone shows first did nothing, and nothing said why.

/** Settings with one dismissed send, inside the frame web/voice.html draws round it, as far as the dim needs. */
async function framedDismissedPage(script = SCRIPT) {
  const page = await dismissedPageOf(["a message to read whole"], script);
  await page.el("fold-settings-all").click();
  const frame = [
    ["topbar", "frame-body", "dock"],
    ["frame-body", "error-wrap", "status-line", "screens"],
    ["screens", "screen-main", "screen-settings"],
    ["screen-settings", "settings-panes"],
  ];
  page.document.body.append(page.el("topbar"), page.el("frame-body"), page.el("dock"));
  for (const [parent, ...kids] of frame.slice(1)) {
    for (const kid of kids) assertMarkupContains(parent, kid);
    page.el(parent).append(...kids.map((kid) => page.el(kid)));
  }
  // The stylesheet says what the fixture does: Settings' box clips, and the pill floats.
  assert.match(cssBlock("#screen-settings"), /overflow-y:\s*auto/, "web/voice.css no longer makes Settings' box scroll");
  assert.match(cssBlock("#status-line"), /position:\s*absolute/, "web/voice.css no longer floats the status pill");
  page.clipOverflowOf(["screen-settings"], ["status-line"]);
  return page;
}

/** The ids of the page's own elements dimmed where they stand. */
const underDimIds = (page) => [...page.elements.values()].filter((node) => node.hasAttribute("data-under-dim")).map((node) => node.id);

test("`#351 settings-polish`: what the pop-out makes inert beyond the box its shadow is drawn in is dimmed where it stands, and nothing twice", async () => {
  const page = await framedDismissedPage();
  await dismissedOpenerOf(dismissedItems(page)[0]).click();
  assert.equal(page.el("dismissed-send-popout").hidden, false);
  // Beyond #screen-settings, which clips the shadow, and laid out beside it: the top bar with its Back,
  // the dock, the error line, the main screen and the send card's confirmation are inert and dimmed where
  // they stand. The pill, which floats over the box, is under the shadow already: the test after this.
  assert.deepStrictEqual(underDimIds(page).sort(), ["dock", "error-wrap", "post-confirm", "screen-main", "topbar"],
    "the elements dimmed are not those beyond Settings' scrolling box");
  for (const id of underDimIds(page)) assert.equal(page.el(id).hasAttribute("inert"), true, `#${id} is dimmed and not inert`);
  // Inside the box the shadow is the dim: what is inert there is not dimmed again.
  for (const id of ["fold-dismissed", "dismissed-sends", "settings-channels", "fold-settings-voice-agent", "fold-storage"]) {
    assert.equal(inertNow(page.el(id)), true, `#${id} can still be reached behind the pop-out`);
    assert.equal(page.el(id).hasAttribute("data-under-dim"), false, `#${id}, under the shadow, is dimmed twice`);
  }
  // The stylesheet: the dim beyond the box is the shadow's own depth.
  const shadow = /box-shadow:\s*0 0 0 100vmax rgb\(0 0 0 \/ ([0-9.]+)\)/.exec(cssBlock(".data-popout"));
  assert.ok(shadow, "the pop-out's dim is no longer a shadow of one colour");
  const filter = /filter:\s*brightness\(([0-9.]+)\)/.exec(cssBlock("[data-under-dim]"));
  assert.ok(filter, "what is dimmed beyond the box is not dimmed by a brightness filter");
  assert.equal(Number(filter[1]), Number((1 - Number(shadow[1])).toFixed(2)),
    `the dim beyond the box keeps ${filter[1]} of each colour and the shadow keeps ${1 - Number(shadow[1])}`);
  // Every way it goes gives all of it back: Close here, and the rest as the test above.
  await page.el("dismissed-send-close").click();
  assert.deepStrictEqual(underDimIds(page), [], "something is still dimmed after Close");
  // Where nothing round the pop-out clips its shadow, the shadow covers the window and nothing more is dimmed.
  const open = await dismissedPageOf(["a message"]);
  await dismissedOpenerOf(dismissedItems(open)[0]).click();
  assert.deepStrictEqual(underDimIds(open), [], "something was dimmed where the shadow already covers it");
  // The negative control: a pop-out that makes the top bar inert and leaves it bright, as before.
  const bright = await framedDismissedPage(brokenScript('      if (beyond && !floatsOverDimBox(beside)) beside.setAttribute("data-under-dim", "");\n', ""));
  await dismissedOpenerOf(dismissedItems(bright)[0]).click();
  assert.equal(bright.el("topbar").hasAttribute("inert"), true, "the negative control's top bar is not inert");
  assert.deepStrictEqual(underDimIds(bright), [], "the negative control dimmed the top bar anyway, so the check above proves nothing");
});

// `#351 settings-polish`, from the review of its second round. The pill was dimmed where it stood too,
// and Settings' box made a stacking context of its own to keep the shadow off it; then the box, and the
// pop-out in it, were drawn under the pill, which floats over the foot of the screen. Opened low on the
// screen, the pop-out is brought on screen by its foot, and the pill, inert and dimmed, covered Copy,
// Put back and Close, for six seconds or for as long as a call was not answering.

test("`#351 settings-polish`: the status pill, floating over Settings' box, is left to the pop-out's shadow, and the pop-out is drawn over it", async () => {
  const page = await framedDismissedPage();
  page.el("status-line").hidden = false;
  await dismissedOpenerOf(dismissedItems(page)[0]).click();
  assert.equal(page.el("dismissed-send-popout").hidden, false);
  // Inert, so nothing reaches it, and not dimmed where it stands: it is under the shadow, which dims it.
  assert.equal(page.el("status-line").hasAttribute("inert"), true, "the pill can still be reached behind the pop-out");
  assert.equal(page.el("status-line").hasAttribute("data-under-dim"), false, "the pill, under the shadow, is dimmed again where it stands");
  // What stands beside the box is still dimmed: only what floats over it is left to the shadow.
  for (const id of ["topbar", "dock"]) assert.equal(page.el(id).hasAttribute("data-under-dim"), true, `#${id} is no longer dimmed`);
  // ...and nothing round the pop-out is made a box of its own that the pill could be drawn over.
  assert.deepStrictEqual([...page.elements.values()].filter((node) => node.hasAttribute("data-dim-box")).map((node) => node.id), [],
    "a box round the pop-out is still set apart from the pill");
  // The stylesheet: the pop-out is drawn over the frame's overlays, the pill among them.
  const zOf = (selector) => Number((/z-index:\s*(-?\d+)/.exec(cssBlock(selector)) || [])[1]);
  assert.ok(zOf(".data-popout") > zOf("#status-line"),
    `the pop-out's z-index, ${zOf(".data-popout")}, does not put it over the pill's, ${zOf("#status-line")}`);
  // A pill that comes up while the pop-out is up is under it in the same way.
  await page.el("dismissed-send-close").click();
  page.el("status-line").hidden = true;
  await dismissedOpenerOf(dismissedItems(page)[0]).click();
  page.el("status-line").hidden = false;
  assert.equal(page.el("status-line").hasAttribute("data-under-dim"), false, "a pill hidden when the pop-out opened is dimmed again when it comes up");
  await page.el("dismissed-send-close").click();
  assert.equal(page.el("status-line").hasAttribute("inert"), false, "Close did not give the pill back");
  // The negative control: a pop-out that dims everything beyond its box, the pill included, as before.
  const twice = await framedDismissedPage(brokenScript("if (beyond && !floatsOverDimBox(beside))", "if (beyond)"));
  await dismissedOpenerOf(dismissedItems(twice)[0]).click();
  assert.equal(twice.el("status-line").hasAttribute("data-under-dim"), true,
    "the negative control left the pill to the shadow anyway, so the check above proves nothing");
});

test("`#351 settings-polish`: a row of Recently dismissed is named by its place, its age and its first line, and still holds the whole text", async () => {
  const cut = sourceConstant("DISMISSED_LABEL_CHARS");
  const lines = `\n  \ndeploy note: the cache is warm again\nthe second line, which the row's name leaves out\n${"and more after it ".repeat(20)}`;
  const one = `${"a single line of words ".repeat(12)}to the end`;
  const page = await dismissedPageOf([lines, one]);
  const nameOf = (item) => dismissedOpenerOf(item).getAttribute("aria-label") ?? dismissedOpenerOf(item).text();
  const metaOf = (item) => item.descendants().find((node) => node.hasClass("dismissed-send-meta")).textContent;
  const [single, multi] = dismissedItems(page);
  assert.equal(nameOf(multi), `${metaOf(multi)}: deploy note: the cache is warm again`, "the row is not named by its place, age and first line");
  assert.match(multi.text(), /the second line[\s\S]*and more after it/, "the row no longer holds the whole text for find-in-page");
  // One long line is cut where a word ends, as a name, not read out whole.
  assert.ok(nameOf(single).startsWith(`${metaOf(single)}: a single line of words`));
  assert.ok(nameOf(single).endsWith("…") && nameOf(single).length <= metaOf(single).length + 2 + cut,
    `a one-line message's row is named by ${nameOf(single).length} characters`);
  assert.match(single.text(), /to the end$/);
  // The negative control: a row with no name of its own is named by everything in it.
  const blind = await dismissedPageOf([lines], brokenScript('    open.setAttribute("aria-label", dismissedRowLabel(held));\n', ""));
  assert.match(nameOf(dismissedItems(blind)[0]), /the second line/, "the negative control's row is named by one line anyway, so the check above proves nothing");
});

test("`#348 settings-dynamic-lists`: Connection status puts each part of the detail on its own line; the banner keeps one", async () => {
  const page = newPage();
  await signIn(page);
  (await startTalking(page)).close();
  await page.settle();
  const lines = page.el("settings-detail").textContent.split("\n");
  assert.ok(lines.length >= 3, `the readout is not one fact to a line: ${JSON.stringify(lines)}`);
  assert.ok(lines.every((line) => line.length > 0 && !line.includes(" · ")), `a line still holds two facts: ${JSON.stringify(lines)}`);
  assert.equal(page.el("connection-detail").textContent, lines.join(" · "), "the banner and the readout say different things");
  assert.match(cssRules(CSS_CODE, ".connection-panel > .data-line").join("\n"), /white-space:\s*pre-line/,
    "the stylesheet draws the lines as one");
});

/**
 * What is wrong with Settings' folds against `#346`, as sentences; empty when nothing is. A function of
 * the markup, the stylesheet and the script, so each negative control can hand it one of them broken.
 */
function foldProblems(html, css, script) {
  const problems = [];
  const code = html.replace(/<!--[\s\S]*?-->/g, " ");
  const settings = code.slice(code.indexOf('id="screen-settings"'), code.indexOf('id="screen-help"'));
  const folds = [];
  // Every section but Server: its title is the button, and everything after the heading is the body.
  for (const [, id] of settings.matchAll(/<section class="settings-section" id="settings-([a-z-]+)"/g)) {
    const section = settingsSection(SETTINGS_SECTIONS[[...settings.matchAll(/<section class="settings-section" id="settings-([a-z-]+)"/g)].findIndex((m) => m[1] === id)], code);
    if (id === "server") {
      if (/fold-toggle|fold-body/.test(section)) problems.push("the Server section folds, hiding which server this is");
      continue;
    }
    const slug = `settings-${id}`;
    folds.push(slug);
    const toggle = new RegExp(`<h2 id="settings-${id}-title"[^>]*><button class="fold-toggle" id="fold-${slug}" type="button" aria-expanded="false" aria-controls="fold-${slug}-body">[^<]+</button>(?:<span class="settings-scope-note">[^<]*</span>)?</h2>`);
    if (!toggle.test(section)) problems.push(`${slug}'s title is not the button that folds it`);
    const body = new RegExp(`</h2>(?:\\s*<button class="help-link"[^>]*>\\?</button>\\s*</div>)?\\s*<div class="fold-body" id="fold-${slug}-body" hidden="until-found">`);
    if (!body.test(section)) problems.push(`${slug}'s body is not right under its title, hidden until found`);
  }
  // Every headed group: its heading is the button, its body under the heading's row.
  for (const group of settings.matchAll(/<section class="settings-group"[^>]*>\s*<div class="group-head">\s*<h3\b[^>]*>([\s\S]*?)<\/h3>([\s\S]*?)<\/div>\s*([\s\S]{0,140})/g)) {
    const button = /^<button class="fold-toggle" id="fold-([a-z-]+)" type="button" aria-expanded="false" aria-controls="fold-\1-body">[^<]+<\/button>$/.exec(group[1]);
    if (!button) {
      problems.push(`the group "${group[1].replace(/<[^>]+>/g, "")}" does not fold`);
      continue;
    }
    folds.push(button[1]);
    if (!new RegExp(`^<div class="fold-body" id="fold-${button[1]}-body" hidden="until-found">`).test(group[3])) {
      problems.push(`${button[1]}'s body is not under its heading, hidden until found`);
    }
  }
  // Folded is "until found", never plain hidden, or the browser's find cannot reach the words.
  if (/<div class="fold-body"[^>]*\shidden(?!="until-found")/.test(settings)) problems.push("a fold is plainly hidden, out of find's reach");
  if (/<details\b/.test(settings.replace(/<details id="voice-system-prompt-fold"/, ""))) problems.push("a fold is a <details>");
  const hiddenRule = cssRules(css, "[hidden]").join("\n");
  if (/display:\s*none/.test(hiddenRule)) problems.push("[hidden] takes hidden-until-found out of find as well");
  if (!/display:\s*none\s*!important/.test(cssRules(css, '[hidden]:not([hidden="until-found"])').join("\n"))) {
    problems.push("nothing hides an ordinary [hidden] any more");
  }
  if (cssRules(css, ".fold-body").some((rule) => /display:\s*none|visibility:\s*hidden/.test(rule))) {
    problems.push("the stylesheet hides a fold's body itself");
  }
  // The script's list is the markup's, both ways.
  const listed = scriptFolds(script);
  const missing = folds.filter((slug) => !listed.includes(slug));
  const extra = listed.filter((slug) => !folds.includes(slug));
  if (missing.length) problems.push(`SETTINGS_FOLDS does not list ${missing.join(", ")}`);
  if (extra.length) problems.push(`SETTINGS_FOLDS lists ${extra.join(", ")}, which the markup does not have`);
  return problems;
}

test("`#346 settings-collapsible`: every section but Server and every group in them folds, hidden until found", () => {
  assert.deepStrictEqual(foldProblems(HTML, CSS_CODE, SCRIPT_CODE), []);
  // 31 since `#481 slack-app-deep-link` added Opening links, counted and checked above.
  assert.equal(scriptFolds(SCRIPT_CODE).length, 34, "a fold went missing from the list, or one was added unchecked");
});

test("`#346 settings-collapsible`: the check refuses a fold out of find's reach, an unfolding group, and a list that disagrees (negative controls)", () => {
  const plain = HTML.replace('<div class="fold-body" id="fold-microphone-body" hidden="until-found">', '<div class="fold-body" id="fold-microphone-body" hidden>');
  assert.deepStrictEqual(foldProblems(plain, CSS_CODE, SCRIPT_CODE), [
    "microphone's body is not under its heading, hidden until found",
    "a fold is plainly hidden, out of find's reach",
  ]);
  const oldCss = CSS_CODE.replace('[hidden]:not([hidden="until-found"]) {', "[hidden] {");
  assert.notEqual(oldCss, CSS_CODE, "the negative control changed nothing");
  assert.deepStrictEqual(foldProblems(HTML, oldCss, SCRIPT_CODE), [
    "[hidden] takes hidden-until-found out of find as well",
    "nothing hides an ordinary [hidden] any more",
  ]);
  const flat = HTML.replace(/<button class="fold-toggle" id="fold-typing"[^>]*>Typing<\/button>/, "Typing");
  assert.deepStrictEqual(foldProblems(flat, CSS_CODE, SCRIPT_CODE), ['the group "Typing" does not fold', "SETTINGS_FOLDS lists typing, which the markup does not have"]);
  const unlisted = SCRIPT_CODE.replace(/(const SETTINGS_FOLDS = \[[^\]]*?)  "token",\n/, "$1");
  assert.notEqual(unlisted, SCRIPT_CODE, "the negative control changed nothing");
  assert.deepStrictEqual(foldProblems(HTML, CSS_CODE, unlisted), ["SETTINGS_FOLDS does not list token"]);
  const server = HTML.replace('<h2 id="settings-server-title">Server</h2>',
    '<h2 id="settings-server-title"><button class="fold-toggle" id="fold-settings-server" type="button">Server</button></h2>');
  assert.deepStrictEqual(foldProblems(server, CSS_CODE, SCRIPT_CODE), ["the Server section folds, hiding which server this is"]);
});

test("`#346 settings-collapsible`: everything is folded as the page loads; a title opens its section, a heading its group, and again folds it", async () => {
  const page = newPage();
  await signIn(page);
  await page.el("open-settings").click();
  for (const slug of scriptFolds(SCRIPT_CODE)) assert.equal(settingsFoldState(page, slug), false, `${slug} is open as the page loads`);
  await page.el("fold-settings-voice-agent").click();
  assert.equal(settingsFoldState(page, "settings-voice-agent"), true);
  // The section opens to its groups' headings, each still folded.
  assert.equal(settingsFoldState(page, "resuming"), false, "opening the section opened its groups too");
  await page.el("fold-resuming").click();
  assert.equal(settingsFoldState(page, "resuming"), true);
  assert.equal(settingsFoldState(page, "relay"), false);
  await page.el("fold-resuming").click();
  assert.equal(settingsFoldState(page, "resuming"), false, "the heading did not fold its group again");
  // What is open stays open while the page is: through Help and back, and out of Settings and in.
  await page.el("fold-microphone").click();
  await page.el("help-link-microphone").click();
  await page.el("close-help").click();
  await page.el("close-settings").click();
  await page.el("open-settings").click();
  assert.equal(settingsFoldState(page, "microphone"), true, "the group folded itself behind the reader's back");
  assert.equal(settingsFoldState(page, "settings-voice-agent"), true);
  // A reload folds everything again: folded by default.
  const again = reloadWith(new Map(page.storage), []);
  await signIn(again);
  assert.equal(settingsFoldState(again, "microphone"), false, "a fold was remembered across a reload");
});

test("`#346 settings-collapsible`: the browser's find opening a fold is heard, and its button says it is open", async () => {
  // A match in a folded group: the browser opens the group's body and the section's around it, firing
  // `beforematch` on each, outermost last (measured in Chromium 149), and removes their `hidden` itself.
  const page = newPage();
  await signIn(page);
  await page.el("open-settings").click();
  for (const slug of ["microphone", "settings-voice-agent"]) {
    page.el(`fold-${slug}-body`).removeAttribute("hidden");
    await page.el(`fold-${slug}-body`).dispatch("beforematch");
  }
  assert.equal(settingsFoldState(page, "microphone"), true, "the group's button still says it is folded");
  assert.equal(settingsFoldState(page, "settings-voice-agent"), true);
  // The negative control: a page that does not listen leaves the buttons saying folded over open bodies,
  // which `foldState` refuses.
  const deaf = newPage(new Map(), brokenScript(
    '  el(`fold-${slug}-body`).addEventListener("beforematch", () => setFold(slug, true));\n', ""));
  await signIn(deaf);
  await deaf.el("open-settings").click();
  deaf.el("fold-microphone-body").removeAttribute("hidden");
  await deaf.el("fold-microphone-body").dispatch("beforematch");
  assert.throws(() => settingsFoldState(deaf, "microphone"), /says false over a body hidden as null/);
});

test("`#346 settings-collapsible`: Expand all opens every fold and then reads Fold all, which folds them all", async () => {
  const page = newPage();
  await signIn(page);
  await page.el("open-settings").click();
  const all = page.el("fold-settings-all");
  assert.equal(all.textContent, "Expand all");
  await page.el("fold-settings-messages").click();
  assert.equal(all.textContent, "Expand all", "one fold open, and the button offers to fold");
  await all.click();
  for (const slug of scriptFolds(SCRIPT_CODE)) assert.equal(settingsFoldState(page, slug), true, `Expand all left ${slug} folded`);
  assert.equal(all.textContent, "Fold all");
  await page.el("fold-typing").click();
  assert.equal(all.textContent, "Expand all", "with a fold folded the button still offers to fold");
  await all.click();
  await all.click();
  for (const slug of scriptFolds(SCRIPT_CODE)) assert.equal(settingsFoldState(page, slug), false, `Fold all left ${slug} open`);
  assert.equal(all.textContent, "Expand all");
});

test("`#351 settings-polish`: Expand all and Fold all count only the folds on the page, not a hidden group's", async () => {
  const page = newPage();
  page.agentSettingsUrl = null;
  await signIn(page);
  await page.el("open-settings").click();
  // Neither Provider (no console to link to) nor the message cache (none to manage) is shown here.
  assert.equal(page.el("agent-settings").hidden, true);
  assert.equal(page.el("message-cache-group").hidden, true);
  const hidden = ["provider", "message-cache"];
  const all = page.el("fold-settings-all");
  // Every fold the reader can see, opened by hand: nothing is left to expand.
  for (const slug of scriptFolds(SCRIPT_CODE).filter((one) => !hidden.includes(one))) await page.el(`fold-${slug}`).click();
  assert.equal(all.textContent, "Fold all", "with every fold on the page open, the button still offers to expand them all");
  // A group shown folded, as Provider is once the server names a console, is something to expand again.
  page.agentSettingsUrl = "https://voice.example/agents/1";
  page.el("api-token").value = "write-token-aaaaaaaaaaaaaaaa";
  await page.el("save-token").click();
  await page.settle();
  assert.equal(page.el("agent-settings").hidden, false, "the server's console link did not show Provider");
  assert.equal(all.textContent, "Expand all", "a folded group come into sight is not counted");
  await all.click();
  assert.equal(settingsFoldState(page, "provider"), true);
  assert.equal(all.textContent, "Fold all");
  // Fold all folds every fold, a hidden group's too, so one shown later is folded with the rest.
  await all.click();
  for (const slug of scriptFolds(SCRIPT_CODE)) assert.equal(settingsFoldState(page, slug), false, `Fold all left ${slug} open`);
  // The negative control: a button that counts every fold, shown or not.
  const blind = newPage(new Map(), brokenScript(
    'el("fold-settings-all").textContent = shownFolds().some(', 'el("fold-settings-all").textContent = SETTINGS_FOLDS.some('));
  blind.agentSettingsUrl = null;
  await signIn(blind);
  await blind.el("open-settings").click();
  for (const slug of scriptFolds(SCRIPT_CODE).filter((one) => !hidden.includes(one))) await blind.el(`fold-${slug}`).click();
  assert.equal(blind.el("fold-settings-all").textContent, "Expand all",
    "the negative control counted only the folds on the page anyway, so the check above proves nothing");
});

// --- `#345 settings-multi-column` -----------------------------------------------------------------
//
// The owner, 2026-10-18: "on wide desktop windows, tile Settings' top-level panes into width-dependent
// columns, each pane kept whole and packed, order and focus order unchanged; phone/narrow single column."

/**
 * What is wrong with Settings' columns against `#345`, as sentences; empty when nothing is. A function
 * of the markup and the stylesheet, so each negative control can hand it one of them broken.
 */
function columnProblems(html, css) {
  const problems = [];
  const code = html.replace(/<!--[\s\S]*?-->/g, " ");
  const settings = code.slice(code.indexOf('id="screen-settings"'), code.indexOf('id="screen-help"'));
  const inside = (selector) => cssRules(mediaBodyOf(css, DESKTOP_QUERY), selector).join("\n");
  const outside = (selector) => cssRules(cssWithoutIn(css, DESKTOP_QUERY), selector).join("\n");
  // Width-dependent columns, on a wide desktop window only.
  const panes = inside(".settings-panes");
  const width = /column-width:\s*([\d.]+)rem/.exec(panes);
  if (!width) problems.push("the panes have no column width on a desktop window");
  else if (Number(width[1]) < 20 || Number(width[1]) > 34) problems.push(`a column of ${width[1]}rem is not a pane's width`);
  if (/column/.test(outside(".settings-panes"))) problems.push("the panes are in columns on a phone too");
  if (!/max-width:\s*(?:[4-9]\d|1\d\d)rem/.test(inside("#screen-settings > .settings-column"))) {
    problems.push("the settings screen is still held to one pane's width on a desktop window");
  }
  if (!/max-width:\s*34rem/.test(cssRules(css, ".settings-column").join("\n"))) problems.push("the phone's column lost its measure");
  // Each pane kept whole.
  if (!/break-inside:\s*avoid/.test(inside(".settings-panes > .settings-section"))) problems.push("a pane can break across two columns");
  // Order and focus order unchanged: nothing reorders the panes, and they are in the box in the order they were.
  const reorders = [".settings-panes", ".settings-panes > .settings-section", ".settings-section"]
    .map((selector) => cssRules(css, selector).join("\n")).join("\n");
  if (/(?:^|[\s;{])order:|grid-row|column-span:\s*all|flex-direction:\s*\w+-reverse/.test(reorders)) {
    problems.push("a rule reorders the panes");
  }
  if (/tabindex="[1-9]/.test(settings)) problems.push("a positive tabindex reorders the focus");
  // The box to its own closing tag, found by counting, since sections hold <div>s of their own.
  const opens = settings.indexOf('<div class="settings-panes" id="settings-panes">');
  let box = "";
  if (opens >= 0) {
    const tags = /<div\b|<\/div>/g;
    tags.lastIndex = opens;
    let depth = 0;
    for (let tag = tags.exec(settings); tag; tag = tags.exec(settings)) {
      depth += tag[0] === "</div>" ? -1 : 1;
      if (depth === 0) {
        box = settings.slice(opens, tag.index);
        break;
      }
    }
  }
  const held = [...box.matchAll(/<section class="settings-section" id="settings-([a-z-]+)"/g)].map((m) => m[1]);
  if (held.join(" ") !== "voice-agent messages channels screen storage") {
    problems.push(`the panes box holds ${held.join(", ") || "nothing"}, not the five sections in their order`);
  }
  if (!/<section class="settings-section" id="settings-server"[\s\S]*<div class="settings-panes"/.test(settings)) {
    problems.push("the server's address is not above the panes");
  }
  return problems;
}

test("`#345 settings-multi-column`: on a desktop window the sections tile into columns, each whole, in their order; a phone keeps one", () => {
  assert.deepStrictEqual(columnProblems(HTML, CSS_CODE), []);
});

test("`#345 settings-multi-column`: the check refuses columns on a phone, a pane that breaks, a reorder, and a pane out of the box (negative controls)", () => {
  const block = mediaBodyOf(CSS_CODE, DESKTOP_QUERY);
  const panesRule = /\.settings-panes \{[^}]*\}/.exec(block)[0];
  const everywhere = CSS_CODE.replace(panesRule, "") + `\n${panesRule}\n`;
  assert.deepStrictEqual(columnProblems(HTML, everywhere), [
    "the panes have no column width on a desktop window",
    "the panes are in columns on a phone too",
  ]);
  const breaking = CSS_CODE.replace(/(\.settings-panes > \.settings-section \{[^}]*?)break-inside:\s*avoid;/, "$1");
  assert.notEqual(breaking, CSS_CODE, "the negative control changed nothing");
  assert.deepStrictEqual(columnProblems(HTML, breaking), ["a pane can break across two columns"]);
  const reordered = CSS_CODE.replace(/(\.settings-panes > \.settings-section \{)/, "$1\n    order: -1;");
  assert.deepStrictEqual(columnProblems(HTML, reordered), ["a rule reorders the panes"]);
  const narrow = CSS_CODE.replace(/(#screen-settings > \.settings-column \{\s*)max-width:\s*125rem;/, "$1max-width: 34rem;");
  assert.notEqual(narrow, CSS_CODE, "the negative control changed nothing");
  assert.deepStrictEqual(columnProblems(HTML, narrow), ["the settings screen is still held to one pane's width on a desktop window"]);
  // Storage and connection taken out of the box and put before it: out of the columns, and out of order.
  const storage = settingsSection("Storage and connection");
  const out = HTML.replace(`${storage}</section>`, "").replace('<div class="settings-panes" id="settings-panes">',
    `${storage}</section><div class="settings-panes" id="settings-panes">`);
  assert.notEqual(out, HTML, "the negative control changed nothing");
  assert.deepStrictEqual(columnProblems(out, CSS_CODE),
    ["the panes box holds voice-agent, messages, channels, screen, not the five sections in their order"]);
});

test("`#345 settings-multi-column`: a heading pressed is brought back on screen, since its section can move to another column", async () => {
  const page = newPage();
  await signIn(page);
  await page.el("open-settings").click();
  const before = page.el("fold-settings-messages").scrolledIntoView;
  await page.el("fold-settings-messages").click();
  assert.equal(page.el("fold-settings-messages").scrolledIntoView, before + 1, "opening a section left its heading where the columns put it");
  await page.el("fold-settings-messages").click();
  assert.equal(page.el("fold-settings-messages").scrolledIntoView, before + 2, "folding it did too");
  // Expand all moves everything, and brings no one heading anywhere.
  const group = page.el("fold-typing").scrolledIntoView;
  await page.el("fold-settings-all").click();
  assert.equal(page.el("fold-typing").scrolledIntoView, group, "Expand all scrolled to a heading of its own accord");
});
