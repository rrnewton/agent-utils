"use strict";
// vibe-talk voice page.
//
// The call flow, in order:
//
//   1. ask THIS server for a signed conversation URL, authenticated with the write-scope token;
//   2. open a WebSocket to that URL;
//   3. stream microphone audio up as base64 16 kHz PCM, and play the agent's audio back down.
//
// Four rules hold here, and each one is a failure this page is meant not to have:
//
// * No vendor script, no CDN, no framework, no build step. Everything below is plain browser API,
//   so the page cannot break because a third-party bundle moved, and it loads on a phone with a
//   bad connection. The app-like frame is CSS (see web/voice.css), not a runtime.
// * No silent degradation. If the mint fails, if the socket closes, or if the agent negotiates an
//   audio format this page cannot decode, the page SAYS SO. It never falls back to an unsigned
//   URL, and it never sits there looking connected while nothing works.
// * Agent text and channel text are UNTRUSTED. Nothing from either is ever assigned as markup.
//   Every visible fragment is built as an element whose text is set with textContent.
// * The interface does not imply continuity the session does not have. See `noteConversationEnded`
//   and `onClear`: a hang-up loses the agent's context and says so, and clearing the screen is
//   labelled and described as clearing the screen. What it says on the SURFACE, though, is two or
//   three words on a separator; the sentences live one tap inside it. The majority of the space
//   belongs to the transcript and the controls.

// The older page at / shared this key, so a browser signed in to either one holds its token here.

// `#369 pairing-link`. A pairing link is the app's address with `#pair=CODE`: a single-use code,
// good for a few minutes, that the server trades once for the write token. The code is a
// credential for as long as it lives, so it comes out of the address FIRST — before any request
// this page makes, and before anything could copy the address — with `replaceState`, which leaves
// no history entry holding it. The fragment, never the query: a browser never sends a fragment to
// any server, so the code reaches no log but the one request that redeems it, in its body. It is
// redeemed only when the person taps Sign this device in (`devicePairing.redeem`), never on load: a link
// scanner that runs pages would otherwise spend it, and hold the token.
//
// The offer and its redemption are web/src/view/settings `mountPairing` (`#624 voice-page-modularity`),
// made here, with the code, so it holds it from the page's first statement.
const openedWithPairingCode = takePairingCode();
/** @type {import("./src/view/settings/index.ts").Pairing} */
const devicePairing = mountPairing({
  code: openedWithPairingCode,
  lookup: (id) => el(id),
  redeem: (code) => fetch("/api/v1/pair/redeem", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ code }),
    cache: "no-store",
  }),
  saveToken: () => saveToken(),
  signIn: () => signIn(),
  hasToken: () => Boolean(token()),
  hydrateFromCache: () => hydrateFromCache(),
  reopenSavedView: () => reopenSavedView(),
  showScreen: (name) => showScreen(name),
  showError: (text) => showError(text),
  clearError: () => clearError(),
  setStatus: (text) => setStatus(text),
  guard: (run) => guardQuietly(run),
});

/** The code in `#pair=`, with the fragment already gone from the address; null when there is none. */
function takePairingCode() {
  const location = window.location;
  const hash = location && typeof location.hash === "string" ? location.hash : "";
  const part = hash.replace(/^#/, "").split("&").find((one) => one.startsWith("pair="));
  if (part === undefined) return null;
  const history = window.history;
  if (history && typeof history.replaceState === "function") {
    history.replaceState(history.state, "", `${location.pathname || "/"}${location.search || ""}`);
  }
  try {
    return decodeURIComponent(part.slice("pair=".length));
  } catch (_error) {
    return "";
  }
}
// The key this page used before the service was renamed. It is READ and never written: a browser
// that signed in to the old name still holds the token under here, on the same origin, and the
// rename is why the page is suddenly asking for it again. Without this the first load after a
// rename is indistinguishable from a broken deployment — the owner is signed out of a URL that
// worked yesterday and nothing says why. Adopting the old value automatically was the other
// option and is deliberately not taken: a token that was revoked in the meantime would produce a
// refusal loop nobody could see the cause of, and silently reusing a credential the owner cannot
// see is worse than asking for it once.
const COLLAPSED_LINES_KEY = DEVICE_SETTINGS.collapsedLines.key;
const READ_SPEED_KEY = DEVICE_SETTINGS.readSpeed.key;
const MARK_OWN_KEY = DEVICE_SETTINGS.markOwnRead.key;
const COMBINE_KEY = DEVICE_SETTINGS.combineMessages.key;
const HIDE_EMPTY_KEY = DEVICE_SETTINGS.hideEmptyMessages.key;
const RICH_HEADERS_KEY = DEVICE_SETTINGS.richHeaders.key;
const IN_APP_LINKS_KEY = DEVICE_SETTINGS.inAppLinks.key;
/** `#580 open-in-chat-jump`. The view a sent card's Open in channel lands in: "flat" (All) or "main". */
const OPEN_IN_VIEW_KEY = DEVICE_SETTINGS.openInView.key;
const SUMMARIZE_COLLAPSED_KEY = DEVICE_SETTINGS.summarizeCollapsed.key;
const ACTIVE_CHANNEL_KEY = "vibe-talk.voice.active-channel";

// --- two columns on a desk: which column the code is working on -------------------------------
//
// `#229 desktop-two-column`. On a desk wide enough, the channel can be read in two columns: Main in
// #scroll-area on the left, and one thread, or the list of threads, in #side-column on the right.
// The thread on the right is drawn by the same code that draws a thread on the left, and that code
// keeps what it is showing in module state and in the elements of one pane. So the right column has
// elements of its own (each `side-` id standing in for one of the pane's, `COLUMN_IDS`) and state of
// its own: records on the column object by field group (`olderPages`, `replyArrows`, `loadState`,
// ...; `#624 voice-page-modularity`, plan step 15), each reached through the column worked on. The
// code works on one column at a time: the LEFT ONE, except inside `inColumn(columns.side, ...)`, which
// makes the right column the one worked on, runs, and puts the left column back before it returns. Nothing is left pointing at the right column between
// one event and the next, so code that never heard of a second column keeps meaning the left one.

/** The elements of the channel's pane, and the right column's that stand in for each: one owner in
 * web/src/view/channel/columnwork.ts (`COLUMN_IDS`, `#624 voice-page-modularity`). */

/**
 * The two columns. `left` says the column has been worked on and left since it was last made afresh
 * (`useColumn`, `resetSideColumn`); `mode` is what the right column shows: the list of threads, or one thread.
 *
 * `searched` is what the column's last pass of the search counted (`applySearch`), for the count in
 * the bar, which speaks for every column on screen.
 *
 * `searchWalk` is the column's walk back through older pages for the search while one is under way,
 * and `searchNote` what the last one found, kept under its control until the search changes (`#262
 * search-load-older`, `searchOlder`).
 *
 * `fromCards` says the thread the right column shows was entered from its card, so that ← goes back
 * out to the cards (`#277 thread-card-focus`, `enterCard`).
 *
 * `older`, `arrows`, `load`, `entry`, `pins`, `tools`, `filter`, `list`, `rows`, `selection`, `composer`, `done`, `place` and `gather` are the column's own records of its older pages
 * (`freshOlderPages`, `olderPages`), its reply arrows (`freshReplyArrows`, `replyArrows`), its reads'
 * state (`freshLoadState`, `loadState`), entering a view (`freshEntryState`, `entryState`), its Pinned
 * filter (`freshPinsView`, `pinsView`), its scroll tools' pass (`freshScrollTools`, `scrollTools`) and
 * its filters' hold (`freshFilterState`, `filterState`), what its list has drawn (`freshListState`,
 * `listState`), the rows it was drawn from (`freshTimelineRows`, `timelineRows`) and what it shows
 * (`freshSelection`, `selection`), its message box (`freshComposerState`, `composerState`) and what it
 * shows as Done (`freshDoneState`, `doneState`) and where its list stands against its newest message
 * (`freshScrollPlace`, `scrollPlace`) and what is gathered in its list (`freshGatherState`,
 * `gatherState`; `#624 voice-page-modularity`, plan step 15): every field group a column keeps, each
 * on the column itself. One owner in web/src/view/channel/columnwork.ts (`createColumns`), made here
 * from the page's fresh records.
 *
 * @typedef {{matched: number, loaded: number, tally: any, pinned: boolean}} SearchCount
 * @typedef {{name: string, left: boolean, searched: SearchCount | null,
 *   searchWalk: any, searchNote: {key: string, text: string} | null,
 *   older: ReturnType<typeof freshOlderPages>, arrows: ReturnType<typeof freshReplyArrows>,
 *   load: ReturnType<typeof freshLoadState>, entry: ReturnType<typeof freshEntryState>,
 *   pins: ReturnType<typeof freshPinsView>, tools: ReturnType<typeof freshScrollTools>,
 *   filter: ReturnType<typeof freshFilterState>, list: ReturnType<typeof freshListState>,
 *   rows: ReturnType<typeof freshTimelineRows>, selection: ReturnType<typeof freshSelection>,
 *   composer: ReturnType<typeof freshComposerState>, done: ReturnType<typeof freshDoneState>,
 *   place: ReturnType<typeof freshScrollPlace>, gather: ReturnType<typeof freshGatherState>}} ColumnFields
 * @type {{main: ColumnFields, side: ColumnFields & {mode: "list" | "thread", fromCards: boolean}}}
 */
const columns = createColumns({
  older: () => freshOlderPages(), arrows: () => freshReplyArrows(), load: () => freshLoadState(),
  entry: () => freshEntryState(), pins: () => freshPinsView(), tools: () => freshScrollTools(),
  filter: () => freshFilterState(), list: () => freshListState(), rows: () => freshTimelineRows(),
  selection: (view) => freshSelection(view), composer: () => freshComposerState(),
  done: () => freshDoneState(), place: () => freshScrollPlace(), gather: () => freshGatherState(),
});
/**
 * The column the code is working on now — the left one, except inside `inColumn` — and whether that is the
 * right one, for `el`, which asks on every call: one owner in web/src/view/channel/columnwork.ts (`#624
 * voice-page-modularity`). The page destructures `useColumn` and `inColumn` under their old names.
 *
 * @type {import("./src/view/channel/index.ts").ColumnWorkPort<ColumnFields>}
 */
const columnWork = createColumnWork(columns);
const { useColumn, inColumn } = columnWork;

/**
 * `#349 channel-settings-all`. The value of Settings' channel picker's first choice, All channels:
 * not an id any chat service gives a channel. Up here because the picker is filled during start-up.
 */
const ALL_CHANNELS = "*";

/**
 * The element with this id: in the column being worked on, for the ids of the channel's pane (see
 * `COLUMN_IDS`). Typed by the kind of element voice.html gives the id (web/src/view/dom `ElementById`,
 * generated from the markup by web/ids.mjs, `#624 voice-page-modularity`), so the checker knows `.value`
 * and `.checked` exist on a form control; an id not known as a literal is a plain HTMLElement.
 *
 * @template {string} K
 * @param {K} id
 * @returns {K extends keyof import("./src/view/dom/index.ts").ElementById ? import("./src/view/dom/index.ts").ElementById[K] : HTMLElement}
 */
function el(id) {
  // The column worked on decides (web/src/view/dom `elementIn`, `#624 voice-page-modularity`).
  // Typed by the id, as the JSDoc above says; the lookup itself answers an HTMLElement.
  return /** @type {any} */ (elementIn(id, columnWork.side(), COLUMN_IDS));
}

/**
 * The selection's one owner (web/src/core/navigation, `#624 voice-page-modularity`): the channel
 * chosen to read. Made on first use, when the bar's picker is subscribed to it as its projection. A
 * `var` with no initialiser, so a use before this line has run finds it undefined rather than in its
 * dead zone, and this line, when it runs, does not throw away an owner already made.
 *
 * @type {import("./src/core/navigation/index.ts").Navigation | undefined}
 */
var pageNavigation;

/** {@link pageNavigation}, made if it is not yet. */
function selectionOwner() {
  if (!pageNavigation) {
    pageNavigation = createNavigation();
    // The bar's picker shows the selection, and nothing else: no reader asks the picker which
    // channel is chosen. It has no right-column stand-in, so `el` always finds the bar's.
    pageNavigation.subscribe((snapshot) => {
      el("discord-channel").value = snapshot.channelId;
    });
  }
  return pageNavigation;
}

/**
 * The id of the channel chosen to read ("" for none). Page-wide: both columns read one channel.
 *
 * @returns {string}
 */
function selectedChannelId() {
  return selectionOwner().snapshot().channelId;
}

/**
 * Choose channel `id` to read, as the bar's picker takes a value: one it does not list chooses no
 * channel at all, as setting a `<select>` to a value it has no option for leaves it on none. The
 * caller says what the choice means (`changeSelectedChannel`); this only records it.
 *
 * @param {string} id
 */
function chooseChannel(id) {
  selectionOwner().selectChannel(channelDirectory.channels().some((channel) => String(channel.id) === String(id)) ? String(id) : "");
}

/**
 * The element with this id in the LEFT column whichever column is being worked on: for the few
 * things that belong to the left column alone (the call's pane beside the channel's, the chips and
 * the pull gesture over #scroll-area, the keyboard's pane for it) and may be asked about from inside
 * `inColumn`.
 *
 * @param {string} id
 */
function leftEl(id) {
  return document.getElementById(id);
}

/** The origin receiving the token and API requests, shown before and after sign-in. */
function deploymentOrigin() {
  const origin = window.location && typeof window.location.origin === "string"
    ? window.location.origin.trim()
    : "";
  return origin || "this server";
}

for (const id of ["signin-server-url", "settings-server-url"]) {
  el(id).textContent = deploymentOrigin();
}

/** The host this page was opened at, port included: `#342 settings-server-hostname`. */
function deploymentHost() {
  const host = window.location && typeof window.location.host === "string"
    ? window.location.host.trim()
    : "";
  if (host) return host;
  const origin = deploymentOrigin().replace(/^[a-z][a-z0-9+.-]*:\/\//i, "");
  return origin.split("/")[0] || deploymentOrigin();
}

const settingsReadouts = mountSettingsReadouts({
  lookup: (id) => el(id),
  deploymentHost: () => deploymentHost(),
  pageOutOfDate: () => pageOutOfDate(),
  get pageBuild() { return PAGE_BUILD; },
  get serverPageBuild() { return serverFacts.pageBuild(); },
  get voice() { return voiceProvider.current(); },
  renderFolds: () => settingsFolds.renderAll(),
  deviceSettings: () => deviceSettings(),
});

settingsReadouts.serverHost();

// ONE status line. It used to be two — a word under the header and a sentence at the foot — which
// is how the closed state managed to announce itself three times in three vocabularies. The
// sentence is the line; the state is the dot beside it.
//
// `#63 status-line-placement` made it TRANSIENT. It was a permanent row in the dock, holding a
// strip of a phone screen on every frame for a line that is blank most of the time. It now appears
// when there is something to say and takes itself away.
//
// A message that goes away can hide something the owner never saw, so nothing that MUST survive is
// carried here and nowhere else: a failure is in `#error` until it is fixed, a close code is in
// the connection detail on the settings screen, a conversation boundary is a seam in the
// transcript, and the live/muted/idle state is on the controls themselves. What is left is a thing
// that was true a moment ago.
// The line, the state dot, the error panel and the connection details are view/shell's (`createShellNotices`,
// below, `STATUS_DISMISS_MS`; `#624 voice-page-modularity`).

// The floating chips can wrap or disappear as actions become available. Keep the banner above
// their measured height instead of giving both overlays the same bottom edge. Observing the tools
// also handles font/viewport changes and their parent screen being hidden, without moving history.
//
// `#194 thread-picker-polish`. The same measurement is the room the channel composer keeps beside
// itself (see `#channel-composer` in web/voice.css; above it since `#245 done-feedback`, which also
// stands the chips on the box, `placeChipsOverComposer`), so the chips float over that room and not
// over the box being typed in. That room is at the foot of the scrolled content, so a reader parked
// at the newest line would be left short of it whenever it grew: keep that reader at the foot.
// `#241 voice-composer` gave the call's message box the same room, so the same holds on the call's
// view: a long turn arriving folded brings up "Expand all", the room grows by the chip, and a
// reader following the call would otherwise be left a chip short of the foot and stop following.
//
// `#280 scroll-stability`. THE LISTS KEEP THEIR ROOM WHILE THEIR SCREEN IS HIDDEN. The chips go with the
// main screen and are measured at nothing, and a room of nothing written where the lists read it took that
// room out of them on every other screen. Chromium laid the list out without it as the main screen came
// back, and only then were the chips measured again: a reader 40px up All with Summarise and Expand all up
// was within the slack of the foot of a list 36px short, and was taken to the foot as the room came back.
// So #screen-main holds the room as last measured on it, which the lists and their boxes read, and changes
// it only while it is shown; the status pill, which stands on every screen, reads the measure as it is.
// `#1185 chip-height-snap`. Whether Main's reader is following its foot, as the foot keeper notes
// it (one state, from the reader's own scrolls and landings): the keepers mount at the boot below,
// long after these observers, so their answer is held here once made and the re-derived test used
// until then.
var mainFollowState = null;

/**
 * Whether the chips' and the strip's keepers may take Main's list to its foot as they resize:
 * the follow state while the keepers can say it, with a filter's or a quick reply's hold still
 * vetoing; the re-derived test (`followsNewest`) only before they mount. A reader resting a few
 * pixels above the foot, inside the old slack, is not following, and is left where they are.
 *
 * @param {HTMLElement} area
 */
function chipsFollowFoot(area, grew = 0) {
  if (!mainFollowState) return followsNewest(area);
  if (heldFilterRow() !== null || quickReplyHolds(area)) return false;
  // The keeper's state, or the reader standing at the foot within its rest room where the state
  // has not heard their landing (a screen coming back, a list whose scrolls were another's):
  // past that room a reader resting up the list is left where they are (`#1185`). Either way the
  // reader must stand within the slack plus THIS observer's own growth of the foot: content that
  // grew elsewhere (a row at the foot laid out taller) strands a follower where they stand, and
  // the jump to it is offered, not taken (`#207`).
  if (!atBottom(area, BOTTOM_SLACK_PX + grew)) return false;
  return mainFollowState() || atBottom(area, FOLLOW_REST_PX);
}

let scrollToolsHeight = 0;
if (typeof window.ResizeObserver === "function") {
  new window.ResizeObserver(([entry]) => {
    const height = entry.contentRect.height;
    const grew = Math.max(0, height - scrollToolsHeight);
    scrollToolsHeight = height;
    const area = el("scroll-area");
    // `#221 read-modes`. A reader just landed on a thread's first unread is reading that, however near
    // the end of the list it is: the chips changed because the entry opened the message's fold.
    const landed = screens.screen() === "main" && screens.view() === "discord" && entryLandingHolds();
    // `#243 filter-keeps-selection`: nor one whose message a filter change holds in place. `#241 voice-composer`:
    // the call's transcript follows too, not only the channel.
    const pinned = !landed && screens.screen() === "main" && chipsFollowFoot(area, grew);
    const room = height > 0 ? `calc(${height}px + 0.5rem)` : "0px";
    el("frame-body").style.setProperty("--scroll-tools-clearance", room);
    // `#433 read-strip-low`. The row's height itself, without the half-rem the room keeps over
    // it: the read-aloud strip stands on the row's top edge, with no gap, so it hugs the row as
    // it wraps, empties or is hidden away.
    el("frame-body").style.setProperty("--scroll-tools-height", `${height}px`);
    const screen = leftEl("screen-main");
    if (!screen.hidden) screen.style.setProperty("--scroll-tools-clearance", room);
    if (landed) placeEntryLanding(entryState().landing.row, entryState().landing.first);
    if (pinned) area.scrollTop = area.scrollHeight;
    // The room the channel's box keeps for them is above it, so it moved with them.
    placeChipsOverComposer();
    // `#451 read-strip-followups`: and the strip docked on this row moved with it, so a place a
    // desk reader dragged it to is held inside the window against the row as it now stands — a
    // chip arriving or the row wrapping is no window resize to do it by that path. (Called after
    // `placeChipsOverComposer`, which this measure also moves; that one holds the strip again for
    // the box's own growth, and holding twice in one report holds it once.)
    readAloudOwner().placeReadSteer();
  }).observe(el("scroll-tools"));
}

// `#433 read-strip-low`. The read-aloud strip's own measured height is the room everything over
// it keeps: the status pill stacks on it while both are up, and the list's foot keeps it so the
// last message still scrolls clear. Zero while it is hidden (a `display: none` box measures
// nothing), so none of that room is kept for a strip that is not there. Kept beside the chips'
// measurement, and held on the hidden screen for the reason that one is (`#280`, above).
let readSteerHeight = 0;
if (typeof window.ResizeObserver === "function") {
  new window.ResizeObserver(() => {
    // The whole box, padding and border included: a content-box measure would leave what stacks
    // over the strip standing that much into it. Half a rem over it is the air the pill keeps
    // over whatever it stands on, stated here because the chips' room states it only while a
    // chip is showing.
    const height = el("read-steer").offsetHeight || 0;
    const grew = Math.max(0, height - readSteerHeight);
    readSteerHeight = height;
    const room = height > 0 ? `calc(${height}px + 0.5rem)` : "0px";
    const area = el("scroll-area");
    const landed = screens.screen() === "main" && screens.view() === "discord" && entryLandingHolds();
    const pinned = !landed && screens.screen() === "main" && chipsFollowFoot(area, grew);
    el("frame-body").style.setProperty("--read-steer-clearance", room);
    // `scrubber-full-width`. The height itself, without the air: on a phone the strip playing is a
    // row of its own over the dock, and the list ends at its top edge (`data-read-strip`). Held on
    // the hidden screen as the room is, so the list comes back laid out with it (`#280`).
    el("frame-body").style.setProperty("--read-steer-height", `${height}px`);
    const screen = leftEl("screen-main");
    if (!screen.hidden) screen.style.setProperty("--read-steer-height", `${height}px`);
    if (!screen.hidden) screen.style.setProperty("--read-steer-clearance", room);
    if (landed) placeEntryLanding(entryState().landing.row, entryState().landing.first);
    if (pinned) area.scrollTop = area.scrollHeight;
    // The strip floats in the same band as the chips over the channel's box, so it moved with it.
    placeChipsOverComposer();
    // `#442 read-strip-order`: a strip moved on a desk that grew is held inside the window again.
    readAloudOwner().placeReadSteer();
  }).observe(el("read-steer"));
}

// `#245 done-feedback`. The chips float over the foot of the list, and the channel's message box is
// the last thing in the list, so as the list scrolls the box passes under them: from review, the
// Undo chip stood on Send in the 412px captures, the list a little short of its end. So wherever the
// box reaches down into the strip the chips and the status pill over them float in, they stand on
// its top edge instead, never lower, as a sticky row would, and at the very end in the room the box
// keeps above itself (`#channel-composer` in web/voice.css). A box that ends above that strip — a
// channel too short to scroll — leaves them at the foot, where they cover nothing; lifted over it
// they stood on the list's last rows and the menus those open. This writes `--composer-top`, how far
// above the foot of the list the box's top edge is (0 when they stay at the foot); the stylesheet
// stands the chips, and the status pill and the pull's report on them, on the higher of their own
// place and that edge. On every scroll, so measured only near the end of the list: further up the
// box is below the screen, and three numbers the jump reads already say so.
// (web/src/view/shell `createChipsOverComposer`, `#624 voice-page-modularity`; `CHIPS_PILL_REM` there.)
const chipsOverComposer = createChipsOverComposer({
  lookup: (id) => leftEl(id),
  el: (id) => el(id),
  onChannel: () => screens.screen() === "main" && screens.view() === "discord",
  rootFontPx: () => rootFontPx(),
  root: () => document.documentElement,
  placeReadSteer: () => readAloudOwner().placeReadSteer(),
});
const { placeChipsOverComposer } = chipsOverComposer;

/**
 * The status line, the state dot beside it, the error panel and the connection details: one owner in
 * web/src/view/shell/notices.ts (`#624 voice-page-modularity`). The page destructures its commands under
 * their old names; taking the credential out of what is said (`redact`) and holding the reader as the panel
 * comes and goes are the page's.
 */
const shellNotices = createShellNotices({
  clock: clockOnUse(),
  el: (id) => el(id),
  redact: (text) => redact(text),
  // `voice-unresponsive-signal`: while the service is not answering, the line falls back to saying so.
  unresponsive: () => (voiceCall().session.health && voiceCall().session.health.unresponsive
    ? voiceCall().UNRESPONSIVE_STATUS : null),
  holdingReader: (mutate) => holdingReader(mutate),
  detailChanged: (text) => connectionReadouts.detail(text),
});
const { setStatus, statusSays, dismissStatus, setState, showError, clearError, showDetail, addDetail, dismissBanner } = shellNotices;

// The state dot (`setState`): one of idle, working, live, unresponsive, ended, suspended, error, which
// web/voice.css colours. `suspended` is `#54 resume-recovery`: the socket died while the page was in the
// background, neither `ended` (the reader chose to hang up) nor `error` (something is broken). `unresponsive`
// is `voice-unresponsive-signal`: the socket is open and the call may recover, but the service has stopped
// producing anything audible or readable.

// Nothing this page displays may contain a credential.
//
// The server redacts its own secrets before answering, but this page holds one the server has
// never seen: the API token in this browser. An error message is assembled from whatever came
// back, and a server — or something in between it and here — is free to echo a request back
// verbatim. So the last thing before any text reaches the DOM is this: the token, if it is in
// there, is replaced. It is one line, it costs nothing, and it removes a whole class of "the
// error message leaked the key" from being possible at all.
function redact(text) {
  let out = String(text);
  const secrets = [token(), el("api-token").value || ""];
  for (const secret of secrets) {
    // Short strings are not credentials, and blanket-replacing one would mangle ordinary words.
    if (secret.length >= 8) {
      out = out.split(secret).join("[redacted]");
    }
  }
  return out;
}

// The whole point of an earlier pass: a failure is SHOWN, in the page, in words the owner can act
// on. The reported bug was that a 502 naming a missing API-key permission appeared only in the dev
// console, so the only visible symptom was a page that did nothing. The panel lives outside the
// three screens so it is visible on whichever one is up. `showError` and `clearError` are the shell notices'
// (above). `#268 voice-call-drops`: `stay` keeps the panel until the reader acts — a new call, or its dismiss —
// instead of taking it away after twelve seconds, because a driver glances back at the screen a minute later.

// Call state and controls are owned by view/voice. Construction performs no I/O.
voiceCall();

/** @type {import("./src/view/voice/index.ts").VoiceCall | undefined} */
var pageVoiceCall;
/** The call owner; dependency properties stay live across asynchronous callbacks. */
function voiceCall() {
  if (!pageVoiceCall) pageVoiceCall = createVoiceCall({
    get noteTypedForSpokenYes() { return voiceTranscript().noteTypedForSpokenYes; },
    get setStatus() { return setStatus; },
    get addDetail() { return addDetail; },
    get transcriptSeam() { return voiceTranscript().transcriptSeam; },
    get WebSocket() { return WebSocket; },
    get typedAnswerToCard() { return voiceTranscript().typedAnswerToCard; },
    get noteUserTextForSpokenYes() { return voiceTranscript().noteUserTextForSpokenYes; },
    get line() { return voiceTranscript().line; },
    get recordTurn() { return voiceTranscript().recordTurn; },
    get reportHeard() { return voiceTranscript().reportHeard; },
    get heardTyped() { return voiceTranscript().heardTyped; },
    set heardTyped(value) { voiceTranscript().heardTyped = value; },
    get el() { return el; },
    get reportAnswerOutcome() { return voiceTranscript().reportAnswerOutcome; },
    get storedExactly() { return messageCache.storedExactly; },
    get deviceStorage() { return deviceStorage; },
    get fitGrowingBox() { return fitGrowingBox; },
    get leftEl() { return leftEl; },
    get keepBoxInView() { return keepBoxInView; },
    get currentScreen() { return screens.screen(); },
    get currentView() { return screens.view(); },
    get docsReaderOpen() { return screens.view() === "docs" && documentsView.reading(); },
    get CANNED_PROMPTS() { return /** @type {import("./src/view/settings/index.ts").CannedPrompt[]} */ (CANNED_PROMPTS); },
    get token() { return token; },
    get apiDecoded() { return apiDecoded; },
    get readingMode() { return readAloudOwner().readingMode; },
    get nowPlaying() { return readAloudOwner().nowPlaying; },
    get pendingRead() { return readAloudOwner().pendingRead; },
    get navigator() { return typeof navigator === "undefined" ? undefined : navigator; },
    get showError() { return showError; },
    get callAudioRoute() { return audioRoute.current(); },
    set callAudioRoute(value) { audioRoute.set(/** @type {import("./src/core/call/index.ts").CallAudioRoute | null} */ (value)); },
    get redact() { return redact; },
    get api() { return api; },
    get clearError() { return clearError; },
    get guardQuietly() { return guardQuietly; },
    get conversationalVoice() { return voiceProvider.current(); },
    get setState() { return setState; },
    get startAwaitingAnswer() { return voiceTranscript().startAwaitingAnswer; },
    set startAwaitingAnswer(value) { voiceTranscript().startAwaitingAnswer = value; },
    get fetchResume() { return fetchResume; },
    get showDetail() { return showDetail; },
    get micSettings() { return microphoneSettings.read; },
    get openCallMicrophone() { return openCallMicrophone; },
    get echoCancellationRefused() { return voiceTranscript().echoCancellationRefused; },
    get browserPorts() { return browserPorts; },
    get applyCallOutput() { return applyCallOutput; },
    get refreshAudioDeviceSelects() { return audioDevices.refresh; },
    get renderAudioRouteReadout() { return audioDevices.renderRoute; },
    get resumeViaProfile() { return voiceTranscript().resumeViaProfile; },
    get armRecapForCall() { return voiceTranscript().armRecapForCall; },
    get dismissBanner() { return dismissBanner; },
    get conversationOpen() { return voiceTranscript().conversationOpen; },
    set conversationOpen(value) { voiceTranscript().conversationOpen = value; },
    get startPostWatch() { return voiceTranscript().startPostWatch; },
    get conversationIdFrom() { return voiceTranscript().conversationIdFrom; },
    get noteCallRecap() { return voiceTranscript().noteCallRecap; },
    get recapFor() { return voiceTranscript().recapFor; },
    get expectStartMarker() { return voiceTranscript().expectStartMarker; },
    get lastEndCause() { return voiceTranscript().lastEndCause; },
    get handOverAgentPrompt() { return agentSettings.handOverPrompt; },
    get renderAfterHandOver() { return agentSettings.renderAfterHandOver; },
    get noteConversationEnded() { return voiceTranscript().noteConversationEnded; },
    get noteSessionStartedForSpokenYes() { return voiceTranscript().noteSessionStartedForSpokenYes; },
    get heardEpoch() { return voiceTranscript().heardEpoch; },
    set heardEpoch(value) { voiceTranscript().heardEpoch = value; },
    get noteConversationNamed() { return voiceTranscript().noteConversationNamed; },
    get spokenYes() { return voiceTranscript().spokenYes; },
    get hearAssistantForSpokenYes() { return voiceTranscript().hearAssistantForSpokenYes; },
    get noteYesCandidate() { return voiceTranscript().noteYesCandidate; },
    get hearUserForSpokenYes() { return voiceTranscript().hearUserForSpokenYes; },
    get settleYesCandidate() { return voiceTranscript().settleYesCandidate; },
    get noteCutForSpokenYes() { return voiceTranscript().noteCutForSpokenYes; },
    get extendRedraftWait() { return voiceTranscript().extendRedraftWait; },
    get atBottom() { return atBottom; },
    get followIfPinned() { return followIfPinned; },
    get releaseYesHold() { return voiceTranscript().releaseYesHold; },
    get turnNode() { return voiceTranscript().turnNode; },
    get applySearch() { return applySearch; },
    get isFolded() { return isFolded; },
    get setFolded() { return setFolded; },
    get flushRecordsAfterLive() { return voiceTranscript().flushRecordsAfterLive; },
    get recordingBroken() { return voiceTranscript().recordingBroken; },
    get recordQueue() { return voiceTranscript().recordQueue; },
    set recordQueue(value) { voiceTranscript().recordQueue = value; },
    get abandonWatchedRecap() { return voiceTranscript().abandonWatchedRecap; },
    get withdrawProfileRecap() { return voiceTranscript().withdrawProfileRecap; },
    get endReplayAnswerWait() { return voiceTranscript().endReplayAnswerWait; },
    get resetSpokenYes() { return voiceTranscript().resetSpokenYes; },
    get armProfileRecap() { return voiceTranscript().armProfileRecap; },
    get resumeAllowed() { return callResume.allowed(); },
    get resumeWanted() { return resumeWanted; },
    get sendReplayFrame() { return voiceTranscript().sendReplayFrame; },
    get watchProfileRecap() { return voiceTranscript().watchProfileRecap; },
    get freshBecause() { return voiceTranscript().freshBecause; },
    get renderCallRecap() { return voiceTranscript().renderCallRecap; },
    get REPLAY_ANSWER_WAIT_MS() { return voiceTranscript().REPLAY_ANSWER_WAIT_MS; },
    get noteReplacedV1Session() { return voiceTranscript().noteReplacedV1Session; },
    get offerReplayAtSessionStart() { return voiceTranscript().offerReplayAtSessionStart; },
    get noteReplayAck() { return voiceTranscript().noteReplayAck; },
    get noteTurnCompleteForSpokenYes() { return voiceTranscript().noteTurnCompleteForSpokenYes; },
    get judgeAnswerAtTurnEnd() { return voiceTranscript().judgeAnswerAtTurnEnd; },
    get settleYesCandidates() { return voiceTranscript().settleYesCandidates; },
    get stopPostWatch() { return voiceTranscript().stopPostWatch; },
    get withdrawArmedRecap() { return voiceTranscript().withdrawArmedRecap; },
    get resumeLast() { return callResume.last(); },
    set resumeLast(value) { callResume.setLast(/** @type {import("./src/core/call/index.ts").ResumeLast} */ (value)); },
    get renderResumeState() { return renderResumeState; },
    get renderReadModeControl() { return renderReadModeControl; },
    get renderReadTransport() { return readAloudOwner().renderReadTransport; },
    get readInProgress() { return readAloudOwner().readInProgress; },
    get readState() { return readAloudOwner().readState; },
    get closeSpeedPopover() { return closeSpeedPopover; },
    get resumeNote() { return resumeNote; },
    get RELAY_MAX_CHARS() { return RELAY_MAX_CHARS; },
    get channelLabel() { return channelLabel; },
    get RELAY_PREAMBLE() { return RELAY_PREAMBLE; },
    get RELAY_FENCE_OPEN() { return RELAY_FENCE_OPEN; },
    get RELAY_FENCE_CLOSE() { return RELAY_FENCE_CLOSE; },
    get relayMode() { return relayMode; },
    get RELAY_COALESCE_MS() { return RELAY_COALESCE_MS; },
    get hiddenEmpty() { return hiddenEmpty; },
    get startWatchAll() { return startWatchAll; },
    get stopWatchAll() { return stopWatchAll; },
    get openDocumentNotice() { return () => documentsView.documentNotice(); },
  });
  return pageVoiceCall;
}

/** @type {import("./src/view/voice/index.ts").VoiceTranscript | undefined} */
var pageVoiceTranscript;
/** The call owner; dependency properties stay live across asynchronous callbacks. */
function voiceTranscript() {
  if (!pageVoiceTranscript) pageVoiceTranscript = createVoiceTranscript({
    get session() { return voiceCall().session; },
    get el() { return el; },
    get leftEl() { return leftEl; },
    get api() { return api; },
    get apiDecoded() { return apiDecoded; },
    get columns() { return columns; },
    get currentScreen() { return screens.screen(); },
    get currentView() { return screens.view(); },
    get cardsOpening() { return cardsDrawn.opening(); },
    get threadListShown() { return layoutShown.threadList(); },
    get tokenScope() { return serverFacts.tokenScope(); },
    get voiceSendSetting() { return agentSettings.sendSetting(); },
    get hasEnded() { return voiceCall().hasEnded; },
    set hasEnded(value) { voiceCall().hasEnded = value; },
    get hasSuspended() { return voiceCall().hasSuspended; },
    set hasSuspended(value) { voiceCall().hasSuspended = value; },
    get resumeAllowed() { return callResume.allowed(); },
    get resumeConversationId() { return callResume.conversationId(); },
    set resumeConversationId(value) { callResume.setConversationId(value); },
    get resumeLast() { return callResume.last(); },
    set resumeLast(value) { callResume.setLast(value); },
    get relayAnswerAwaited() { return voiceCall().relayAnswerAwaited; },
    get knownChannels() { return channelDirectory.channels(); },
    get chatProviderName() { return channelDirectory.providerName(); },
    get sidePane() { return sidePane; },
    get COLLAPSE_OVER_CHARS() { return COLLAPSE_OVER_CHARS; },
    get FOLD_LESS() { return FOLD_LESS; },
    get FOLD_MORE() { return FOLD_MORE; },
    get OLDER_TRIGGER_PX() { return OLDER_TRIGGER_PX; },
    get addDetail() { return addDetail; },
    get applySearch() { return applySearch; },
    get atBottom() { return atBottom; },
    get blankResumeLast() { return blankResumeLast; },
    get canSendText() { return voiceCall().canSendText; },
    get cardCameUnderLists() { return cardCameUnderLists; },
    get cardMovedUnderLists() { return cardMovedUnderLists; },
    get chatServiceOf() { return chatServiceOf; },
    get drawnBy() { return drawnBy; },
    get focusList() { return focusList; },
    get foldOnTap() { return foldOnTap; },
    get foldable() { return foldable; },
    get followFoot() { return followFoot; },
    get followIfPinned() { return followIfPinned; },
    get guardQuietly() { return guardQuietly; },
    get heightUnderLists() { return heightUnderLists; },
    get holdingReader() { return holdingReader; },
    get inColumn() { return inColumn; },
    get jumpToSentMessage() { return jumpToSentMessage; },
    get keepTurnFold() { return voiceCall().keepTurnFold; },
    get liveFollowers() { return liveFollowers; },
    get normaliseHasMore() { return olderPager.normaliseHasMore; },
    get noteTypedTurn() { return voiceCall().noteTypedTurn; },
    get placeChipsOverComposer() { return placeChipsOverComposer; },
    get preservingScroll() { return preservingScroll; },
    get reconnectedSessionRefused() { return voiceCall().reconnectedSessionRefused; },
    get recoverUnsettledTurns() { return voiceCall().recoverUnsettledTurns; },
    get redact() { return redact; },
    get renderControls() { return voiceCall().renderControls; },
    get renderEmptyState() { return renderEmptyState; },
    get renderJumpNewest() { return renderJumpNewest; },
    get renderResumeState() { return renderResumeState; },
    get renderScrollTools() { return renderScrollTools; },
    get resumeArmed() { return resumeArmed; },
    get resumeWanted() { return resumeWanted; },
    get scrollToNewest() { return scrollToNewest; },
    get searchable() { return searchable; },
    get sendClientEvent() { return voiceCall().sendClientEvent; },
    get sendVibeTalkFrame() { return voiceCall().sendVibeTalkFrame; },
    get setStatus() { return setStatus; },
    get settleNoticeForRecap() { return voiceCall().settleNoticeForRecap; },
    get showError() { return showError; },
    get tapRow() { return tapRow; },
    get threadListOnScreen() { return threadListOnScreen; },
  });
  return pageVoiceTranscript;
}

/** @type {import("./src/view/voice/index.ts").ReadAloud | undefined} */
var pageReadAloud;
/** The call owner; dependency properties stay live across asynchronous callbacks. */
function readAloudOwner() {
  if (!pageReadAloud) pageReadAloud = createReadAloud({
    get el() { return el; },
    openSentenceReply: (id, text, box) => openSentenceReply(id, text, box),
    readingMoved: () => readingJump.refresh(),
    get leftEl() { return leftEl; },
    get deviceSettings() { return deviceSettings; },
    get guardQuietly() { return guardQuietly; },
    get setStatus() { return setStatus; },
    get api() { return api; },
    get renderControlBar() { return renderControlBar; },
    get browserPorts() { return browserPorts; },
    fetch: (...args) => fetch(...args),
    get browser() { return typeof navigator === "undefined" ? undefined : navigator; },
    audioAvailable: () => typeof Audio === "function",
    get clearError() { return clearError; },
    get heldMessage() { return heldMessage; },
    get session() { return voiceCall().session; },
    get showError() { return showError; },
    get eachColumn() { return eachColumn; },
    get renderChannelRows() { return renderChannelRows; },
    get dismissMessages() { return dismissMessages; },
    // `#886 voice-page-autoplay`. What reads after a read heard to its end.
    nextAfterRead: (parts) => autoplay.nextAfter(parts),
    get currentView() { return screens.view(); },
    get selectedChannelId() { return selectedChannelId; },
    get shownChannelList() { return /** @type {() => HTMLOListElement} */ (shownChannelList); },
    get threadListShown() { return layoutShown.threadList(); },
    get sideColumnUp() { return sideColumnUp; },
    get sideShowing() { return sideShowing; },
    get rowOnScreen() { return rowOnScreen; },
    get inColumn() { return inColumn; },
    get columns() { return columns; },
    get threadForMessageId() { return threadForMessageId; },
    get renderControls() { return voiceCall().renderControls; },
    get withThreadQuery() { return withThreadQuery; },
    get token() { return token; },
  });
  return pageReadAloud;
}


/**
 * The device's API token, on its own handle (web/src/core/settings, `#624 voice-page-modularity`),
 * kept in the device's storage. Made on first use; a `var` with no initialiser, like `serverApi`.
 *
 * @type {import("./src/core/settings/index.ts").Credentials | undefined}
 */
var pageCredentials;

/** {@link pageCredentials}, made if it is not yet. */
function credentialStore() {
  if (!pageCredentials) pageCredentials = createCredentials(deviceStorage());
  return pageCredentials;
}

/** The token, or "" for none. Throws where the browser refuses storage. */
const token = () => credentialStore().token();

/**
 * True when this browser has no token under the current key but does have one under the key this
 * page used before the rename. Never throws (`Credentials.signedOutByRename`).
 */
function signedOutByRename() {
  return credentialStore().signedOutByRename();
}

// --- navigation orchestration (`#304`, `#423`, `#462`, `#543`, `#580`) -------------------------
//
// The phone's back gesture and the browser's Back, Back and Forward through the places visited,
// deep links and in-app message links: the orchestration — reading where the reader stands,
// deciding what a Back closes, showing a visited place, keeping the address in step, applying the
// link the page opened by, and jumping to a message a link names — is one owner in
// web/src/view/shell/navigation.ts (`createPageNavigation`, `#624 voice-page-modularity`), which
// creates the `core/navigation` owners (`createBackStack`, `createVisits`, `createDeepLinks`,
// `createLinkJumps`) from the prelude's factories. The long rationale for each rule stays in that
// file's section heads and in the core owners' headers. Every dep below is a thunk: this wiring
// runs where the legacy sections stood, long before `screens` (the screens section), the layout
// choice and the channel store are created, and nothing may read them until a returned function
// is called. The page destructures the orchestration under its old names, so callers do not change.

/** This page load's own name for the entries it makes: an entry with another is from before a reload. */
const BACK_SESSION = `${Date.now().toString(36)}.${Math.random().toString(36).slice(2, 10)}`;

/** @type {import("./src/view/shell/index.ts").PageNavigation} */
const shellNavigation = createPageNavigation({
  createBackStack, createVisits, createDeepLinks, createLinkJumps, visitKey, deepLinkQuery,
  clock: clockOnUse(),
  session: BACK_SESSION,
  screens: () => screens,
  el: (id) => el(id),
  leftEl: (id) => leftEl(id),
  inSide: () => columnWork.side(),
  mainColumn: () => columns.main,
  inColumn: (column, act) => inColumn(column, act),
  sideColumn: () => columns.side,
  sideMode: () => columns.side.mode,
  sideThreadId: (channel) => { const held = sideThreads.get(channel); return held ? String(held.id) : null; },
  selectedChannelId: () => selectedChannelId(),
  selection: () => selection(),
  layoutColumns: () => layoutShown.columns(),
  layoutThreadList: () => layoutShown.threadList(),
  layoutFit: () => layoutShown.fit(),
  listBefore: () => layoutChoice.listBefore(),
  sideThreadsHas: (channel) => sideThreads.has(channel),
  replyDocked: () => replyState.docked,
  rowMenuOpen: () => rowMenus.open(),
  promptsOpen: () => promptsTray.open(),
  actionMenuOpen: () => actionMenu.open(),
  closeSpeedPopover: () => closeSpeedPopover(),
  setPromptsOpen: (open) => setPromptsOpen(open),
  setActionMenuOpen: (open) => setActionMenuOpen(open),
  closeDismissedPopout: () => dismissedSendsView.close(),
  focusList: (pane) => focusList(/** @type {any} */ (pane)),
  sidePane: () => sidePane,
  focusLost: () => focusLost(),
  closeSideThread: () => closeSideThread(),
  closeHelp: () => closeHelp(),
  closeSettings: () => closeSettings(),
  closeReply: () => closeReply(),
  documentOpen: () => documentsView.reading(),
  documentDepth: () => documentsView.depth(),
  documentOutlineOpen: () => documentsView.outlineOpen(),
  closeDocumentOutline: () => documentsView.closeOutline(),
  closeDocument: () => documentsView.showList(),
  threadListBack: () => threadListBack(),
  leaveThread: () => leaveThread(),
  showScreen: (name) => showScreen(name),
  showHelp: (entry) => showHelp(entry),
  showView: (name) => showView(name),
  helpEntries: () => helpFolds.entries(),
  threadListOffered: () => threadListOffered(),
  threadStillKnown: (id) => threadStillKnown(id),
  threading: () => providerFacts.threading(),
  chooseView: (which) => chooseView(which),
  selectSideThread: (id) => selectSideThread(id),
  openThreadFromList: (id) => openThreadFromList(id),
  chooseOneColumn: (choice) => chooseOneColumn(choice),
  defaultChannelView: () => defaultChannelView(),
  threadsOfChannel: () => channelStore.current().threads,
  threadIdOf: (thread) => thread.id,
  token: () => token(),
  channels: () => channelDirectory.channels(),
  chooseChannel: (id) => chooseChannel(id),
  changeSelectedChannel: () => changeSelectedChannel(),
  viewKey: () => viewKey(),
  viewsHas: (key) => channelStore.current().views.has(key),
  readsBusy: () => channelReads.busy(),
  readsStarted: () => replayLedger.started(),
  seekMessage: (target, land, elsewhere) => seekMessage(target, land, elsewhere),
  rowHolding: (id) => rowHolding(id),
  answeredElsewhere: (target) => answeredElsewhere(target),
  providerNameOf: (channel) => { const held = providerOfChannel(channel); return held && typeof held.name === "string" && held.name.trim() ? held.name.trim() : ""; },
  providerName: () => channelDirectory.providerName(),
  threadOf: (message) => threadOf(message),
  setStatus: (text) => setStatus(text),
  showError: (message) => showError(message),
  guardQuietly: (run) => guardQuietly(run),
  settleFocus: () => settleFocus(),
  replyOnScreen: () => replyOnScreen(),
  setLinkedMark: (channel, id) => linkedMark.set(channel, id),
  cardsCycle: () => cardsCycle(),
  rowShown: (row) => rowShown(/** @type {any} */ (row)),
  visibleList: () => visibleList(),
  rowOnScreen: (row) => rowOnScreen(row),
  eachColumn: (act) => eachColumn(act),
  openReadRun: (row) => openReadRun(/** @type {any} */ (row)),
  setSearchOpen: (open) => setSearchOpen(open),
  finishPackSlide: () => finishPackSlide(),
  headOfList: (area, row) => headOfList(area, row),
  renderScrollTools: () => renderScrollTools(),
  revealRow: (row, center) => revealRow(row, center),
  idsOf: (row) => idsOf(/** @type {any} */ (row)),
  inAppLinksOn: () => inAppLinks.current(),
  openDocument: (key) => {
    chooseMajorView("docs");
    void documentsView.show(key);
  },
  channelLabel: (channel) => channelLabel(channel),
  renderedText: (node) => renderedText(node),
  inThisColumn: (act) => inThisColumn(act),
  captureScroll: () => captureScroll(),
  listPlaceTop: (area) => listPlaceTop(area),
  sideShowing: () => sideShowing(),
  restoreScroll: (place) => restoreScroll(place),
  scrollToNewest: () => scrollToNewest(),
  bumpReplyJump: () => replyJumpTicket.bump(),
  settleUiState: () => uiState.settle(),
  readEnteredChannel: (restored) => readEnteredChannel(restored),
  viewRestored: () => viewScroll.restored(),
  stopDiscordPolling: () => stopDiscordPolling(),
  storedOpenInView: () => storedOpenInView(),
  writeClipboardText: (text) => writeClipboardText(text),
  copiedRefusedText: () => COPY_REFUSED,
  pullWaitMs: () => PULL_WAIT_MS,
  delay: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
  userActivation: () => (typeof navigator === "object" && navigator ? navigator.userActivation : null),
  locationSearch: () => window.location.search || "",
  locationOrigin: () => (typeof window === "object" && window.location && window.location.origin ? String(window.location.origin) : ""),
  locationPathname: () => (typeof window === "object" && window.location && window.location.pathname) || "/",
  navigationIndex: () => {
    const navigation = typeof window === "object" && window ? window.navigation : null;
    const entry = navigation ? navigation.currentEntry : null;
    return entry && Number.isInteger(entry.index) && entry.index >= 0 ? entry.index : null;
  },
  historyPort: () => {
    const history = typeof window === "object" && window ? window.history : null;
    if (!history || typeof history.pushState !== "function" || typeof history.replaceState !== "function" ||
        typeof history.back !== "function" || typeof history.go !== "function") return null;
    return history;
  },
  layoutRestoring: () => layoutShown.restoring(),
});
const {
  backStack, visits, deepLinks, linkJumps, backHistory, historyIndex, mayPushHistory, openMenus,
  closeOpenMenu, backLevels, screenLevelsNow, closeSideByX, closeBackLevel, scheduleBackSync,
  onBackPop, startBackStack, channelPlace, currentPlace, notePlace, channelPlaceShowable,
  placeShowable, showChannelPlace, showPlace, placeFromKeyboard, noteDeepLinkReaderMove,
  deepLinkReaderMoved, deepLinkRefOf, deepLinkFromAddress, deepLinkFor, syncDeepLinkAddress,
  linkedRowShown, landOnDeepLink, waitForDeepLink, applyDeepLink, copyDeepLink, keepsInApp,
  messageLinkLabel, wireMessageLinks, onMessageLinkTap, linkJumpSpot, withLinkJumps,
  arriveOnChannel, arriveAtPlace, jumpToMessageLink, jumpToSentMessage, returnFromLinkJump,
  putBackOnSpot,
} = shellNavigation;

// `#890 voice-page-linked-highlight`. The message a link landed on, marked until the reader puts the mark out (Esc, its
// ×, another link): one owner in web/src/view/channel/linkedmark.ts, drawn on every row and card as it is drawn
// (`discordNode`, `sideCard`), so neither the selection nor a list drawn again takes it away.
const linkedMark = createLinkedMark({
  rowsDrawn: () => ["discord-log", "side-log", "pinned-log", "side-pinned-log", "side-cards"]
    .flatMap((id) => {
      const list = leftEl(id);
      return list ? /** @type {HTMLElement[]} */ ([...list.children]) : [];
    }),
  // A card holds its thread's first message.
  idsOf: (row) => {
    if (!row.hasAttribute("data-card")) return idsOf(row);
    const root = /** @type {any} */ (row).cardRoot;
    return root && root.id !== undefined && root.id !== null ? [String(root.id)] : [];
  },
  channel: () => String(selectedChannelId()),
  rowShown: (row) => linkedRowShown(row),
  // Put out by the reader, the address lets go of the message too, so a reload does not mark it again.
  // Only where the address still names that message: a link landing since has a place of its own there.
  dismissed: (linked) => {
    const focus = deepLinks.focus();
    if (!focus || String(focus.message) !== linked.id || String(focus.channel) !== linked.channel) return;
    deepLinks.setFocus(null);
    syncDeepLinkAddress();
  },
});

/**
 * Whether a link to a message opens it here: ON unless the reader has turned it off, kept on the device and
 * synced. One owner in web/src/core/settings/inapplinks.ts (`#624 voice-page-modularity`), with what was
 * stored and turning it on or off; Settings draws it.
 *
 * @type {import("./src/core/settings/index.ts").InAppLinks}
 */
const inAppLinks = createInAppLinks({
  deviceSettings: () => deviceSettings(),
  render: (on) => messagePreferences.inAppLinks.render(on),
});

/** Settings presents message preferences; each feature still owns and applies its value. */
const messagePreferences = mountMessagePreferences({
  lookup: (id) => el(id),
  deviceSettings: () => deviceSettings(),
  status: (text) => setStatus(text),
  persistCollapsedLines: (n) => collapsedLines.persist(n),
  changeCollapsedLines: (value) => collapsedLinesChanged(value),
  persistCardMaxLines: (n) => cardMaxLines.persist(n),
  changeCardMaxLines: (value) => cardMaxLinesChanged(value),
  applyQuoteReadPercent: (value) => quoteReadPercent.apply(value),
  applyInAppLinks: (on) => inAppLinks.apply(on),
  redrawMessageLinks: () => redrawMessageLinks(),
  applySummarizeCollapsed: (on) => applySummarizeCollapsed(on),
  applyRichHeaders: (on) => applyRichHeaders(on),
  applyMarkOwnRead: (on) => ownReadSwitch.apply(on),
  applyHideEmptyMessages: (on) => hideEmptySwitch.apply(on),
  redrawHiddenEmptyMessages: () => {
    if (providerFacts.threading()) {
        eachColumn(renderCachedTimeline);
        // ...and the thread cards, whose unread counts it changes: the Threads list's, which that redraw
        // leaves alone, and the right column's.
        eachColumn(() => {
          if (selection().view === "threads") el("thread-list").replaceChildren(...threadCards(timelineRows().threads));
        });
        renderSideColumn();
      } else if (hideEmptySwitch.on() && !readModes.hiding()) {
        regroupChannelRows();
      } else {
        guardQuietly(() => (readModes.hiding() ? loadTodo : (options) => timelineView.loadDiscord(options))({ keepPosition: true, ownAct: true, readAs: "mutation" }))();
      }
  },
  applyCombineMessages: (on) => combineSwitch.apply(on),
  regroupChannelRows: () => eachColumn(regroupChannelRows),
  chooseOpenInView: (view) => chooseOpenInView(view),
  storedOpenInView: () => storedOpenInView(),
});

/**
 * Draw again every body the switch decides the links of, from what is held, as for Empty messages: each
 * column's rows, the thread list's cards and the right column's.
 */
function redrawMessageLinks() {
  if (providerFacts.threading()) {
    eachColumn(renderCachedTimeline);
    eachColumn(() => {
      if (selection().view === "threads") el("thread-list").replaceChildren(...threadCards(timelineRows().threads));
    });
    renderSideColumn();
  } else {
    eachColumn(regroupChannelRows);
  }
}

/**
 * The view a sent card's Open in channel lands in (`#580 open-in-chat-jump`): "flat", All, every
 * message in the order sent, unless the reader chose "main", the threads. ABSENT MEANS ALL, the
 * owner's choice: "I suggest that the all view with all chronological messages be the default".
 *
 * @returns {"flat" | "main"}
 */
function storedOpenInView() {
  return deviceSettings().get(DEVICE_SETTINGS.openInView);
}

/** Draw the choice as stored. Never writes it: an absent choice stays absent, so it syncs nothing. */
function renderOpenInView() {
  messagePreferences.openInView.render();
}

/** Keep the reader's choice, `view`, and draw it. */
function chooseOpenInView(view) {
  try {
    deviceSettings().keep(DEVICE_SETTINGS.openInView, view);
  } catch (_error) {
    // A browser that refuses storage keeps the shipped default.
  }
  renderOpenInView();
}

// --- screens ----------------------------------------------------------------------------------
//
// Three screens, no router and no framework: they are three sections stacked in one grid cell and
// `hidden` picks. The explanatory text that earns no space once you know what the page is lives on
// the sign-in screen; the connection details and the knobs live in settings; the main screen is
// the transcript and the two controls, and nothing else.

// The screens (`SCREENS`), which is up, where Settings' and Help's ways out go, and the view the main
// screen shows: one owner in web/src/view/shell/screens.ts (`#624 voice-page-modularity`).
const screens = createScreens();

// --- documents (`documents-view-client`) -------------------------------------------------------
//
// The third major view: the documents linked in chat, listed and read in
// web/src/view/documents. The page's whole share is this mount, the `docs`
// branch of `showView`, `pressViewSwitch` delegating to the selector, Ctrl+5,
// and the `?v=docs` deep link in `reopenSavedView`. No document state lives
// here; the view owns its list, its reader and its speaking.

/** The mounted Documents view. */
const documentsView = createDocumentsView({
  api: (path, options) => api(path, options),
  byId: (id) => el(id),
  readDocument: (doc) => readAloudOwner().readDocument(doc),
  // Page-side kind-and-id awareness (`#1039` stage 3): the reader names only its
  // own document's key; whether that document's is the read playing (or pending)
  // is read off the owner's surface here, so no other read is ever stopped.
  stopSpeaking: (key) => {
    const owner = readAloudOwner();
    const id = `doc:${key}`;
    const playing = owner.nowPlaying;
    if ((playing && playing.kind === "document" && playing.id === id) || owner.pendingRead === id) {
      owner.stopReading();
    }
  },
  onStatus: (text) => setStatus(text),
  onLevelChange: () => shellNavigation.scheduleBackSync(),
  onBack: () => shellNavigation.documentBack(),
  onJumpToMention: (mention) => {
    void shellNavigation.jumpToSentMessage({
      service: "discord",
      channel: mention.channel_id,
      message: mention.message_id,
      thread: mention.thread_id,
    });
  },
});

/** The three-way selector over `#view-switch`; its press is `pressViewSwitch`. */
const documentsSelector = createViewSelector({
  byId: (id) => el(id),
  current: () => /** @type {"voice"|"discord"|"docs"} */ (screens.view()),
  choose: (view) => chooseMajorView(view),
});

/** Show major view `view` (voice/discord/docs), with each view's entry work. */
function chooseMajorView(view) {
  uiState.settle();
  showView(view);
  if (view === "discord") {
    readEnteredChannel(viewScroll.restored());
    return;
  }
  stopDiscordPolling();
  if (view === "docs") void documentsView.show(documentsDeepLinkKey());
}

/** One `?name=value` of the address, decoded; null when absent. */
function documentsQueryValue(name) {
  for (const part of (window.location.search || "").replace(/^\?/, "").split("&")) {
    const eq = part.indexOf("=");
    if (eq === -1 || part.slice(0, eq) !== name) continue;
    try {
      return decodeURIComponent(part.slice(eq + 1).replace(/\+/g, " "));
    } catch {
      return null;
    }
  }
  return null;
}

/** The document key `?v=docs&d=KEY` names, or undefined for the list. */
function documentsDeepLinkKey() {
  if (documentsQueryValue("v") !== "docs") return undefined;
  return documentsQueryValue("d") || undefined;
}

// Ctrl+5 (Cmd+5 on a Mac) for Documents, beside Ctrl+0 for the call's view.
// Ctrl+9 (Cmd+9) goes straight there too, next to Ctrl+0 (owner, thread 214).
document.addEventListener("keydown", (event) => {
  if ((event.ctrlKey || event.metaKey) && !event.altKey && !event.shiftKey && (event.key === "5" || event.key === "9")) {
    event.preventDefault();
    chooseMajorView("docs");
  }
});

// Keys are the reader at work (reading, stepping): put the cards away at once.
document.addEventListener("keydown", () => documentsSelector.hideCards(), true);

// A tap anywhere but the switch or its cards puts the cards away at once.
document.addEventListener("pointerdown", (event) => {
  const target = /** @type {{closest?: (selector: string) => unknown} | null} */ (event.target || null);
  if (target && typeof target.closest === "function" && (target.closest("#view-cards") || target.closest("#view-switch"))) return;
  documentsSelector.hideCards();
}, true);

/**
 * Showing a screen — everything that goes with the change, in order, the screen itself shown last:
 * one owner in web/src/view/shell/screenshowing.ts (`createScreenShowing`, `#624
 * voice-page-modularity`). The page destructures its command under its old name; what each thing
 * done there does is handed in as it was taken here, every one of them read as it is called, so a
 * screen shown during start-up finds them as they then stand.
 *
 * @type {import("./src/view/shell/index.ts").ScreenShowing}
 */
const screenShowing = createScreenShowing({
  screens,
  el: (id) => el(id),
  closeOverscroll: () => closeOverscroll(),
  holdChannelPlace: () => holdChannelPlace(),
  postCardBackToMain: () => voiceTranscript().postCardBackToMain(),
  placePostCard: () => voiceTranscript().placePostCard(),
  disarmClear: () => voiceTranscript().disarmClear(),
  browserReading: () => readAloudOwner().readAloudPlayback === "browser" || (readAloudOwner().nowPlaying !== null && readAloudOwner().nowPlaying.playback === "browser"),
  stopReading: () => readAloudOwner().stopReading(),
  closeDismissedSends: () => dismissedSendsView.close(false),
  renderDismissedSends: () => dismissedSendsView.render(),
  renderJumpNewest: () => renderJumpNewest(),
  placeChipsOverComposer: () => placeChipsOverComposer(),
  renderControlBar: () => renderControlBar(),
  renderColumns: () => renderColumns(),
  pointSettingsAtReading: () => channelSettings.pointSettingsAtReading(),
  refreshUpstreamClear: () => upstreamClearBox.refresh(),
  refreshChannelHistory: () => channelHistoryBox.refresh(),
  refreshAgentPrefix: () => agentPrefix.refresh(),
  renderIdentities: () => identityList.render(),
  focusLost: () => focusLost(),
  settleFocus: () => settleFocus(),
  scheduleBackSync: () => scheduleBackSync(),
});
const { showScreen } = screenShowing;

// Where Back on Help goes — Settings, which is where Help is opened from, unless `?` opened it
// (`keysFromKeyboard`) — and where Settings' way out goes are the screens' (`Screens.helpReturn`,
// `Screens.beforeSettings`), put back as each screen is arrived at (`Screens.arrive`).


/**
 * Leave Settings for the screen it was opened from: what its Back button does, and Escape.
 *
 * `#224 keyboard-shortcuts`. Every control on Settings keeps its value the moment it changes, so
 * leaving is the whole of "save and exit". The one thing done first is to let go of a field that
 * still holds the focus: a click on Back takes the focus from it, which is what fires its `change`,
 * and Escape must lose no more than that click does.
 */
function closeSettings() {
  const held = /** @type {HTMLElement | null} */ (document.activeElement);
  if (held && !focusLost() && typeof held.blur === "function") held.blur();
  showScreen(screens.beforeSettings());
}

// --- Settings, folded -----------------------------------------------------------------------------
//
// `#346 settings-collapsible`. Every section on Settings and every headed group in them folds, folded on
// every load and hidden until found, so the browser's find still opens one: web/src/view/settings
// `mountSettingsFolds`, with the folds and the words web/src/present/settings says (`#624
// voice-page-modularity`).

/**
 * The folds on Settings: whether one is open, open or fold one, whether one is on the page, and the
 * button over them all redrawn, as a group shown or hidden must (`settingsReadouts.agentLink`, `serverCacheBox`).
 *
 * @type {import("./src/view/settings/index.ts").SettingsFolds}
 */
const settingsFolds = mountSettingsFolds({
  lookup: (id) => el(id),
  closeDismissed: () => dismissedSendsView.close(false),
});

/**
 * Open Help, optionally at one entry.
 *
 * `#85 voice-desktop-review`. The deep link is the thing that makes the split survivable: moving the
 * paragraphs off the settings screen is only an improvement if the paragraph about the switch you
 * are looking at is still one tap away. Without it this would be a filing cabinet.
 *
 * A null slug opens the top, which is what the standing "Open help" button at the head of Settings
 * wants: it is not about any one control.
 */
function showHelp(slug) {
  showScreen("help");
  helpFolds.openAt(slug);
}

// --- Help, folded ---------------------------------------------------------------------------------
//
// `#246 help-sections`. Every entry of Help is a native <details>, folded but the one asked for, under
// one button that opens or folds them all: web/src/view/settings `mountHelpFolds`; the entries a ?
// reaches are web/src/present/settings's (`HELP_TOPICS`, `#624 voice-page-modularity`).

/**
 * Help's folds: its entries, the button over them redrawn, and the screen opened at one entry.
 *
 * @type {import("./src/view/settings/index.ts").HelpFolds}
 */
const helpFolds = mountHelpFolds({
  lookup: (id) => el(id),
  byId: (id) => document.getElementById(id),
});

// The view the main screen shows ("voice" for the call's, "discord" for the channel's) is the screens'
// (`Screens.view`, `#624 voice-page-modularity`).

/**
 * The call, or the channel. One switch with two positions, and the word on it names the position
 * it is IN — not a pair of labels one of which has been styled into looking disabled.
 *
 * Deliberately touches NOTHING in `session`. Switching views during a call must not disturb the
 * call: no socket close, no track stop, no re-acquire, no change to the mute state.
 */
/**
 * Where the reader was in each view, so switching does not throw it away.
 *
 * Both panes share ONE scroll container, so leaving a view loses its position unless somebody
 * writes it down. Nobody did: every switch called `scrollToNewest`, which meant glancing at the
 * transcript and coming back put a reader who was forty messages into a backlog at the bottom
 * again, with no way to find where they had been.
 *
 * `null` means "never been here", which is one of the two cases where landing on the newest message
 * is right — arriving at the TOP of a long channel is the failure the unconditional jump was
 * originally protecting against, and that reasoning was sound for a FIRST entry and wrong for
 * every one after it.
 *
 * THE OTHER CASE IS `atNewest`, and a saved `top` alone cannot express it. A scrollTop is the offset
 * the newest line HAPPENED to be at when the reader left; it is not the instruction "keep me on the
 * newest". Restore it after four turns have arrived in a transcript nobody was watching and the
 * reader lands where the bottom used to be, several turns above the end, with nothing saying so —
 * which is losing their place, not keeping it. This is the same rule the channel's own re-read
 * settles by (`settleAfterRead`): follow the newest for a reader who was on it, restore the offset
 * for a reader who was not. Kept by web/src/core/navigation `createViewPlaces`, with whether the last entry
 * was a return (`restored`), which the read that follows needs.
 *
 * @type {import("./src/core/navigation/index.ts").ViewPlaces<string>}
 */
const viewScroll = createViewPlaces();

/**
 * What the selected channel's chat service can do — threads, its own read cursor, how its live updates are
 * delivered and how often the server reads it: one owner in web/src/core/channels/providerfacts.ts (`#624
 * voice-page-modularity`).
 */
const providerFacts = createProviderFacts();
/**
 * Whether the selected channel holds any thread at all, from its last history page (or yes while a thread is
 * open): one owner in web/src/core/channels/threadpresence.ts (`#624 voice-page-modularity`).
 *
 * @type {import("./src/core/channels/index.ts").ThreadPresence}
 */
const threadPresence = createThreadPresence();
/**
 * A column's own record of what it is showing (`#624 voice-page-modularity`, plan step 15, as
 * `freshOlderPages`): `view`, which list (main, threads, flat or thread); `threadId` and `thread`, the
 * thread selected and its summary; `origin`, the list a thread was opened from. The left column starts
 * on Main only until the channel's provider is known (`openChannelView` decides it from then on); the
 * right column starts on a thread.
 *
 * @param {string} [view]
 * @returns {{view: string, threadId: any, thread: any, origin: string}}
 */
function freshSelection(view = "thread") {
  return { view, threadId: null, thread: null, origin: "main" };
}

/** The column being worked on's record of what it is showing (`freshSelection`). */
function selection() {
  return columnWork.active().selection;
}
/**
 * A column's own record of the rows its list was last drawn from (`#624 voice-page-modularity`, plan
 * step 15, as `freshOlderPages`): `messages`, the messages of a Main, All or thread list; `threads`,
 * the thread summaries of the Threads list.
 *
 * @returns {{messages: any[], threads: any[]}}
 */
function freshTimelineRows() {
  return { messages: [], threads: [] };
}

/** The column being worked on's record of the rows its list was last drawn from (`freshTimelineRows`). */
function timelineRows() {
  return columnWork.active().rows;
}
const channelContexts = new Map();
/**
 * What the reader has typed in each message box, by box key, and the device's copy (web/src/core/outbox
 * `createChannelDrafts`, `#624 voice-page-modularity`).
 *
 * @type {import("./src/core/outbox/index.ts").ChannelDrafts}
 */
const channelDrafts = createChannelDrafts({ storage: deviceStorageOnUse(), makeRoom: () => messageCache.dropMessageCache() });

/**
 * `#819 voice-page-quoted-reply-and-draft-chip`. What was written in quick replies and not sent, by channel,
 * message and quoted words, kept on the device as it is written: the same kind of store, under its own key.
 *
 * @type {import("./src/core/outbox/index.ts").ChannelDrafts}
 */
const quickDrafts = createChannelDrafts({ storage: deviceStorageOnUse(), makeRoom: () => messageCache.dropMessageCache(), key: QUICK_DRAFTS_KEY });

/**
 * `#819 voice-page-quoted-reply-and-draft-chip`. Every draft by where it is written, its channel and when it
 * was last written, kept on the device, for the Draft chip to find them newest first (web/src/core/outbox
 * `createDraftLedger`). The owner lost a dictated reply.
 *
 * @type {import("./src/core/outbox/index.ts").DraftLedger}
 */
const draftLedger = createDraftLedger({ storage: deviceStorageOnUse(), makeRoom: () => messageCache.dropMessageCache(), now: () => Date.now() });

/**
 * A column's own record of its message box (`#624 voice-page-modularity`, plan step 15, as
 * `freshOlderPages`): `key`, the box the column is composing in (`channelComposerKey`), whose draft
 * `channelDrafts` holds.
 *
 * @returns {{key: string}}
 */
function freshComposerState() {
  return { key: "" };
}

/** The column being worked on's record of its message box (`freshComposerState`). */
function composerState() {
  return columnWork.active().composer;
}

// Local send records never enter the provider message list or its read/archive/reply machinery.
// How long one attempt may run, how long a lost answer is asked after, and how often a slow
// send's progress is asked (`OUTGOING_TIMEOUT_MS`, `OUTGOING_OUTCOME_MS`, `OUTGOING_PROGRESS_MS`)
// are the send service's, in web/src/core/outbox (`createOutboxDispatch`, `#624
// voice-page-modularity`), with every attempt, wait and automatic-retry hold it keeps.
// `#466 reply-promotes-thread`. How long a message this page posted is kept though a page of the
// channel does not carry it (`ownPostStands`) is web/src/core/outbox's (`OWN_POST_GRACE_MS`, `#624
// voice-page-modularity`).
// `#500 sent-receipts-pile-up`. How long a confirmed send's "Sent." receipt is shown where the message
// itself is not drawn (`SENT_RECEIPT_MS`) is web/src/core/outbox's, with `sentReceiptRetires`.
// `#195 send-resilience`. When a send whose answer was lost goes out again on its own, and the waits
// before it, are web/src/core/outbox's (`nextRetryAt`, `#624 voice-page-modularity`).
/**
 * The outbox composition state — the outbox, what each send has been observed doing, a saved
 * entry as the live outbox holds it, the sends dismissed from it and their Settings view, and
 * the dismissal the Undo chip takes back — is web/src/core/outbox's now (`createOutboxComposition`,
 * the step 17 composition root's outbox half, `#624 voice-page-modularity`). Made here, in the
 * page's order, beside the send service it feeds.
 *
 * @type {import("./src/core/outbox/index.ts").OutboxComposition}
 */
const outboxComposition = createOutboxComposition({
  storage: () => deviceStorageOnUse(),
  makeRoom: () => messageCache.dropMessageCache(),
  keepsOwnPosts: (channel) => storeKeepsOwnPosts(channel),
  now: () => Date.now(),
  lookup: (id) => el(id),
  channelLabel: (id) => channelLabel(id),
  styled: (tag, className, text) => styled(tag, className, text),
  writeClipboard: (text) => writeClipboardText(text),
  copyRefused: () => COPY_REFUSED,
  putBackDismissed: (id) => sendTakeBack.putBack(id),
  scheduleBackSync: () => scheduleBackSync(),
  mountDismissedSends: (deps) => mountDismissedSends(/** @type {any} */ (deps)),
  clock: clockOnUse(),
  sendUndoEnded: () => sendTakeBack.renderUndo(),
  resumeRetries: () => outboxDispatch.resumeRetries(),
});
const { outbox, outgoingObservations, outgoingFromSaved, dismissedSends, dismissedSendsView, sendUndo } = outboxComposition;

// The attempt in progress for each send, whoever waits for its attempt to end and the credential it
// was queued under, one pass at a time over the outbox, every automatic retry's hold and timer, and
// the running number in new sends' ids are the send service's own (web/src/core/outbox
// `createOutboxDispatch`, made below with the outbox's other owners, `#624 voice-page-modularity`).
/** The column a reply that did not go was sent from, and the one in it not yet been down to,
 * are web/src/view/channel's now (`createColumnReplies`, `#624 voice-page-modularity`). */
/**
 * Which layout of the channel is on screen — one column, two, or the thread list — whether this window
 * draws two columns, the one-column view asked for with the layout, whether the page itself is putting a
 * view back, and the reader's places held across a change of layout for the way back: one owner in
 * web/src/view/channel/layoutshown.ts (`#624 voice-page-modularity`), which says what each place is for.
 *
 * @type {import("./src/view/channel/index.ts").LayoutShown<
 *   {view: string, thread: string | null, summary: any, origin: string | null},
 *   {place: ReturnType<typeof readerPlace>, view: string, channel: string},
 *   {place: {newest: boolean, id: string | null, offset: number}, placed: {newest: boolean, id: string | null, offset: number} | null, channel: string},
 *   {left: NonNullable<ReturnType<typeof leftColumnPlace>>, placed: NonNullable<ReturnType<typeof leftColumnPlace>>, from: "one" | "two"}>}
 */
const layoutShown = createLayoutShown();

/**
 * Where the bar is... (layout choice) — see web/src/core/navigation/layout.ts.
 *
 * @type {import("./src/core/navigation/index.ts").LayoutChoice}
 */
const layoutChoice = createLayoutChoice({
  device: deviceStorageOnUse(),
  window: {
    getItem: (key) => windowStorage().getItem(key),
    setItem: (key, value) => windowStorage().setItem(key, value),
    removeItem: (key) => windowStorage().removeItem(key),
  },
});

/** Two columns on a desk are web/src/view/channel's now (`createTwoColumns`, `#624`). */
const twoColumns = createTwoColumns({
  el: (id) => el(id),
  leftEl: (id) => leftEl(id),
  layoutChoice: /** @type {any} */ (layoutChoice),
  layoutShown: /** @type {any} */ (layoutShown),
  mainColumn: () => columns.main,
  sideColumn: () => columns.side,
  activeColumn: () => columnWork.active(),
  columnSide: () => columnWork.side(),
  useColumn: (column) => useColumn(/** @type {any} */ (column)),
  inColumn: (column, act) => inColumn(/** @type {any} */ (column), act),
  screen: () => screens.screen(),
  view: () => screens.view(),
  threading: () => providerFacts.threading(),
  selectedChannelId: () => selectedChannelId(),
  selection: () => selection(),
  channelCanon: () => channelStore.current(),
  settleFocus: () => settleFocus(),
  setStatus: (text) => setStatus(text),
  noteDeepLinkReaderMove: () => noteDeepLinkReaderMove(),
  guardQuietly: (fn) => guardQuietly(fn),
  renderThreadSelect: () => renderThreadSelect(),
  scheduleColumnLinks: () => scheduleColumnLinks(),
  scheduleBackSync: () => scheduleBackSync(),
  cardAnchor: () => cardAnchor(),
  renderSideCards: () => renderSideCards(),
  readerPlace: () => readerPlace(),
  keepReaderPlace: (place) => keepReaderPlace(/** @type {any} */ (place)),
  changeChannelView: (view, thread, summary, origin, place) => changeChannelView(view, thread, /** @type {any} */ (summary), origin, /** @type {any} */ (place)),
  viewOnScreen: () => /** @type {any} */ (viewOnScreen()),
  rememberView: (view) => uiState.remember(/** @type {any} */ (view)),
  settleUiState: () => uiState.settle(),
  saveUiState: () => saveUiState(),
  configApplied: () => serverFacts.configApplied(),
  hydrateChannelScope: () => hydrateChannelScope(),
  restoreChannelComposer: () => restoreChannelComposer(),
  selectSideThread: (id, summary) => selectSideThread(id, /** @type {any} */ (summary)),
  showSideList: () => showSideList(),
  renderSearchInAll: () => renderSearchInAll(),
  refreshSearchCount: () => refreshSearchCount(),
  closeReply: (options) => closeReply(options),
  replyState: () => replyState,
  nodeWithin: (area, node) => nodeWithin(area, /** @type {any} */ (node)),
  focusLost: () => focusLost(),
  endSearchWalk: (column) => endSearchWalk(/** @type {any} */ (column)),
  endRepliesWalk: () => endRepliesWalk(),
  dropReplyArrowTap: () => dropReplyArrowTap(),
  putRepliesBack: () => putRepliesBack(),
  rememberChannelDraft: () => rememberChannelDraft(),
  rememberChannelContext: () => rememberChannelContext(),
  reopenPendingReply: () => reopenPendingReply(),
  renderMainToggle: () => renderMainToggle(),
  renderOrderToggle: () => renderOrderToggle(),
  placeMainCard: () => placeMainCard(),
  renderUnpackToggle: () => renderUnpackToggle(),
  renderRepliesSearch: () => renderRepliesSearch(),
  noteLinkedSelection: () => noteLinkedSelection(),
  closeOverscroll: () => closeOverscroll(),
  renderSideFreshness: () => renderSideFreshness(),
  setTextIfChanged: (el, text) => setTextIfChanged(el, text),
  nameSideRegion: (name) => nameSideRegion(name),
  paintThreadCard: (el, choice, tally) => paintThreadCard(el, /** @type {any} */ (choice), /** @type {any} */ (tally)),
  replyTally: () => replyTally(),
  knownThreadChoice: (id, fallback) => knownThreadChoice(id, /** @type {any} */ (fallback)),
  openThreadChoice: () => openThreadChoice(),
  threadName: (choice, limit) => threadName(/** @type {any} */ (choice), limit),
  threadStillKnown: (id) => threadStillKnown(id),
  defaultChannelView: () => defaultChannelView(),
  openingChannelView: () => openingChannelView(),
  followIfPinned: (pinned, slack, view) => followIfPinned(pinned, slack, view),
  scrollToNewest: () => scrollToNewest(),
  pinsView: () => pinsView(),
  leaveThreadList: (next) => leaveThreadList(next),
  catchUpHeldView: () => catchUpHeldView(),
  readFilterKeeps: () => readFilterKeeps(),
  settleAfterRead: (messages, options) => settleAfterRead(/** @type {any} */ (messages), /** @type {any} */ (options)),
  timelineRows: () => timelineRows(),
  hiddenEmpty: (message) => hiddenEmpty(/** @type {any} */ (message)),
  hidingRead: () => hidingRead(),
  channelReadPosition: (area) => channelReadPosition(area),
  syncUnreadReplies: () => syncUnreadReplies(),
  placeCardAnchor: (place, follow) => placeCardAnchor(place, follow),
  cardsPlacedUnderLists: () => cardsPlacedUnderLists(),
  sameCardPlace: (a, b) => sameCardPlace(a, b),
  liveFollowersMain: () => liveFollowers().main,
  transcriptPlacePostCard: () => voiceTranscript().placePostCard(),
  transcriptCardShown: () => voiceTranscript().postWatch.shown !== null,
  reopenedViewsIncludes: (view) => REOPENED_VIEWS.includes(view),
  channelOnScreen: () => channelOnScreen(),
});
const { sideColumnUp, threadListOnScreen, sideThreads, sideShowing, COLUMNS_TOO_NARROW, inThisColumn, columnSteps, eachColumn, inOtherColumn, followInOtherColumn, threadListOffered, columnsOffered, renderColumnToggle, sideHasBox, toggleHasBox, renderColumns, moveLayout, leftColumnPlace, keepLeftColumnPlace, sameLeftPlace, setLayout, noteListBefore, oneColumnBeforeList, columnsFromKeyboard, toggleColumns, chooseView, chooseOneColumn, showOneColumnView, recordViewChoice, leftColumnToMain, enterColumns, leaveColumns, showOneColumnLeft, enterThreadList, renderSideMode, renderSideColumn, sideThreadChoice } = twoColumns;

/**
 * The separator between the two columns, dragged to change their widths: web/src/view/channel's
 * now (`createColumnSplitter`, `#1186 two-column-splitter`). Kept on the device with the other
 * client settings; applied here, before anything is drawn, as the stored settings are.
 *
 * @type {import("./src/view/channel/index.ts").ColumnSplitter}
 */
const columnSplitter = createColumnSplitter({
  lookup: (id) => leftEl(id),
  keep: (pct) => deviceSettings().keep(DEVICE_SETTINGS.twoColumnSplit, pct),
  clampSplit: (value) => clampTwoColumnSplit(value),
  defaultSplit: DEFAULT_TWO_COLUMN_SPLIT,
  columnsOn: () => layoutShown.columns(),
  mainColumn: () => columns.main,
  sideColumn: () => columns.side,
  inColumn: (column, act) => inColumn(/** @type {any} */ (column), act),
  readerPlace: () => readerPlace(),
  keepReaderPlace: (place) => keepReaderPlace(/** @type {any} */ (place)),
  scheduleColumnLinks: () => scheduleColumnLinks(),
});
columnSplitter.apply(deviceSettings().get(DEVICE_SETTINGS.twoColumnSplit));

/** The right column's cards are web/src/view/channel's now (`createSideCards`, `#624`). */
const sideCards = createSideCards({
  leftEl: (id) => leftEl(id),
  cardView: (choice) => cardView(/** @type {any} */ (choice)),
  messageText: (message) => messageText(message),
  stillToDo: (root, dismissed) => stillToDo(root, dismissed),
  channelCanon: () => channelStore.current(),
  sideColumn: () => columns.side,
  inColumn: (column, act) => inColumn(/** @type {any} */ (column), act),
  unseenReplies: (id, root, tally, summary) => unseenReplies(id, /** @type {any} */ (root), /** @type {any} */ (tally), /** @type {any} */ (summary)),
  repliesPartlyHeld: (id, root, held, tally) => repliesPartlyHeld(id, /** @type {any} */ (root), held, /** @type {any} */ (tally)),
  exactReplyCount: (id, root, tally, summary) => exactReplyCount(id, /** @type {any} */ (root), /** @type {any} */ (tally), /** @type {any} */ (summary)),
  pendingInto: (id) => pendingInto(id),
  threadCount: (count, exact) => threadCount(count, /** @type {any} */ (exact)),
  unpackHeldMessages: (canon, choice, same) => unpackHeldMessages(/** @type {any} */ (canon), /** @type {any} */ (choice), same),
  unpackSignature: (choice, held) => unpackSignature(/** @type {any} */ (choice), held),
  unpackReadSignature: (held, dismissed, filter) => unpackReadSignature(held, dismissed, /** @type {any} */ (filter)),
  unpackFilter: () => unpackFilter,
  foldedOneIds: (key) => foldedOneIds(key),
  viewKey: (view, id) => viewKey(view, id),
  threadChoices: () => threadChoices(),
  inThreadOrder: (choices, order) => inThreadOrder(/** @type {any} */ (choices), order),
  cardOrderOnScreen: () => cardOrderOnScreen(),
  watchPendingSummaries: (choices, listId) => watchPendingSummaries(/** @type {any} */ (choices), listId),
  cardIsSummarized: (choice) => cardIsSummarized(/** @type {any} */ (choice)),
  briefAge: (at) => briefAge(at),
  threadName: (choice, limit) => threadName(/** @type {any} */ (choice), limit),
  unpackWatch: () => unpackWatch,
  dressChannelRow: (row, pass) => dressChannelRow(row, /** @type {any} */ (pass)),
  answeredBy: (rows, viewing) => answeredBy(rows, viewing),
  replyTally: () => replyTally(),
  readMode: () => readModes.mode(),
  readRunsSuspended: () => readRunsSuspended(),
  cardPackingMode: () => cardPacking.mode(),
  combineOn: () => combineSwitch.on(),
  nowPlaying: () => readAloudOwner().nowPlaying,
  pendingRead: () => readAloudOwner().pendingRead,
  markReadingCard: (card) => readAloudOwner().markReadingCard(card),
  mainRowsOnScreen: () => mainRowsOnScreen(),
  mainRowMessages: (except) => mainRowMessages(except),
  messageLook: (message) => messageLook(message),
  readLook: (message, dismissed, filter) => readLook(message, dismissed, /** @type {any} */ (filter)),
  olderThreadsToRead: () => olderThreadsToRead(),
  keepingCardPlace: (act) => keepingCardPlace(act),
  sideCard: (card) => sideCard(/** @type {any} */ (card)),
  countSideCard: (card, input) => countSideCard(card, /** @type {any} */ (input)),
  paintLitCard: (card, id) => paintLitCard(card, id),
  unpackBlock: (choice, tally) => unpackBlock(/** @type {any} */ (choice), /** @type {any} */ (tally)),
  redrawUnpackBlock: (choice, rows, tally) => redrawUnpackBlock(/** @type {any} */ (choice), rows, /** @type {any} */ (tally)),
  doneArchived: () => doneState().archived,
  setDoneArchived: (archived) => { doneState().archived = archived; },
  caughtUpDone: (archived, shown) => caughtUpDone(archived, /** @type {any} */ (shown)),
  mainRunsAmong: (main, times) => mainRunsAmong(main, /** @type {any} */ (times)),
  mainRows: (run, tally) => mainRows(/** @type {any} */ (run), /** @type {any} */ (tally)),
  foldCardRuns: (list) => foldCardRuns(list),
  filterSideCards: () => filterSideCards(),
  cardsCycle: () => cardsCycle(),
  sideCardsShown: () => sideCardsShown(),
  outboxValues: () => outbox.values(),
  eachColumn: (act) => eachColumn(act),
  renderOutgoingMessages: () => renderOutgoingMessages(),
  replyState: () => replyState,
  placeInlineReply: () => placeInlineReply(),
  reopenPendingReply: () => reopenPendingReply(),
  setTextIfChanged: (el, text) => setTextIfChanged(el, text),
  threadPagesLoading: () => threadList.threadPagesLoading(),
  selectedChannelId: () => selectedChannelId(),
  cardState: (choice, read) => cardState(/** @type {any} */ (choice), read),
});
const { cardsDrawn, nameSideRegion, sideCardText, cardLines, sideCardRootText, threadIsRead, sideCardUnread, sideCardReplies, cardMessageCount, sideCardParts, placeChildren, redressMainRows, renderSideCards } = sideCards;

/** A filter changed: the reader's message stays where it is (`createFilterPlace`, `#624`). */
const filterPlaceOwner = createFilterPlace({
  el: (id) => el(id),
  activeFilter: () => columnWork.active().filter,
  activeColumn: () => columnWork.active(),
  columnSide: () => columnWork.side(),
  mainColumn: () => columns.main,
  sideColumn: () => columns.side,
  inColumn: (column, act) => inColumn(/** @type {any} */ (column), act),
  eachColumn: (act) => eachColumn(act),
  screen: () => screens.screen(),
  view: () => screens.view(),
  renderOlderControl: () => renderOlderControl(),
  dismissStatus: () => dismissStatus(),
  filterPlace: () => filterPlace(),
  pinsView: () => pinsView(),
  visibleList: () => visibleList(),
  listEmptied: () => listEmptied(),
  atBottom: (area) => atBottom(area),
  quickReplyHolds: (area) => quickReplyHolds(area),
  endSearchWalks: () => endSearchWalks(),
  scrollToNewest: () => scrollToNewest(),
  renderJumpNewest: () => renderJumpNewest(),
  guardQuietly: (fn) => guardQuietly(fn),
  setStatus: (text) => setStatus(text),
  sidePane: () => /** @type {any} */ (sidePane),
  channelPane: () => /** @type {any} */ (channelPane),
  markOf: (pane) => markOf(/** @type {any} */ (pane)),
  markedRow: (pane, mark) => markedRow(/** @type {any} */ (pane), /** @type {any} */ (mark)),
  rowInView: (pane, row) => rowInView(/** @type {any} */ (pane), row),
  takeRow: (pane, row, focus) => takeRow(/** @type {any} */ (pane), row, focus),
  rowOf: (pane, row) => rowOf(/** @type {any} */ (pane), row),
  paneRows: (pane) => paneRows(/** @type {any} */ (pane)),
  releaseRow: (pane, row) => releaseRow(/** @type {any} */ (pane), row),
  rowTimeTabStop: (row, on) => rowTimeTabStop(row, on),
  drawSelection: () => drawSelection(),
  hidingRead: () => hidingRead(),
  threading: () => providerFacts.threading(),
  timelineRows: () => timelineRows(),
  olderPages: () => olderPages(),
  loadOlder: () => loadOlder(),
  walkCursorOwed: () => walkCursorOwed(),
  channelContextKey: () => channelContextKey(),
  scrollAnchor: (area) => /** @type {any} */ (scrollAnchor(area)),
  listPlaceTop: (area) => listPlaceTop(area),
  floatingClearance: (area) => floatingClearance(area),
  messageTop: (row, id) => messageTop(row, id),
  rowMessages: (row) => rowMessages(/** @type {any} */ (row)),
  clearEntryLanding: () => { entryState().landing = null; entryState().threadOwed = null; entryState().threadAwaitsRead = false; },
  sideShowing: () => sideShowing(),
});
const { filterChanges, filterKey, filterState, endFilterWalk, anchorAt, anchorHolder, anchorRow, nearestShownRow, currentFilterHold, heldFilterRow, readerActed, followsNewest, keepFilterHold, filterAnchor, keepingFilterPlace, placeFilterAnchor, settlePendingFilterPlace } = filterPlaceOwner;

/** Channel views, threads and the channel composer are web/src/view/channel's now
 * (`createChannelViews`, `#624 voice-page-modularity`). */
const channelViews = createChannelViews({
  el: (id) => el(id),
  leftEl: (id) => leftEl(id),
  columnSide: () => columnWork.side(),
  channelCanon: () => /** @type {any} */ (channelStore.current()),
  view: () => screens.view(),
  threading: () => providerFacts.threading(),
  entryState: () => entryState(),
  selection: () => selection(),
  readMode: () => readModes.mode(),
  transcriptShown: () => voiceTranscript().postWatch.shown,
  selectedChannelId: () => selectedChannelId(),
  heldMessage: (id) => heldMessage(id),
  providerOfChannel: (channel) => providerOfChannel(channel),
  providersSize: () => channelDirectory.providers().size,
  repliesInThreadCapability: () => channelDirectory.capabilities().repliesInThread,
  threadOf: (message) => threadOf(message),
  rowMessages: (row) => rowMessages(row),
  renderSideColumn: () => renderSideColumn(),
  renderThreadSelect: () => renderThreadSelect(),
  renderThreadHeading: () => renderThreadHeading(),
  followReplyStore: () => followReplyStore(),
  markListView: () => markListView(),
  paintAllThreadFocus: () => paintAllThreadFocus(),
  pinsView: () => pinsView(),
  renderOutgoingMessages: () => renderOutgoingMessages(),
  syncCanonDismissed: () => syncCanonDismissed(),
  timelineRows: () => timelineRows(),
  olderPages: () => olderPages(),
  captureScroll: () => captureScroll(),
  loadState: () => loadState(),
  doneState: () => doneState(),
  idsOf: (row) => idsOf(row),
  shownRowFor: (row) => shownRowFor(row),
  atBottom: (area) => atBottom(area),
  noteDeepLinkReaderMove: () => noteDeepLinkReaderMove(),
  putDockedReplyAway: () => putDockedReplyAway(),
  replyDocked: () => replyState.docked,
  replyColumn: () => replyState.column,
  activeColumn: () => columnWork.active(),
  putRepliesBack: () => putRepliesBack(),
  dropReplyArrowTap: () => dropReplyArrowTap(),
  rememberChannelDraft: () => rememberChannelDraft(),
  setPinnedOnly: (only, kept) => setPinnedOnly(only, kept),
  viewOnScreen: () => /** @type {any} */ (viewOnScreen()),
  saveUiState: () => saveUiState(),
  scheduleBackSync: () => scheduleBackSync(),
  voiceCall: () => voiceCall(),
  threadOutrunsStore: (threadId, summary) => threadOutrunsStore(threadId, /** @type {any} */ (summary)),
  heldViewRows: () => heldViewRows(),
  drawProjection: () => drawProjection(),
  drawHeldRows: (rows, freshAt, freshness) => drawHeldRows(/** @type {any} */ (rows), freshAt, freshness),
  setChannelFreshness: (state) => setChannelFreshness(/** @type {any} */ (state)),
  saveChannelScope: () => saveChannelScope(),
  walkCursorOwed: () => walkCursorOwed(),
  maybeLoadOlder: () => maybeLoadOlder(),
  allReachesThread: (canon, threadId, summary) => allReachesThread(/** @type {any} */ (canon), threadId, /** @type {any} */ (summary)),
  storeLacksReplies: (threadId, summary) => storeLacksReplies(threadId, /** @type {any} */ (summary)),
  projectView: () => projectView(),
  channelReadPosition: (area) => channelReadPosition(area),
  readFilterKeeps: () => readFilterKeeps(),
  settleAfterRead: (messages, options) => settleAfterRead(/** @type {any} */ (messages), /** @type {any} */ (options)),
  hiddenEmpty: (message) => hiddenEmpty(message),
  hidingRead: () => hidingRead(),
  renderChannelLoading: (busy, quiet) => renderChannelLoading(busy, quiet),
  restoreChannelComposer: () => restoreChannelComposer(),
  renderOlderControl: () => renderOlderControl(),
  restoreScroll: (position) => restoreScroll(/** @type {any} */ (position)),
  renderCachedTimeline: () => renderCachedTimeline(),
  syncUnreadReplies: () => syncUnreadReplies(),
  renderChannelFreshness: () => renderChannelFreshness(),
  payThreadEntry: () => payThreadEntry(),
  rowsMissDone: () => rowsMissDone(),
  scrollToNewest: () => scrollToNewest(),
  renderJumpNewest: () => renderJumpNewest(),
  threadCards: (threads) => threadCards(threads),
  loadDiscordTimeline: (options) => timelineView.loadDiscord(/** @type {any} */ (options)),
  api: (path, options) => api(path, /** @type {any} */ (options)),
  deviceStorage: () => deviceStorageOnUse(),
  tokenScope: () => serverFacts.tokenScope(),
  channelRecord: (id) => channelDirectory.channels().find((candidate) => String(candidate.id) === String(id)),
  timeOfRecord: (record) => timeOf(/** @type {any} */ (record), "timestamp"),
  threadOfMessage: (message) => threadOf(/** @type {any} */ (message)),
  layoutShown: () => layoutShown,
  stopReading: () => readAloudOwner().stopReading(),
  setReadingMode: (on) => { readAloudOwner().readingMode = on; },
  readRunsForget: (key) => readRuns.forget(key),
  viewKey: (view, threadId) => viewKey(view, threadId),
  rememberView: (view) => uiState.remember(/** @type {any} */ (view)),
  forgetChosenView: (test) => uiState.forgetChosen(test),
  settleUiState: () => uiState.settle(),
  payReadCursor: () => channelReads.payCursor(),
  readsBusy: () => channelReads.busy(),
  readContext: () => channelReads.readContext(),
  channelContexts: () => channelContexts,
  listState: () => listState(),
});
const { threadHues, channelContextKey, heldThreadRecord, channelRepliesInThread, threadReplyCount, threadReplies, threadForMessageId, withThreadQuery, restoreThreadHues, noteServerThreadHuesIn, seedServerThreadHues, threadHue, threadCount, renderChannelNavigation, renderColumnLists, rememberChannelContext, readerPlace, channelCard, keepReaderPlace, readBehindSwitch, changeChannelView, inheritAllCover, catchUpHeldView, catchUpFromAll } = channelViews;


const columnRepliesOwner = createColumnReplies({
  el: (id) => el(id),
  outbox,
  retryOf: (id) => outboxDispatch.retryOf(id),
  retryPending: (id) => outboxDispatch.retryPending(id),
  inPlaceDrawn: (column) => outgoingInPlaceDrawn.get(column),
  layoutColumns: () => layoutShown.columns(),
  isSide: (column) => column === columns.side,
  isMain: (column) => column === columns.main,
  sideShowing: () => sideShowing(),
  inColumn: (column, act) => inColumn(column, act),
  columnKey: (column) => column.name,
  activeColumn: () => columnWork.active(),
  setStatus: (text) => setStatus(text),
  renderJumpNewest: () => renderJumpNewest(),
});
const { columnReplies, unseenReplies: unseenColumnReplies, reportColumnReply, unseenColumnReply } = columnRepliesOwner;

/** The arrow in a reply's gutter, the jump to what it answers and the way back are
 * web/src/view/channel's now (`createReplyArrows`, `#624 voice-page-modularity`). */
const replyArrowsOwner = createReplyArrows({
  el: (id) => el(id),
  leftEl: (id) => leftEl(id),
  inThisColumn,
  guardQuietly: (fn) => guardQuietly(fn),
  gatherFromReply: (row) => gatherFromReply(row),
  filterState: () => filterState(),
  endFilterWalk: () => endFilterWalk(),
  endSearchWalks: () => endSearchWalks(),
  selectLanded: (row) => selectLanded(row),
  setSearchOpen: (open) => setSearchOpen(open),
  openReadRun: (row) => openReadRun(/** @type {any} */ (row)),
  renderScrollTools: () => renderScrollTools(),
  setStatus: (text) => setStatus(text),
  channelContextKey: () => channelContextKey(),
  heldMessage: (id) => heldMessage(id),
  inView: (message, view, threadId) => inView(/** @type {any} */ (message), view, threadId),
  selection: () => selection(),
  threading: () => providerFacts.threading(),
  olderPages: () => olderPages(),
  viewKey: () => viewKey(),
  channelCanon: () => channelStore.current(),
  selectedChannelId: () => selectedChannelId(),
  changeChannelView: (view) => changeChannelView(view),
  channelReadsBusy: () => channelReads.busy(),
  loadOlder: () => loadOlder(),
  walkCursorOwed: () => walkCursorOwed(),
  hidingRead: () => hidingRead(),
  timelineMessages: () => timelineRows().messages,
  hiddenEmpty: (message) => hiddenEmpty(/** @type {any} */ (message)),
  readHiding: () => readModes.hiding(),
  layoutColumns: () => layoutShown.columns(),
  columnSteps: () => columnSteps(),
  activeArrows: () => columnWork.active().arrows,
  replyJumpTicket: () => createStreamGeneration(),
});
const { replyJumpTicket, replyArrows, dropReplyArrowTap, replyArrow, rowHolding, listPlaceTop, floatingClearance, drawnTop, rowOnScreen, revealRow, putOutLanding, replyReturnRow, backToReply, landOnAnswered, answeredElsewhere, historyReadToStart, jumpToAnswered, seekMessage, measureReplyArrows, holdingReplyArrows, rootFontPx } = replyArrowsOwner;

/** Gathering a message's replies under it, the bridge and the X are web/src/view/channel's now
 * (`createReplyGathering`, `#624 voice-page-modularity`). */
const gatheringOwner = createReplyGathering({
  el: (id) => el(id),
  inThisColumn,
  threading: () => providerFacts.threading(),
  selection: () => selection(),
  channelContextKey: () => channelContextKey(),
  knownThreadChoice: (id, fallback) => knownThreadChoice(id, fallback),
  selectedChannelId: () => selectedChannelId(),
  channelCanon: () => channelStore.current(),
  loadCanon: (channel) => loadCanon(channel),
  heldMessage: (id) => heldMessage(id),
  heldThreadRecord: (message, held) => heldThreadRecord(message, held),
  rowMessages: (row) => rowMessages(/** @type {any} */ (row)),
  glom: (messages) => glom(/** @type {any} */ (messages)),
  discordNode: (group) => discordNode(/** @type {any} */ (group)),
  unpackMessages: (thread, copies, flag, filter) => unpackMessages(/** @type {any} */ (thread), /** @type {any} */ (copies), flag, /** @type {any} */ (filter)),
  unpackHeldMessages: (canon, thread, sameChannel) => unpackHeldMessages(/** @type {any} */ (canon), /** @type {any} */ (thread), sameChannel),
  unpackNeedsRead: (thread, copies) => unpackNeedsRead(/** @type {any} */ (thread), /** @type {any} */ (copies)),
  queueUnpackRead: (id) => queueUnpackRead(id),
  unpackFilter: () => unpackFilter,
  layoutColumns: () => layoutShown.columns(),
  sideColumn: () => columnWork.side(),
  openThread: (id) => openThread(id),
  searchOpen: () => searchField.open(),
  setSearchOpen: (open) => setSearchOpen(open),
  seekMessage: (target, arrive, openAll) => seekMessage(target, arrive, openAll),
  rowHolding: (id) => rowHolding(id),
  setStatus: (text) => setStatus(text),
  renderChannelRows: () => renderChannelRows(),
  renderScrollTools: () => renderScrollTools(),
  countOf: (count) => countOf(count),
  hidingRead: () => hidingRead(),
  threadReplies: (thread, message) => threadReplies(thread, message),
  timelineRows: () => timelineRows(),
  renderJumpNewest: () => renderJumpNewest(),
  renderReplyCounts: () => renderReplyCounts(),
  measureReplyArrows: () => measureReplyArrows(),
  cardRow: (thread) => cardRow(thread),
  finishPackSlide: () => finishPackSlide(),
  glideBehavior: () => glideBehavior(),
  threadHue: (id) => threadHue(String(id)),
  openThreadRootId: () => openThreadRootId(),
  openThreadChoice: () => openThreadChoice(),
  rootInward: (choice) => rootInward(/** @type {any} */ (choice)),
  isOpenThreadRoot: (row, id) => isOpenThreadRoot(row, id),
  replyArrows: () => replyArrows(),
  takeClaimedClick: () => rowGestures.takeClaimedClick(),
  drawnTop: (row) => drawnTop(row),
  floatingClearance: (area) => floatingClearance(area),
  activeGather: () => columnWork.active().gather,
  activeList: () => columnWork.active().list,
});
const { gatherState, listState, gatheredParent, gatheredThread, heldMessagesById, replyLinks, repliesTo, conversationHead, gatheredRows, listedMessages, arrangeChannelRows, gatherFromReply, toggleGathered, gatherReplies, gatherNow, gatherReport, keepOnScreen, scatterReplies, putRepliesBack, renderReplyLinks, bridgeTop, glideToBridgeTop, headOfList, bridgePiece, ungatherButton } = gatheringOwner;

/** Done's own act and its put back: web/src/view/channel's now (`createDoneAct`, `#624`). */
const doneAct = createDoneAct({
  api: (path, options) => api(path, /** @type {any} */ (options)),
  clockOnUse: () => clockOnUse(), el: (id) => el(id),
  channelStore: () => channelStore, providerFacts,
  columnWork: () => columnWork, columns: () => columns, readModes: () => readModes, readLedger: () => readLedger,
  lastDismissal: () => lastDismissal, inboxLedger: () => inboxLedger,
  dismissStatus: () => dismissStatus(), setStatus: (text) => setStatus(text), statusSays: (text) => statusSays(text),
  doneHeldHere: (ids) => doneHeldHere(ids), doneSettlesHere: () => doneSettlesHere(),
  followDoneEverywhere: (ids, column) => followDoneEverywhere(/** @type {any} */ (ids), /** @type {any} */ (column)),
  guardQuietly: (fn) => guardQuietly(fn), heldDonePart: (held, ids) => heldDonePart(/** @type {any} */ (held), /** @type {any} */ (ids)),
  hidingRead: () => hidingRead(), holdingMessage: (act) => holdingMessage(act), holdingReplyArrows: (act) => holdingReplyArrows(act),
  idsOf: (row) => idsOf(/** @type {any} */ (row)), inColumn: (column, act) => inColumn(/** @type {any} */ (column), act), inOtherColumn: (act) => inOtherColumn(act),
  listState: () => listState(), listedMessages: () => listedMessages(), loadTodo: (options) => loadTodo(options),
  markHeldDone: (ids, done, settles) => markHeldDone(/** @type {any} */ (ids), done, /** @type {any} */ (settles)), messageText: (m) => messageText(/** @type {any} */ (m)),
  nextUndoAt: () => nextUndoAt(), pinsView: () => pinsView(), readSpeaksFor: (payload, id) => readSpeaksFor(payload, id),
  refreshAfterLiveMutation: () => refreshAfterLiveMutation(), renderCachedTimeline: () => renderCachedTimeline(),
  renderChannelRows: () => renderChannelRows(), renderChannelSeam: (summary) => renderChannelSeam(summary),
  renderReadModeControl: () => renderReadModeControl(), renderReplyLinks: () => renderReplyLinks(),
  renderScrollTools: () => renderScrollTools(), renderSideColumn: () => renderSideColumn(),
  retryingWrite: (fn, opts) => retryingWrite(fn, /** @type {any} */ (opts)), rowMessages: (row) => rowMessages(/** @type {any} */ (row)),
  saveChannelScope: () => saveChannelScope(), selectedChannelId: () => selectedChannelId(), selection: () => selection(),
  sendBookmark: (staged, opts) => sendBookmark(staged, opts), settleHeldDone: (held, done) => settleHeldDone(/** @type {any} */ (held), done),
  singleDoneBump: (messages) => singleDoneBump(/** @type {any} */ (messages)), stageBookmark: (channel, to) => stageBookmark(/** @type {any} */ (channel), /** @type {any} */ (to)),
  syncUnreadReplies: () => syncUnreadReplies(), threadForMessageId: (id) => threadForMessageId(/** @type {any} */ (id)),
  timeOf: (m, field) => timeOf(m, field), todoSummary: () => todoSummary(),
  token: () => token(), tokenFingerprint: (t) => tokenFingerprint(t),
  undoLabel: (d) => undoLabel(d), undoName: (text) => undoName(text),
  voiceTranscript: () => voiceTranscript(), writeInBatches: (route, ids, landed) => writeInBatches(route, ids, landed),
});
const { doneState, noteArchived, dismissalName, drawUndoLabel, renderTodoControls, backlogIsArmed, disarmBacklog, armBacklog, todoView, toggleArchived, markRowDone, forgetDoneFlash, drawDoneFlash, drawDoneFlashes, unflashIds, unflashDone, flashDone, refreshAfterInboxChange, renderDoneChange, putBackRowsOff, queueInboxWrite, inboxWriteOwner, inboxPost, sendLeftOverInboxWrites, noteInboxWrite, settleInboxWrite, noteDoneSaving, doneSaved, dismissMessages, dealtWithWords, markReadUpstream, restoreMessages, mirrorArchived, doneSaving } = doneAct;

/** @typedef {import("./src/core/channels/index.ts").ReplyTally} ReplyTally */
/** Reply counts and the way down to them: web/src/view/channel's now (`createReplyCounts`, `#624`). */
const replyCounts = createReplyCounts({
  answeredElsewhere: (target) => answeredElsewhere(target), cardSplit: (choice) => cardSplit(/** @type {any} */ (choice)),
  el: (id) => el(id), focusMinimally: (box) => focusMinimally(/** @type {any} */ (box)),
  gatherCount: (id, message, held) => gatherCount(id, message, held), guardQuietly: (fn) => guardQuietly(fn),
  hiddenEmpty: (m) => hiddenEmpty(m), hidingRead: () => hidingRead(), idsOf: (row) => idsOf(/** @type {any} */ (row)),
  inColumn: (column, act) => inColumn(/** @type {any} */ (column), act), inThisColumn: (fn) => inThisColumn(fn),
  inView: (m, view, threadId) => inView(m, view, /** @type {any} */ (threadId)), knownChannel: (id) => knownChannel(/** @type {any} */ (id)),
  listState: () => listState(), openReadRun: (row) => openReadRun(/** @type {any} */ (row)), openThread: (id) => openThread(id),
  openThreadChoice: () => openThreadChoice(), openThreadRootId: () => openThreadRootId(),
  outgoingReplyKey: () => outgoingReplyKey(), paintThreadStrip: (row, id) => paintThreadStrip(/** @type {any} */ (row), id),
  pendingInto: (id) => pendingInto(id), quoteLabelFits: (button, plain, full) => quoteLabelFits(/** @type {any} */ (button), plain, full),
  renderScrollTools: () => renderScrollTools(), replyArrows: () => replyArrows(), revealRow: (row) => revealRow(/** @type {any} */ (row)),
  rootInward: (choice) => rootInward(/** @type {any} */ (choice)), rowHolding: (id) => rowHolding(id),
  rowMessages: (row) => rowMessages(/** @type {any} */ (row)), rowShown: (row) => rowShown(/** @type {any} */ (row)),
  seekMessage: (target, land, elsewhere) => seekMessage(target, land, elsewhere),
  selectedChannelId: () => selectedChannelId(), selection: () => selection(),
  sentFromThisPage: (m) => sentFromThisPage(m), setSearchOpen: (open) => setSearchOpen(open),
  setStatus: (text) => setStatus(text), shownNode: (node) => shownNode(/** @type {any} */ (node)),
  stillToDo: (m, done) => stillToDo(m, done), threadCount: (count, exact) => threadCount(count, exact),
  threadHue: (id) => threadHue(id), threadReplyCount: (id, m) => threadReplyCount(id, /** @type {any} */ (m)),
  timeOf: (m, field) => timeOf(m, field), viewKey: (view, threadId) => viewKey(view, /** @type {any} */ (threadId)),
  channelStore: () => channelStore, providerFacts, broadcastSupported: () => providerFacts.broadcastSupported(),
  columns: () => columns, layoutShown: () => layoutShown, outbox: () => outbox, outboxDispatch: () => outboxDispatch,
  quoteOwner: () => quoteOwner,
});
const { REPLY_BUTTON_TITLE, unansweredRoot, paintThreadBadge, startThread, unansweredThread, heldUnansweredRoot, replyTally, rootThreadOf, unseenReplies, threadOutrunsStore, storeLacksReplies, repliesPartlyHeld, rootReplyState, holdsUnreadReplies, readFilterKeeps, unpackFilter, drawThreadChips, heldReplies, labelReplyButton, drawReplyLabel, renderReplyCounts, nextReplyTarget, labelReplyMarker, replyMarker, jumpToReply, landOnReply } = replyCounts;

/** A reply that is sent marks what it answers Done, and the chat service's marker after it:
 * web/src/view/channel's now (`createSentRead`, `#624 voice-page-modularity`). */
const sentRead = createSentRead({
  el: (id) => el(id),
  replyColumn: () => replyState.column,
  mainColumn: () => columns.main,
  sideColumn: () => columns.side,
  sideShowing: () => sideShowing(),
  inColumn: (column, act) => inColumn(/** @type {any} */ (column), act),
  rowHolding: (id) => rowHolding(id),
  threading: () => providerFacts.threading(),
  channelCanon: () => channelStore.current(),
  selectedChannelId: () => selectedChannelId(),
  readHiding: () => readModes.hiding(),
  doneArchived: () => doneState().archived,
  inboxLedger: () => inboxLedger,
  inboxWriteOwner: () => inboxWriteOwner(),
  queueInboxWrite: (act) => queueInboxWrite(act),
  inboxPost: (path, body) => inboxPost(path, /** @type {any} */ (body)),
  toggleArchived: (ids, row) => toggleArchived(ids, row),
  bookmarkAnchor: (channel) => bookmarkAnchor(channel),
  setStatus: (text) => setStatus(text),
  doneSavingHas: (id) => doneSaving.has(id),
  doneSaved: (ids, still) => doneSaved(/** @type {any} */ (ids), still),
  providerCount: () => channelDirectory.providers().size,
  providerOfChannel: (channel) => providerOfChannel(channel),
  upstreamReadMark: () => channelDirectory.capabilities().upstreamReadMark,
  heldUnansweredRoot: (thread) => heldUnansweredRoot(thread),
  outboxValues: () => outbox.values(),
  chatServiceOf: (channel) => chatServiceOf(channel),
  api: (path, options) => api(path, /** @type {any} */ (options)),
  readableFailure: (error, service) => readableFailure(error, service),
  serviceReadWords: (mark) => serviceReadWords(mark),
});
const { answeredRow, doneWhereNotDrawn, answeredDone, doneHeldHere, markAnsweredDone, channelMarksUpstream, rootThisStarts, channelIsOneThread, serviceMarkNotAsked, markReadInService, markSentRead } = sentRead;

/**
 * Entering a thread, on its first unread message (`createThreadEntry`, `#624
 * voice-page-modularity`): the landing owed on entry and kept across its read.
 *
 * @type {import("./src/view/channel/index.ts").ThreadEntry}
 */
const threadEntry = createThreadEntry({
  el: (id) => el(id),
  activeEntry: () => columnWork.active().entry,
  channelContextKey: () => channelContextKey(),
  selection: () => selection(),
  channelViewsHas: (key) => channelStore.current().views.has(key),
  viewKey: () => viewKey(),
  applyReadRuns: () => applyReadRuns(),
  atBottom: (area) => atBottom(area),
  renderScrollTools: () => renderScrollTools(),
  requestVisibleSummaries: () => requestVisibleSummaries(),
  floatingClearance: (area) => floatingClearance(area),
  drawnTop: (row) => drawnTop(row),
  openThreadRootId: () => openThreadRootId(),
  isOpenThreadRoot: (row, id) => isOpenThreadRoot(row, id),
  rowIsRead: (row) => rowIsRead(row),
  toFoot: (area) => toFoot(area),
  setJumpNewest: (wanted) => setJumpNewest(wanted),
  screen: () => screens.screen(),
  view: () => screens.view(),
  pinsView: () => pinsView(),
});
const { entryState, payThreadEntry, entryLandingMark, keepEntryLanding, landOnFirstUnread, placeEntryLanding, entryLandingHolds } = threadEntry;




// `#193 compose-draft-loss`. A send dismissed from the outbox, kept for Undo and then for a while
// longer. See "taking a send back"; how many are kept, for how long and where are web/src/core/outbox's
// (`DISMISSED_SENDS_LIMIT`, `DISMISSED_SENDS_DAYS`, `#624 voice-page-modularity`).
// How long the Undo chip offers to take a Dismiss back is web/src/core/outbox's (`SEND_UNDO_MS`).
// How much of a message a control quotes to name it is web/src/core/outbox's (`sendSnippet`).
// How much of a message's first line names its row in Recently dismissed to a screen reader is
// web/src/present/settings's (`DISMISSED_LABEL_CHARS`, `#624 voice-page-modularity`).
const CHANNEL_DRAFT_REFUSED = "This browser could not save the draft. Keep this page open until you send it.";
/**
 * `#443 undo-key-general`. The order undoable acts were done in, for the one undo key to take the
 * newest back first. Every offer an Undo chip makes is stamped from this as it is made — a Done
 * (`lastDismissal`), a send dismissed from the outbox (`sendUndo`), a bookmark taken away
 * (`bookmarkUndos`) — and a stamp is never reused, so two offers up at once always have an order.
 */
const undoStamps = createUndoStamps();

/** The next undo stamp: later acts stamp higher (web/src/view/shell `createUndoStamps`). */
function nextUndoAt() {
  return undoStamps.next();
}

function showView(name) {
  // `#277 reply-in-column`. The call's view is one list, with no column for a reply to be open in.
  if (name !== "discord") putDockedReplyAway();
  // Written down BEFORE the panes swap, because after that the scroll position belongs to
  // whichever pane is now up and says nothing about where the reader was.
  const leaving = screens.view();
  if (leaving && leaving !== name) {
    const area = el("scroll-area");
    viewScroll.leave(leaving, { top: area.scrollTop, atNewest: atBottom(area) });
    // ...and, leaving the channel, the message the reader was on, for a reopen. `#189 restore-ui-state`.
    if (leaving === "discord") holdChannelPlace();
  }
  screens.setView(name);
  el("pane-voice").hidden = name !== "voice";
  el("pane-discord").hidden = name !== "discord";
  el("pane-docs").hidden = name !== "docs";
  el("docs-fetch").hidden = name !== "docs";
  // `#269 approval-card-inline`. A waiting draft is the next message of the list now on screen, put
  // there BEFORE the place in that list is restored, which counts it.
  voiceTranscript().placePostCard();
  const discord = name === "discord";
  // `#845 voice-toggle-colours`. Pressed, lit in the accent, is the call's view; the channel's is the darker one.
  el("view-switch").setAttribute("aria-pressed", name === "voice" ? "true" : "false");
  el("view-switch-label").textContent = discord ? "Channel" : name === "docs" ? "Documents" : "Voice";
  el("view-switch").title = discord
    ? "Showing the channel. Press to show the documents."
    : name === "docs"
      ? "Showing the documents. Press to show the call."
      : "Showing the call. Press to show the channel.";
  documentsSelector.sync();
  // Both panes share ONE scroll container, so a switch swaps the content out from under a
  // scrollTop that belonged to the other pane. On a FIRST entry, landing on the newest message is
  // right and landing at the top is badly wrong: arriving at the top of a long channel means
  // scrolling past everything already read to reach the thing you opened it for.
  //
  // On every entry AFTER that, it is the reader's place, and throwing it away is the same defect
  // one level up. A glance at the transcript and back should not cost forty messages of scrolling.
  //
  // ...unless the place they left was THE NEWEST LINE, which is an instruction rather than an
  // offset: turns can land in the transcript while the reader is over on the channel, and putting
  // them back at the number the bottom used to be is the same "your place moved" defect wearing the
  // fix's clothes. See `viewScroll`.
  const top = viewScroll.enter(name);
  const returning = top !== null;
  if (returning) {
    el("scroll-area").scrollTop = top;
  } else {
    scrollToNewest();
  }
  // Only when the reader really was taken to the newest message. Clearing it on a RETURN would
  // hide a chip that is pointing at something they have not seen.
  if (!returning) {
    jumpNewestWanted[name] = false;
  }
  // Told to the caller (`viewScroll.restored()`, kept by `enter` above), because restoring the position
  // here is only half of it: entering the channel also RE-READS it, and a read that does not know it is a
  // return settles to the newest message and undoes this immediately. See the view-switch handler.
  // The chips belong to the list you are looking at, and you are now looking at a different one.
  renderScrollTools();
  renderChannelFreshness();
  // A channel word ("fetching the channel…") is not the Documents reader's news.
  if (name === "docs") dismissStatus();
  // Leaving the channel takes the reading with it: audio that goes on playing over the transcript
  // is audio the reader cannot see the source of, on a list where the next thing it does is
  // archive a message.
  // `#1039 documents-read-parity`: kind-aware. Entering Documents stops a message read (and
  // clears its mode) but keeps a document read — that view is its source; entering the channel
  // or the call stops a document read, whose strip and source are the reader's.
  const readOwner = readAloudOwner();
  const nowReading = readOwner.nowPlaying;
  const documentReading = Boolean(
    (nowReading && nowReading.kind === "document") ||
    (!nowReading && typeof readOwner.pendingRead === "string" && readOwner.pendingRead.startsWith("doc:")));
  if (!discord) {
    readOwner.readingMode = false;
    if (name !== "docs" || !documentReading) readOwner.stopReading();
  } else if (documentReading) {
    readOwner.stopReading();
  }
  // Read replaces Talk in this view, so the control pane has to be re-decided on every switch.
  voiceCall().renderControls();
  // ...and so does the channel picker, which is a member of the bar now rather than a row at the
  // top of the scrollback. LAST, and after `currentView` is set, for the same reason `showScreen`
  // ends this way: the bar decides what it shows from the view that is now up. `#83
  // channel-selector-in-bar`.
  renderControlBar();
  renderChannelNavigation();
  // `#229 desktop-two-column`. One column or two is the channel's layout.
  renderColumns();
  // `#245 done-feedback`. The chips stand on the channel's message box, which only the channel has.
  placeChipsOverComposer();
  // The call's controls (Sound, Clear, Talk) are not the Documents view's.
  if (screens.screen() === "main") el("control-pane").hidden = name === "docs";
  saveUiState();
  // `#224 keyboard-shortcuts`. The list on screen takes the focus, so that the scroll keys move it.
  // Above all off the view switch: left holding the focus from the click that pressed it, it took
  // the next Space as another press and switched straight back. Only on the main screen: a view
  // changed behind Settings must not take the focus from the control the reader is on there.
  if (leaving !== name && screens.screen() === "main") settleFocus();
  // `#304 back-gesture-in-app`. The call's view waits in front of whatever the channel had open.
  scheduleBackSync();
}

// --- connection details -------------------------------------------------------------------------
//
// Shown once in a banner that takes itself away, and permanently reachable from settings. Kept in
// a variable rather than read back out of the DOM, so appending to it cannot pick up a redaction
// marker and re-redact it.

// The details, the banner and its dismissal are the shell notices' (`showDetail`, `addDetail`, `dismissBanner`,
// `BANNER_DISMISS_MS`, above).

/** The permanent connection readouts; their live values stay with the channel's owner. */
const connectionReadouts = mountConnectionReadouts({
  lookup: (id) => el(id),
  live: {
    get delivery() { return providerFacts.liveDelivery(); },
    get pollSeconds() { return providerFacts.livePollSeconds(); },
    attached: () => liveFollow.attached(),
  },
});

// --- transcript -----------------------------------------------------------------------------

/**
 * Go to the newest line — deliberately, because somebody asked.
 *
 * This used to run on EVERY arrival, which is the defect `#47 scrollback-stability` is about: the
 * reader scrolls up to find what the assistant said two minutes ago, a turn lands, and the page
 * throws them back to the bottom. Every remaining caller is a deliberate act — entering a view,
 * loading a channel for the first time, tapping "Newest" — or an arrival that happened while the
 * reader was already parked at the bottom, which is `followIfPinned` below.
 */
function scrollToNewest() {
  const area = el("scroll-area");
  // A message a filter change held in place is let go: the reader is going somewhere else. `#243`.
  filterState().hold = null;
  // `#338 column-overscroll`: a reader in the room past the right column's foot is on that line already.
  followFoot(area);
  setJumpNewest(false);
}

/**
 * Was the reader already parked on the newest message?
 *
 * A browser clamps scrollTop to scrollHeight - clientHeight, so "at the bottom" is never
 * scrollTop === scrollHeight however hard `scrollToNewest` pushes. The slack absorbs that, plus
 * sub-pixel layout and a thumb that stopped a few pixels short — at which point the reader still
 * means "I am at the bottom", and the page should still follow.
 *
 * `#338 column-overscroll`. Measured from the foot of the rows, so a reader anywhere in the room open
 * past the right column's foot is parked on the newest message too (`footGap`).
 */
const BOTTOM_SLACK_PX = 24;

function atBottom(area, slack = BOTTOM_SLACK_PX) {
  return footGap(area) <= slack;
}

// --- room past the foot of the right column ---------------------------------------------------
//
// `#338 column-overscroll`. The owner, 2026-10-15: on a desktop monitor the newest thread in the right
// column sits at the very bottom of the screen, which is where the reader's attention is but the least
// comfortable place to read. So the foot stays where a list opens and is followed from, and the wheel
// and the page keys go on PAST it, into blank room, until the last card's top is at half the column's
// height: the card being read can be brought to the middle of the screen.
//
// THE ROOM IS OPENED BY THE READER GOING ON DOWN FROM THE FOOT, and only then. #side-overscroll, the
// last thing in the right column's scroller, has no height at all until a reader resting at the foot
// turns the wheel down, presses PgDn or Space, or ↓ past the last message (`openOverscroll`); it is then
// given the height that lets the last card's top come up to the middle, in the same task as that event
// and before the browser scrolls for it, so that very scroll goes into it. So every list opens, lands
// and is followed exactly as before, its scrollbar at the bottom at the foot (`#280 scroll-stability`:
// "the thread view also feels like it is not starting out scrolled all the way to the bottom"), and
// nothing the page does to place a reader — a landing, a jump to a row, the glide — can put them past
// the foot. The foot holds the scroll that got there: a turn of the wheel opens the room only after
// the wheel has rested (`OVERSCROLL_REST_MS`), and a held key never does, so a fling to the newest
// stops on it, and the next turn goes on.
//
// WHILE IT IS OPEN IT IS THE FOOT. `footGap` measures from the foot of the rows, so `atBottom` holds
// anywhere in the room: the jump stays down, and the reader is a reader on the newest line to every
// rule that follows one (`#280 scroll-stability`). Followed, they are not moved while the foot is still
// on screen: a reply arriving opens in the room under the card they are reading, and only one taller
// than they are past the foot takes them on to it (`followFoot`). The room is fitted again to the last
// card whenever the rows change (`fitOverscroll`), so it always ends where that card's top reaches
// the middle, and a reader at its end stays at it. The last card changing size under a reader in the
// room — highlighted, put out, drawn shorter — is fitted to before the frame is drawn, and the card left
// where the reader had it (`holdOverscrollReader`): shrinking, it took the end of the list up with it,
// and the browser's clamp to the new end dropped the card by what it lost.
//
// IT IS CLOSED when the reader goes back up past the foot (it is below the screen then, so nothing on
// screen moves), when they come to rest at the foot without going into it, when the column shows
// something else — another thread, the cards, another layout, channel, view or screen, another search or
// filter over its rows — and by the reader's own way to the newest line, End and the Newest chip, which
// land on the foot as a list opens.
//
// MEASURING CAN CLOSE IT. `overscrollRoom`, and so `footGap`, `footTop` and `atBottom`, close a room left
// open over something the column no longer shows before they measure by it, so every measure agrees with
// what is on screen: a caller reading them in the middle of a redraw may see the room go.
//
// AT A DESK ONLY: web/voice.css gives #side-overscroll a box in the desktop regime alone, and the script
// asks whether it has one (`overscrollOffered`), as it asks whether the right column has (`sideHasBox`).

/**
 * The rooms past the foot of the right column — the reader's (`#338 column-overscroll`) and the one held
 * for a card (`#459 pack-keeps-card-still`) — with every measure of the foot, fitting, following, and the
 * wheel's and scroll's part in them: one owner in web/src/view/channel/footroom.ts (`#624
 * voice-page-modularity`). What the column shows, whether it is up, and its last card are here.
 */
// The keyboard's room in a list, for the foot's measures (`#1144 scrollback-typing-jump`): the
// keyboard owner is made at the boot below, long after the rooms, so its answer is held here once
// made and read as no room until then.
var screenKeyboardRoom = null;
const footRooms = createFootRooms({
  clock: clockOnUse(),
  side: () => leftEl("side-scroll"),
  overscrollBox: () => leftEl("side-overscroll"),
  overextendBox: () => leftEl("side-overextend"),
  context: () => overscrollContext(),
  offered: () => overscrollOffered(),
  sideUp: () => sideColumnUp(),
  lastCard: () => lastColumnCard(),
  paddingBottom: (area) => (typeof window.getComputedStyle === "function"
    ? parseFloat(window.getComputedStyle(area).paddingBottom) || 0 : 0),
  pageStep: (area) => pageStep(area),
  keyboardRoom: (area) => (screenKeyboardRoom ? screenKeyboardRoom.roomIn(area) : 0),
});
const {
  overscrollRoom, overextendRoom, footGap, footTop, closeOverscroll, waitOverscrollIdle, openOverscroll, fitOverscroll,
  noteOverscrollReader, holdOverscrollReader, followFoot, toFoot, closeOverextend, sizeOverextend, fitOverextend,
} = footRooms;

/**
 * What the right column shows, for the room opened over it: the room closes on anything else. The search
 * and the filters over its rows too (`searchKey`): a search typed in the room hides cards under the reader,
 * and lands them on the foot of what it leaves rather than in blank room fitted to another card.
 */
function overscrollContext() {
  return JSON.stringify([layoutShown.columns(), layoutShown.threadList(), screens.screen(), screens.view(), columns.side.mode,
    readModes.mode(), inColumn(columns.side, () => [channelContextKey(), pinsView().only, searchKey()])]);
}

/** Whether the room may be open: the right column on screen at a desk, where web/voice.css gives it a box. */
function overscrollOffered() {
  const room = leftEl("side-overscroll");
  return screens.screen() === "main" && screens.view() === "discord" && sideColumnUp() &&
    typeof room.getClientRects === "function" && room.getClientRects().length > 0;
}

/**
 * The last card of the right column that is drawn: its last thread card over the cards, else the last row
 * of its thread, a reply still in the outbox under it included. Null where none is drawn.
 */
function lastColumnCard() {
  const lists = columns.side.mode === "thread"
    ? inColumn(columns.side, () => [el("outgoing-log"), shownChannelList()])
    : [leftEl("side-cards")];
  for (const list of lists) {
    const rows = [...list.children];
    for (let at = rows.length - 1; at >= 0; at -= 1) {
      // `#419 threads-view-main-messages`: over the cards, the last card is a card, not a row among them.
      if (rows[at].hasAttribute("data-unpack-thread") || rows[at].hasAttribute("data-main-row")) continue;
      if (rows[at].getBoundingClientRect().height > 0) return rows[at];
    }
  }
  return null;
}

// --- room past the foot held for a card packed where it stands -------------------------------------
//
// `#459 pack-keeps-card-still`. The owner, 2026-10-23: packing a thread card a few threads from the foot of
// the Threads view made it jump to the bottom of the screen, because the list would not scroll past its
// end to leave it where it was. "In the specific case of packing threads I would really like the thread
// card I act on by clicking to STAY STILL, even if that means the collapsed child messages leave a big
// VOID on the screen and show us 'past the end' by a larger margin. I'm OK with the scroll wheel stopping
// where it does now ... if I scroll up I cannot scroll back down to the overextended state."
//
// So the card acted on is put back where it was on the screen (`keepingCardStill`), and where that is
// further down the list than it now scrolls, #side-overextend, the last thing in the scroller, is given
// the height that makes it reachable: blank room past the foot, under the void the packed messages left.
//
// ONE WAY. The room is only ever as tall as the reader stands past the foot of the rows, fitted again at
// every scroll, every change of the rows' size and every redraw (`fitOverextend`): scrolling up takes it
// away under the reader, so they cannot scroll back down into it; rows arriving at the foot fill it, and
// the reader is not moved while it lasts (`followFoot`); and another pack makes it as tall as that pack
// needs, never the sum of the two. The reader's own scrolling never opens it, and the room the wheel
// opens past the foot (`#338 column-overscroll`) is closed as it opens and not opened over it. Like that
// room it is the foot while it is open (`footGap`), and it closes as the reader goes back up past the
// foot, when the column shows something else, and by the reader's own way to the newest line (`toFoot`).

// The room held for a card, its measure, closing and fitting (`overextendRoom`, `closeOverextend`,
// `sizeOverextend`, `fitOverextend`) are the foot rooms' owner's, above (`#624 voice-page-modularity`).

/**
 * Run `change` to the cards with thread `id`'s card held exactly where it is on the screen, past the foot
 * of the list where that is what it takes. A card wholly above the screen — ^ pressed deep in its
 * unpacked block — is held at the top of it instead, so what is left is never a screen of blank room. Whether the
 * card was held: not where it was not drawn before, or is not after.
 *
 * @param {string} id
 * @param {() => void} change
 */
function keepingCardStill(id, change, slide = false) {
  const area = leftEl("side-scroll");
  // A slide still running is ended first: everything is measured where it is laid out.
  finishPackSlide();
  const row = cardRow(id);
  const box = row && rowShown(row) ? row.getBoundingClientRect() : null;
  const stage = area.getBoundingClientRect().top;
  const before = box ? (box.bottom <= stage ? stage : box.top) : null;
  const next = slide && row ? itemAfterBlock(row) : null;
  const was = next ? { key: slideKey(next), top: next.getBoundingClientRect().top } : null;
  change();
  const now = cardRow(id);
  if (before === null || !now || !rowShown(now)) return false;
  const top = area.scrollTop + now.getBoundingClientRect().top - before;
  // The wheel's room past the foot is fitted to the last card and closed above the foot on its own
  // schedule, either of which would move the card; this room stands in for it.
  closeOverscroll();
  sizeOverextend(area, top);
  area.scrollTop = top;
  if (was) slideIntoPlace(now, was);
  return true;
}

// --- the gap a packed thread leaves, held and then closed (#460 pack-gap-slide) -------------------------
//
// The owner, 2026-10-23, after `#459 pack-keeps-card-still`: "when I pack a thread it could leave a gap in
// the message stream for a moment, and the cards below slide up into position rather than jumping
// instantaneously." Unpack is the mirror: the thread opens in place and the cards below slide down.
//
// A FLIP over the settled layout, as the drill-in's flight is (`flyThreadCard`) and as
// ai_docs/transient/2026-10-22-thread-list-animation.md proposes for the list (its step 1, the gap that
// holds and then closes). The cards are redrawn, the packed card held still and the reader's place kept
// exactly as without any motion (`keepingCardStill`); then everything below the thread on the screen is
// drawn, by a transform alone, where it was, and let go: after `--pack-gap-hold` it slides home over
// `--pack-slide`, both on #screen-main in web/voice.css, and 0 for a reader whose system asks for less
// motion, when nothing moves at all. Unpacked, the thread's rows fade in as the cards slide down from under
// them, with no hold; the card and its rows are drawn over what slides (`slideOver`), so it never covers them. Nothing is laid out again and nothing waits: every measure after the slide is the
// one there would have been without it. Only what is on the screen moves, and by no more than its height,
// so a long thread packed costs one screenful of motion. Not for Pack all, whose threads each leave a gap
// of their own; not while a reply is open under a row, whose box is anchored to it; and ended at once by
// the reader's own wheel or key on the list, any redraw of the cards, or another pack.

/** A duration the stylesheet says on #screen-main, in ms: 0 where it says none (`cardFlightMs`). */
function screenMotionMs(name) {
  const style = typeof window.getComputedStyle === "function" ? window.getComputedStyle(leftEl("screen-main")) : null;
  const said = style ? String(style.getPropertyValue(name)).trim() : "";
  const parsed = /^(\d+(?:\.\d+)?)(ms|s)$/.exec(said);
  return parsed ? Number(parsed[1]) * (parsed[2] === "s" ? 1000 : 1) : 0;
}

/**
 * The slides running and the card drawn over them, ending them, what an item of the cards is across a
 * redraw, the items after a card's thread, and the slide itself: one owner in
 * web/src/view/channel/packslide.ts (`#624 voice-page-modularity`). Its durations are the stylesheet's.
 */
const packSlider = createPackSlides({
  cards: () => leftEl("side-cards"),
  list: () => leftEl("side-list"),
  area: () => leftEl("side-scroll"),
  motionMs: (name) => screenMotionMs(name),
  replyDocked: () => replyState.docked,
  hidden: () => document.hidden,
});
const { finishPackSlide, slideKey, itemAfterBlock, slideIntoPlace } = packSlider;

/**
 * Leaving the reader looking at the same thing across a change: the list on screen, the message their eye
 * is on, where they are captured as and put back by, and following a new turn only where they were at the
 * newest line (web/src/view/channel `createScrollKeeping`, `#624 voice-page-modularity`), in the column
 * being worked on. The page destructures them under their old names.
 */
const scrollKeeping = createScrollKeeping({
  el: (id) => el(id),
  onMain: () => screens.screen() === "main",
  currentView: () => screens.view(),
  channelView: () => selection().view,
  pinnedOnly: () => pinsView().only,
  heldFilterRow: () => heldFilterRow(),
  searchWalk: () => columnWork.active().searchWalk,
  atBottom: (area) => atBottom(area),
  scrollToNewest: () => scrollToNewest(),
  renderJumpNewest: () => renderJumpNewest(),
  setJumpNewest: (on, view, turns) => setJumpNewest(on, view, turns),
});
const {
  visibleList, shownChannelList, scrollAnchor, preservingScroll, holdingReader, captureScroll, restoreScroll, followIfPinned,
} = scrollKeeping;

/** The invitation only makes sense while there is nothing else in the transcript. */
function renderEmptyState() {
  el("empty-state").hidden = el("transcript").children.length > 0;
}

// --- which day a message was sent ----------------------------------------------------------------
//
// `#260 timestamp-hover`. A row's time is the hour and minute; hovering it says the day as well.
// How a day, a time and an age are worded is web/src/present/time's (`dayLabel`, THE ONE DAY
// FORMATTER, and its section note); what is left here draws them.

/** Say a time element's full day on the status line: a tap on a phone, Enter on the keys. */
function sayTimeDay(node) {
  const at = new Date(String(node.getAttribute("datetime") || ""));
  if (Number.isNaN(at.getTime())) return;
  setStatus(fullLocalAt(at));
}

/**
 * A tap (or click) on a message's time says its full day on the status line: the one press on
 * a phone that says when a message was from, without the details sheet a press-and-hold on
 * the row opens. The press itself is the row's as it always was — at zero collapsed lines the
 * header is all of a row there is to press, and a press there must still open it.
 * `#479 message-time-full-date`.
 *
 * @param {Event} event
 */
function onTimeTap(event) {
  const node = /** @type {Element | null} */ (event ? event.target : null);
  if (!node || typeof node.getAttribute !== "function" || String(node.tagName).toLowerCase() !== "time") return;
  if (!DAY_TITLED.has(String(node.className))) return;
  freshenDayTitle(event);
  sayTimeDay(/** @type {Element} */ (node));
}

/**
 * Enter or Space on a focused time says its day, as a tap does; every other key is the row's.
 * `#479 message-time-full-date`.
 *
 * @param {KeyboardEvent} event
 */
function onTimeKey(event) {
  if (!event || (event.key !== "Enter" && event.key !== " ")) return;
  const node = /** @type {Element | null} */ (event.target);
  if (!node || typeof node.getAttribute !== "function" || String(node.tagName).toLowerCase() !== "time") return;
  if (!DAY_TITLED.has(String(node.className))) return;
  event.stopPropagation();
  event.preventDefault();
  freshenDayTitle(event);
  sayTimeDay(/** @type {Element} */ (node));
}

// --- long messages, folded -----------------------------------------------------------------------
//
// `#47 scrollback-stability`. Both lists on this page carry long messages — an assistant answers in
// paragraphs, and so does a coding agent posting into a channel — and ten of those in a row is a
// list nobody can skim. `foldable` below is called from `line()` AND from `discordNode()`, so this
// is not two similar behaviours that will drift; it is one behaviour, and the transcript and the
// channel cannot disagree about it.
//
// The open questions on the issue, answered here rather than left to the reader of a diff:
//
//   * COLLAPSED SIZE IS MEASURED IN LINES — the reader's count (`#425 collapsed-lines`, three
//     until they say otherwise), by `-webkit-line-clamp` in web/voice.css. What the reader is
//     being protected from is a wall of text, and a wall is a number of lines. The character
//     count below decides only WHETHER to fold, and does not move with the count: at a high
//     count a barely-long message on a wide column may already show whole behind its More, as
//     one could at three lines on the widest column before the count could move.
//   * A LONG MESSAGE ARRIVES ALREADY FOLDED. Folding it after the reader has seen it is a jump in
//     the list for no reason they asked for, which is the same class of defect as the scroll one.
//   * A SHORT MESSAGE GETS NO CONTROL AT ALL. A fold button that would reveal nothing, on every
//     line of the list, is chrome charging rent.
//   * THE STATE IS NOT PERSISTED across a reload. It was once true that there was nothing to
//     persist it against; `#48 transcript-storage` and `#128 transcript-history` have since made
//     the transcript durable and browsable, so the honest statement is that a reopened record
//     arrives folded, deliberately: opening a message is an act on the list in front of you, not
//     a preference, and a restored page that came back open would be a wall of text every time.
//   * COLLAPSE ALL IS ONE ACTION, not a preference. It puts the list back the way it arrived.
//
// 280 characters is about four lines at phone width — comfortably past the three the fold shows by
// default, so a folded message ordinarily has something behind it.
// Folds, summaries and the chips over the list are web/src/view/channel's now (`createFolding`,
// `#624 voice-page-modularity`): which messages are folded and opened, summaries asked for as you
// scroll, the jump to the newest message, the scroll tools' pass, and ; / : stepping. Why each
// behaves as it does is told there.
const collapsedLines = createCollapsedLines({
  deviceSettings: () => deviceSettings(),
  render: (n) => messagePreferences.collapsedLines.render(n),
});

/**
 * `#837 voice-page-jump-to-reading`. The chip back to the message being read aloud (web/src/view/voice
 * `createReadingJump`), over the main screen only.
 */
const readingJump = createReadingJump({
  chip: () => leftEl("jump-reading"),
  rows: () => {
    if (screens.screen() !== "main" || !pageReadAloud) return [];
    const rows = pageReadAloud.readingRowsShown();
    const tools = leftEl("scroll-tools");
    return rows.length > 0 && typeof window.getComputedStyle === "function" &&
      window.getComputedStyle(tools).visibility === "hidden" ? [] : rows;
  },
});

const folding = createFolding({
  clock: clockOnUse(),
  collapsedLines,
  byId: el,
  leftById: leftEl,
  inThisColumn,
  inOtherColumn,
  eachColumn,
  readerActed,
  preservingScroll,
  threadHue: (id) => Number(threadHue(String(id))),
  threadOf,
  rowMessages,
  currentScope: () => channelStore.current().scope,
  renderSummariesSetting: (on) => messagePreferences.summaries.render(on),
  keepSummarize: (on) => {
    try {
      deviceSettings().keep(DEVICE_SETTINGS.summarizeCollapsed, on);
    } catch (_error) {
      // A browser that refuses storage still honours the choice for this session.
    }
  },
  storedSummarize: () => deviceSettings().get(DEVICE_SETTINGS.summarizeCollapsed),
  summarizeHeldOn: () => {
    const held = settingsSync().agreed(SUMMARIZE_COLLAPSED_KEY);
    return Boolean(held) && held.value === "1";
  },
  api,
  withThreadQuery,
  selectedChannelId,
  threadForMessageId,
  setStatus,
  view: () => screens.view(),
  screen: () => screens.screen(),
  searchLinks: () => searchField.open() && searchField.links(),
  activePlace: () => columnWork.active().place,
  activeTools: () => columnWork.active().tools,
  activeName: () => columnWork.active().name,
  side: () => columnWork.side(),
  sideMode: () => columns.side.mode,
  sidePane: () => /** @type {any} */ (sidePane),
  channelPane: () => /** @type {any} */ (channelPane),
  fitOverscroll,
  footGap,
  bottomSlackPx: BOTTOM_SLACK_PX,
  unseenReplies: unseenColumnReplies,
  unseenReply: unseenColumnReply,
  listEmptied: () => listEmptied(),
  draftBelowView: () => voiceTranscript().draftBelowView(),
  renderJumpBookmark: () => renderJumpBookmark(),
  refreshReadingJump: () => readingJump.refresh(),
  visibleList,
  shownChannelList,
  applySearch: () => applySearch(),
  placeBookmarkLine: () => placeBookmarkLine(),
  keepSelection: (...args) => keepSelection(...args),
  replyReturnRow,
  actionMenuOpen: () => actionMenu.open(),
  renderActionMenu: () => actionMenu.render(),
  renderSendUndo: () => renderSendUndo(),
  renderBookmarkUndo: () => renderBookmarkUndo(),
  channelOnScreen: (...args) => channelOnScreen(...args),
  rowIsRead: (row) => rowIsRead(row),
  markedRow: (pane, mark) => markedRow(/** @type {any} */ (pane), /** @type {any} */ (mark)),
  markOf: (pane) => markOf(/** @type {any} */ (pane)),
  rowInView: (pane, row) => rowInView(/** @type {any} */ (pane), row),
  paneRows: (pane) => paneRows(/** @type {any} */ (pane)),
  anchorAt,
  isTextField,
  nodeWithin,
  entryState,
  placeFilterAnchor,
  filterHold: () => filterState().hold,
  setFilterHold: (hold) => { filterState().hold = /** @type {any} */ (hold); },
  listPlaceTop,
  modHeld: (event) => modHeld(event),
});
const {
  summaryRows, jumpNewestWanted, inScrollback,
  foldsOf, setFolded, isFolded, foldable, applySummarizeCollapsed, storedSummarizeCollapsed,
  summariesSettingLanded, renderSummaries, requestVisibleSummaries, setSummaryMode,
  setJumpNewest, renderJumpNewest,
  holdingScrollTools, renderScrollTools, setAllFolded, foldStepFromKeyboard,
} = folding;

/** The reading column's line count is web/src/view/channel's now (`createReadingColumn`,
 * `#624 voice-page-modularity`). */
const readingColumn = createReadingColumn({
  foldsOf: (list) => foldsOf(list),
  setFolded: (entry, folded) => setFolded(entry, folded),
  isFolded: (entry) => isFolded(entry),
  collapsedLines,
  savedCollapsedLines: (count) => messagePreferences.collapsedLines.saved(count),
  stepCollapsedLinesPreference: (direction) => messagePreferences.collapsedLines.step(direction),
  screen: () => screens.screen(),
  mainColumn: () => columns.main,
  sideColumn: () => columns.side,
  inColumn: (column, act) => inColumn(column, act),
  captureScroll: () => captureScroll(),
  restoreScroll: (mark) => restoreScroll(mark),
  sideShowing: () => sideShowing(),
  keepingCardPlace: (act) => keepingCardPlace(act),
  defaultCollapsedLines: DEFAULT_COLLAPSED_LINES,
  documentById: (id) => document.getElementById(id),
});
const { refreshElisionLines, collapsedLinesChanged, stepCollapsedLines } = readingColumn;

// Searching what is on screen, and the lines between rows, are web/src/view/channel's now
// (`createSearchRows`, `#624 voice-page-modularity`): the language on the page, the filter's
// pass, the hits drawn, and the seams. The field itself stays instantiated here, where the
// shell mounts wire its marks.
const searchField = createSearchField({
  clock: clockOnUse(),
  // The reader stays on what they were reading — the message the keystroke's own change held, while it
  // holds (`captureScroll`) — because the line a folded match grows under it (`searchExcerpt`) is height.
  redraw: (apply) => {
    const mark = () => {
      apply();
      eachColumn(renderScrollTools);
      if (sideColumnUp()) filterSideCards();
    };
    if (screens.screen() === "main") preservingScroll(mark);
    else mark();
  },
});

const searchRows = createSearchRows({
  searchField,
  language: {
    parsedQueries: createParsedQueries(),
    openedGaps: createOpenedGaps(),
    searchSubject,
    searchQueryMatches,
    searchQueryTree,
  },
  byId: el,
  leftById: leftEl,
  view: () => screens.view(),
  screen: () => screens.screen(),
  side: () => columnWork.side(),
  sideShowing: () => sideShowing(),
  sideColumnUp: () => sideColumnUp(),
  inColumn,
  inThisColumn,
  mainColumn: () => columns.main,
  mainColumnRecord: () => columns.main,
  sideColumnRecord: () => columns.side,
  setSearched: (count) => { columnWork.active().searched = count; },
  searchWalkHeld: () => Boolean(columnWork.active().searchWalk),
  preservingScroll,
  applyReadRuns: () => applyReadRuns(),
  threading: () => providerFacts.threading(),
  heldMessages: () => channelStore.current().messages,
  currentChannel: () => channelStore.current().channel,
  selectedChannelId,
  threadOf,
  messageText: (message) => messageText(message),
  messageLinks: (message) => messageLinks(message),
  selectionView: () => selection().view,
  authorChoosing: () => authorChoosing(),
  authorPasses,
  authorEmptySentence: (a, b, c) => authorEmptySentence(a, b, c),
  gatheredParent,
  putRepliesBack,
  pinsOnly: () => pinsView().only,
  showRowLinks: (row) => showRowLinks(row),
  takeDownRowLinks: (row) => takeDownRowLinks(row),
  heldLinks: (row) => heldLinks(row),
  hiddenKindNames: (counts) => hiddenKindNames(counts),
  renderLinkKindCounts: (kinds) => renderLinkKindCounts(kinds),
  drawCardGaps: (list) => drawCardGaps(list),
  openedCardGaps: (create) => openedCardGaps(create),
  openUnpackGap: (row, below) => /** @type {any} */ (openUnpackGap(/** @type {any} */ (row), below)),
  openCardGap: (row, below) => /** @type {any} */ (openCardGap(/** @type {any} */ (row), below)),
  renderReplyLinks,
  renderOlderControl: () => renderOlderControl(),
  scheduleServerSearch: () => scheduleServerSearch(),
  placeInlineReply: () => placeInlineReply(),
  placeRowsInPlaceAgain: () => placeRowsInPlaceAgain(),
  replyDocked: () => replyState.docked,
  changeChannelView,
  channelViewWords: () => channelViewWords(),
  historyReach: () => historyReach(),
  historyReadToStart,
  messagesWords,
  searchMomentWords,
  reachBackWords,
  reachClause,
  oldestLoadedMs: () => oldestLoadedMs(),
  holdsWholeHistory: () => serverCacheBox.holdsWholeHistory(),
  hidingRead: () => readModes.hiding(),
  threadListOnScreen: () => threadListOnScreen(),
  cardCount: () => cardsDrawn.count(),
  pinProblem: () => pinStore.problem(),
  pinRevision: () => pinStore.revision(),
  pinChannel: () => pinStore.channel(),
  currentChannelId: () => currentChannelId(),
  isFolded,
  readRunsSuspended: () => readRunsSuspended(),
  searchKey: () => searchKey(),
  viewKey: () => viewKey(),
  runDaysOf: (row) => runDaysOf(row),
  placeInReadRun: (row, role, count) => placeInReadRun(row, role, count),
  readRunCount: (row) => readRunCount(row),
  changing: () => filterChanges.changing(),
  currentFilterHold,
  keepFilterHold,
  visibleList,
});
const {
  markSearchSoon, markSearchNow, searchParsed, searchTyped, searchIdentity, searchable,
  applySearch, searchInAll, renderSearchInAll, renderSearchCount, refreshSearchCount,
  listEmptied, drawSeams, seamsAfterMidnight, openSearchGap, inOpenedGap,
  rowsChosen, rowDay, markSearchHits, messageMatches, threadsWithHits, rowMatches,
  sayCardCount, openedGaps, placeSeam, seamDrawn,
} = searchRows;

// --- a filter changed: the reader's message stays where it is on the screen ----------------------
//
// `#243 filter-keeps-selection`. The owner, 2026-10-12: "In long transcripts we must be very careful
// about storing/losing our place in the scroll history and be very intentional. One of my most hated
// features of other chat clients is that it will lose my place. When it is a very long history this
// is like losing your place in a book or a long podcast. Here's a particular thing I just wanted to
// do -- jump to my last pinned message: go to search, filter for pinned, select the message. But then
// as soon as I clear the filter, the screen jumps and I don't have the message I just found. The rule
// here is that when you adjust the filter, the highlighted message MUST stay on the screen. Ideally it
// stays on the screen in the same Y location without a "jump.""
//
// Each filter used to place the reader its own way: Pinned and Links put them at the newest of what
// was left and, turned off, back at the scroll offset they had before — an offset in a list that the
// pins they had just found were not even part of. Now ONE RULE, applied by ONE HELPER,
// `keepingFilterPlace`, for every change of what the list shows: the search text typed or cleared,
// the search closed (Escape or the glass), Pinned and Links on or off, the kinds of link under Links,
// and the read mode. A view switch is not a filter, and keeps its own landing rules.
//
//   * BEFORE the change, the anchor: the selected message (`#239 single-selection`; drawn, or chosen
//     silently by a tap) while it shows; else the message the last change kept in place, while the
//     reader has not scrolled since (`filterState().hold`), which makes turning a filter on and off again a
//     round trip; else today's place-keeping — the newest line for a reader on it, or the first
//     message showing under the floating line. Its message ids, its time and how far its top is below
//     the top of the list's box are noted.
//   * AFTER, its row in the list as now drawn goes back to that height, as near as the list's ends
//     allow: the line of the collapsed run it has joined, or — when the new filter hides it — the
//     nearest message shown, in channel order, which the status line says when it was the selection.
//     A message older than everything loaded, as an old pin is, is walked back to, a bounded number of
//     pages, with the step-back line saying "Loading earlier messages…", and placed when it comes. The
//     selection goes with it, its ring and its focus as they were.
//   * AND THEN NOTHING ELSE MOVES IT until the reader scrolls or acts in the list (`readerActed`):
//     not following the newest line on an arrival or when the chips change height, not a read
//     landing behind the change, and not a thread's entry landing (`#221 read-modes`), which the
//     change spends. The reader's own act — a press in the list, another message chosen, a key on
//     the messages, a fold — ends the hold, and is measured from where the reader is.

// A filter changed: the reader's message stays where it is on the screen, by web/src/view/channel's
// filter-kept place (`createFilterPlace`, `#624 voice-page-modularity`).

function setSearchOpen(open) {
  if (!open) {
    keepingFilterPlace(() => showSearchOpen(false));
    return;
  }
  showSearchOpen(true);
}

/** `setSearchOpen`, without holding the reader's place. */
function showSearchOpen(open) {
  // `#262 search-load-older`. Closed over a filter choosing rows, Hide read comes back (`hidingRead`).
  const wasHidingRead = hidingRead();
  searchField.setOpen(open);
  if (!open) {
    searchField.setQuery("");
    // `#249 search-highlight`. And its marks, at once.
    searchField.setMarked("");
    markSearchSoon();
    el("search-field").value = "";
    // `#206 pin-message`. The Pinned filter lives in this bar, and goes with it for the reason the
    // query does: a filter left on behind a control that is no longer on screen is the state this
    // feature must never be in. `#211 link-filter`: so does Links. Placed once, for the whole of
    // closing, by `keepingFilterPlace`.
    setLinksOnly(false);
    setPinnedOnly(false);
    // `#251 author-filter`. And whose messages.
    setAuthorFilter("everyone");
  }
  el("search-toggle").setAttribute("aria-pressed", open ? "true" : "false");
  // `#197 floating-search`. The open bar floats over the top of the list, and web/voice.css keys
  // two things off this: the bar's own shape, and the room the list makes for it at its head so
  // the first match is never underneath. That room is added ABOVE a reader who may have scrolled
  // down, which is exactly the change `holdingReader` exists for.
  holdingReader(() => {
    if (open) el("screen-main").setAttribute("data-searching", "");
    else el("screen-main").removeAttribute("data-searching");
  });
  renderControlBar();
  // The pill shares the line the bar grows along, and gives way to it.
  renderChannelFreshness();
  // Both columns: there is one search bar. `#229 desktop-two-column`.
  if (hidingRead() !== wasHidingRead) eachColumn(renderCachedTimeline);
  eachColumn(renderScrollTools);
  if (sideColumnUp()) refilterSideCards();
  if (open) {
    el("search-field").focus();
  }
}

// The links in what is on screen, whose messages, and the kinds of link are
// web/src/view/channel's now (`createRowLinks`, `#624 voice-page-modularity`): a row's links
// shown in place of its text, the author filter, and the kind buttons. The kinds kept on this
// device stay present/rows' owner, instantiated here and handed in; `authorChoosing` and the
// kinds' `hidden` keep their names for their other consumers.
const linkKinds = createLinkKinds({
  storage: () => deviceStorage(),
  rawStored: () => deviceSettings().get(DEVICE_SETTINGS.linksRaw),
  keepRaw: (on) => deviceSettings().keep(DEVICE_SETTINGS.linksRaw, on),
});

const LINK_RAW_KEY = DEVICE_SETTINGS.linksRaw.key;

const rowLinksOwner = createRowLinks({
  linkKinds,
  byId: el,
  messageLinks: (message) => messageLinks(message),
  renderedHtmlOf,
  rawTextOf,
  searchOpen: () => searchField.open(),
  linksOn: () => searchField.links(),
  setLinks: (on) => searchField.setLinks(on),
  view: () => screens.view(),
  currentChannelId: () => currentChannelId(),
  selectionView: () => selection().view,
  selectionThreadId: () => selection().threadId || "",
  keepingFilterPlace,
  liftingHideRead: (change) => liftingHideRead(change),
  renderScrollTools,
  inOtherColumn,
  requestVisibleSummaries,
  hidingRead: () => hidingRead(),
  ownReadOn: () => ownReadSwitch.on(),
});
const {
  authorFilters, showRowLinks, takeDownRowLinks, heldLinks, showingLinks, filterPlace,
  setLinksOnly, authorChoosing, setAuthorFilter, cycleAuthorFilter, authorEmptySentence,
  setLinkKindShown, setLinksRaw, renderLinkKinds, renderLinkKindCounts, hiddenKindNames,
} = rowLinksOwner;

/**
 * Bring the whole of `box` into the view of `area`, the list it is in, as `keepComposerInView` says:
 * the box whole, or its top where the list is shorter than it. `#340 composer-grow-box` made it any
 * composer's, for a box growing under the reader's typing.
 *
 * @param {HTMLElement} box
 * @param {HTMLElement} area
 */
function keepBoxInView(box, area) {
  const rect = box.getBoundingClientRect();
  const top = rect.top - area.getBoundingClientRect().top + area.scrollTop;
  const bottom = top + rect.height;
  const view = area.clientHeight || 0;
  if (top >= area.scrollTop && bottom <= area.scrollTop + view) return;
  area.scrollTop = rect.height <= view ? bottom - view : top;
}

/**
 * `#429 keyboard-hides-bar`. The boxes in the lists a reader writes in, each field with the box it is in:
 * the channel's (in Main's list, or among the cards since `#419 threads-view-main-messages`), the right
 * column's, and a reply open in the stream (`#410 inline-reply-editor`), in whichever list holds it. The
 * call's is `writingToAgent`'s.
 */
const LIST_BOXES = [
  ["channel-compose-text", "channel-composer"],
  ["side-compose-text", "side-composer"],
  ["reply-text", "column-reply"],
  ["reply-text", "side-reply"],
];

/**
 * The box in `area` the reader is writing in, or null. `#429 keyboard-hides-bar`: the on-screen keyboard
 * takes its height from the list, and a list that shrinks keeps its scroll position, so the channel's box
 * at its foot, or a reply's under its message, went below the window, Send with it, as the call's had
 * before `#241 voice-composer` kept it in view. The keepers of the two lists bring this box into view
 * whole as their list shrinks, if it was in view before (`boxWasInView`): a reader who scrolled up to read
 * the message being answered is left there.
 *
 * @param {HTMLElement} area
 * @returns {HTMLElement | null}
 */
function boxBeingWritten(area) {
  if (screens.screen() !== "main") return null;
  const held = document.activeElement;
  for (const [field, box] of LIST_BOXES) {
    if (held === leftEl(field) && nodeWithin(area, leftEl(box))) return leftEl(box);
  }
  return null;
}

/**
 * Whether any of `box` was in the view of `area` while the list was `height` tall. The list shrinks from
 * its foot with its scroll position kept, so where the box stands below the list's top is where it stood.
 *
 * @param {HTMLElement} box
 * @param {HTMLElement} area
 * @param {number} height
 */
function boxWasInView(box, area, height) {
  const rect = box.getBoundingClientRect();
  const top = rect.top - area.getBoundingClientRect().top;
  return top < height && top + rect.height > 0;
}

/**
 * `#429 keyboard-hides-bar`. Keep the box being written in `area` in view as the list shrinks from
 * `seen.list` to `now.list`, if it was in view; whether it did.
 *
 * @param {HTMLElement} area
 * @param {{list: number} | null} seen
 * @param {{list: number}} now
 */
function keepWrittenBox(area, seen, now) {
  if (seen === null || seen.list <= 0 || now.list >= seen.list) return false;
  const box = boxBeingWritten(area);
  if (box === null || !boxWasInView(box, area, seen.list)) return false;
  keepBoxInView(box, area);
  return true;
}

// --- the box that grows -------------------------------------------------------------------------
//
// `#340 composer-grow-box`. The owner, on a desk: pasting a long message left the message box tiny,
// while the room left of Send was empty. So that room holds a Grow checkbox, in each of the three
// boxes the composers' shared rule draws (the call's, the channel's and the right column's), and
// while it is ticked the box grows with what is typed or pasted, up to about 60% of the window, and
// scrolls inside beyond that, so Send stays on screen.
//
//   * A DESK'S ALONE. web/voice.css draws the checkbox, and lets the box grow, in its desktop block
//     only; this script never asks the window what it is (`growShown` asks the checkbox).
//   * OFF BY DEFAULT, one choice for the three boxes, kept on this device. Absent means off, and the
//     default is not written down as a choice.
//   * THE BROWSER SIZES THE BOX where it can, by `field-sizing: content`. Where it cannot, the box is
//     measured as it changes and its height written for the stylesheet to use (`fitGrowingBox`); the
//     cap is the stylesheet's either way.
//   * A SEND CARD WINS: while one is up, its box is one line beside Send, as `#269 approval-card-inline`
//     made it, and the checkbox is not drawn. The right column's box, which no card touches, grows on.
//   * NOT THE REPLY BOX, in this change: it has Cancel left of Send, and a cap of its own in a column.
//   * SEND STAYS IN VIEW while the box grows under the reader's typing: its composer is brought into
//     its list whole (`keepBoxInView`). Only for the box with the focus, so a draft put back into a box
//     under a reader up the list moves nothing (`#280 scroll-stability`).
//   * AND AS GROW IS TICKED: the owner pastes, sees the box tiny, then ticks Grow, and the box that
//     grows then is not the one with the focus, which has left it: Space, and a click in Chromium,
//     put it on the checkbox; a click in Safari or Firefox on a Mac puts it nowhere, as they focus no
//     checkbox a click ticks. A reader at the foot of a list is kept at its foot as Grow changes the
//     boxes in it (`applyComposerGrow`); one a little above it, the checkbox on screen, has the
//     checkbox kept where it was, under the pointer, and Send with it; one who scrolled it away is not
//     moved. The checkbox ticked is the one the change came from, never read off the focus.

/** Whether the browser sizes a field to its content itself. Chromium does; Firefox does not yet. */
const FIELD_SIZING = Boolean(window.CSS && typeof window.CSS.supports === "function" &&
  window.CSS.supports("field-sizing", "content"));

// The boxes' growth is web/src/view/composer's (`createComposerBoxes`, made below with the
// boxes' keys, `#624 voice-page-modularity`); its Grow owner is `composerGrowth` there.

// --- Enter, in every message box ----------------------------------------------------------------
//
// `#225 enter-to-send`. The owner: "In main chat view enter is send. In reply view enter is newline
// and I must press the actual button (mouse!). Let's make it consistent between the two views."
//
// Each box had grown its own opinion. The call's box sent on any Enter, the channel's (and so a
// thread's, which is the same box) sent on Enter without Shift, and the reply screen's had no
// handler at all, so Enter there was a new line and sending took the mouse. ONE handler now decides
// for all of them, and the reader decides what it decides: "Enter to send" on the Settings screen.

// The message boxes, each of which hands its Enter to `onComposerKey` (`COMPOSER_FIELDS`), are
// web/src/view/composer's, with what a key does in them (`#624 voice-page-modularity`).

/**
 * Does a bare Enter send? ON unless the reader has turned it off — see `enterSends.stored`.
 *
 * ON because that is what the channel box always did, and what the call's box did too: the owner
 * asked for the reply screen to agree with them, not for them to change.
 *
 * Ctrl+Enter (Cmd+Enter on an Apple keyboard) sends EITHER WAY. So turning this off costs nothing
 * but the habit: the reader who wants Enter for new lines still has a key that sends, and the one
 * who leaves it on can use the chord without first remembering which way the setting is.
 *
 * One owner in web/src/core/settings/keychoices.ts (`#624 voice-page-modularity`), with what was stored
 * and turning it on or off, which keeps the choice and then says it (`renderEnterSends`).
 *
 * @type {import("./src/core/settings/index.ts").EnterSends}
 */
const enterSends = createEnterSends({
  deviceSettings: () => deviceSettings(),
  render: () => renderEnterSends(),
});

// What Enter does, said in Settings and on each box's return key, is web/src/view/composer's
// (`createComposerBoxes`, `#624 voice-page-modularity`).

/**
 * What a key does in the message boxes, and when each last took a paste (web/src/view/composer
 * `createComposerKeys`, `#624 voice-page-modularity`): on the clock, and on "Enter to send"
 * (`enterSends`), read as each key arrives.
 *
 * @type {import("./src/view/composer/index.ts").ComposerKeys}
 */
const composerKeys = createComposerKeys({ now: () => Date.now(), enterSends: () => enterSends.current() });

/**
 * The message boxes' growth and their Enter (`createComposerBoxes`, `#624 voice-page-modularity`):
 * whether they grow, which list each is in, what Enter does said aloud, and what each last took
 * as a paste. See web/src/view/composer/boxes.ts.
 *
 * @type {import("./src/view/composer/index.ts").ComposerBoxes}
 */
const composerBoxes = createComposerBoxes({
  el: (id) => el(id),
  leftEl: (id) => leftEl(id),
  replyScroller: () => replyPane().scroller(),
  atBottom: (area, slack) => (slack === undefined ? atBottom(area) : atBottom(area, slack)),
  followFoot: (area) => followFoot(area),
  keepBoxInView: (box, area) => keepBoxInView(box, area),
  activeElement: () => document.activeElement,
  bottomSlack: BOTTOM_SLACK_PX,
  fieldSizing: FIELD_SIZING,
  keepGrow: (on) => deviceSettings().keep(DEVICE_SETTINGS.composerGrow, on),
  storedGrow: () => deviceSettings().get(DEVICE_SETTINGS.composerGrow),
  enterSendsCurrent: () => enterSends.current(),
  platform: () => (navigator && navigator.platform) || "",
  noteInsertion: (box, event) => composerKeys.noteInsertion(box, event),
});
const { composerGrowth, storedComposerGrow, applyComposerGrow, renderComposerGrow, growShown, growingBoxList, fitGrowingBox, fitGrowingBoxes, keepGrowingBoxInView, modKeyName, sendChordName, renderEnterSends, noteBoxInsertion, watchBoxInsertions } = composerBoxes;

/**
 * What a keystroke in a message box does: send, or leave the box to handle it.
 *
 * `send` is the box's own send — the very function its Send button calls — so the key and the
 * button cannot become two opinions about sending, and the drafts, the outbox and its idempotency
 * are reached exactly as a tap reaches them.
 *
 *   * Ctrl+Enter or Cmd+Enter sends, whatever the setting says.
 *   * A bare Enter sends while "Enter to send" is on. While it is off it is left to the box, and a
 *     textarea's own Enter is a new line, which is the reason to turn it off.
 *   * Shift+Enter or Alt+Enter, without Ctrl or Cmd, is never a send: it is how a new line is asked
 *     for while Enter sends, and with the setting off it is a new line anyway.
 *   * NOTHING while an input method is composing. Japanese, Chinese and Korean input, and a phone's
 *     predictive text, use Enter to ACCEPT the word being built; a send there would post the
 *     message with that word still uncommitted. `isComposing` is the standard signal, and keyCode
 *     229 is what Safari and older Chromium report for the same keystroke instead.
 *   * NEVER ON A REPEAT. One press is one send. A key held down past the repeat delay fires its
 *     repeats as keydowns too, and since `#248 composer-key` a Ctrl+Enter pressed outside a box
 *     puts the focus IN the box, so its own repeats arrive here: they sent whatever draft the box
 *     was holding, a quarter of a second after the reader only asked to go to it. A held send
 *     key's repeats still have their default taken, so they type no run of new lines either.
 *
 * Every box is a textarea, the call's included since `#241 voice-composer`. The call's used to be a
 * one-line `<input>`, whose bare Enter is a form submission, so an Enter that did not send was
 * swallowed there; a textarea's own Enter is a new line, which is what the setting being off asks
 * for, so nothing is swallowed anywhere now.
 *
 *   * NEVER AS PART OF A PASTE (`paste-never-sends`). The owner, 2026-10-26, on Android: "as soon
 *     as I press paste on Android, it is sending it immediately without giving me a chance to edit
 *     ... Just pasting something should never immediately send it." A keyboard's clipboard paste
 *     hands Chrome its text without a composition, and Chrome on Android turns each newline the
 *     keyboard commits into a trusted Enter keydown: the paste typed its own Enter, and that Enter
 *     sent. So an Enter arriving within `PASTE_ENTER_MS` of a paste, a drop, or text of more than
 *     one character put in outside a composition (`noteBoxInsertion`) is that insertion's new line
 *     and is left to the box. A word being composed is committed through a composition, so the
 *     Enter pressed after typing one still sends.
 *
 * Returns what `send` returned, or undefined when nothing was sent. A browser ignores a listener's
 * return value; the page suite awaits it, so a send is finished before the next assertion reads it.
 *
 * @param {KeyboardEvent} event
 * @param {() => unknown} send
 * @param {any} [box] the box the key was pressed in: its own listener says, as `event.target` would
 * @returns {unknown}
 */

// How long after a paste an Enter is part of it (`PASTE_ENTER_MS`), when each box last took one, each
// box's new-line key (`NEWLINE_KEYS`) and putting a new line in a box (`insertNewline`) are
// web/src/view/composer's (`paste-never-sends`, `#575 mobile-keyboard-compose`).

// --- the keyboard -------------------------------------------------------------------------------
//
// `#224 keyboard-shortcuts`. The owner, in the installed app on a Mac desk: "pgup/pgdown and home/end
// don't work even though this is basically a website inside the installed PWA ... I want to make
// sure those plus "/" and "Ctrl-s" for opening search work. Plus a key to go to the settings, and to
// save and exit settings, though I think the saving is implicit." And of the keymap proposed after a
// study of how other chat clients do it: "go ahead and implement the keyboard shortcuts on desktop."
//
// `#248 composer-key` gave Ctrl+S (Cmd+S) to the message box, at the owner's later word: "When I'm
// using keyboard, I want to jump to text entry and start typing. Google chat uses Cmd-s for that and
// I'm ok with that." The search keeps "/" and its glass. See `writeFromKeyboard`.
//
// `#344 search-shortcut` gave the search Ctrl+/ (Cmd+/), from anywhere, at the owner's word: "want
// reliable cmd-/ for search ... Sometimes just pressing '/' doesn't work ... prefer it to just be Cmd-/.
// Currently that pops open help ... map it to Cmd-? (i.e. Cmd-shift-/)." So the list of keys is "?" and
// Ctrl+? (Cmd+?) now. See `searchFromAnywhere` and `keyDispatch.onEverywhereKey`.
//
// WHY THE SCROLL KEYS DID NOTHING. A browser scrolls with PgUp, PgDn, Space, Home, End and the arrows
// by scrolling the box that holds the focus (with nothing focused, the box the last click landed in)
// and, failing both, the DOCUMENT. This page's document never scrolls: the frame is a fixed grid and
// each list scrolls inside it (#scroll-area, #side-scroll, and the Settings, Help, Threads and
// sign-in screens), and none of them could hold the focus. So after a load, after any control that
// hides itself once pressed (every Back button does), and after a click on anything in the dock (the
// view switch, the channel picker, Hide read) the keys went to the document and moved nothing. Space
// was worse: the view switch kept the focus from the click that opened the channel, so Space pressed
// it again and left the channel. A click on a message made the keys work until the next click
// elsewhere, which is why it looked like a key that worked only sometimes. Measured in Chromium
// before the fix (tests/keyboard_browser.py, run against the old page); `contain: size layout` on
// #scroll-area (`#226 page-energy-profile`) is not involved, as the keys behave the same without it.
//
// THE FIX IS TO LET THE BROWSER DO IT. Every scroller can take the focus (`tabindex="-1"` in
// web/voice.html: from script and a click, never a Tab stop), the screen's scroller takes it whenever
// the screen or the view changes (`settleFocus`), and a scroll key arriving from somewhere that
// cannot scroll anything hands it the focus on the way past (`scrollKeys.routeScrollKey`). The browser's own
// default, which runs after this page's listener, then scrolls it, at the browser's distances and
// with its smoothing. The page scrolls a list itself only where the browser's default would do
// something else with the key: PgUp and PgDn in a text field (a page of caret movement) and on a
// picker (another option), Home and End on a picker, Ctrl+Home and Ctrl+End from anywhere, and Home
// and End over the channel, which also select the message there.
//
// ONE TABLE, `KEYMAP`, and one listener that walks it, `keyDispatch.onPageKey`. Three things are read off the
// table: what each key does, the list of every key on the Help screen (`keyHints.render`), and the
// `aria-keyshortcuts` of the buttons that have one, so the list a reader is shown cannot drift from
// what the keys do. The message boxes keep their own Enter (`onComposerKey`), the reading-width handle
// its arrows and the search field its Escape; each runs first, at its element, and a key it handled
// arrives here with `defaultPrevented` and is left alone. The table names those keys too, as answered
// by someone else, so that the list is whole. The keys heard before all of them are the search's
// chord and the message box's, which a binding marked `everywhere` hands to a second listener, at the
// window, in the capture phase (`keyDispatch.onEverywhereKey`, `#344 search-shortcut`, `#403 composer-key-global`);
// it walks the same table.
//
// THE RULES, which are the proposal's section 5:
//
//   * A key that types a character (a letter, a digit, a mark) never fires while the reader types in
//     a field or works a picker or a slider, nor with Ctrl, Cmd or Alt held, which leaves every browser
//     chord alone. Escape and the Ctrl/Cmd chords do fire in a field: a field is where the reader is
//     when they want the search, Settings or the ends of the list, or to mark a message Done (Ctrl+E,
//     `cmd-e-done`), and the search's Ctrl+/ (Cmd+/) is heard before any element's own listener
//     (`keyDispatch.onEverywhereKey`). Nothing fires while an input method
//     composes, and a phone keyboard, which reports keyCode 229 and types only into fields, never
//     reaches the table. A chord takes Ctrl or Cmd on every platform (the owner asked for Ctrl+S on
//     his Mac, then for the search and now for the message box), except in a text field on a Mac, where Ctrl and a letter macOS edits text with
//     (Ctrl+K, Ctrl+A, Ctrl+E and the rest) is the field's: there the page's chord is Cmd only
//     (`keyDispatch.macFieldEditKey`).
//   * Those character keys can all be turned off in Settings, "Single-key shortcuts", on by default:
//     what WCAG 2.1.4 asks of them. The arrows, Escape, Enter and the chords stay.
//   * THE SELECTED MESSAGE IS REAL FOCUS ON ITS ROW, so a screen reader announces the row and its
//     buttons are a Tab away. A row has no `tabindex` until it is selected (a tap on a phone must not
//     give a row the focus and light it) and the selected one has `tabindex="0"`, so Shift+Tab from its
//     buttons comes back to it. It is kept by message id, because rows are rebuilt under the reader
//     (`expandedIds` says why), per list, and put back after every rebuild (`keepSelection`). A key
//     about one message never acts on one scrolled out of sight: it brings the ring to the first
//     message showing instead (`rowForAct`).
//   * ONE SELECTED MESSAGE, DRAWN ONE WAY (`#239 single-selection`). The owner, on a desk: a message
//     lit by the mouse and then ↓ left two messages lit at once, the mouse's and the keyboard's, and
//     which one Enter would open was anybody's guess. So the mouse and the keys move the same
//     selection, and the selection is the only thing that lights a row: `data-selected` on that row,
//     drawn by web/voice.css as the ring and the tint together, whichever put it there; a row lit by
//     `:hover` alone no longer exists. A key that moves the selection takes it from wherever the mouse
//     put it, and a REAL mouse move over a row takes it to that row, the focus with it while the focus
//     is on a list or on a control a click focused (`pointRow`, `focusFollowsSelection`). A move is
//     real when the pointer itself moved: a list scrolled under a pointer standing still, by a key or
//     a new message, takes nothing (`mouseMoved`). A click selects the row too, drawn; a tap on a
//     touch screen selects SILENTLY, with no focus and nothing drawn, because a touch screen has no
//     hover and a lit row there could not be put out. The row a jump lands on (`revealRow`) becomes
//     the selection, so the flash that says "here" is on the message every key now acts on, and goes
//     out when the selection moves on.
//   * PER LIST, NOT PER PAGE. A list the keys move through is a `KeyPane` (its scroller, its rows,
//     and the room its floating line and its chips take) and nothing below names #scroll-area except
//     the channel pane. A second column is a second entry in `KEY_PANES`; the pane holding the focus
//     is the one the keys move, else the one used last, and a click there notes its row. Each pane
//     keeps a selection, and only the pane in use draws its own, so two columns still show one lit
//     message: a mouse moved into the other column takes the selection there, and the keys with it.
//   * Escape never marks anything read (the proposal's question 5), and no key starts a call (question
//     9): the call's view takes no keys but Escape and the search, Settings and help of the page.
//   * Browser history is not touched: Back in the browser still leaves the page. A follow-up.

// Where a key landed (isTextField, keepsOwnKeys, pressedBySpace, isLink, answersEnter, scrollsItself)
// is read in web/src/view/dom/ (`#624 voice-page-modularity`).

/** Whether nothing holds the focus: the body has it, as after a load or a control that hid itself. */
function focusLost() {
  const held = document.activeElement;
  return !held || held === document.body || held === document.documentElement;
}

// Whether a node and everything around it is shown (`shownNode`) is web/src/view/dom's.

// Whether a node is one of the message boxes, where Escape leaves the field before it goes Back
// (`isComposer`, see `onEscape`), is web/src/view/composer's.

// --- the lists the keys move through

// What a selection kept per list is (`KeySelection`), and a list the keys move through (`KeyPane`), are
// web/src/view/shell's (`#624 voice-page-modularity`).
/** @typedef {import("./src/view/shell/index.ts").KeySelection} KeySelection */
/** @typedef {import("./src/view/shell/index.ts").KeyPane} KeyPane */

/**
 * The keyboard composition: every list the keys move through and every owner the keys dispatch
 * through, built in order (web/src/app/keyboard.ts, `#624 voice-page-modularity`, step 17 move 2).
 * The page destructures what it still calls under the old names; what each key does on a row, a
 * channel or the screen is handed in as it was taken here, every one of them read as it is called,
 * so a key pressed after start-up finds them as they then stand. Owners whose acts call later
 * owners keep their thunk shape inside the composition exactly as they had it here.
 *
 * @type {import("./src/app/index.ts").KeyboardComposition}
 */
const keyboard = createKeyboardComposition({
  screens,
  el: (id) => el(id),
  leftEl: (id) => leftEl(id),
  documentRoot: () => document.documentElement,
  mainColumn: () => columns.main,
  sideColumn: () => columns.side,
  inColumn: (column, act) => inColumn(column, act),
  inSide: () => columnWork.side(),
  activeColumn: () => columnWork.active(),
  shownChannelList: () => shownChannelList(),
  floatingClearance: (area) => floatingClearance(area),
  channelContextKey: () => channelContextKey(),
  selectionView: () => selection().view,
  pinsOnly: () => pinsView().only,
  threadListShown: () => layoutShown.threadList(),
  sideColumnUp: () => sideColumnUp(),
  threadListOnScreen: () => threadListOnScreen(),
  rowShown: (row) => rowShown(/** @type {any} */ (row)),
  idsOf: (row) => idsOf(/** @type {any} */ (row)),
  shownRowFor: (row) => shownRowFor(row),
  rowTimeTabStop: (row, on) => rowTimeTabStop(row, on),
  readerActed: () => readerActed(),
  putOutLanding: (row) => putOutLanding(row),
  noteLinkedSelection: () => noteLinkedSelection(),
  focusLost: () => focusLost(),
  replyLandingGapPx: () => REPLY_LANDING_GAP_PX,
  replyDocked: () => Boolean(replyState.docked),
  replyPane: () => /** @type {any} */ (replyPane()),
  rowMenuOpen: () => Boolean(rowMenus.open()),
  openRowMenu: () => rowMenus.open(),
  toFoot: (area) => toFoot(area),
  endFilterWalk: () => endFilterWalk(),
  closeOverscroll: () => closeOverscroll(),
  openOverscroll: (area) => openOverscroll(area),
  scrollToNewest: () => scrollToNewest(),
  isTextField: (node) => isTextField(node),
  scrollsItself: (node) => scrollsItself(node),
  keepsOwnKeys: (node) => keepsOwnKeys(node),
  pressedBySpace: (node) => pressedBySpace(node),
  entryLandingHolds: () => entryLandingHolds(),
  entryLandingFirst: () => entryState().landing.first,
  setStatus: (text) => setStatus(text),
  rowIsRead: (row) => rowIsRead(row),
  isDone: (id) => doneState().archived.has(id),
  toggleArchived: (ids, row) => toggleArchived(ids, row),
  guardQuietly: (run) => guardQuietly(run),
  selectedChannelId: () => selectedChannelId(),
  markRowDone: (ids, row) => markRowDone(ids, row),
  doneSaved: (ids) => doneSaved(ids),
  markReadInService: (channel, id) => markReadInService(channel, id),
  doneReadWords: (mark, also) => doneReadWords(mark, also),
  serviceReadWords: (mark, also) => serviceReadWords(mark, also),
  localReadWords: (wasDone, also) => localReadWords(wasDone, also),
  statusSays: (text) => statusSays(text),
  dismissStatus: () => dismissStatus(),
  showingLinks: (row) => showingLinks(row),
  isFolded: (entry) => isFolded(/** @type {import("./src/view/channel/index.ts").FoldEntry} */ (/** @type {unknown} */ (entry))),
  setFolded: (entry, folded) => setFolded(/** @type {import("./src/view/channel/index.ts").FoldEntry} */ (/** @type {unknown} */ (entry)), folded),
  atBottom: (area) => atBottom(area),
  preservingScroll: (act) => preservingScroll(act),
  renderScrollTools: () => renderScrollTools(),
  rowMessages: (row) => rowMessages(/** @type {any} */ (row)),
  isOpenThreadRoot: (row) => isOpenThreadRoot(row, openThreadRootId()),
  viewKey: (kind, id) => viewKey(kind, id),
  foldedOnes: () => foldedOnes,
  closeReadRun: (key, ids) => readRuns.close(key, ids),
  renderSideColumn: () => renderSideColumn(),
  applyReadRuns: () => applyReadRuns(),
  openReadRun: (row) => openReadRun(/** @type {any} */ (row)),
  liveUnpackedRow: (row) => liveUnpackedRow(row),
  liveCardRow: (row) => liveCardRow(row),
  actOnRow: (row, fold) => actOnRow(row, fold),
  cardForArrow: (target) => cardForArrow(target),
  columnsShown: () => layoutShown.columns(),
  isComposer: (node) => isComposer(node),
  closeThread: () => closeThread(),
  sideThreadId: () => inColumn(columns.side, () => String(selection().threadId)),
  cardRowOnRight: (id) => cardRowOnRight(id),
  closeSideThread: () => closeSideThread(),
  toggleMessageDetails: (row, messages) => toggleMessageDetails(row, messages),
  upstreamUrlOf: (message) => upstreamUrlOf(message),
  upstreamServiceName: (message) => upstreamServiceName(message),
  copyUpstreamLink: (url, service) => copyUpstreamLink(url, service),
  copyDeepLink: (message) => copyDeepLink(message),
  readCardAloud: (card) => readAloudOwner().readCardAloud(card),
  readRowAloud: (row) => readAloudOwner().readAloud(idsOf(row)),
  combinedContent: (messages) => combinedContent(messages),
  renderedHtmlOf: (message) => renderedHtmlOf(message),
  rawTextOf: (message) => rawTextOf(message),
  writeClipboardText: (text) => writeClipboardText(text),
  closeSettings: () => closeSettings(),
  // The keys' switch keeps its two-way meaning (the call's view or the channel's, never
  // Documents between them); the button's press is the three-way `pressViewSwitch`.
  pressViewSwitch: () => chooseMajorView(screens.view() === "voice" ? "discord" : "voice"),
  chooseView: (which) => chooseView(which),
  cycleReadMode: () => cycleReadMode(),
  readModeSaid: () => READ_MODE_SAID[readModes.mode()],
  searchOpen: () => searchField.open(),
  setSearchOpen: (open) => setSearchOpen(open),
  setPinnedOnly: (on) => setPinnedOnly(on),
  linksOnly: () => searchField.links(),
  setLinksOnly: (on) => setLinksOnly(on),
  cycleAuthorFilter: () => cycleAuthorFilter(),
  doneOffer: () => lastDismissal.current(),
  undoDismissal: () => undoDismissal(),
  sendOffer: () => sendUndo.current(),
  sendHeld: (id) => Boolean(dismissedSends.find(id)),
  putBackDismissedSend: (id) => putBackDismissedSend(id),
  bookmarkOffer: () => bookmarkUndos.current(),
  currentChannelId: () => currentChannelId(),
  undoBookmarkRemoval: () => undoBookmarkRemoval(),
  threadListBack: () => threadListBack(),
  mainRowsOnScreen: () => mainRowsOnScreen(),
  channelsInPickerOrder: () => channelsInPickerOrder(),
  chooseChannel: (id) => chooseChannel(id),
  changeSelectedChannel: () => changeSelectedChannel(),
  showScreen: (name) => showScreen(name),
  showHelp: (entry) => showHelp(entry),
  openMenus: () => openMenus(),
  closeOpenMenu: (name) => closeOpenMenu(name),
  closeRowMenuOnEscape: (event) => closeRowMenuOnEscape(event),
  replyKeysHere: () => replyKeysHere(),
  closeReply: () => closeReply(),
  putOutLitCard: () => putOutLitCard(),
  clearAllThreadFocus: () => clearAllThreadFocus(),
  clearLinkedMark: () => linkedMark.clearOnEscape(),
  quoteFromKeyboard: () => quoteOwner.quoteSelectionNow(),
  modKeyName: () => modKeyName(),
  manyChatServices: () => channelDirectory.providers().size > 1,
  chatServiceName: () => chatServiceName(),
  threadOrderWords: () => THREAD_ORDER_NAMES.map((name) => THREAD_ORDERS[name].name).join(", "),
  minCollapsedLines: () => MIN_COLLAPSED_LINES,
  maxCollapsedLines: () => MAX_COLLAPSED_LINES,
  placeFromKeyboard: (step) => placeFromKeyboard(step),
  threadStripFromKeyboard: (target) => threadStripFromKeyboard(target),
  foldStepFromKeyboard: (step, event, target) => foldStepFromKeyboard(step, event, target),
  collapsedLinesFromKeyboard: (step, target) => collapsedLinesFromKeyboard(step, target),
  packFromKeyboard: (target) => packFromKeyboard(target),
  unpackFromKeyboard: () => unpackFromKeyboard(),
  bookmarkFromKeyboard: (target) => bookmarkFromKeyboard(target),
  bookmarkJumpFromKeyboard: () => bookmarkJumpFromKeyboard(),
  orderFromKeyboard: () => orderFromKeyboard(),
  mainRowsFromKeyboard: () => mainRowsFromKeyboard(),
  columnsFromKeyboard: () => columnsFromKeyboard(),
  childByClass: (row, className) => childByClass(row, className),
  deviceSettings: () => deviceSettings(),
  keyTimingSetting: () => deviceSettings().get(DEVICE_SETTINGS.keyTiming),
  locationSearch: () => window.location.search || "",
  toggleDocumentOutline: () => {
    if (screens.view() !== "docs" || !documentsView.reading()) return false;
    documentsView.toggleOutline();
    return true;
  },
});
const {
  channelPane, sidePane, KEY_PANES, paneColumn, lastPane, keySelection,
  paneFor, visibleScroller, paneRows, rowOf, paneRowFor, markOf, markedRow, markNames, releaseRow, activePane,
  drawSelection, takeRow, noteRow, currentRow, drawnRow, nearRow, rowInView, firstRowInView, selectRow, bringIntoView,
  keepSelection, inputKinds, pointerSelection,
  drawnBy, keysInUse, onPointerMove, onPointerDown, noteClickedRow, onPaneFocus, selectLanded, followPageScroll,
  focusList, settleFocus, behindThreadList, inColumnGone, scrollKeys, selectionKeys,
  menuItems, stepFromKeyboard, keyContext, rowForAct, doneKeys, nextUnreadFromKeyboard, doneFromKeyboard,
  readCollapse, readCollapseFromKeyboard, foldKeys, foldForRead, foldOneFromKeyboard, rowKeys,
  openFromKeyboard, pressRowButton, threadFromKeyboard, leaveThread, threadOutFromKeyboard, columnFromKeyboard,
  replyFromKeyboard, menuFromKeyboard, readFromKeyboard, detailsFromKeyboard, copyLinkFromKeyboard, deepLinkFromKeyboard,
  highlightCopy, COPY_REFUSED, selectedText, channelOnScreen, viewKeys,
  voiceFromKeyboard, viewFromKeyboard, readModeFromKeyboard, filterFromKeyboard, undoFromKeyboard,
  writeKeys, composeFromKeyboard, writeFromKeyboard, newestFromComposer, channelKeys, stepChannel, pickChannel, channelPicked,
  screenKeys, searchFromKeyboard, searchFromAnywhere, settingsFromKeyboard, closeHelp, keysFromKeyboard, replyOnScreen, onEscape,
  KEYMAP, singleKeys, keyDispatch, keyTimingInAddress, keyTiming, rowShortcuts, sayRowShortcut, keyHints,
} = keyboard;

// --- the canned prompts -------------------------------------------------------------------------
//
// `#60 canned-prompt-buttons`. Two questions worth a button because they are the ones actually
// asked, every time. They go out through `sendUserMessage` like any typed turn, so a tap with no
// call open reports itself rather than doing nothing, and the sentence lands in the transcript as
// the reader's own words — which it is.
//
// A LIST, not two cases. More of these are expected, and the shape of that expectation is that a
// third button is one entry here plus one pair of elements in web/voice.html — never a new code
// path. Every loop below iterates this.

const cannedPrompts = mountCannedPrompts({
  lookup: (id) => el(id),
  storage: deviceStorageOnUse(),
});

/**
 * Whether the tray of canned prompts is open, said on its opener and by the tray (`#60
 * canned-prompt-buttons`): one owner in web/src/view/shell/promptstray.ts (`#624
 * voice-page-modularity`), which says why it is one button for all of them and why it is not kept.
 * An open tray is a level, with an entry of its own (`#304 back-gesture-in-app`).
 *
 * @type {import("./src/view/shell/index.ts").PromptsTray}
 */
const promptsTray = createPromptsTray({ byId: (id) => /** @type {any} */ (el(id)), changed: () => scheduleBackSync() });
const setPromptsOpen = promptsTray.setOpen;

/**
 * `#820 voice-page-action-menu`. The list's action menu, open or not, and its line for a list with nothing to
 * do: web/src/view/shell `createActionMenu` (`#624 voice-page-modularity`). An open menu is a level, with an
 * entry of its own, as the tray is (`#304 back-gesture-in-app`).
 */
const actionMenu = createActionMenu({ byId: (id) => /** @type {any} */ (el(id)), changed: () => scheduleBackSync() });
const setActionMenuOpen = actionMenu.setOpen;

/**
 * `#880 voice-page-menu-placement`. The channel's row of big reader buttons, shown or hidden from the action
 * menu and kept on this device: web/src/view/shell `createReaderRow`. web/voice.css takes the row away.
 */
const readerRow = createReaderRow({
  byId: (id) => /** @type {any} */ (el(id)),
  root: () => document.documentElement,
  stored: () => deviceSettings().get(DEVICE_SETTINGS.readerHidden),
  keep: (hidden) => deviceSettings().keep(DEVICE_SETTINGS.readerHidden, hidden),
});

/**
 * `#886 voice-page-autoplay`. On, a read heard to its end goes on to the next unread message below it, in the view
 * it was read in (the pane the keys moved last first): web/src/view/voice `createAutoplay`, kept on this device.
 */
const autoplay = createAutoplay({
  byId: (id) => /** @type {any} */ (el(id)),
  stored: () => deviceSettings().get(DEVICE_SETTINGS.autoplay),
  keep: (on) => deviceSettings().keep(DEVICE_SETTINGS.autoplay, on),
  panes: () => [...new Set([activePane(), ...KEY_PANES].filter(Boolean))],
  paneRows: (pane) => paneRows(pane),
  idsOf: (row) => idsOf(/** @type {any} */ (row)),
  rowIsRead: (row) => rowIsRead(row),
  paneRowFor: (pane, id, card) => paneRowFor(pane, id, card),
  read: (row) => (row.hasAttribute("data-card")
    ? guardQuietly(() => readAloudOwner().readCardAloud(row))()
    : guardQuietly(() => readAloudOwner().readAloud(idsOf(row)))()),
  setStatus: (text) => setStatus(text),
});

// --- the server ------------------------------------------------------------------------------

/**
 * What {@link api} throws. `#624 voice-page-modularity`: web/src/core/api says what each field means.
 *
 * @typedef {import("./src/core/api/index.ts").ApiRequestError} ApiRequestError
 */

/**
 * How many timeline reads the page has sent, of any view, by any path, and where each answer's read
 * stood among them (web/src/core/sync `createReadOrder`, `#624 voice-page-modularity`).
 * `remote-ops-audit` W7: see `apiDecoded` and `abandonedReadStands`; `#308 live-reply-in-main`: see
 * `sentNumberOf`.
 *
 * @type {import("./src/core/sync/index.ts").ReadOrder}
 */
const readOrder = createReadOrder();

/**
 * The browser's capabilities as the page's modules take them (web/src/platform/web, `#624
 * voice-page-modularity`): transport, timers, cancellation and storage. Each looks its browser
 * global up at every call, as the page's own calls did, so a browser refusing storage throws at the
 * same moment it always did. Made on first use; a `var` with no initialiser, like `serverApi`.
 *
 * @type {import("./src/platform/web/index.ts").WebPorts | undefined}
 */
var webPorts;

/** {@link webPorts}, made if it is not yet. */
function browserPorts() {
  if (!webPorts) webPorts = createWebPorts();
  return webPorts;
}

/** The device's storage, `localStorage`, which every window of this origin shares. */
function deviceStorage() {
  return browserPorts().deviceStorage;
}

/**
 * {@link deviceStorage} for an owner made before the ports are: each call looks the port up, so making
 * the owner makes no port early.
 *
 * @returns {import("./src/ports/index.ts").StoragePort}
 */
function deviceStorageOnUse() {
  return {
    getItem: (key) => deviceStorage().getItem(key),
    setItem: (key, value) => deviceStorage().setItem(key, value),
    removeItem: (key) => deviceStorage().removeItem(key),
  };
}

/**
 * The page's clock for an owner made before the ports are, as {@link deviceStorageOnUse} is its storage.
 *
 * @returns {import("./src/ports/index.ts").ClockPort}
 */
function clockOnUse() {
  return {
    setTimeout: (run, ms) => browserPorts().clock.setTimeout(run, ms),
    clearTimeout: (handle) => browserPorts().clock.clearTimeout(handle),
  };
}

/** This window's own storage, `sessionStorage`. */
function windowStorage() {
  return browserPorts().windowStorage;
}

/**
 * The settings this device keeps (web/src/core/settings `DEVICE_SETTINGS`), in its storage: each
 * setting's key, encoding and default said once there. Made on first use, like `webPorts`.
 *
 * @type {import("./src/core/settings/index.ts").DeviceSettings | undefined}
 */
var pageDeviceSettings;

/** {@link pageDeviceSettings}, made if it is not yet. */
function deviceSettings() {
  if (!pageDeviceSettings) pageDeviceSettings = createDeviceSettings(deviceStorage());
  return pageDeviceSettings;
}

/**
 * The page's requests to vibe-talk (web/src/core/api, `#624 voice-page-modularity`), on the
 * browser's transport, timers and cancellation, made the first time the page asks anything. A `var`
 * with no initialiser, so a request made before this line has run finds it undefined rather than in
 * its dead zone, and this line, when it runs, does not throw away an instance already made.
 *
 * @type {import("./src/core/api/index.ts").ServerApi | undefined}
 */
var serverApi;

/** {@link serverApi}, made if it is not yet. */
function theServer() {
  if (!serverApi) {
    serverApi = createApi(browserPorts(), {
      token,
      // A token the server no longer accepts, or a read it refuses, ends this device's copy of
      // what that token read.
      readRefused: () => messageCache.dropMessageCache(),
      // `remote-ops-audit` W7. Every timeline read the page sends passes here, and is counted as it
      // is sent, so a read given up on can tell whether any was sent after it (`abandonedReadStands`).
      asking: (type) => {
        if (type === "TimelineResponse") readOrder.noteAsked();
      },
      answerer: () => sentenceStart(chatServiceOf(selectedChannelId())),
    });
  }
  return serverApi;
}

/**
 * One request to vibe-talk, with the token, and one error taxonomy out of it: every field of the
 * server's error answer passed through (web/src/core/api `request`).
 *
 * @param {string} path
 * @param {import("./src/core/api/index.ts").ApiRequestOptions} [options]
 * @returns {Promise<any>}
 */
function api(path, options) {
  return theServer().request(path, options);
}

/**
 * {@link api}, with the answer checked against the wire contract before anything reads it
 * (web/src/core/api `decoded`).
 *
 * @template {keyof VibeTalk.Contract} K
 * @param {K} type the contract type the route answers with
 * @param {string} path
 * @param {{method?: string, body?: unknown, signal?: AbortSignal}} [options]
 * @returns {Promise<VibeTalk.Contract[K]>}
 */
function apiDecoded(type, path, options) {
  return theServer().decoded(type, path, options);
}

/**
 * A request to vibe-talk that stops waiting after `ms`, failing as a timeout the reader can be
 * told about rather than as a "refreshing…" nobody ends (web/src/core/api `within`, `#195
 * send-resilience`). The poll does not read that view again until the read answers, the server's
 * own bound has passed, or a live event says the service is answering (`readHeld`,
 * `remote-ops-audit` W7).
 *
 * `abandoned`, when given, is handed the request still on the wire when the wait is given up, and
 * the request is then not aborted: its answer, if it comes, is the caller's to use or drop
 * (`abandonRead`). Without it the request is aborted, as it always was.
 *
 * @template T
 * @param {number} ms
 * @param {(signal: AbortSignal | undefined) => Promise<T>} request
 * @param {((answer: Promise<T>) => void) | null} [abandoned]
 * @returns {Promise<T>}
 */
function within(ms, request, abandoned = null) {
  return theServer().within(ms, /** @type {(signal: unknown) => Promise<T>} */ (request), abandoned);
}

// --- what a slow or failing chat service means, in words ------------------------------------------
//
// `#195 send-resilience`. The server's sentence for a provider failure is a diagnosis —
// `HTTP 502 chat_error: Google Chat returned HTTP 502: {"message":"… timed out …"}` — which is
// right in a log and wrong on a phone, where it was all the reader saw of a refresh that did not
// land, in a red banner that came back every poll for as long as the service stayed slow. These say
// what it means instead: the provider's own words without the JSON around them, and a timeout —
// most of what a slow service does — as "slow to answer". A network failure and a refusal are
// already sentences, and keep them.

// `SLOW_WORDS`, `providerWords`, `isSlowFailure` and `readableFailure` are web/src/core/api's
// (`#624 voice-page-modularity`).

/** The chat service a channel is read through, as a noun phrase for a sentence. */
function chatServiceOf(channel) {
  const provider = providerOfChannel(channel);
  return (provider && typeof provider.name === "string" && provider.name.trim()) || chatServiceName();
}

/** {@link readableFailure} for the channel on screen. */
function readableChannelFailure(error) {
  return readableFailure(error, chatServiceOf(selectedChannelId()));
}
voiceTranscript().initializeHeardPage(Math.random().toString(36).slice(2, 10));


// --- microphone settings ------------------------------------------------------------------------

// Three knobs on the audio the browser hands us. They are exposed because each one has a failure
// mode the owner has actually hit, and none of them is guessable from the outside: the agent
// answering its own voice on speakerphone, background noise reaching the transcriber, and — the
// awkward one — automatic gain amplifying a silent room until a phone being set down crosses the
// speech threshold and gets transcribed as a word.
const microphoneSettings = mountMicrophoneSettings({
  lookup: (id) => el(id),
  storage: deviceStorageOnUse(),
  streamOpen: () => Boolean(voiceCall().session.stream),
});

// The constraint object handed to getUserMedia.
//
// Echo cancellation and noise suppression are already stated explicitly, so a toggle simply
// supplies the value that was hard-coded.
//
// Automatic gain control is the careful one. It is NOT in the constraint object at all by default,
// so whatever the browser's own default is, is what applies — in practice on, but the spec makes
// that the implementation's choice, not a promise. Passing `autoGainControl: true` would convert an
// implicit default into an explicit request, and those are not guaranteed to be the same thing on
// every browser. So the ON state — the default — says nothing at all, and only the OFF state is
// stated. With every toggle in its default position this returns the byte-for-byte constraint
// object the page has always sent.
function audioConstraints(settings) {
  const audio = {
    channelCount: 1,
    echoCancellation: settings.echoCancellation,
    noiseSuppression: settings.noiseSuppression,
  };
  if (!settings.autoGainControl) {
    audio.autoGainControl = false;
  }
  return { audio };
}

// --- audio devices: `#619 voice-bluetooth-output` -------------------------------------------------
//
// WHY A CALL COULD PLAY ON THE PHONE SPEAKER WITH AIRPODS CONNECTED (diagnosed 2026-10-28 from
// Chromium source, media/base/android/java/src/org/chromium/media/):
//
// Music (Spotify, and this page's read-aloud) plays in MODE_NORMAL, where Android routes media
// audio to A2DP — the AirPods. A voice call is different: opening capture makes Chromium enter
// MODE_IN_COMMUNICATION (`AudioManagerAndroid.setCommunicationAudioModeOn`), where ONE paired
// communication device carries both directions. Which device that is comes from the CAPTURE
// device the page asked for: with no `deviceId`, Chromium's default communication-device choice
// (`CommunicationDeviceSelector.DeviceHelpers.selectDefaultDevice`) prefers wired, then USB,
// then Bluetooth, then the SPEAKERPHONE — so a headset Chrome does not see as an available
// communication device (an A2DP-only connection, or the built-in microphone chosen by default)
// leaves the call on the speaker even though music reaches the headset. The fix available to a
// page is therefore on the INPUT side: ask for the Bluetooth/headset capture device explicitly,
// and Chromium selects the paired Bluetooth communication device for playback too.
//
// Output selection proper (`HTMLMediaElement.setSinkId`, `AudioContext.setSinkId`,
// `selectAudioOutput`) is desktop-Chromium only: MDN's browser-compat data lists
// `HTMLMediaElement.setSinkId` as unsupported on Chrome for Android, so on the phone there is
// no output picker to offer — the picker below is hidden where neither form exists, and the
// call's output follows the communication route its input chose. Read-aloud deliberately
// keeps NO sink: it stays a media playback, on A2DP like music, and never enters communication
// mode. Echo cancellation stays ON by default (`#264 voice-send-approval`): it is part of why
// Chromium treats this capture as a call, and the speakerphone/car cases keep it.


/**
 * The route of the call in progress, for the readout and the call-end record; null between calls: one
 * owner in web/src/core/call/audioroute.ts (`#624 voice-page-modularity`). Which device a label names
 * is {@link classifyAudioDevice}'s.
 *
 * @type {import("./src/core/call/index.ts").AudioRoute}
 */
const audioRoute = createAudioRoute({ classify: (label) => classifyAudioDevice(label) });

const audioDevices = mountAudioDevices({
  lookup: (id) => el(id),
  storage: deviceStorageOnUse(),
  mediaDevices: () => navigator.mediaDevices,
  outputSinkSupported: () => outputSinkSupported(),
  deviceLabel: (device) => deviceLabel(device),
  classifyAudioDevice: (label) => classifyAudioDevice(label),
  stream: () => voiceCall().session.stream,
  audio: () => voiceCall().session.audio,
  route: () => audioRoute.current(),
});

/**
 * The one read of a media device's, track's or track-settings' label in this section: the
 * channel-name guard in src/http/api.rs counts label reads across this file, and a device
 * label is not a channel label — so it goes through here, once, without that spelling.
 */
function deviceLabel(device) {
  const { label } = device || {};
  return typeof label === "string" ? label : "";
}

/**
 * The kind of one device, from the label the browser reports: the closed set the call-end
 * record carries (`AudioRoute` in src/voice_health.rs). Bluetooth is matched FIRST, because a
 * Bluetooth headset's label also says "headset".
 */
function classifyAudioDevice(label) {
  const words = String(label || "").toLowerCase();
  if (/bluetooth|airpods|hands[ -]?free|\bhfp\b|\bsco\b|\bble\b/.test(words)) return "bluetooth";
  if (/usb/.test(words)) return "usb";
  if (/speaker/.test(words)) return "speakerphone";
  if (/earpiece|receiver/.test(words)) return "earpiece";
  if (/headset|headphone|earbud|earphone/.test(words)) return "wired_headset";
  return "other";
}

/**
 * The capture device a call should open, from `devices` (an `enumerateDevices()` list):
 * the reader's stored choice when it is still there; else a Bluetooth/headset input, so the
 * call's communication route follows the headset; else the browser's "communications" device;
 * else null, meaning keep the stream the default constraints already opened.
 */
function pickCallInput(devices, prefs) {
  const inputs = (devices || []).filter((d) => d && d.kind === "audioinput" && d.deviceId);
  if (prefs.inputId) {
    const chosen = inputs.find((d) => d.deviceId === prefs.inputId);
    if (chosen) return chosen;
  }
  return inputs.find((d) => classifyAudioDevice(deviceLabel(d)) === "bluetooth")
    || inputs.find((d) => d.deviceId === "communications" || /communications/.test(deviceLabel(d).toLowerCase()))
    || null;
}

/**
 * Whether this browser lets the CALL choose an output: call playback is Web Audio, so the
 * form that matters is `AudioContext.setSinkId` (desktop Chromium). A media-element-only
 * `setSinkId` could not aim the call, so it does not earn the picker.
 */
function outputSinkSupported() {
  const Context = browserPorts().audio.contextClass();
  return Boolean(Context && Context.prototype && "setSinkId" in Context.prototype);
}

/**
 * Open the call's microphone: the default constraints first (which is also what grants the
 * permission that lets `enumerateDevices` name anything), then — when a better capture device
 * is present — once more with that device named exactly. `session.stream`'s callers see only
 * the final stream; the first is stopped before the second opens.
 */
async function openCallMicrophone(mic) {
  let stream = await browserPorts().microphone.open(audioConstraints(mic));
  audioRoute.begin();
  const trackOf = (opened) => (opened && typeof opened.getTracks === "function" ? opened.getTracks()[0] : null);
  const noteTrack = (opened) => {
    const track = trackOf(opened);
    let settings = {};
    try {
      settings = track && typeof track.getSettings === "function" ? track.getSettings() : {};
    } catch (_error) {
      settings = {};
    }
    const label = deviceLabel(track) || deviceLabel(settings);
    audioRoute.noteInput(label);
  };
  noteTrack(stream);
  if (!navigator.mediaDevices || typeof navigator.mediaDevices.enumerateDevices !== "function") return stream;
  let devices = null;
  try {
    devices = await navigator.mediaDevices.enumerateDevices();
  } catch (_error) {
    return stream;
  }
  const chosen = pickCallInput(devices, audioDevices.preferences());
  const current = trackOf(stream);
  let currentId;
  let currentLabel = "";
  try {
    const settings = current && typeof current.getSettings === "function" ? current.getSettings() : {};
    currentId = settings.deviceId;
    currentLabel = deviceLabel(current) || deviceLabel(settings);
  } catch (_error) {
    currentId = undefined;
  }
  if (!chosen || chosen.deviceId === currentId
    || (deviceLabel(chosen) && currentLabel && deviceLabel(chosen) === currentLabel)) return stream;
  const fallback = stream;
  const constraints = audioConstraints(mic);
  constraints.audio.deviceId = { exact: chosen.deviceId };
  try {
    stream = await browserPorts().microphone.open(constraints);
  } catch (_error) {
    // The chosen device went away between listing and opening: the stream already open
    // carries the call rather than the call dying on a device race.
    return fallback;
  }
  for (const track of fallback.getTracks()) track.stop();
  noteTrack(stream);
  audioRoute.settleInput(deviceLabel(chosen));
  return stream;
}

/**
 * Aim the call's playback where the reader chose, where the browser allows it at all. On
 * Android Chrome neither sink API exists, so this does nothing there BY DESIGN: the output
 * follows the communication route `openCallMicrophone` set up. Read-aloud never comes here.
 */
async function applyCallOutput(context) {
  if (!context || typeof context.setSinkId !== "function") return;
  const prefs = audioDevices.preferences();
  if (!prefs.outputId) return;
  try {
    await context.setSinkId(prefs.outputId);
    audioRoute.noteSink(String(context.sinkId || prefs.outputId));
  } catch (_error) {
    // The chosen output went away between Settings and the call: the default carries it,
    // and the record keeps the route actually in use rather than the choice that failed.
    return;
  }
  try {
    const devices = await navigator.mediaDevices.enumerateDevices();
    const chosen = (devices || []).find((d) => d.deviceId === prefs.outputId);
    if (chosen) audioRoute.noteOutput(deviceLabel(chosen));
  } catch (_error) {
    // No list, no kind: the readout says the sink id instead.
  }
}

// --- the control bar ------------------------------------------------------------------------
//
// `#58 control-bar`. The strip carrying the gear and the Voice/Discord switch, and — from `#59
// text-entry-button` and `#60 canned-prompt-buttons` onward — every other small button.
//
// ONE bar, TWO possible parents. `#control-bar-top` sits in the header and `#control-bar-bottom`
// sits in the dock directly above the big buttons; `barPlacement.set` moves the single element between
// them. Two copies would be two sets of ids, and the page would then have to keep two gears and
// two switches agreeing about which screen is up.
//
// The bar is declared in the BOTTOM mount in web/voice.html rather than being created here, so the
// default placement is a fact about the served markup: a page whose script dies still shows the
// controls where they belong.

/**
 * Where the bar is, where the reader last put it, and the moves between its two parents:
 * web/src/view/settings `mountBarPlacement`, in the words of web/src/present/settings (`#624
 * voice-page-modularity`).
 *
 * @type {import("./src/view/settings/index.ts").BarPlacement}
 */
const barPlacement = mountBarPlacement({
  lookup: (id) => el(id),
  storage: () => deviceStorage(),
  renderControlBar: () => renderControlBar(),
});

/** What is on the bar right now is web/src/view/shell's now (`createControlBar`, `#624`). */
const controlBar = createControlBar({
  el: (id) => el(id),
  screen: () => screens.screen(),
  view: () => screens.view(),
  actionMenuOpen: () => actionMenu.open(),
  renderActionMenu: () => actionMenu.render(),
  screenHasTitle: (screen) => Object.prototype.hasOwnProperty.call(SCREEN_TITLES, screen),
  readAloudAgent: () => readAloudOwner().agentReadAloud,
  columnsOffered: () => columnsOffered(),
  promptsOpen: () => promptsTray.open(),
  setPromptsOpen: (open) => setPromptsOpen(open),
  setActionMenuOpen: (open) => setActionMenuOpen(open),
  searchOpen: () => searchField.open(),
  renderLinkKinds: () => renderLinkKinds(),
  barWhere: () => barPlacement.where(),
});
const { renderControlBar } = controlBar;

// PACK_VIEWS and what is on the bar right now are web/src/view/shell's now (`createControlBar`,
// `#624 voice-page-modularity`).

// --- the reading column ---------------------------------------------------------------------
//
// `#55 voice-desktop-app`. On a wide screen the page holds its two lists to a column instead of
// letting them fill the window, and the reader chooses how wide that column is. Two controls set
// the one value — the handle on the edge of the column and the slider in Settings — because the
// handle needs a mouse and a keyboard needs something it can tab to.
//
// WHICH LAYOUT IS IN FORCE IS DECIDED ENTIRELY IN CSS, by `@media (min-width: 900px) and
// (pointer: fine)` at the foot of web/voice.css. Nothing here asks `matchMedia`, and nothing here
// looks at a user-agent string: this code only ever sets a number, and the stylesheet decides
// whether that number means anything. That is what keeps one regime rather than two that can
// disagree, and it is why the handle can be wired unconditionally.
//
// The unit is CHARACTERS, not pixels. What makes a line hard to read is how many characters are on
// it, so that is what the reader is choosing and what gets stored.

/**
 * The reader's own type size for message text, and the − and + beside its slider (`#230
 * text-size-buttons`): web/src/view/settings `mountTextSize`, in the words of web/src/present/settings
 * (`#624 voice-page-modularity`). Kept on the device.
 *
 * @type {import("./src/view/settings/index.ts").TextSize}
 */
const textSize = mountTextSize({
  lookup: (id) => el(id),
  keep: (pct) => deviceSettings().keep(DEVICE_SETTINGS.msgScale, pct),
  keptAs: (pct) => deviceSettings().keptAs(DEVICE_SETTINGS.msgScale, pct),
});

// What was stored, putting a count on the page (the root's `--collapsed-lines` and
// `data-collapsed-lines`) and keeping it, read back, are `collapsedLines`'s: web/src/core/settings
// (`#624 voice-page-modularity`).

// The count's work — recounting every drawn fold under it, and putting a new one on between
// the scroll keepers — is web/src/view/channel's now (`createReadingColumn`, `#624`).

// --- thread card lines (#522 thread-card-max-lines) -----------------------------------------------
//
// The owner, 2026-10-25: "I HATE having big thread cards... So if the message-root is bigger than a
// certain size (let's say K lines, with default K set to 5 lines), let's summarize it even with a
// single-response and push the root message inward to be the first child. If summarization is
// broken/disabled then I still want to SPLIT this over-large message. Put only the first K lines in
// the thread card and append "..." to the end. Then display the remainder of the message as a
// virtual first child underneath with "..." prepended."
//
//   * K IS A SETTING, Thread card lines under Messages, 1 to 12 and 5 until the owner says otherwise,
//     kept on the server (`#457 settings-server-side`) — which READS it: a root longer than K lines is
//     named from its first reply there, where a short one waits for a third message (`#484`).
//   * LINES BY A PROXY, the server's own (src/threads.rs, `CARD_LINE_CHARS`): each line of the
//     message that is not blank wrapped word by word at 36 characters, what a line of a phone's card
//     holds (measured in Chromium at 393x852; `wrapCardLine`). The server decides from it, and has no
//     screen; so the page decides whether a root is long by the same count of the same text — the
//     message's own `content` — and the two agree on which threads get a summary early. Where the
//     card is drawn wider, the same words take fewer lines: never more than about K.
//   * SUMMARISED, the root goes inward as the card's first child, exactly as under any summarised card
//     (`#405 root-under-summary`): nothing new.
//   * NOT SUMMARISED — summaries off, failing, or not back yet — THE ROOT IS SPLIT (`#528`: between
//     lines of its formatted body, `cardUnits`): the card says its first K lines and "…", and unpacked, the rest is
//     the first row under it, starting "…" (`cardSplit`). Not for one line left over, which the card
//     shows: a row of its own for one line is less compact than the line. That row is the root's own row, its words cut
//     to the rest: the same author and time, and no buttons of its own (web/voice.css), so a reply,
//     a reaction, Done or a receipt from it is the root's. It is kept by Hide read and never folded
//     into a run, as a summarised card's root child is, and when the summary lands the card simply
//     says it, the root child taking the continuation's place, the card held where it stands
//     (`keepingCardPlace`).

// `#529 page-build-stale-notice`. The owner, 2026-10-26, of a thread card drawn the old way: "Maybe
// I'm running a stale version". A launch always runs the deployed page — the shell is revalidated
// every time (`#223 asset-caching`) — but a page left open across a deploy runs its old script until
// it is reloaded, and nothing on it said so. Now every timeline read says which page script the
// server serves (`page_build`, the content hash its shell names it by), and a page whose own script
// was loaded under another hash shows "New version — reload" on the line over the list, and says
// both builds in Settings' Server section. A press reloads; nothing reloads by itself, so no call,
// draft or reading place is ever taken away unasked.

/**
 * This page's script's content hash: the hash in its name when the shell loaded the page build's
 * script, `/assets/voice.<hash>.js` (`#624 voice-page-modularity`), else the `v=` of the address an
 * older shell loaded it from, `/voice.js?v=<hash>`; "" without either.
 */
const PAGE_BUILD = (() => {
  try {
    const script = /** @type {HTMLScriptElement | null} */ (document.currentScript);
    const src = script && typeof script.src === "string" ? script.src : "";
    const found = /\/assets\/voice\.([0-9a-f]{32})\.js(?:[?#]|$)/.exec(src) || /[?&]v=([0-9a-f]+)(?:&|#|$)/.exec(src);
    return found ? found[1] : "";
  } catch (_error) {
    return "";
  }
})();

/**
 * What the server has said about itself — the page build it serves, the scope it saw on the saved token,
 * whether it names threads, and whether its `/client-config` has answered in this page: one owner in
 * web/src/core/api/serverfacts.ts (`#624 voice-page-modularity`).
 */
const serverFacts = createServerFacts();

/** Take what read `payload` says of the server's page build, and draw what it means for this page. */
function noteServerPageBuild(payload) {
  const build = payload && typeof payload.page_build === "string" ? payload.page_build : "";
  if (!serverFacts.notePageBuild(build)) return;
  settingsReadouts.pageBuild();
}

/** Whether the server serves a newer page than this one runs: both builds known, and different. */
const pageOutOfDate = () => PAGE_BUILD !== "" && serverFacts.pageBuild() !== "" && serverFacts.pageBuild() !== PAGE_BUILD;

/** The device's copy of K, kept on the server for every device (`#457 settings-server-side`). */
const CARD_MAX_LINES_KEY = DEVICE_SETTINGS.cardMaxLines.key;

/**
 * K in force, with what was stored, putting K on the page and keeping it: one owner in
 * web/src/core/settings/messagelimits.ts (`#624 voice-page-modularity`). Its `apply` is K's only writer:
 * the slider says it, and a K that changed draws every card list again by it (`redraw`).
 *
 * @type {import("./src/core/settings/index.ts").CardMaxLines}
 */
const cardMaxLines = createCardMaxLines({
  deviceSettings: () => deviceSettings(),
  // `#528 thread-card-markdown-and-cap`: what a card clamped to K lines is clamped to (web/voice.css).
  render: (n) => messagePreferences.cardMaxLines.render(n),
  redraw: () => {
    cardsDrawn.forget();
    threadCardsDrawn.clear();
    if (sideColumnUp()) renderSideColumn();
    if (selection().view === "thread") renderThreadHeading();
    if (selection().view === "threads") el("thread-list").replaceChildren(...threadCards(timelineRows().threads));
  },
});

/** The device's copy of the share, kept on the server for every device (`#457 settings-server-side`). */
const QUOTE_READ_PERCENT_KEY = DEVICE_SETTINGS.quoteReadPercent.key;

/**
 * The share in force, a percentage, with what was stored and putting a share on the page: one owner in
 * web/src/core/settings/messagelimits.ts (`#624 voice-page-modularity`). Its `apply` is the share's only
 * writer; Settings' slider draws it, and keeps a changed share.
 *
 * @type {import("./src/core/settings/index.ts").QuoteReadPercent}
 */
const quoteReadPercent = createQuoteReadPercent({
  deviceSettings: () => deviceSettings(),
  render: (n) => messagePreferences.quoteReadPercent.render(n),
});


/** A summary on its way is owed a read: the Threads list read now, past its spacing, and the cards drawn again. */
async function summaryWatchFired() {
  if (screens.view() !== "discord") return;
  threadDirectoryReadAt.delete(String(selectedChannelId()));
  await refreshThreadDirectory();
  renderThreadHeading();
  renderSideColumn();
  // A thread open across the window: its root row is the card's inward child or not by the card's layout.
  if (selection().view === "thread") renderReplyLinks();
}

// Thread card lines and how much a quote must cover are web/src/view/channel's now
// (`createCardLines`, `#624 voice-page-modularity`): where a card stops, its head and
// continuation, summaries on their way, and what a reply marks read. K and the share stay
// their settings owners', instantiated above and handed in.
const cardLinesOwner = createCardLines({
  cardMaxLines,
  quoteReadPercent,
  bodies: () => messageBodies,
  rawTextOf,
  headingOf,
  threadNames: () => serverFacts.threadNames(),
  cardIsSummarized: (choice) => cardIsSummarized(choice),
  pendingInto,
  heldMessages: () => channelStore.current().messages,
  threadOf,
  threadWords: (choice) => threadWords(/** @type {any} */ (choice)),
  threadName: (choice, chars) => threadName(/** @type {any} */ (choice), chars),
  setTimer: (run, ms) => setTimeout(run, ms),
  clearTimer: (timer) => clearTimeout(/** @type {any} */ (timer)),
  guardQuietly,
  view: () => screens.view(),
  selectedChannelId,
  forgetCards: () => cardsDrawn.forget(),
  clearThreadCards: () => threadCardsDrawn.clear(),
  redrawCards: () => {
    if (sideColumnUp()) renderSideColumn();
    if (selection().view === "thread") renderThreadHeading();
    if (selection().view === "threads") el("thread-list").replaceChildren(...threadCards(timelineRows().threads));
  },
  keepingCardPlace,
  savedLines: (n) => messagePreferences.cardMaxLines.saved(n),
  stepLines: (direction) => messagePreferences.cardMaxLines.step(direction),
  quotePercentChanged: (value) => messagePreferences.quoteReadPercent.changed(value),
  stepPercent: (direction) => messagePreferences.quoteReadPercent.step(direction),
  renderedText,
  summaryWatchFired,
});
const {
  cardMaxLinesChanged, stepCardMaxLines, quoteReadPercentChanged, stepQuoteReadPercent,
  replyMarksRead, rootOverflowsCard, cardCut, cardHead, continuationOf, cardPending,
  cardView, rootInward, cardSplit, cutForCard, watchPendingSummaries,
} = cardLinesOwner;

/**
 * Ctrl+( and Ctrl+) (Cmd on a Mac): one collapsed line fewer or more, and the status line says the
 * count it came to. Never in a text field, where the chord is the field's — a bracket typed, and
 * on some layouts a brace — so the key is left alone there and the browser keeps it; everywhere
 * else the chord is taken, at the ends of the range too, so it is never the browser's zoom
 * instead. A held chord steps on, as holding a zoom would, and stops at the end.
 *
 * @param {1 | -1} direction
 * @param {HTMLElement} target
 */
function collapsedLinesFromKeyboard(direction, target) {
  if (isTextField(target)) return false;
  const n = collapsedLinesChanged(collapsedLines.current() + direction);
  setStatus(`Collapsed messages show ${collapsedLinesWords(n)}.`);
  return true;
}

/** What was stored, clamped. A missing or corrupt entry is the default, never an error. */
function storedMsgScale() {
  return deviceSettings().get(DEVICE_SETTINGS.msgScale);
}

/** What was stored, clamped. A missing or corrupt entry is the default, never an error. */
function storedReadingWidth() {
  return deviceSettings().get(DEVICE_SETTINGS.readingWidth);
}

/**
 * The reading column's width, from Settings' slider or the handle on the column's edge, its drag and
 * its keys: web/src/view/settings `mountReadingWidth`, in the words of web/src/present/settings (`#624
 * voice-page-modularity`). Kept on the device; the stylesheet decides whether it means anything.
 *
 * @type {import("./src/view/settings/index.ts").ReadingWidth}
 */
const readingWidthControl = mountReadingWidth({
  lookup: (id) => el(id),
  keep: (ch) => deviceSettings().keep(DEVICE_SETTINGS.readingWidth, ch),
  keptAs: (ch) => deviceSettings().keptAs(DEVICE_SETTINGS.readingWidth, ch),
});

/**
 * When the page was last hidden, and so how long the reader was away (`#203 incremental-refresh`): one owner
 * in web/src/core/sync/away.ts (`#624 voice-page-modularity`).
 *
 * @type {import("./src/core/sync/index.ts").AwayClock}
 */
const awayClock = createAwayClock();

function onVisibility() {
  voiceCall().syncScreenWakeLock();
  if (document.visibilityState === "hidden") {
    awayClock.hide(Date.now());
    // Only a call can be suspended. Hiding an idle page is not an event.
    if (voiceCall().session.socket) {
      voiceCall().hiddenDuringCall = true;
    }
    // `#226 page-energy-profile`. Nor is refreshing a channel nobody can see.
    pauseWhileHidden();
    return;
  }
  voiceCall().visibleAt = Date.now();
  const away = awayClock.show(voiceCall().visibleAt);
  // `#195 send-resilience`. A phone holds a hidden page's timers; a send whose wait ran out while
  // the reader was away goes now, not a further wait after they are back.
  resumeOutgoingRetries();
  // `#226 page-energy-profile`. What the stream delivered while the page was hidden is drawn now,
  // from the store, and the arrows a hidden page did not measure are measured.
  const owed = catchUpHiddenArrivals();
  if (replyArrows().owed) measureReplyArrows();
  inOtherColumn(() => {
    if (replyArrows().owed) measureReplyArrows();
  });
  const resume = hiddenStream.takeResume();
  const pollDue = channelPoll.paused() && channelPoll.dueAt() <= voiceCall().visibleAt;
  // `#203 incremental-refresh`. Coming back to the channel after a while is the moment a reader
  // most wants it current, and a phone held the poll's timer the whole time it was away. Only what
  // changed is asked for, so this costs one small read rather than the next poll's wait. `#226`:
  // and after any absence the stream delivered something in, the poll came due in, or that let the
  // stream go.
  if ((away >= VISIBLE_REFRESH_MS || owed !== null || pollDue || resume !== null) &&
      screens.view() === "discord" && providerFacts.threading() && serverFacts.configApplied() && selectedChannelId()) {
    refreshQuietly(async () => {
      try {
        if (owed === "mutation") refreshAfterLiveMutation();
        else {
          const since = replayLedger.started();
          await timelineView.loadDiscord({ keepPosition: true, reason: "visible" });
          await readSideColumn("visible", since);
        }
      } finally {
        // The stream comes back once that read has brought what it missed, so what it replays from
        // the last event it saw is held already rather than drawn a row at a time.
        resumeLiveStream(resume);
      }
    })();
    resumePolling("fresh");
    return;
  }
  // An edit the stream delivered is read for wherever the reader is, as it was when it arrived.
  if (owed === "mutation") refreshAfterLiveMutation();
  resumeLiveStream(resume);
  resumePolling(pollDue ? "now" : "rest");
}
voiceCall().initializePageInstance();


// --- the offline message cache -------------------------------------------------------------------
//
// `#18 offline-message-cache`. A reload, a phone reclaiming the tab, or a cold start of the
// installed app used to open on an empty channel that filled only when the network answered — and
// never, offline. The page now keeps a bounded snapshot of what it last showed, per signed-in
// identity, channel, view and thread, and draws it BEFORE any request completes. The server stays
// the authority: every snapshot is refreshed in the background and merged by message id, and until
// that refresh succeeds the screen says how old the rows it is showing are.
//
// localStorage, deliberately, rather than IndexedDB. It is synchronous, so the snapshot is drawn in
// the same task that runs this script instead of after an open-and-upgrade round trip; the drafts
// and the outbox already live there; and a few hundred messages fit well inside the bounds below.
// CacheStorage is never used for any of it: a response kept there outlives sign-out and is served
// by whatever a service worker decides, which is why the API answers `Cache-Control: no-store`.
//
// ONE key holding one envelope, always read, changed and written in one go and never mirrored in a
// variable: another tab, or the main app on this origin, can replace the token underneath this
// page, and a copy held in memory would write the previous identity's messages straight back. The
// rows on SCREEN are such a copy, so they carry the identity that drew them, and a page that finds
// the token changed underneath it clears them before it reads, draws or saves anything.
//
// THE IDENTITY is a fingerprint of the saved token, never the token itself. It labels which
// credential read these rows, and anything that disagrees with it — another token, no token, a
// refusal — deletes the whole envelope rather than filtering it. Messages that credential can no
// longer read must not stay on the device.
const messageCache = createOfflineCache({
  storage: () => deviceStorageOnUse(),
  token: () => token,
  channels: () => channelDirectory.channels(),
  stillToDo: (message, done) => stillToDo(message, /** @type {any} */ (done)),
}).cache;

/**
 * A column's own record of its reads' state (`#624 voice-page-modularity`, plan step 15, as
 * `freshOlderPages`): `freshness` and `freshAt` and `refreshing` below; `lineTimer`, the loading line's
 * timer (`renderChannelLoading`); `generation`, bumped by each read the column starts, so a completed
 * read can tell whether it still describes the column (`currentDiscordLoad`).
 *
 * @returns {{freshness: string, freshAt: number, refreshing: boolean, lineTimer: any, generation: number}}
 */
function freshLoadState() {
  return { freshness: "fresh", freshAt: 0, refreshing: false, lineTimer: null, generation: 0 };
}

/**
 * A column's own record of where its list stands against its newest message (`#624
 * voice-page-modularity`, plan step 15): `jumpWanted`, the arrival its jump is for;
 * `inScrollback`, whether it is in its scrollback. The folds owner reads them
 * (web/src/view/channel/folding.ts).
 *
 * @returns {{jumpWanted: boolean | number, inScrollback: boolean}}
 */
function freshScrollPlace() {
  return { jumpWanted: false, inScrollback: false };
}

/**
 * A column's own record of its scroll tools' pass: `held`, how many redraws are under way that
 * pass over the scroll tools once, at their end; `owed`, whether a pass was asked for inside one.
 *
 * @returns {{held: number, owed: boolean}}
 */
function freshScrollTools() {
  return { held: 0, owed: false };
}

/** The column being worked on's record of where its list stands (`freshScrollPlace`). */
function scrollPlace() {
  return columnWork.active().place;
}

/** The column being worked on's record of its scroll tools' pass (`freshScrollTools`). */
function scrollTools() {
  return columnWork.active().tools;
}

/** The column being worked on's record of its reads' state (`freshLoadState`). */
function loadState() {
  return columnWork.active().load;
}

/*
 * How the channel rows on screen relate to the server: "fresh" from a read in this page, "saved"
 * from the device and not yet refreshed, or "offline"/"failed" when the refresh did not succeed —
 * "slow" when it failed because the chat service did not answer in time.
 **/
// Kept per column: `loadState().freshness` (`freshLoadState`, `#624 voice-page-modularity`).
// When the rows on screen were last known to be current, or 0 when there are none. Kept per column: `loadState().freshAt` (`freshLoadState`, `#624 voice-page-modularity`).
/*
 * Whether a read of the channel is on the wire, which the freshness pill says ("· refreshing…").
 * Its own state rather than read off the loading line: a read behind rows a view switch has already
 * drawn shows no loading line. `#220 view-switch-instant`.
 **/
// Kept per column: `loadState().refreshing` (`freshLoadState`, `#624 voice-page-modularity`).
// Whether `/client-config` has answered in this page, as opposed to the saved shell standing in, is the server
// facts' (`ServerFacts.configApplied`).

/** The fingerprint of the token the channel rows on screen were read or drawn for (web/src/core/sync `createScreenIdentity`). */
const screenIdentity = createScreenIdentity(tokenFingerprint(token()));

// --- one store per channel, and the views projected from it ---
//
// Main, Threads, All and each thread are FOUR ANSWERS FROM ONE CHANNEL, and the page holds the
// channel once: every message any view has read, by id, in time order, and every thread summary.
// A view is a projection of that store — Main is what was posted to the channel directly, thread
// roots included; All is everything; a thread is its own messages — so switching view is local
// work, a live arrival reaches every view at once, and a correction read in one view is the row
// every other view shows.
//
// A PROJECTION NEVER INVENTS COVERAGE. Each view remembers how far back its OWN pages reached, and
// shows the store only from there up: Main's newest page does not prove anything about the thread
// replies between its rows, so All is not drawn from it. A view no page has covered is read.
//
// NOR DOES A LIVE ARRIVAL INVENT MEMBERSHIP. `#185 reply-new-thread`. A provider's stream may carry
// a message without saying which thread it is in — the Google Chat bridge pushes every message in
// that shape — so a copy without a thread record is not evidence that the message is on the main
// channel. `unplaced` holds the ids that arrived that way and that no page has carried since: a
// reply to one of them waits for a read rather than guessing the main channel.
//
// SUCH A COPY IS DRAWN ON MAIN ONLY UNTIL A READ PLACES IT. `#308 live-reply-in-main`. Most of what
// arrives that way was posted to the channel itself, and drawing it nowhere until a read lands would
// hold every one of those back for as long as the read takes. But a thread reply arrives that way too,
// and the read behind it is of the view on screen: Main's carries no thread reply, so the reply stayed
// in Main and out of its thread until a reload. So the page reads All for every message the stream
// left unplaced that the read behind it did not place (`placeUnplaced`), and the moment any later copy
// — that read, a delta, a page of any view, another live copy — names the message's thread, it leaves
// Main for its thread in every column, the counts and cards with it, and the reader stays where they
// are reading (`followPlacement`).

/**
 * The selected channel's store, held by its owner (web/src/core/channels `createChannelStore`, `#624
 * voice-page-modularity`): `channelStore.current()` is the store, and opening another channel starts
 * its store afresh. Here where the variable it replaces stood.
 */
const channelStore = createChannelStore();

// `emptyCanon`, `viewOfKey`, `timeOf`, `inTimeOrder`, `threadOf` and `sameMessage` are
// web/src/core/channels's (`#624 voice-page-modularity`), with the store's shape (`ChannelCanon`).

const viewKey = (view = selection().view, thread = selection().threadId) => (view === "thread" ? `thread:${thread}` : view);


// `mergeIntoCanon` and `upsertThreadSummary` are web/src/core/channels's, and take the store they
// fold into (`#624 voice-page-modularity`).





/** The delta pages `foldTimelineDelta` found changed nothing. `#226 page-energy-profile`. */



/** The read state of the rows on screen is the store's read state for those rows. */
function syncCanonDismissed() {
  if (!providerFacts.threading() || channelStore.current().channel !== String(selectedChannelId())) return;
  // `#407 unpack-row-parity`. Over the cards the right column has no rows on screen to speak for.
  if (columnWork.side() && columns.side.mode !== "thread") return;
  for (const message of timelineRows().messages) {
    const id = String(message.id);
    if (doneState().archived.has(id)) channelStore.current().dismissed.add(id);
    else channelStore.current().dismissed.delete(id);
  }
}






/**
 * Record what the channel holds. Called after every read that succeeded and after every local
 * change to the rows — a live arrival, an acknowledged send, an archive — so a reload shows what
 * the reader last saw.
 *
 * NOT in the legacy to-do filter: that list is `/todo`, a filtered answer, and saving it as the
 * channel would show a reload a channel with its dealt-with messages missing.
 */
function saveChannelScope() {
  if (!selectedChannelId() || (readModes.hiding() && !providerFacts.threading()) || !screenBelongsToToken()) return;
  const cache = messageCache.readMessageCache();
  if (!cache) return;
  const key = String(selectedChannelId());
  const now = Date.now();
  /** @type {import("./src/core/channels/index.ts").CacheEntry | null} */
  let entry = null;
  if (providerFacts.threading()) {
    if (channelStore.current().channel !== key) return;
    syncCanonDismissed();
    // The store as the device keeps it (web/src/core/channels `savedTimelineOf`).
    entry = savedTimelineOf(channelStore.current(), now, bookmarkToSave(key));
  } else {
    // In time order, gathered replies or not (`#215 reply-coalesce`): a reload draws these as they
    // are saved, and nothing it draws is gathered.
    const messages = listedMessages();
    const shown = new Set(messages.map((message) => String(message.id)));
    entry = {
      mode: "page",
      savedAt: loadState().freshAt || now,
      usedAt: now,
      messages,
      threads: [],
      more: olderPages().moreAbove !== false,
      cursor: null,
      // `#578 search-status-clarity`: where the rows saved stop being one stretch, if they do.
      gapFrom: plainReach.gapFrom(),
      dismissed: [...doneState().archived].filter((id) => shown.has(id)),
      bookmark: bookmarkToSave(key),
    };
  }
  messageCache.trimCacheEntry(entry, MESSAGE_CACHE_ROWS);
  cache.scopes[key] = entry;
  messageCache.writeMessageCache(cache, key);
}

/**
 * Draw the selected channel's saved rows, if this device has them: the store's projection of the
 * current view in timeline mode, the saved page in legacy mode.
 */
function hydrateChannelScope() {
  const channel = selectedChannelId();
  if (!channel || (readModes.hiding() && !providerFacts.threading())) return false;
  screenBelongsToToken();
  if (providerFacts.threading()) {
    if (channelStore.current().channel !== String(channel)) loadCanon(channel);
    inheritAllCover(selection().view, selection().threadId, selection().thread);
    return drawProjection();
  }
  const cache = messageCache.readMessageCache();
  const key = String(channel);
  const entry = cache ? cache.scopes[key] : undefined;
  if (!entry || entry.mode !== "page") return false;
  entry.usedAt = Date.now();
  messageCache.writeMessageCache(cache, key);
  restoreSavedBookmark(key, entry.bookmark);
  const payload = {
    channel: knownChannel(channel),
    messages: entry.messages,
    has_more: entry.more,
    next_before: entry.cursor || null,
    dismissed: entry.dismissed,
  };
  try {
    el("discord-log").replaceChildren();
    const loaded = applyNewestPage(/** @type {any} */ (payload), true);
    plainReach.restore(entry.gapFrom);
    renderChannelSeam(channelSummary(loaded, olderPager.loadedIsWhole(olderPages().moreAbove), channelName(payload.channel)));
  } catch (_error) {
    clearChannelScreen();
    messageCache.forgetChannelScopes((id) => id === key);
    return false;
  }
  const rows = entry.messages;
  listState().newestId = rows.length > 0 ? String(rows[rows.length - 1].id) : null;
  loadState().freshAt = entry.savedAt;
  setChannelFreshness("saved");
  renderOutgoingMessages();
  return true;
}

/**
 * Before `/client-config` answers: put the channel picker and the saved rows up from the shell, so
 * a reload opens on the application rather than on the sign-in form and an empty channel.
 */
function hydrateFromCache() {
  const cache = messageCache.readMessageCache();
  if (!cache || !cache.shell) return false;
  const shell = cache.shell;
  providerFacts.setThreading(shell.threading_supported === true);
  channelDirectory.setCapability("threading", providerFacts.threading());
  providerFacts.setBroadcast(shell.broadcast_supported === true);
  channelDirectory.setCapability("broadcast", providerFacts.broadcastSupported());
  channelDirectory.setChannels(shell.channels);
  learnProviders(shell.providers);
  whoIsWho.setDeploymentCodingAgents(shell);
  if (shell.self_author_id) noteSelfAuthor(shell.self_author_id);
  whoIsWho.setOwnerAuthorId(shell.owner_author_id);
  whoIsWho.learnOwnerIds(shell.owner_author_ids);
  fillChannelSelect("discord-channel");
  // `#355 provider-grouped-channels`. Settings by service, from the saved shell as from client-config.
  channelSettings.fillServiceSelect();
  fillChannelSelect("settings-channel");
  const provider = providerOfChannel(selectedChannelId());
  if (provider) { providerFacts.setThreading(provider.threading_supported === true); providerFacts.setBroadcast(provider.broadcast_supported === true); }
  channelSettings.renderChannelBox();
  // Before the composer and the rows: both are the view's. `#189 restore-ui-state`.
  openChannelView();
  restoreChannelComposer();
  hydrateChannelScope();
  return true;
}

/** Take every channel row off the screen and out of memory. */
function clearChannelScreen() {
  // From the left column, whichever column's read found the credential refused. `#229`.
  if (columnWork.side()) return inColumn(columns.main, clearChannelScreen);
  ++loadState().generation;
  readAloudOwner().stopReading();
  readAloudOwner().readingMode = false;
  channelContexts.clear();
  channelStore.open();
  timelineRows().messages = [];
  timelineRows().threads = [];
  doneState().archived = new Set();
  olderPages().moreAbove = false;
  olderPages().cursor = null;
  channelReads.payCursor();
  listState().newestId = null;
  el("discord-log").replaceChildren();
  el("thread-list").replaceChildren();
  el("channel-summary").replaceChildren();
  el("timeline-notice").hidden = true;
  // The pins this credential read go with the rest of what it read. The bookmarks are kept: they
  // are the server's, read again with the channel, and a write of one still on its way is kept track
  // of for the read that follows (`noteBookmark`). Only another credential forgets them
  // (`forgetBookmarks`). The rows the line was drawn on are gone.
  forgetPins();
  readingBookmark.forgetDrawn();
  el("pinned-log").replaceChildren();
  renderOlderControl();
  loadState().freshAt = 0;
  setChannelFreshness("fresh");
  screenIdentity.claim(tokenFingerprint(token()));
  // `#229 desktop-two-column`. What the right column read goes with the rest, and the threads selected
  // there, which this credential read.
  sideThreads.clear();
  resetSideColumn();
}

/**
 * Whether the rows on screen were read for the token saved now. When another tab or the main app
 * has replaced it, they are cleared here — false — so they can be neither saved under the new
 * identity nor merged into its first read.
 */
function screenBelongsToToken() {
  if (screenIdentity.belongsTo(tokenFingerprint(token()))) return true;
  clearChannelScreen();
  forgetBookmarks();
  // Reading the envelope deletes it when it, too, belongs to the previous token.
  messageCache.readMessageCache();
  return false;
}

/** Sign-out, another token, or a refusal: nothing this credential read may stay behind. */
function forgetMessages() {
  if (columnWork.side()) return inColumn(columns.main, forgetMessages);
  // `#277 reply-in-column`. A reply open in a column answers a message that is going; its words are
  // kept as a draft, as every reply put down is.
  putDockedReplyAway();
  messageCache.dropMessageCache();
  clearChannelScreen();
  forgetBookmarks();
  // `#892 voice-page-optimistic-done`. Nor the Done writes it kept to send again.
  inboxLedger.clear();
  // ...nor where the reader was in it: the record names a thread this credential read, and the
  // next one opens the channel as a first visit does. `#189 restore-ui-state`.
  uiState.forget();
  openChannelView();
  restoreChannelComposer();
}

/** A channel read failed. Say so over the rows it leaves standing — or take them away. */
function noteChannelReadFailure(error) {
  // `#195 send-resilience`. Whatever reports this — the banner, for a read the reader asked for —
  // says what it means rather than the server's diagnosis of it.
  if (error) error.message = readableChannelFailure(error);
  if (error && error.refused) {
    forgetMessages();
    return;
  }
  if (error && error.status === 404 && error.code === "unknown_channel") {
    const channel = String(selectedChannelId());
    messageCache.forgetChannelScopes((id) => id === channel);
    clearChannelScreen();
    return;
  }
  // With nothing on screen the error itself is the whole report.
  if (loadState().freshAt) {
    setChannelFreshness(error && error.network ? "offline" : isSlowFailure(error) ? "slow" : "failed");
    // `#195 send-resilience`. Said, over the rows it is about: a refresh nobody asked for needs no
    // banner on top of it. See `refreshQuietly`.
    if (error) error.shownInPill = true;
  }
}

function setChannelFreshness(state) {
  loadState().freshness = state;
  renderChannelFreshness();
}

/**
 * Over the top of the list rather than in it: a sentence at the head of the history scrolls away
 * with it, and the reader is usually at the bottom, which is exactly where "these are old" has to
 * be visible. Hidden while the pull-to-refresh pill is in the same place — a pull at the top. A
 * pull up from the newest line puts that pill at the foot of the list, and this one stays.
 */
function renderChannelFreshness() {
  // The pill floats over the left column and speaks for it. `#229 desktop-two-column`: the right
  // column's freshness is its own, said on a line of that column (`#231 two-column-polish`).
  if (columnWork.side()) {
    renderSideFreshness();
    return;
  }
  const pill = el("channel-freshness");
  const gesture = el("pull-refresh");
  const at = stamp(loadState().freshAt);
  const refreshing = loadState().refreshing;
  // CURRENT IS SAID TOO, not left blank: a blank pill reads the same as a page that never managed
  // to refresh, which is what a reader opening the app in the car could not tell apart.
  const current = loadState().freshAt
    ? `${liveFollow.attached() && providerFacts.liveDelivery() !== "off" ? "Live · updated" : "Updated"} ${at}` +
      (refreshing ? " · refreshing…" : "")
    : "";
  const text = staleWords(loadState().freshness, loadState().freshAt, refreshing, "messages", () => chatServiceOf(selectedChannelId())) || current;
  pill.textContent = text;
  pill.setAttribute("data-state", loadState().freshness);
  // It gives way to a pull at the TOP, which draws in the same place (a pull up at the foot leaves
  // "Offline" or "Refresh failed" standing, `#188 pull-refresh-bottom`), and — since `#197
  // floating-search` put the search glass on the same line — to the open search bar, which grows
  // along that line across the top of the list.
  pill.hidden = text === "" || screens.view() !== "discord" ||
    (!gesture.hidden && gesture.getAttribute("data-edge") !== "end") || searchField.open();
  reserveFreshnessRoom(text !== "");
}


/**
 * The right column's line saying how old its rows are: drawn only when it changed, and only when
 * they are not current. `#231 two-column-polish`. A thread says what its own reads have said, as
 * the pill says Main's; the cards say what the thread list's reads have said, by the newest that
 * landed — else by the left column's, whose store the cards are drawn from — and nothing once one
 * lands again.
 */
function renderSideFreshness() {
  const line = leftEl("side-freshness");
  let state = "fresh";
  let text = "";
  if (layoutShown.columns() && columns.side.mode === "thread") {
    [state, text] = inColumn(columns.side, () =>
      [loadState().freshness, staleWords(loadState().freshness, loadState().freshAt, loadState().refreshing, "messages", () => chatServiceOf(selectedChannelId()))]);
  } else if (sideColumnUp()) {
    const channel = String(selectedChannelId());
    state = threadList.threadListFailure(channel) || "fresh";
    const at = threadList.threadListLandedAt(channel) || inColumn(columns.main, () => loadState().freshAt);
    // With no time to give, the state alone, rather than a time nothing was read at.
    text = state === "fresh" ? "" : at ? staleWords(state, at, false, "threads", () => chatServiceOf(selectedChannelId()))
      : state === "offline" ? "Offline" : "Refresh failed";
  }
  setTextIfChanged(line, text);
  if (line.getAttribute("data-state") !== state) line.setAttribute("data-state", state);
  const hide = text === "";
  if (line.hidden === hide) return;
  // The line's height comes out of the list's below it, which browser scroll anchoring does not
  // hold: a reader of the thread keeps the line they are on, or the newest line (`holdingReader`).
  const toggle = () => {
    line.hidden = hide;
  };
  if (layoutShown.columns() && columns.side.mode === "thread") inColumn(columns.side, () => holdingReader(toggle));
  // `#261 two-column-connectors`: and a reader of the cards keeps theirs, the foot above all.
  else keepingCardPlace(toggle);
}

/**
 * `#32 freshness-pill-overlap`. The pill's height at the head of the channel's list, so that the pill
 * never covers the list's header. Keyed to there being something to say, not to the pill being
 * shown: giving way to a pull or to the other view must not shift the list.
 *
 * `#324 live-pill-covers-row`. The room is the list's top edge moved down to the floating line's
 * foot (web/voice.css), no longer padding at the head of the pane, which scrolled away and left the
 * pill over whatever row was passing. The attribute on the pane is still what asks for it.
 *
 * Through `holdingReader`, because the room is added ABOVE a reader who has scrolled down, and
 * browser scroll anchoring is suppressed for exactly this change — a change of the scroller's own
 * size above every row. Without it a failed refresh pushed the line being read, or the newest line,
 * down by the pill's height.
 */
function reserveFreshnessRoom(reserve) {
  const pane = el("pane-discord");
  if (pane.hasAttribute("data-freshness") === reserve) return;
  const toggle = () => {
    if (reserve) pane.setAttribute("data-freshness", "");
    else pane.removeAttribute("data-freshness");
  };
  // The pane is not on screen in the other view; the list that is must not be moved on its behalf.
  if (screens.view() === "discord") holdingReader(toggle);
  else toggle();
}

/**
 * The page drew a snapshot for one channel and mode; `/client-config` may describe another.
 *
 * `#189 restore-ui-state`. The view is decided again rather than reset to Main, so a channel the
 * saved shell called threadless whose provider has threads after all opens in All — here, once —
 * unless the reader chose a view. Nothing is read here: with the channel on screen, what asked for
 * sign-in reads the view now up next — a reopen's owed read, or the poll of a page that opened
 * offline — and a read of its own would be a second one.
 */
function reconcileChannelSnapshot(before) {
  if (before.key === channelContextKey() && before.threading === providerFacts.threading()) return;
  clearChannelScreen();
  openChannelView();
  threadPresence.forget();
  hydrateChannelScope();
  restoreChannelComposer();
  voiceCall().renderControls();
  // Threads or none, as sign-in says: one column or two decided again. `#229 desktop-two-column`.
  renderColumns();
  saveUiState();
}

// The record itself — read back from this window's storage and the device's, checked on the way back
// and the old version brought forward (UI_STATE_KEY, UI_STATE_VERSION, UI_STATE_CHANNELS,
// UI_STATE_CHARS, REOPENED_VIEWS) — the view chosen in each recent channel, whether what the page
// reopens on is settled, and the place last held in the list, are web/src/core/navigation's
// (`createUiState`, `#624 voice-page-modularity`); what is on screen, and writing the record, are here.
const uiState = createUiState({
  deviceStorage: () => deviceStorage(),
  windowStorage: () => windowStorage(),
  identity: () => tokenFingerprint(token()),
  viewKey: (view, thread) => viewKey(view, thread),
});

/** Which view a channel opens in: web/src/view/channel's now (`createOpeningView`, `#624`). */
const openingViewOwner = createOpeningView({
  uiState: () => uiState, providerFacts,
  channelStore: () => channelStore, threadPresence: () => threadPresence, columns: () => columns,
  readModes: () => readModes, screens: () => screens, columnWork: () => columnWork, messageCache: () => messageCache,
  reopenedViews: REOPENED_VIEWS, uiStateKey: UI_STATE_KEY, uiStateVersion: UI_STATE_VERSION, uiStateChars: UI_STATE_CHARS,
  captureScroll: () => captureScroll(), channelContextKey: () => channelContextKey(),
  deepLinkFromAddress: () => deepLinkFromAddress(), deepLinkReaderMoved: () => deepLinkReaderMoved(),
  el: (id) => el(id),
  idsOf: (row) => idsOf(/** @type {any} */ (row)), inColumn: (column, act) => inColumn(/** @type {any} */ (column), act),
  listPlaceTop: (area) => listPlaceTop(area), loadCanon: (channel) => loadCanon(/** @type {any} */ (channel)),
  renderColumns: () => renderColumns(), restoreChannelComposer: () => restoreChannelComposer(),
  restoreScroll: (mark) => restoreScroll(mark), scheduleBackSync: () => scheduleBackSync(),
  selectedChannelId: () => selectedChannelId(), selection: () => selection(),
  token: () => token(), tokenFingerprint: (value) => tokenFingerprint(value),
  upsertThreadSummary: (canon, summary) => upsertThreadSummary(canon, summary),
  viewKey: (view, thread) => viewKey(view, thread), visibleList: () => visibleList(),
  windowStorage: () => windowStorage(), deepLinks: () => deepLinks, sideColumnUp: () => sideColumnUp(),
});
const { windowItem, keepWindowItem, defaultChannelView, threadStillKnown, openingChannelView, openChannelView, holdChannelPlace, savedThreadSummary, viewOnScreen, saveUiState, returnToSavedPlace } = openingViewOwner;
/**
 * Reopen on the channel when that is where the reader left the page — once, as soon as there is a
 * channel to show: at load from the saved shell, or after sign-in when there was none. Its view was
 * decided already, by `openChannelView`; this enters it and puts the reader back on their message.
 * The view's one newest page is owed from here, and `payReopenRead` reads it.
 */
function reopenSavedView() {
  // `?v=docs` (with `&d=KEY` for one document) opens Documents (`documents-view-client`).
  // Before the settled guard: an explicit address outranks a saved place.
  if (documentsQueryValue("v") === "docs") {
    uiState.settle();
    showView("docs");
    void documentsView.show(documentsDeepLinkKey());
    saveUiState();
    return;
  }
  if (uiState.settled()) return;
  uiState.settle();
  const saved = uiState.saved();
  if (saved && saved.view === "discord" && selectedChannelId() && screens.view() !== "discord") {
    showView("discord");
    returnToSavedPlace();
    holdChannelPlace();
    reopenReadOwed.owe(replayLedger.started());
  }
  saveUiState();
}

/**
 * The read a reopen owes the channel it put on screen: one owner in web/src/core/sync/reopen.ts (`#624
 * voice-page-modularity`), which says why it waits.
 *
 * NOT READ AT LOAD, though the rows are drawn then. A read sent before `/client-config` answers
 * uses a token nobody has proved: refused, its failure lands over sign-in's own explanation of the
 * refusal and arms a poll behind the sign-in form. And a read sent before the live stream attaches
 * cannot answer for the stream's replay tail (`#43 replay-burst-double-read`): an edit out of that
 * tail — an agent's placeholder edited in place, on nearly every reopen — waits on it in vain, and
 * makes a second read of the same newest page.
 */
const reopenReadOwed = createReopenRead();

/**
 * Read what the reopen put on screen, once sign-in has answered: at once on a channel without
 * threads, whose replay is not accounted for by read, and on one with threads when the live stream
 * has attached. A refusal reads nothing, and leaves it owed to the token that replaces this one.
 */
function payReopenRead() {
  const owed = reopenReadOwed.take();
  if (!owed) return;
  const pay = () => {
    // Signed out while it waited: owed to the next sign-in, as after a refusal.
    if (!token()) {
      reopenReadOwed.keep(owed);
      return;
    }
    if (screens.view() !== "discord" || replayLedger.started() !== owed.after) return;
    // Merged into the rows the snapshot drew, keeping the reader's message, as a channel change
    // does; with nothing drawn it is a first visit and settles on the newest.
    readEnteredChannel(loadState().freshAt > 0);
  };
  if (providerFacts.threading()) whenLiveAttached(pay);
  else pay();
}

// --- channel views, threads and the channel composer ------------------------------------------
// Which view of the channel is shown, and how the switch is drawn, are web/src/view/channel's
// now (`createChannelViews`, `#624 voice-page-modularity`).

// --- entering a thread, on its first unread message ---------------------------------------------
//
// `#221 read-modes`. The owner: "When I see a message that has replies in a thread (the agents
// replies), I click into the thread to read the response. But I get to the new screen and the reply
// I'm reading is contracted. I would like the heuristic to instead be to expand the first unread
// message when jumping into the thread. This gives me exactly what I want to read."
//
// So entering a thread puts its first unread message on screen, under the floating line, and opens
// its fold — with the thread's root above it at the head of the list when both fit, since the root
// is what the reply answers. UNREAD is what both read modes call unread (`rowIsRead`): not Done, not
// the reader's own words, not a placeholder read automatically. A thread with nothing unread opens
// as it always has: on its newest message, folded.
//
// ONCE, ON ENTRY. The fold is opened through `setFolded`, so the message stays open across every
// redraw after — and a reader who folds it again has folded it for good, as with any message they
// fold. Nothing after the entry moves the reader or opens anything.

// Entering a thread, on its first unread message, is web/src/view/channel's now
// (`createThreadEntry`, `#624 voice-page-modularity`).

// The thread selector and older threads are web/src/view/channel's now (`createThreadList`,
// `#624 voice-page-modularity`): what threads the store knows and what they are called, the
// view picker and heading, opening and closing a thread, the walk back, the cards, and a row's
// thread decoration. Changing the view and the selection stay the page's, injected; the
// unanswered-thread group stays here until the reply-counts move.
const threadList = createThreadList({
  byId: el,
  leftById: leftEl,
  inThisColumn,
  inColumn,
  side: () => columnWork.side(),
  threading: () => providerFacts.threading(),
  currentCanon: () => channelStore.current(),
  selectedChannelId,
  selection: () => selection(),
  pendingThreadTimes,
  messageText: (m) => messageText(/** @type {any} */ (m)),
  timeOf: (m) => timeOf(m, "timestamp"),
  sideColumnUp: () => sideColumnUp(),
  renderSideColumn,
  threadListOffered,
  layoutFit: () => layoutShown.fit(),
  layoutColumns: () => layoutShown.columns(),
  layoutThreadList: () => layoutShown.threadList(),
  changeChannelView,
  defaultChannelView,
  chooseView,
  openThreadFromList,
  selectSideThread,
  closeSideThread,
  setLayout,
  paintCard: (row, choice, tally) => paintThreadCard(row, choice, /** @type {any} */ (tally)),
  fetchTimeline: (path) => apiDecoded("TimelineResponse", path),
  foldTimelineDelta: (p, sent, view, thread) => foldTimelineDelta(/** @type {any} */ (p), sent, view, /** @type {any} */ (thread)),
  foldTimelinePage: (p, o, view, thread) => foldTimelinePage(/** @type {any} */ (p), o, view, /** @type {any} */ (thread)),
  isCursorRefusal,
  refreshFailed,
  saveChannelScope,
  syncUnreadReplies: () => syncUnreadReplies(),
  renderSideFreshness,
  sideCardReplies,
  sideCardUnread,
  threadIsRead,
  cardOpener,
  toggleLitCard,
  paintLitCard,
  enterCard,
  cardMessageCount,
  searchable,
  threadHue: (id) => threadHue(String(id)),
  threadReplyCount,
  threadCount,
  repliesOf: (message) => threadReplies(message.id, message),
  paintThreadBadge,
  unansweredRoot,
  startThread,
  openReply: (messages, row) => openReply(messages, row),
  openReplyFromRow: (row) => openReply(rowMessages(row), row),
  sayRowShortcut,
  guardQuietly,
  toggleGathered,
  rootOverflowsCard: (root) => rootOverflowsCard(root),
  threadsViewKey: () => viewKey("threads"),
  setTextIfChanged,
  mainColumn: () => columns.main,
  replyTally,
  pageLimit: DISCORD_PAGE_LIMIT,
  currentComplete: () => channelStore.current().complete,
  reconcileMs: () => TIMELINE_RECONCILE_MS,
  withPendingThreads: (summaries) => withPendingThreads(/** @type {any} */ (summaries)),
  cardView: (choice) => cardView(choice),
  threadReplies: (id, message) => threadReplies(id, message),
});
const {
  noteThreadListLanded, threadWords, threadName, threadChoices, openThreadChoice,
  knownThreadChoice, openThreadName, olderThreadsOnServer, renderThreadSelect,
  renderThreadHeading, onThreadSelect, refreshThreadDirectory, openThread, closeThread,
  olderThreadsToRead, loadOlderThreads, olderThreadsFailure, threadCards, addThreadDecoration, gatherCount,
  noteNewestThreadPage,
} = threadList;

// --- older threads, from the thread list ---------------------------------------------------------
//
// `#194 thread-picker-polish` gave the picker "… Older threads", which opened a Threads screen: every
// thread the store knew, and "Load older" stepping back through the channel's thread list one server
// page at a time. `#281 view-keys` retired that screen — the owner, of it: "the duplicative threads
// view also doesn't respect the 1/3 keystrokes anyway to jump out of it" — for the thread list, the
// right column's cards across the window. Its paging is the cards' own now: "Older threads…" above
// the oldest card reads the next page back where the reader is, and the page folds into the
// channel's store like any other read, so the cards, the picker and the heading name what it found.

/**
 * The walk back through one channel's thread list: the server's `next_before` for the next older page
 * (null with none known yet, or none left), whether a read is out, and how the last one failed.
 */

/**
 * Say that a read is on the wire, or that none is. `quiet` is a read behind rows already on screen
 * for a view switch, `#220 view-switch-instant`: the pill says it is refreshing and the list shows
 * no loading line, which over rows the reader is already reading would only say "wait".
 *
 * `delayed` is a refresh nobody asked for over rows on screen — the poll, the read behind a live
 * message, coming back to the page — and its line shows only if it is still out after
 * `LOADING_LINE_DELAY_MS`. `#226 page-energy-profile`: shown at once, the line and its spinner went
 * in above the whole history and came out again a moment later on every poll, laying the list out
 * twice and animating, which was most of what a refresh with nothing new still cost. The pill says
 * "refreshing…" all the while, and a slow refresh still shows the line.
 */
function renderChannelLoading(loading, quiet = false, delayed = false) {
  const indicator = el("channel-loading");
  loadState().refreshing = loading;
  if (loadState().lineTimer !== null) {
    clearTimeout(loadState().lineTimer);
    loadState().lineTimer = null;
  }
  if (loading && delayed && !quiet && indicator.hidden) {
    loadState().lineTimer = setTimeout(() => {
      loadState().lineTimer = null;
      if (loadState().refreshing) indicator.hidden = false;
    }, LOADING_LINE_DELAY_MS);
  } else {
    indicator.hidden = !loading || quiet;
  }
  // "Saved 14:05" and "Saved 14:05 · refreshing…" are different claims; only the second waits.
  renderChannelFreshness();
  if (!loading) return;
  indicator.textContent = selection().view === "threads"
    ? "Loading threads…"
    : selection().view === "thread"
      ? "Loading thread…"
      : "Loading messages…";
}

/** How long a refresh nobody asked for may be out before its loading line shows: one second. `#226`. */
const LOADING_LINE_DELAY_MS = 1000;
// The timer that shows that line, or null. Kept per column: `loadState().lineTimer` (`freshLoadState`, `#624 voice-page-modularity`).

/**
 * Whether a read was asked for by nothing the reader did — the poll, a live message, coming back to
 * the page, the stream's replay or an edit it carried — every reason a queued read stands for
 * included. `#226 page-energy-profile`.
 *
 * @param {{reason?: string | (string | undefined)[]} | undefined} options
 */
function backgroundRead(options) {
  const reasons = options && Array.isArray(options.reason) ? options.reason : [options ? options.reason : undefined];
  return reasons.every((reason) => ["poll", "live", "visible", "replay", "mutation"].includes(String(reason)));
}


// (`createThreadStrip`, `#624 voice-page-modularity`): the strip on each row, the selection it
// makes, and Main brought up to date with its threads' replies.
const threadStripOwner = createThreadStrip({
  byId: el,
  inThisColumn,
  inColumn,
  mainColumn: () => columns.main,
  side: () => columnWork.side(),
  selection: () => selection(),
  selectedChannelId,
  heldMessages: () => channelStore.current().messages,
  threadHue: (id) => threadHue(String(id)),
  sayRowShortcut,
  setStatus,
  threadName,
  knownThreadChoice,
  channelOnScreen,
  layoutThreadList: () => layoutShown.threadList(),
  pinsOnly: () => pinsView().only,
  rowForAct,
  threading: () => providerFacts.threading(),
  hidingRead: () => hidingRead(),
  readFilterKeeps,
  renderCachedTimeline: () => renderCachedTimeline(),
  renderChannelRows,
  hiddenEmpty: (m) => hiddenEmpty(m),
  drawnIds: () => [...el("discord-log").children].flatMap(idsOf),
  fromGatheredThread,
  canonChannel: () => channelStore.current().channel,
  timelineMessages: () => timelineRows().messages,
});
const {
  allThreadSelected, drawingAll, markListView, paintThreadStrip, paintAllThreadFocus,
  toggleAllThreadFocus, clearAllThreadFocus, threadStripFromKeyboard, syncUnreadReplies,
} = threadStripOwner;

/**
 * Whether message id `id` is at or after position `from`. Both are ids in the order the server keeps
 * them in, decimal numbers of any length (`MessageId::numeric`): longer is later, and at one length
 * the text compares as the number does. False for anything that is not such a number.
 */
function atOrAfter(id, from) {
  if (!/^[0-9]+$/.test(id) || !/^[0-9]+$/.test(from)) return false;
  const [a, b] = [id.replace(/^0+(?=.)/, ""), from.replace(/^0+(?=.)/, "")];
  return a.length !== b.length ? a.length > b.length : a >= b;
}

/**
 * What a page says about read state, into the store. `#222 unread-replies`.
 *
 * A page is the server's answer for every message on it, Done or not, whichever view it was read for
 * — so a read of All landing while the reader is in Main still tells the store which replies are
 * Done. Main's newest page and its deltas say more (`reply_dismissals`): every mark from a position
 * up, complete there, so every message the store holds from that position is Done exactly when it is
 * named, the replies Main never draws among them.
 *
 * Answers whether that changed anything the store held (`#226 page-energy-profile`).
 */
function foldReadState(payload, view) {
  let changed = false;
  const mark = (id, done) => {
    if (channelStore.current().dismissed.has(id) === done) return;
    if (done) channelStore.current().dismissed.add(id);
    else channelStore.current().dismissed.delete(id);
    changed = true;
  };
  const done = new Set((payload.dismissed || []).map(String));
  for (const message of payload.messages || []) {
    const id = String(message.id);
    if (!readSpeaksFor(payload, id)) continue;
    mark(id, done.has(id));
  }
  const marks = payload.reply_dismissals;
  if (view !== "main" || !marks) return changed;
  const named = new Set(marks.messages.map(String));
  const from = String(marks.from);
  for (const message of channelStore.current().messages) {
    const id = String(message.id);
    if (!atOrAfter(id, from) || !readSpeaksFor(payload, id)) continue;
    mark(id, named.has(id));
  }
  return changed;
}

/**
 * What one page says of the replies the store lacks for being old, by thread (web/src/core/channels
 * `noteReplyGapsIn`, `#222 unread-replies`), into the selected channel's store; a newest read of
 * Main may then be followed by one of All, behind it (`queueReadAllForCounts`).
 */
function noteReplyGaps(payload, view, older, delta, before) {
  noteReplyGapsIn(channelStore.current(), payload, view, older, delta, before, queueReadAllForCounts);
}

// `#222 unread-replies`. ALL, READ BEHIND MAIN, FOR ITS COUNTS. A device that has never read All —
// a fresh profile, a cleared copy, opened in Main — holds no reply at all, so every root with replies
// said "N new", Hide read kept every one, and that held through every poll and reload, and through
// Done on the root, until the reader happened to open All. And while the reader stays on Main a reply
// Main's count reports is "1 new" until something brings it. One read of All brings the replies with
// their read state, and the counts are exact: so when a read of Main finds roots whose counts outrun
// what the store holds, All is read once, behind it, with no loading line and nothing waiting on it —
// a view switch included, which is drawn from the store as ever, and draws what this brought.
//
// BOUNDED, because it is not free: Slack's All is one history read and one per thread it carries,
// about eleven of the chat service's calls. At most once per `COUNTS_ALL_READ_MS` per channel, counted
// from the last read of All of any kind — the reader's own, or one saved with the device's copy — and
// only for news: a root the last such read already saw at the same count does not ask again, so a
// thread All does not carry (past Slack's ten) is not a reason to read All every few minutes. Not
// while the page is hidden. A failure waits out the same interval; Main's own read is not affected.





// `#222 unread-replies`. DONE HERE RACES THE READS ON THE WIRE. A read sent before a Done or a put
// back reached the server answers with the read state from before it, and folding that would undo
// the reader's own act until the next read: a reply dealt with in its thread counted as unread on
// Main again. So each local write is remembered by message id until a read sent after it settled can
// speak for that message: while the write is on its way no read does, and once it is settled only a
// read sent after. The epoch moves at every write's start and end; a read records it when sent. The
// epoch, the local writes and when each read was sent are the ledger's (web/src/core/annotations
// `createReadLedger`, `#624 voice-page-modularity`), here where they stood.
const readLedger = createReadLedger();

/** Whether `payload`'s read state for message `id` is newer than every local write to it. */
function readSpeaksFor(payload, id) {
  return readLedger.speaksFor(payload, id);
}

/**
 * When `payload`'s read was sent, as the epoch then. Now for a payload no read sent — the store drawn
 * as a page, say — which holds the local state already.
 */
function sentEpochOf(payload) {
  return readLedger.sentEpochOf(payload);
}

/**
 * Where `payload`'s read was sent in the order of every timeline read (`readOrder`). Now — after every
 * read sent so far — for a payload no read sent. `#308 live-reply-in-main`: of two newest pages of a
 * view, the one sent later is the newer, whichever lands first.
 */
function sentNumberOf(payload) {
  return readOrder.sentNumberOf(payload);
}

/**
 * Where each copy of a message in the store stands in the order of reads (`sentNumberOf`), the copy it
 * was put together on, and whether it says the message as it was posted (web/src/core/channels
 * `createCopyOrder`, `#310 stale-live-read-flicker`, which says what each means and why): the facts
 * private to the ledger, reached only by its named commands. Here where the maps it replaces stood.
 */
const copyLedger = createCopyOrder();


/** Copy `copy`, the store's, stands at `at`, or later (`CopyOrder.orderCopy`). */
function orderCopy(copy, at) {
  copyLedger.orderCopy(copy, at);
}

/** Copy `copy`, put together here from `from`, stands where `from` stood (`CopyOrder.buildCopyOn`). */
function buildCopyOn(copy, from) {
  copyLedger.buildCopyOn(copy, from);
}

/** The store's copies of `messages` stand where the read numbered `number` was sent (`CopyOrder.orderCopiesRead`). */
function orderCopiesRead(messages, number) {
  copyLedger.orderCopiesRead(channelStore.current(), messages, number);
}

/** Whether the store's `copy` of message `id` is older than the read numbered `number` (`CopyOrder.copyOlderThan`). */
function copyOlderThan(copy, id, number, held, says = false) {
  return copyLedger.copyOlderThan(copy, id, number, held, says);
}

/**
 * Sending the timeline's reads, the paths and reason words they carry, and joining what
 * landed (web/src/core/sync `createTimelineReader`, `#624 voice-page-modularity`).
 *
 * @type {import("./src/core/sync/index.ts").TimelineReader}
 */
const timelineReader = createTimelineReader({
  decoded: (path, options) => apiDecoded("TimelineResponse", path, /** @type {{signal?: AbortSignal}} */ (options)),
  ledger: readLedger,
  order: readOrder,
  // Read as it is used: the constant itself is declared with the read it sizes, below.
  get pageLimit() { return DISCORD_PAGE_LIMIT; },
  channelId: () => selectedChannelId(),
  view: () => selection().view,
  threadId: () => selection().threadId,
  withThreadQuery,
  searchWalkActive: () => Boolean(columnWork.active().searchWalk),
  storeChannel: () => channelStore.current().channel,
  selectedChannel: () => String(selectedChannelId()),
  cover: () => channelStore.current().views.get(viewKey()),
  complete: () => channelStore.current().complete,
  mutationAwaits: () => mutationAwaitsRead(),
  refreshFailed,
  now: () => Date.now(),
});

/**
 * What a Done or a put back changed in the store (web/src/core/channels `HeldDoneMark`).
 *
 * @typedef {import("./src/core/channels/index.ts").HeldDoneMark} HeldDoneMark
 */

/**
 * Done, or put back, here: the store's read state for `ids` follows at once, as `doneState().archived` does,
 * so a reply dealt with from All, a thread or the Pinned list counts as read on Main straight away;
 * and no read sent before the write settles undoes it (`readSpeaksFor`). Answers what to give
 * `settleHeldDone` when the write has settled.
 *
 * `settles`: Done issued from Main's list, where a root's count of new replies is drawn, settles the
 * replies of its roots no read has placed (`settleUnknownReplies`). Done in All, in a thread or in the
 * Pinned list, where nothing says a root has new replies, does not. A put back, from anywhere, takes
 * back what Done settled (`unsettleReplies`).
 *
 * @param {string[]} ids
 * @param {boolean} done
 * @param {boolean} [settles]
 * @returns {HeldDoneMark}
 */
function markHeldDone(ids, done, settles = false) {
  const marked = ids.map(String);
  // The epoch moves, and each has one more write on its way (`ReadLedger.writeStarted`).
  readLedger.writeStarted(marked);
  const canon = providerFacts.threading() && channelStore.current().channel === String(selectedChannelId())
    ? channelStore.current() : null;
  // The store's part (web/src/core/channels `markDoneIn`): its read state, and the gaps Done settles.
  return markDoneIn(canon, marked, done, settles, stillToDo);
}

/** Whether Done issued now is issued from Main's list, where roots say they have new replies (`markHeldDone`). */
const doneSettlesHere = () => providerFacts.threading() && selection().view === "main" && !pinsView().only;

// `settleUnknownReplies` is web/src/core/channels's (`#624 voice-page-modularity`).

// `unsettleReplies` is web/src/core/channels's (`#624 voice-page-modularity`).

/**
 * The write `mark` stands for has settled, `ok` or not. Failed, exactly what it changed in the store
 * goes back — never a copy from before it, which would undo whatever reads folded meanwhile, and in
 * another channel's store would save this one's state into it.
 *
 * @param {HeldDoneMark} mark
 * @param {boolean} ok
 */
function settleHeldDone(mark, ok) {
  // The epoch moves, and each settles there once none is left on its way (`ReadLedger.writeSettled`).
  readLedger.writeSettled(mark.ids);
  // Failed, exactly what it changed in the store goes back (web/src/core/channels `settleDoneIn`).
  settleDoneIn(mark, ok);
}

/**
 * The part of `mark` that names `ids`: for a write sent in batches, some of which the server took
 * before one failed. Each of Done's settlings goes with the part holding its thread's root.
 *
 * @param {HeldDoneMark} mark
 * @param {string[]} ids
 * @returns {HeldDoneMark}
 */
function heldDonePart(mark, ids) {
  const wanted = new Set(ids);
  /** @param {[string, number]} entry */
  const rootIn = ([thread]) => mark.canon !== null && mark.canon.messages.some((/** @type {any} */ message) =>
    threadOf(message) === thread && message.thread.is_root === true && wanted.has(String(message.id)));
  return {
    ...mark,
    ids: mark.ids.filter((id) => wanted.has(id)),
    flipped: mark.flipped.filter((id) => wanted.has(id)),
    settled: mark.settled.filter(rootIn),
    unsettled: mark.unsettled.filter(rootIn),
  };
}

/**
 * Send `ids` to a dismiss or restore `route` in batches the server accepts (`DISMISS_BATCH`), one
 * after another, adding the ids of each batch to `landed` as the server takes it: when one fails, the
 * batches before it stand on the server, and the caller takes back only the rest. Answers the ids
 * the server says it stored.
 *
 * @param {string} route
 * @param {string[]} ids
 * @param {string[]} landed
 * @returns {Promise<string[]>}
 */
async function writeInBatches(route, ids, landed) {
  const stored = [];
  for (let start = 0; start < ids.length; start += DISMISS_BATCH) {
    const batch = ids.slice(start, start + DISMISS_BATCH);
    const part = await inboxPost(route, { messages: batch });
    landed.push(...batch);
    stored.push(...((part && part.messages) || batch).map(String));
  }
  return stored;
}



/**
 * Draw a page: a newest page, an older one (`older`), saved rows (`saved`), or a delta the caller
 * has already folded into the store (`delta`, `#203 incremental-refresh`). A delta is drawn exactly
 * as a refresh that keeps the reader's place is — the store's projection of the view — but it says
 * nothing about older history, so the walk back keeps its cursor, and it names only the archived
 * rows among its own. `rows`, with `saved`, draws those rows of the store in place of a projection:
 * a view no page has covered yet, drawn from what the store holds for it (`drawHeldRows`). `held`, for
 * a newest page, is the store's copies as its read was sent (`heldNow`): it is folded only over the
 * copies older than it (`pageFold`, `#310 stale-live-read-flicker`).
 *
 * @param {any} payload
 * @param {boolean} [older]
 * @param {boolean} [saved]
 * @param {boolean} [delta]
 * @param {any[] | null} [rows]
 * @param {Map<string, any> | null} [held]
 */




// --- reading only what changed -------------------------------------------------------------------
//
// `#203 incremental-refresh`. The owner: "This is the common case inner loop for how we use the
// application all the time. It is important for this to be efficient. Right now my refreshes feel
// very slow." Every refresh used to re-read the newest page of the view, and behind a bridge that
// could mean re-scanning a channel's whole history. Now a newest read hands back `next_after`, and
// a refresh with that cursor asks only for what changed since: a DELTA, folded into the store
// without the newest page's deletion-by-absence, which a delta must never apply.
//
// WHICH READ. A full read on entering a view, changing channel, a stream reset, an expired or
// mismatched cursor, a delta that runs past `TIMELINE_DELTA_PAGES`, and every
// `TIMELINE_RECONCILE_MS`. Otherwise a delta — as far as the channel's deltas reach. A bridge with a
// change record answers COMPLETE deltas, edits and deletions included, so every refresh there is
// one. Any other backend answers additions only, so its channel keeps the full poll it always had
// and its edits and deletions are no slower; deltas there serve a live arrival and a return to the
// page, where speed is the point.

// How old a view's last newest read may be (`TIMELINE_RECONCILE_MS`), how many delta pages one refresh
// follows (`TIMELINE_DELTA_PAGES`), how long a hidden page refreshes after (`VISIBLE_REFRESH_MS`), when
// the pill says the service is slow (`STALE_AS_OF_MS`) and why a read may be a delta (`DELTA_REASONS`)
// are web/src/core/sync's, with the choice of cursor (`#624 voice-page-modularity`).




// Whether the server can no longer continue from a cursor (`isCursorRefusal`) is web/src/core/sync's.

/**
 * `#617 cold-load-first-paint`. After a page the server answered from its own copy, the next reads
 * come soon, one after the other, rather than at the next poll (`STAND_IN_RETRY_MS`): how many have
 * been spent, and whether the next poll should be one, are web/src/core/sync's (`createStandInRetries`,
 * `#624 voice-page-modularity`).
 *
 * @type {import("./src/core/sync/index.ts").StandInRetries}
 */
const standInRetries = createStandInRetries();

/**
 * A read landed: say how fresh the rows are — now, or as of what the server could vouch for.
 *
 * `#617 cold-load-first-paint`: a page from the server's own copy says "Refresh failed" only when
 * that copy is older than `STALE_AS_OF_MS`; a younger one is as current as the pill says ("Updated"
 * at the copy's time). Either way the next read comes soon (`STAND_IN_RETRY_MS`).
 */
function noteFreshRead(payload) {
  const decision = timelineReader.freshDecision(payload);
  loadState().freshAt = decision.freshAt;
  const retryWanted = standInRetries.noteRead(decision.failed);
  // A poll already armed comes forward; one armed after this read takes it up as it is armed.
  if (decision.failed && retryWanted && channelPoll.armed()) {
    scheduleDiscordPoll(Math.max(0, channelPoll.dueAt() - Date.now()));
  }
  setChannelFreshness(decision.freshness);
  // `#206 pin-message`. Every landed read, delta included, says what revision the pins are at.
  notePinsRevision(payload.pins_revision);
  // `#240 reading-bookmark`. ...and where the bookmark is, which another device may have moved.
  noteBookmark(payload);
}






/**
 * `#430 threads-main-own-sent`. The messages this page has sent, by id, for as long as it is open. The
 * owner, 2026-10-22, on a phone: "even when I've toggled Main on in thread list View on mobile. I still
 * don't see the message that I just sent to the main channel." The owner's own messages count as read
 * (`ownReadSwitch`), so under Hide read the Threads view's main rows, rebuilt through the read filter, left
 * it out; Main showed it only because its live append skips that filter, until its next redraw. So a
 * message sent from here is one the read filters keep (`readFilterKeeps`, `mainRowMessages`), in every
 * list alike, for the rest of the page's life: what the reader has just said never vanishes as they say it.
 *
 * `#466 reply-promotes-thread`, `#500 sent-receipts-pile-up`. When each was acknowledged, too, kept on
 * the device for OWN_POST_GRACE_MS, so a reload in the grace keeps the page's own post against a read
 * from older state as the page that sent it did (`ownPostStands`). The owner is web/src/core/outbox
 * `createOwnPosts` (`#624 voice-page-modularity`).
 *
 * @type {import("./src/core/outbox/index.ts").OwnPosts}
 */
const ownPosts = createOwnPosts({
  storage: deviceStorageOnUse(),
  now: () => Date.now(),
});

/** Whether `message` was sent from this page (`ownPosts`). */
const sentFromThisPage = (message) => ownPosts.sentFromHere(String(message.id));

/** Take back what earlier pages of the device sent in the grace (`ownPosts`). */
function loadOwnPosts() {
  ownPosts.load();
}

/**
 * Whether message `id` is one this page posted within OWN_POST_GRACE_MS: a page that does not carry
 * it is not evidence that it is gone.
 *
 * THE OWNER'S REPLY VANISHED. Sent to a Slack message nobody had answered, it was drawn, then gone
 * from every view, and the message it answered went back to "start a thread": "my sent message
 * disappeared, and I didn't see it anywhere on the screen." Reproduced: the stream's copy of the
 * reply asks for a newest page of All (`reason=live`), sent after the post, and the chat service
 * answered it from state that did not have the reply yet — a bridge's lag, Slack's own Flat memory of
 * a thread that had no replies (`#282 slack-flat-skip`). That page carries the message answered with
 * a count of none, so it answers for its whole thread (`allPageAnswersFor`) and took the reply out as
 * deleted upstream; nothing was left to draw it, the receipt having gone as the reply was drawn. The
 * page orders copies by when its reads were sent, which is sound only while every answer is at
 * least as new as every earlier one (replication model, R9 and F3). Until the server says how new
 * each copy is (C7), this page's own fresh post is taken out only once it is told of its removal
 * (`noteRemoved`), never by its absence from a page alone.
 *
 * For copies without the server's versions only. Where a page says its `observed_ms` and a newest
 * page's `settled_below_ms` and the store's copy says its own `observed_ms` (`#473
 * replication-c7-server-versions`; the answer to a post carries it), no stale page can drop a copy
 * observed after it began, and `pageFold` decides by those before this is asked. Either way the
 * page's own post is kept, so its receipt is a display aid (`sentReceiptRetires`, `#500
 * sent-receipts-pile-up`).
 */
function ownPostStands(id) {
  return ownPosts.stands(String(id));
}

/**
 * Whether the page's store keeps its own posts in channel `channel` against a page from older state
 * (`ownPostStands`, and the server's versions where said, `pageFold`): a channel with threads. Without
 * them a read replaces the rows, and the receipt is what draws a confirmed post a stale read left out.
 *
 * @param {string} channel
 */
function storeKeepsOwnPosts(channel) {
  const provider = providerOfChannel(channel);
  return provider ? provider.threading_supported === true
    : providerFacts.threading() && channel === String(selectedChannelId());
}

/**
 * Whether the receipt of `entry`, a send, retires now. The owner, 2026-10-24: "messages I sent stack up
 * in the 'Sent.' cards at the bottom of the screen, and they survive even the page restart." From
 * `#466 reply-promotes-thread` a receipt went only once a READ carried every part, and was saved until
 * then: a reply into a thread the page never reads — sent from Main, or from the Threads view's cards —
 * kept its receipt for good, and every reload put it back.
 *
 * Where the store keeps the page's own posts (`storeKeepsOwnPosts`), a receipt is a display aid: taking
 * it away cannot take the message with it. It goes:
 *   - at once when reads have carried every part, as ever;
 *   - there, once it has been shown SENT_RECEIPT_MS and the selected channel's store holds every part
 *     — the post's answer, the stream's copy or a read's — or a row draws it. Not a send to another
 *     channel: its store is not loaded, and the stream's copy of it is put in none;
 *   - OWN_POST_GRACE_MS after the server confirmed it, whatever the page holds — as long as the page
 *     keeps its own post against a page that does not carry it.
 * Nor is it saved there (`persistOutgoingMessages`), and a confirmed send saved past the grace, or by an
 * earlier build, is not loaded (`loadOutgoingMessages`). Not sent, unconfirmed or on its way, an entry
 * stays for the reader, as before.
 *
 * Decided by web/src/core/outbox `sentReceiptRetires` (`#624 voice-page-modularity`), on what this page
 * knows: the clock, the selected channel and its store.
 *
 * @param {any} entry
 * @param {Set<string>} drawn the ids of the selected channel's messages drawn in this column
 */




// --- a reply's row where its box was ----------------------------------------------------------------
//
// `#467 inline-send-in-place`. The owner: "The compose box jumps to where it should be, in situ in the
// thread... But then the little 'Sending..' box, the one that would show Retry if it fails, jumps back
// down to the BOTTOM, not where the compose box was. This should provide better continuity."
//
// A reply sent from the box in the stream (`#410 inline-reply-editor`) keeps its row there: under the
// message it answered and what is gathered under it (`anchorUnder`), in the column's list when it draws
// that message — a thread, Main, All, in either column — saying how it goes ("Sending…", "Posting part 2
// of 3…", or "Not sent" with Retry and Edit),
// until the reply itself is drawn in its place, which retires the row as it always has. A message sent
// from the box at the foot keeps its row at the foot, where it was written. Drawn as the box is: never
// a row of the list, but named to its row by an anchor of its own (`--outgoing-N`), that row keeping
// room under itself as tall as what is drawn there, so the rows after it move down by that and no
// more, and nothing jumps when the reply takes its place. Without anchor positioning it is drawn after
// the list, at its foot. Not under the row the open reply's box is drawn under: that one place holds
// one thing, and the row goes to the foot until the box moves on (`placeInlineReply` places them again
// as the box opens and goes). Elsewhere, where the message is not drawn — another list, the cards, a
// search that hides it (whose pass places them again) — and under the Pinned filter, at the foot.

/** By column, the rows named as anchors on its last pass, and the lists of rows drawn under them. */
const outgoingInPlaceDrawn = new Map();

/** The row each list drawn in place is named to, for `outgoingRoomWatch`. */
const outgoingRoomAnchors = new WeakMap();


/** Keeps each row's room as tall as what is drawn under it through a resize or a reflow, as the box's is. */
const outgoingRoomWatch = typeof window.ResizeObserver === "function"
  ? new window.ResizeObserver((entries) => {
    for (const { target } of entries) {
      const anchor = outgoingRoomAnchors.get(target);
      if (anchor) fitOutgoingRoom(/** @type {HTMLElement} */ (target), anchor);
    }
  })
  : null;



// --- the thread a send goes into, before it is there ------------------------------------------------
//
// `#466 reply-promotes-thread`. The owner, on Slack: "I expected the main-channel message to be promoted
// to the root of a new thread, and that didn't visually happen, not locally and not quickly ... It would
// be great if promoting a root message to a thread and visually linking the sent message to it happened
// locally, without waiting for some asynchronous server refresh."
//
// A send still in the outbox — on its way, unconfirmed, or not sent and waiting for the reader — counts
// in the thread it goes to, and is shown there: the message it answers reads as that thread's root, with
// the count and colour of a thread (`drawThreadChips`); the Threads view has a card for it
// (`threadCards`); and the thread itself shows the send's row (`outgoingInView`). All of it is derived
// from the outbox at every pass, keyed by the thread's own id (on Slack the message's ts, from its thread
// record, never its message id), and nothing is written into the store's copy of the message answered:
// a read whose copy predates the reply cannot undo it, and a send dismissed takes it back with nothing to
// undo. A send not sent keeps its thread, with Retry, until it is dismissed (replication model §5.2).
// Once sent, the reply is an ordinary held reply, which the count already takes (`max(reported, held)`).
//
// The count adds the sends to what the provider reported and what is held, which overcounts by one a
// send that landed though its answer was lost and that a later read already counts, until it is settled
// (`settleFromServer`).

// The pending threads' key as it stood when the rows were last passed over for it is their own
// (`pendingThreads.noteDrawn`, web/src/core/outbox, `#624 voice-page-modularity`).

/**
 * Which thread each send in the outbox posts into, and how many sends count in each thread of the channel
 * on screen and when the newest was written, worked out once for each change: web/src/core/outbox's
 * (`createPendingThreads`, `#624 voice-page-modularity`).
 */
const pendingThreads = createPendingThreads({
  channel: () => String(selectedChannelId()),
  outboxVersion: () => outbox.version(),
  sends: () => outbox.values(),
  store: () => channelStore.current(),
  repliesInThread: (channel) => channelRepliesInThread(channel),
  threadRecordOf: (message) => heldThreadRecord(message),
  idOf: (message) => message.id,
});

/** The thread `entry` posts into in the channel on screen, or null for the main channel or one not known. */
function outgoingThreadOf(entry) {
  return pendingThreads.threadOf(entry);
}

/** The sends in the outbox for the channel on screen that count in thread `thread`. */
function pendingInto(thread) {
  return pendingThreads.into(thread);
}

/** The threads of the channel on screen sends in the outbox go into, each with when its newest was written. */
function pendingThreadTimes() {
  return pendingThreads.times();
}





/**
 * How many messages the stream has left unplaced in this page, in any channel: the clock `unplacedSince` reads;
 * and the channel whose placing read is on the wire, one at a time (web/src/core/sync `createArrivalCount`,
 * `createChannelInFlight`, `#624 voice-page-modularity`).
 */
const unplacedArrivals = createArrivalCount();
const placingReadInFlight = createChannelInFlight();

/** Placing what the stream left unplaced: web/src/view/channel's now (`createPlacing`, `#624`). */
const placing = createPlacing({
  channelStore: () => channelStore, providerFacts,
  unplacedArrivals: () => unplacedArrivals, placingReadInFlight: () => placingReadInFlight,
  readOrder: () => readOrder, readHolds: () => readHolds, liveEvents: () => liveEvents,
  abandonedReads: () => abandonedReads, channelReads: () => channelReads, copyLedger: () => copyLedger,
  timelineReader: () => timelineReader, columns: () => columns, jumpNewestWanted: () => jumpNewestWanted,
  timelineReconcileMs: TIMELINE_RECONCILE_MS, readTimeoutMs: () => CHANNEL_READ_TIMEOUT_MS,
  hiddenNow: () => document.visibilityState === "hidden",
  eachColumn: (act) => eachColumn(act), el: (id) => el(id),
  foldTimelineDelta: (payload, sent, view, thread) => foldTimelineDelta(/** @type {any} */ (payload), /** @type {any} */ (sent), /** @type {any} */ (view), /** @type {any} */ (thread)),
  foldTimelinePage: (payload, replace, view, thread) => foldTimelinePage(/** @type {any} */ (payload), replace, /** @type {any} */ (view), /** @type {any} */ (thread)),
  hiddenEmpty: (m) => hiddenEmpty(m), hidingRead: () => hidingRead(),
  inColumn: (column, act) => inColumn(/** @type {any} */ (column), act),
  isCursorRefusal: (error) => isCursorRefusal(error),
  noteArchived: (payload, replace) => noteArchived(payload, replace),
  projectView: () => projectView(), readFilterKeeps: () => readFilterKeeps(),
  renderCachedTimeline: () => renderCachedTimeline(), renderChannelNavigation: () => renderChannelNavigation(),
  renderScrollTools: () => renderScrollTools(), saveChannelScope: () => saveChannelScope(),
  selectedChannelId: () => selectedChannelId(), selection: () => selection(),
  sentNumberOf: (payload) => sentNumberOf(payload), settleAfterRead: (shown, opts) => settleAfterRead(shown, /** @type {any} */ (opts)),
  syncUnreadReplies: () => syncUnreadReplies(), timelineRows: () => timelineRows(),
  viewKey: (view, threadId) => viewKey(view, /** @type {any} */ (threadId)),
  within: (ms, fn, late) => within(ms, fn, late),
  channelReadPosition: (area) => channelReadPosition(area), catchUpFromAll: () => catchUpFromAll(),
});
const { removalRecord, noteRemoved, removedAfterRead, believeCarried, removedUpstream, placeUnplacedWaits, queuePlaceUnplaced, placeUnplaced, landPlacingRead, placeFromAnswer, placeFromPage, followPlacement } = placing;
//
// Queueing a send in the order written, placing a reply whose thread is still to be found out, the
// POST and what became of a lost answer, and every automatic retry are the send service's
// (web/src/core/outbox `createOutboxDispatch`, made below with the outbox's other owners, `#624
// voice-page-modularity`); Edit, Dismiss, Undo and putting a dismissed send back are taking a
// send back's (web/src/view/channel `createSendTakeBack`, made there too). What stays here is
// `placeReplyTarget` above: where a reply to a message the stream left unplaced goes is settled
// from this page's channel store and timeline reads, which the send service asks for by that name.


// Settings' Recently dismissed, each send's place, age and first line, is drawn by
// web/src/view/settings (`dismissedSendsView.render`, `#624 voice-page-modularity`).

// --- the pop-out over Recently dismissed ----------------------------------------------------------
//
// `#348 settings-dynamic-lists`. One message, whole: where it was going and when it was dismissed, its
// text as typed, Copy, Put back and Close. It is a menu to the keys and to the back gesture
// (`openMenus`): Escape closes it first, and so does Back, before Settings. Leaving Settings closes it.
//
// `#351 settings-polish`. A MODAL. It dims the screen around it, and the dim used to be only a shadow:
// Tab went on past Close to the controls behind it, and a tap on the dim pressed whatever was under it.
// So while it is up everything else on the page is inert (`inertAround`), which no tap, click or Tab
// reaches and a screen reader does not list, and Tab and Shift+Tab go round its own four stops
// (`dismissedSendsView.trapTab`). It is marked `aria-modal` to say so. Close, Escape and Back are how it goes.
//
// And everything it makes inert is under the dim. The dim is the pop-out's shadow, which Settings'
// scrolling box clips (`dimBoxOf`), so the top bar with its Back stood bright above it, and a tap on
// that Back did nothing while it looked live. What is inert beyond that box and laid out beside it,
// the top bar and the dock, is now dimmed by the same depth where it stands (`data-under-dim`). The
// status pill is not: it floats over the box from outside it, under the pop-out, so the shadow dims it
// already (`floatsOverDimBox`). One dim over the whole screen, nothing dimmed twice, and the pop-out
// drawn over everything, its buttons included when it stands at the foot of the screen where the pill
// is.

// The pop-out, its dim and its trap are web/src/view/settings's (`dismissedSendsView`, `#624
// voice-page-modularity`); taking a send back listens to it, where the legacy script always did.


// --- message text, as the server rendered it ------------------------------------------------
//
// The value of this view is being able to point at a specific real message — the agent has
// described messages that did not exist. So every line carries its author and its message id.
//
// `#217 markdown-blocks`. THIS PAGE NO LONGER RENDERS MARKDOWN. It used to, with a hundred lines of
// its own drawing one block per line: a `- ` item stayed a dash, a list had no indent, and a blank
// line between paragraphs vanished, so the closing lines of an agent's answer ran together. The
// server renders every body now, with an off-the-shelf Markdown renderer and an off-the-shelf
// sanitizer (src/render.rs), and sends the result as `content_html`.
//
// Channel text is third-party data written by whoever is in the channel, and that is unchanged.
// What changed is where it becomes markup: on the server, through a sanitizer that keeps a short
// list of elements and attributes and drops everything else — no script, no style, no event
// handler, no `src`, and a link only to `http` or `https`. The page has exactly ONE sink that
// parses a string, `insertSanitizedHtml`, and the only strings that reach it are `content_html`.
// Message text itself — `content`, a typed draft, an author's name — still goes in through
// `textContent` and nothing else, which is escaping by construction rather than by remembering.
//
// A message with no `content_html` is drawn from `content` as plain paragraphs. That is most chat:
// the server leaves the field empty when rendering would change nothing. It is also a saved
// message from before the field existed, which is drawn as the text it is and never as markup.










// --- rich headers ---------------------------------------------------------------------------------
//
// `#242 rich-headers`. The owner: "we have a lot of wasted space in the header of each message", and
// his agents open their messages with who they are — "[build-bot coordinator] Backup check, 10:52 PM
// ET. All 4 shards are synced." — because every agent posts through the one bot account. The server
// finds those pieces (src/heading.rs) and sends them as `heading`: a tag, a time and a title, and
// `cut`, how much of the message's text they restate. This page shows them in the row's header line
// and hides the words they came from.
//
//   * NOTHING IS LOST, EITHER WAY. The body is never cut. The restated words stay in it, marked
//     `rh-src` (by the server in a rendered body, by `plainParagraphs` in a plain one), and the
//     stylesheet hides them only while it shows the heading. With the setting off, the heading is
//     hidden and the words are drawn where they always were: the row is exactly what it was. The
//     switch is one attribute on the root, so flipping it redraws nothing and asks for nothing.
//   * THE TEXT IS UNTOUCHED. Copy, speech, search and a thread's name read the message as written,
//     or the drawn body as a whole, marked words included.
//   * WHERE IT GOES IS DECIDED BY THE STYLESHEET, from the list's width and a length class this page
//     sets from the strings alone — no layout is read (`#226`). A header that would not fit beside
//     the row's buttons is drawn as the first line of the message instead, in full: never cut short
//     with an ellipsis, which would drop words.
//   * ONLY THE ROW'S FIRST MESSAGE. A combined row (see `glom`) is headed by its first message; the
//     later ones keep their first lines as written.

/**
 * Show headings in the header, or draw every first line as written, and remember which. The root's
 * attribute is the whole of the switch: nothing else here asks whether it is on.
 */
function applyRichHeaders(on) {
  const shown = Boolean(on);
  messagePreferences.richHeaders.render(shown);
  try {
    deviceSettings().keep(DEVICE_SETTINGS.richHeaders, shown);
  } catch (_error) {
    // A browser that refuses storage still honours the choice for this session.
  }
}

/** What was stored. ABSENT MEANS ON: the default is the behaviour, not merely the initial value. */
function storedRichHeaders() {
  // A browser refusing storage reads as on (`DEVICE_SETTINGS.richHeaders.refused`).
  return deviceSettings().get(DEVICE_SETTINGS.richHeaders);
}





// --- who is speaking in the channel -------------------------------------------------------------
//
// Who is who is web/src/core/channels's (`createIdentities`); noting our own account and which
// channel a message is from are web/src/view/channel's (`createSpeakers`, `#624
// voice-page-modularity`). Settings' list stays mounted here, reading the owner.
const whoIsWho = createIdentities({
  storage: () => deviceStorage(),
  identityKey: IDENTITY_KEY,
  providerOf: (channelId) => providerOfChannel(channelId),
});

const identityList = mountIdentityList({
  lookup: (id) => el(id),
  allChannels: () => ALL_CHANNELS,
  known: (id) => Boolean(knownChannel(id)),
  nameOf: (id) => channelName(knownChannel(id)),
  who: whoIsWho,
  choose: (id, bucket) => whoIsWho.choose(id, bucket),
  renderChannelRows: () => renderChannelRows(),
});

const speakers = createSpeakers({
  whoIsWho,
  renderChannelRows,
  renderIdentityList: () => identityList.render(),
  selectedChannelId,
});
const { noteSelfAuthor, messageChannel } = speakers;

// --- the inbox view -------------------------------------------------------------------------

// --- what a channel row has already had done to it ------------------------------------------
//
// `#84 reply-aware-dismissal`, landing the two follow-ups `#50 todo-view` named for itself.
//
// TWO states, and they are two rather than one because they are known two different ways:
//
//   DISMISSED  DECLARED, by the reader, and recorded on the SERVER. That is `#50`'s state, it has
//              an undo and a bulk clear, and it is what the To do filter filters on.
//   REPLIED    DERIVED, from the channel itself: some other LOADED message points at this one.
//              Nobody sets it and nobody can clear it. It is a fact about the conversation.
//
// `#50` left one question open — "nothing in this file has to decide what happens when derived and
// declared disagree" — and landing the derived half is what forces the answer. THE ANSWER IS THAT
// THEY NEVER MEET, because they drive different affordances:
//
//   * DECLARED decides what is in the LIST. Dismissing is an act with an undo behind it, so it may
//     remove a row; the reader can always put it back.
//   * DERIVED decides how a row is DRAWN. Replying dims, and never hides. It is an observation, not
//     an instruction, and an observation that made messages disappear would be the page deciding
//     something on the reader's behalf from evidence it admits is incomplete.
//
// WHAT "REPLIED" HONESTLY MEANS, because the label would otherwise claim more than it can: Discord
// records a reply only on the ANSWERING message, so this can only ever see the answers that are
// LOADED. A reply further back than the reader has walked is invisible here, and a message can
// therefore be dimmed later than it "should" have been. It never goes the other way — a dimmed row
// really was answered — so the error is always in the safe direction, and walking further back with
// Older messages only ever reveals more of them. That asymmetry is precisely why this half only
// dims: being late to dim costs nothing, and being late to HIDE would lose a message.

/**
 * Re-derive every row's state from the list as it now stands.
 *
 * ONE PASS OVER THE WHOLE LIST, called after every mutation, rather than a decision taken when a
 * row is built. Rows arrive from three places — the newest page, a step further back, and a live
 * arrival — and every state here is a fact about the SET rather than about the row:
 *
 *   * "has this been replied to" — the answer to a message loaded an hour ago can arrive in the
 *     next poll, and a step back through the channel can reveal the question a loaded answer
 *     belongs to;
 *   * "is this author the coding agent" — the sole-other-bot guess is a statement about who ELSE
 *     is in the channel, so one more author arriving can change the answer for a row that is
 *     already on screen.
 *
 * Deciding either per row at construction time would be right only until the next thing happened.
 */
// A row's own part, found by walking the row (`childByClass`), is web/src/view/dom's.



function renderChannelRows() {
  // `#206 pin-message`. The Pinned filter's list first, so that the pass below derives its rows as
  // well: it is drawn from the same messages and has to agree with the channel about every one.
  syncPinnedList();
  // `#215 reply-coalesce`. The rows in the order they are meant to be in, gathered or in time
  // order, before anything below derives a row's state from where it is.
  arrangeChannelRows();
  const list = el("discord-log");
  const rows = [...list.children];
  // The channel these rows belong to, read once. Every author learned on this pass was seen HERE,
  // and every bucket drawn on it is a statement about who is in THIS conversation.
  const viewing = selectedChannelId();
  // Every message some LOADED message answers.
  const answered = answeredBy(rows, viewing);
  // The author census FIRST and in full, because `bucketFor` asks how many other bots there are —
  // a question no single row can answer, and one whose answer must not change halfway through the
  // loop that is applying it.
  let learned = false;
  for (const row of rows) {
    learned =
      whoIsWho.noteAuthor(
        viewing,
        row.getAttribute("data-author-id"),
        row.getAttribute("data-author"),
        row.getAttribute("data-author-bot") === "true"
      ) || learned;
  }
  if (learned) {
    whoIsWho.keepSeen();
    // `#402 who-is-who-channel-box`. Only if Settings' box is on the channel that just learned
    // something. Otherwise the list is another channel's, and rebuilding it draws the same rows again.
    if (el("settings-channel").value === viewing) identityList.render();
  }
  // `#222 unread-replies`. The replies the store holds, counted once for every row's thread chip.
  const tally = replyTally();
  const mainList = new Set(rows);
  // `#299 all-thread-strip`. All's rows are drawn in the left column only, never on the right.
  const inAll = drawingAll();
  markListView();
  for (const row of [...rows, ...el("pinned-log").children]) {
    dressChannelRow(/** @type {any} */ (row), { tally, answered, viewing,
      inMain: selection().view === "main" && mainList.has(row), inAll: inAll && mainList.has(row) });
  }
  // `#214 reply-arrow` and `#215 reply-coalesce`. Each reply's gutter, from the rows now around it
  // and the speakers just decided for them.
  renderReplyLinks();
  renderOutgoingMessages();
  // Settings counts what is loaded, and this is where what is loaded changes.
  noiseSettings.renderCount();
  // `#221 read-modes`. LAST, from the read state every row has just been given: under Fold
  // read a message dealt with on this pass joins its run, and a run that grew says so.
  applyReadRuns();
  // `#250 date-separators`. The days, from the rows as the runs leave them, inside whatever anchored
  // mutation drew them: a step back that brings the first row's day with it takes that day's rule off
  // the row the reader is on, and the change of height has to be part of the one the step makes. Not
  // while a filter chooses rows, which the search's pass, after this one, decides.
  if (!rowsChosen()) {
    drawSeams(el("discord-log"));
    drawSeams(el("pinned-log"));
  }
  // `#240 reading-bookmark`. After the runs, which decide which rows are shown: the line goes above
  // the first row shown at or after its message.
  placeBookmarkLine();
  // `#298 thread-card-header`. The card at the top of the right column's thread counts its replies and
  // unread as the rows' chips do, so it follows the same arrivals, reads and Done, in either column;
  // and so does the card at the top of a thread across the window.
  paintSideThreadCard(tally);
  if (!columnWork.side() && selection().view === "thread") paintThreadCard(leftEl("thread-card"), openThreadChoice(), tally);
  // `#299 all-thread-strip`. The thread selected in All, on the rows as they now stand: a live reply of
  // it arrives at full strength, any other message dimmed.
  paintAllThreadFocus();
  // `#407 unpack-row-parity`. Whatever this pass was for — a read aloud starting or ending, a pin, a
  // reply sent — the rows unpacked under the cards are dressed again with it, as message rows.
  // `cardsDrawn.dressedAfter`: not where the cards drawn next dress the one block that changed (`redrawUnpackBlock`).
  if (!columnWork.side() && !cardsDrawn.dressedAfter()) dressUnpackedRows();
  // `#540 read-aloud-desktop`. And a card whose first message a read is reading, or has asked for.
  if (!columnWork.side()) readAloudOwner().paintReadingCards();
  // `#419 threads-view-main-messages`. Main's messages among the cards are Main's rows' messages: what
  // changed them here (a message posted from the box at the foot, an arrival, a Done) is drawn there too.
  // Drawn again only where something they draw changed (`renderSideCards`).
  if (!columnWork.side() && mainRowsOnScreen()) renderSideCards();
  // `#410 inline-reply-editor`. A reply open in the stream, under its row as the rows now stand; and
  // one waiting for its message, opened again once the message is drawn.
  if (replyState.docked) placeInlineReply();
  else if (replyState.pending) reopenPendingReply();
}


/**
 * Swipe a row away, on a device that has swipes.
 *
 * TOUCH AND PEN ONLY. A horizontal mouse drag across a message is how a person SELECTS TEXT, and
 * this list exists so that a specific real message can be quoted and checked — taking that gesture
 * away to save a pointer user one click would be a bad trade. The button in the row's meta line is
 * the way in on a desktop, and it is present on every device, so nothing is reachable only by
 * gesture.
 */
/**
 * What the row stopped printing, shown on the row that was asked about.
 *
 * Inline rather than a dialog: the reader is holding a finger on one row of a list, and a modal
 * would take the list away to answer a question about it. Toggled, so the same gesture closes it.
 *
 * `textContent` throughout — an author name and a channel's own timestamp are third-party text,
 * and this is the one place they are shown in full.
 */
// The reading bookmark is web/src/view/channel's now (`createReadingBookmark`, `#624
// voice-page-modularity`): each channel's line, its chip, its slides on a Done, and its Undo.
const readingBookmark = createReadingBookmark({
  byId: (id) => /** @type {any} */ (el(id)),
  leftById: (id) => /** @type {any} */ (leftEl(id)),
  bumpEpoch: () => readLedger.bump(),
  deviceStorage: deviceStorageOnUse,
  queueWrite: (write) => queueInboxWrite(write),
  putBookmark: (channel, body) => apiDecoded("BookmarkResponse", `/api/v1/channels/${encodeURIComponent(channel)}/bookmark`,
    { method: "PUT", body }),
  removeBookmark: (channel, revision) => {
    const route = `/api/v1/channels/${encodeURIComponent(channel)}/bookmark`;
    return apiDecoded("BookmarkResponse", revision === null ? route : `${route}?if_revision=${revision}`, { method: "DELETE" });
  },
  isBookmarkAnswer: (body) => VibeTalkContract.is("BookmarkResponse", body),
  currentChannelId: () => currentChannelId(),
  eachColumn,
  renderScrollTools,
  side: () => columnWork.side(),
  activeName: () => columnWork.active().name,
  inColumn,
  mainColumn: () => columns.main,
  inThisColumn,
  sentEpochOf,
  saveChannelScope,
  setStatus,
  showError,
  pinsOnly: () => pinsView().only,
  readRunsSuspended: () => readRunsSuspended(),
  threading: () => providerFacts.threading(),
  selectionView: () => selection().view,
  selectionThreadId: () => selection().threadId,
  hidingRead: () => hidingRead(),
  todoHoldsFrom: (at, channel) => todoView.holdsFrom(at, channel),
  fromGatheredThread,
  preservingScroll,
  screen: () => screens.screen(),
  view: () => screens.view(),
  paneHead: () => channelPane.head(),
  takeRow,
  channelPane: () => channelPane,
  sidePane: () => sidePane,
  replyLandingGapPx: () => REPLY_LANDING_GAP_PX,
  threadListOnScreen: () => threadListOnScreen(),
  readerActed,
  seekMessage,
  guardQuietly,
  channelOnScreen,
  keyContext,
  rowForAct,
  drawnBy,
  anchorOfRow,
  nextUndoAt,
  stillToDo,
  archived: () => doneState().archived,
  dismissed: () => channelStore.current().dismissed,
  heldMessages: () => channelStore.current().messages,
  heldViews: () => channelStore.current().views,
  viewKey,
  inView: (m, view, thread) => inView(m, view, /** @type {any} */ (thread)),
  threadOf,
  timeOf,
  currentChannel: () => channelStore.current().channel,
  rowHolding,
  clock: () => clockOnUse(),
});
const {
  heldBookmark, bookmarkAnchor, forgetBookmarks, noteBookmark, bookmarkToSave,
  restoreSavedBookmark, stageBookmark, sendBookmark, placeBookmarkLine, renderJumpBookmark,
  toggleBookmarkAt, bookmarkFromKeyboard, bookmarkJumpFromKeyboard, singleDoneBump,
  throughBump, undoBookmarkBump, removeBookmark, undoBookmarkRemoval, renderBookmarkUndo,
  endBookmarkUndo, bookmarkAbove, bookmarkUndosCurrent,
  jumpToBookmark,
} = readingBookmark;
const bookmarkUndos = { current: bookmarkUndosCurrent };

// --- following a reply up to what it answers --------------------------------------------------
// The arrow in a reply's gutter, the jump to what it answers and the way back are
// web/src/view/channel's now (`createReplyArrows`, `#624 voice-page-modularity`).

// --- gathering a message's replies under it ---------------------------------------------------
// Gathering a message's replies under it, the bridge and the X are web/src/view/channel's now
// (`createReplyGathering`, `#624 voice-page-modularity`).


function toggleMessageDetails(li, messages) {
  const message = messages[0];
  const open = childByClass(li, "msg-details");
  if (open) {
    li.removeChild(open);
    return;
  }
  const details = document.createElement("div");
  details.className = "msg-details";
  const who = document.createElement("div");
  who.textContent = message.author_is_bot
    ? `${message.author} (bot)`
    : String(message.author || "");
  const when = document.createElement("div");
  when.textContent = fullLocalTime(message);
  // `#320 slack-edited-mark`. When it was last edited, which a phone cannot hover the mark to learn.
  const edited = rowEdited(messages);
  const editedLine = document.createElement("div");
  editedLine.className = "msg-edited";
  editedLine.textContent = edited === null ? "" : edited.at === null ? "Edited" : `Edited ${fullLocalAt(edited.at)}`;
  const id = document.createElement("div");
  id.className = "msg-id";
  // EVERY id, on a row that is showing more than one message. This sheet is the answer to "does
  // that message really exist", and a combined row that named only its first constituent would
  // make the second unreachable by exactly the affordance built to reach it — while also hiding
  // the fact that the row is two messages at all.
  id.textContent = `id ${messages.map((m) => String(m.id)).join(" + ")}`;
  // THE BOOKMARK lives here as well as in the ⋯ menu and on Shift+B, rather than on a gesture of its
  // own. Press-and-hold already opens this sheet, and a second long-press meaning something
  // different from the first would be a gesture nobody could discover and everybody would trigger by
  // accident. `#240 reading-bookmark`.
  const mark = document.createElement("button");
  mark.className = "chip";
  mark.setAttribute("type", "button");
  mark.textContent = bookmarkAbove(li) ? "Remove bookmark" : "Bookmark above";
  mark.addEventListener("click", inThisColumn(() => {
    toggleBookmarkAt(li);
    const open = childByClass(li, "msg-details");
    if (open) li.removeChild(open);
  }));
  details.append(who, when, ...(edited === null ? [] : [editedLine]), id, mark);
  li.append(details);
}

/**
 * Do the reader's OWN messages START read?
 *
 * ON unless they have said otherwise. A message this bridge posted is one the owner dictated
 * moments earlier: it is read by the only definition that matters, and leaving it in the queue
 * makes the queue partly a record of things he said rather than things waiting for him.
 *
 * `#549 own-read-stored`. WHERE A MESSAGE'S READ STATE STARTS, and nothing more. The owner,
 * 2026-10-26: "do what the setting says which is to mark them as read IE record the state that they
 * are read and then I can toggle one individually ... it's just a question of whether the state starts
 * as read or starts as unread when the message is ingested". The server decides it once, at the first
 * sight of each message of his (`src/own_read.rs`), and records it as the one read bit, Done; from
 * then on the row is drawn from that bit like any other, with Done or Unarchive, and every count here
 * asks it. Turning the setting off or on changes only messages first seen afterwards; "Apply to
 * existing messages" beside it reaches the rest (`ownReadApply.apply`). What this page does with
 * the flag itself is draw a message of his read before the server's answer for it arrives
 * (`noteStartsRead`).
 *
 * On at first, and applied as the device keeps it as the page loads, where ABSENT MEANS ON: the default is
 * the behaviour, not merely the initial value. One owner in web/src/core/settings/switches.ts
 * (`#624 voice-page-modularity`).
 *
 * @type {import("./src/core/settings/index.ts").KeptSwitch}
 */
const ownReadSwitch = createKeptSwitch({
  setting: DEVICE_SETTINGS.markOwnRead,
  deviceSettings: () => deviceSettings(),
  initial: true,
  render: (on) => messagePreferences.markOwnRead.render(on),
});

/** Settings' apply-to-existing command owns its confirmation and Undo receipt. */
const ownReadApply = mountOwnReadApply({
  lookup: (id) => el(id),
  request: (path, options) => api(path, options),
  setStatus: (text) => setStatus(text),
  selectedChannel: () => selectedChannelId(),
  reloadChannel: (options) => timelineView.loadDiscord(options),
  guard: (run) => guardQuietly(run),
  setTimeout: (run, ms) => setTimeout(run, ms),
  clearTimeout: (timer) => clearTimeout(timer),
});

/**
 * Whether `message` is one of the reader's own whose read state starts read: "Mark my own messages as
 * read" on, not read as noise already, and his by the server's own order (src/own_read.rs
 * `Choice::owns`) — who-is-who first, then the configured accounts (`isOwnerId`), then this bridge's
 * account unless the configuration names it as a coding agent. That last is narrower than how a row
 * is drawn (`bucketFor` draws the bridge's account as "me" regardless), because a guess wider than the
 * server's would draw an agent's message read that the server leaves unread. `#549 own-read-stored`.
 */
function startsRead(message) {
  if (!ownReadSwitch.on() || !message || isNoise(message)) return false;
  const id = String(message.author_id || "");
  if (!id) return false;
  const chosen = whoIsWho.choiceOf(id);
  if (chosen) return chosen === "me";
  if (whoIsWho.isOwnerId(id)) return true;
  return whoIsWho.isSelfId(id) && !whoIsWho.codingAgentsOf(messageChannel(message)).ids.has(id);
}

/**
 * Draw each of `messages` that `startsRead` read now, before the server's answer for it arrives: one
 * this page just sent, or one of his the stream brought. The server decided it at the same first
 * sight, and records it (`src/own_read.rs`), so this only spares a row of his a moment drawn unread
 * between its arrival and the read that follows. Held as a write made here, settled at once
 * (`localReadWrites`): a read sent before it does not undo it, and a read sent after it speaks for the
 * message, so the server's answer is the one that stands. A message the page already holds keeps
 * the state its reads gave it. `#549 own-read-stored`.
 *
 * @param {any[]} messages
 */
function noteStartsRead(messages) {
  const fresh = messages.filter((message) => startsRead(message) &&
    !(providerFacts.threading() && heldMessage(String(message.id))) && !readLedger.written(String(message.id)));
  if (fresh.length === 0) return;
  const epoch = readLedger.bump();
  const here = String(selectedChannelId());
  for (const message of fresh) {
    const id = String(message.id);
    readLedger.seed(id, epoch);
    if (String(message.channel_id) !== here) continue;
    if (providerFacts.threading() && channelStore.current().channel === here) channelStore.current().dismissed.add(id);
    doneState().archived.add(id);
  }
}

/**
 * Does this message belong in the list behind Hide read?
 *
 * ONE definition, for both places the threaded view filters. Not archived, not read automatically,
 * and not the reader's own words when they have said those are read. And for what Fold read
 * collapses and what entering a thread lands on (`#221 read-modes`, `rowIsRead`): a refinement of
 * what counts as unread belongs here, and reaches every read mode at once.
 *
 * `done` is the read state asked: the marks for the rows on screen by default, and the store's for
 * a reply Main does not draw (`#222 unread-replies`, `replyTally`).
 */
function stillToDo(message, done = doneState().archived) {
  return !done.has(String(message.id)) && !isNoise(message) && !hiddenEmpty(message);
}


/**
 * Are empty messages left out? ON unless the reader has turned it off (`#386 hide-empty-messages`).
 * One owner in web/src/core/settings/switches.ts (`#624 voice-page-modularity`).
 *
 * @type {import("./src/core/settings/index.ts").KeptSwitch}
 */
const hideEmptySwitch = createKeptSwitch({
  setting: DEVICE_SETTINGS.hideEmptyMessages,
  deviceSettings: () => deviceSettings(),
  initial: true,
  render: (on) => messagePreferences.hideEmpty.render(on),
});

/**
 * Is the combining on? ON unless the reader has turned it off. One owner in
 * web/src/core/settings/switches.ts (`#624 voice-page-modularity`).
 *
 * @type {import("./src/core/settings/index.ts").KeptSwitch}
 */
const combineSwitch = createKeptSwitch({
  setting: DEVICE_SETTINGS.combineMessages,
  deviceSettings: () => deviceSettings(),
  initial: true,
  render: (on) => messagePreferences.combine.render(on),
});
readAloudOwner().initializeReadSteerOffset();

// What a finger or the mouse does on a row — a tap, a hold, a swipe — and the one claim a gesture puts on
// the click behind it are web/src/view/channel's (`createRowGestures`: SWIPE_START_PX, SWIPE_COMMIT_PX,
// HOLD_MS; `#624 voice-page-modularity`); what each act does is here.
const rowGestures = createRowGestures({
  inThisColumn: (fn) => inThisColumn(fn),
  showingLinks: (row) => showingLinks(row),
  preparedBlocksOf: (row, part) => readAloudOwner().preparedBlocksOf(row, part),
  readingMode: () => readAloudOwner().readingMode,
  nowPlaying: () => readAloudOwner().nowPlaying,
  jumpReading: (index) => readAloudOwner().jumpReading(index),
  readAloud: (ids) => guardQuietly(() => readAloudOwner().readAloud(ids))(),
  idsOf: (row) => idsOf(/** @type {any} */ (row)),
  toggleDetails: (row, messages) => toggleMessageDetails(row, messages),
  swiped: (row, ids, dx) => {
    // THE SAME ACT the Done button performs, not a second notion of "dealt with": one dismissal,
    // recorded on the server, with `#50`'s undo behind it. That is what makes this gesture safe
    // enough to be a gesture — a swipe that did something only this browser remembered, with no
    // way back, would be the worst control on the page.
    //
    // A swipe on an ALREADY archived row puts it back, so the gesture is its own undo on the row
    // the reader is looking at. Symmetric deliberately: a gesture that only ever went one way
    // would make the greyed rows a trap.
    if (selection().view === "thread" && dx > 0) guardQuietly(closeThread)();
    else guardQuietly(() => toggleArchived(ids, row))();
  },
});
const { tapRow, actOnRow, foldOnTap, swipeable } = rowGestures;
/**
 * One message's body as DOM (web/src/view/channel `createMessageBodies`,
 * `#624 voice-page-modularity`): the server's rendering inserted as nodes, plain text as
 * paragraphs, the heading's restated mark, and what a body links to.
 *
 * @type {import("./src/view/channel/index.ts").MessageBodies}
 */
const messageBodies = createMessageBodies({ wireMessageLinks });
const { insertSanitizedHtml, plainParagraphs, messageBody, styled, textBody, anchorsIn, messageText, messageLinks, renderBodyInto, restatedMarked, headingPieces, cardEllipsis, appendCardEllipsis, prependCardEllipsis, unlinkCardBody } = messageBodies;
/**
 * One channel row's dress (web/src/view/channel `createRowDress`, `#624 voice-page-modularity`).
 *
 * @type {import("./src/view/channel/index.ts").RowDress}
 */
const rowDress = createRowDress({
  outboxEntries: () => outbox.values(),
  drawThreadChips, drawDoneFlash, drawRowPin: (row) => drawRowPin(row), whoBucket: (authorId, bot, viewing) => whoIsWho.bucketFor(authorId, bot, viewing),
  archivedIds: () => doneState().archived, isNoise, notNoiseButtonOf,
  readAloudNow: () => { const owner = readAloudOwner(); return { nowPlaying: owner.nowPlaying, pendingRead: owner.pendingRead, readingLabel: () => owner.readingLabel(), markReadingBlock: (row) => owner.markReadingBlock(row) }; },
  unpackOnScreen: () => unpackOnScreen(), inColumn, replyTally, selectedChannelId, idsOf, rowMessages, columns, leftEl,
  isFolded: (entry) => isFolded(/** @type {any} */ (entry)), setFolded: (entry, folded) => setFolded(/** @type {any} */ (entry), folded),
});
const { answeredBy, dressChannelRow, dressUnpackedRows } = rowDress;

// Drawing a channel row is web/src/view/channel's now (`createChannelRows`, `#624
// voice-page-modularity`): empty messages, split posts as one, the row and everything on it,
// its menu, marking read through it, copying, and the channel summary line. Opening a reply
// stays the page's and is handed in.
const channelRows = createChannelRows({
  byId: el,
  inThisColumn,
  threadOf,
  heldThreads: () => channelStore.current().threads,
  heldMessages: () => channelStore.current().messages,
  hideEmptyOn: () => hideEmptySwitch.on(),
  combineOn: () => combineSwitch.on(),
  listedMessages,
  preservingScroll,
  renderChannelRows,
  renderScrollTools,
  renderBodyInto,
  headingOf,
  restatedMarked,
  headingLength,
  headingPieces,
  replyArrow,
  pinned: () => rowDrawing.pinned(),
  main: () => rowDrawing.main(),
  foldable,
  tapRow,
  sayRowShortcut,
  openReply: (messages, from, quote, focus) => openReply(messages, from, quote, focus),
  takeQuote: (button) => takeQuote(button),
  draftMarker: (messages, li) => draftMarker(messages, li),
  replyingTo: (message) => replyingTo(message),
  markReplying: (row, on) => markReplying(row, on),
  addThreadDecoration,
  providerName: () => channelDirectory.providerName(),
  upstreamReadMark: () => providerFacts.upstreamReadMark(),
  scheduleBackSync,
  guardQuietly,
  markReadUpstream,
  storedSlackOpen: () => deviceSettings().get(DEVICE_SETTINGS.slackOpen),
  isPinnedId: (id) => isPinnedId(id),
  togglePin: (messages) => togglePin(messages),
  toggleBookmarkAt,
  copyDeepLink,
  toggleArchived,
  markNotNoise,
  swipeable,
  searchable,
  anchorsIn,
  drawLinkedOn: (li, ids) => linkedMark.drawOn(li, ids),
  idsOf,
  archived: () => doneState().archived,
  onTheirWay: () => [...doneSaving.keys()],
  flashDone,
  unflashDone,
  dismissMessages,
  throughBump,
  doneSaved,
  doneHeldHere,
  selectedChannelId,
  setStatus,
  currentChannelId: () => currentChannelId(),
  channels: () => channelDirectory.channels(),
  providers: () => channelDirectory.providers(),
  serviceLabel,
  providerOfChannel,
  chatServiceName,
  copyRefused: COPY_REFUSED,
  replyButtonTitle: REPLY_BUTTON_TITLE,
  historyReach: () => historyReach(),
  closeOpenMenu,
  clipboard: () => navigator.clipboard,
});
const {
  rowMenus, emptyMessage, hiddenEmpty, glom, regroupChannelRows, channelRowFrame,
  discordNode, closeRowMenuOutside, closeRowMenuOnEscape, markReadThroughHere,
  storedSlackOpen, upstreamServiceName, copyUpstreamLink, copyMessageText,
  writeClipboardText, channelSummary,
} = channelRows;

/** Walking back through the channel: web/src/view/channel's now (`createOlderPages`, `#624`). */
const olderPagesOwner = createOlderPages({
  activeColumn: () => columnWork.active(), el: (id) => el(id),
  transcript: () => voiceTranscript(),
});
const { olderPages, olderPager, plainReach, renderChannelSeam } = olderPagesOwner;

// --- searching further back ------------------------------------------------------------------------
//
// `#262 search-load-older`. The owner, 2026-10-12: "searching for a term XYZ in two column view found
// zero hits; in one column, scrolling up while searching loaded older pages and found 3 ... 2 column
// view should not PRECLUDE loading hits". The search filters what is loaded, and a step back was fired
// only by the list's scroll position reaching its head, which under a filter nothing bounded: measured in
// Chromium, a search that emptied the list chained steps back through the channel's whole history, each
// page drawn and hidden moving the position enough to fire the next; on Main, which holds no replies,
// that is a walk to the start that finds nothing. The step back meanwhile said "Older messages" and
// nothing about finding. So the scroll's own step gives way while a filter chooses rows
// (`maybeLoadOlder`), and the search's walk below is the one way back, bounded, and the reader's.
//
// WHILE A FILTER CHOOSES ROWS (a term, Links or whose messages), each column's step back says "Search
// before Oct 3, 9:02 AM", the oldest message that column reaches back to (to the minute since `#578
// search-status-clarity`). A tap on it, or Enter in the search field, walks
// back AT MOST `SEARCH_OLDER_PAGES` older pages within `SEARCH_OLDER_MS`, and stops at the end of the
// page that brings `SEARCH_OLDER_HITS` new matches. Every tap is the reader's consent to that much and
// no more: nothing chains pages on its own. Meanwhile the control says "Fetching… 50 older messages · Stop",
// and a tap on it stops the read on the wire. Any change to the search ends the walk, and so do closing the
// right column's thread, leaving two columns, another channel or view, a jump, and a filter change's
// own walk back (`walkToFilterAnchor`). One walk at a time on the page.
//
// WHAT IT FOUND is counted only among messages older than where it started, so a message arriving
// live, or a poll, can neither stop it nor be reported as found; and in messages, as the count is. It
// is said once, under the control, until the search changes (`searchNote`), with what was fetched and
// from where, or why nothing could be (`searchWalkNote`). The matches appear under
// the control, which holds still while the walk runs, with `#250 date-separators`' line saying how many
// unmatched messages are between, and `#249 search-highlight`'s marks on them.
//
// THE PAGE ONLY. No route is new, and nothing reaches the server that a scroll up would not have sent.

/** How many older pages one tap of "Search before …", or Enter in the search field, reads at most. */


/** How many new matches end a walk, at the end of the page that brings them: the owner's "found 3". */

/** How long after a walk starts a tap on its control is the same tap again, and not Stop, in milliseconds. */




// --- how far back, with no gaps -------------------------------------------------------------------
//
// `#578 search-status-clarity`. The owner, 2026-10-27, of "Nothing in the 82 messages loaded so far,
// back to yesterday": "It doesn't explain what are these 82 messages loaded. Is that a dense
// chronological history from time T0-present? Or just whatever random sparse set we have loaded? I
// think a dense suffix of messages is the easiest to reason about. You could even say that we have 'X
// messages back to DATETIME with no gaps, plus Y scattered messages before that.'" So the channel's
// summary, the sentence over a search that matched nothing, and what a walk further back found all say
// it in those terms, from one account of the list (`historyReach`), to the minute: a day alone did not
// move when a press brought fifty more messages of the same day, and the owner saw "THE SAME button".
//
// A STRETCH WITH NO GAPS IS ONE THE READS PROVED. Where the chat service has threads, the view's cover
// in the channel's store says how far back its pages reach (`coverView`): each step back is read from
// the cursor the one before it handed over, and a newest read that reaches down into what an earlier
// read carried joins it. One that does not — the page was away while more than a page of messages
// arrived — leaves a gap the rows do not show, and the cover keeps where it is (`cover.dense`): the
// messages above it are the stretch, and those below are SCATTERED. Without threads, the plain page's
// newest read does the same (`PlainReach.gapFrom`). Rows saved on this device that no read has checked claim
// no stretch at all, nor does a list no read has said anything of.
//
// ALL ON A CHAT SERVICE WITH THREADS IS TOP-LEVEL MESSAGES FIRST. Its pages hold every top-level message
// and, on Slack, the replies of at most ten of their threads; and a reply posted since in a thread
// started further back is in no page of the stretch. So All claims its top-level messages, and says of
// the threads started in the stretch how many it holds every reply of (`exactReplyCount`): "every
// top-level message back to Oct 26, 3:14 PM (no gaps), and every reply in 20 of their 31 threads". It
// is whole only when it reaches the start and holds every reply of every thread: the "112 messages …
// all of it is loaded" the owner was shown on 2026-10-25 was the start of the top-level messages alone
// (`#521 server-side-search`, its Phase 0).
//
// WHERE: the search runs in this browser, over what is loaded, and a walk further back fetches older
// messages from the chat service through the server. The server's own search of its message cache,
// and the chat services' search, are designed in `#521 server-side-search` and not built, so nothing
// here says either searched.

/** @typedef {import("./src/present/notices/index.ts").HistoryReach} HistoryReach */

// The server's copy, searched, is web/src/view/channel's now (`createServerSearchCopy`,
// `#624 voice-page-modularity`): which search is on screen and its request, gathering older
// history on request, and searching the threads' replies further back over the cards. Drawing
// what came back stays olderwalks'.
const serverSearchCopy = createServerSearchCopy({
  factories: { createServerSearch, createGathering, createRepliesWalk },
  clock: () => browserPorts().clock,
  byId: el,
  leftById: leftEl,
  selectedChannelId,
  query: () => searchField.query(),
  oldestLoadedMs: () => oldestLoadedMs(),
  fetchAnswer: (path) => apiDecoded("SearchResponse", path),
  renderServerSearch: () => renderServerSearch(),
  guardQuietly,
  cacheOpen: () => serverCacheBox.open(),
  tokenScope: () => serverFacts.tokenScope(),
  prioritize: (channel) => apiDecoded("ChannelHistoryState",
    `/api/v1/channels/${encodeURIComponent(channel)}/history/prioritize`, { method: "POST" }),
  fetchHistoryState: (channel) => apiDecoded("ChannelHistoryState", `/api/v1/channels/${encodeURIComponent(channel)}/history`),
  scheduleServerSearch: () => scheduleServerSearch(),
  cardsOnScreen,
  inColumn,
  syncUnreadReplies: () => syncUnreadReplies(),
  saveChannelScope,
  renderSideColumn,
  serviceWords: () => serviceWords(),
  failureWords,
  repliesFloor: () => repliesFloor(),
  olderThreadsToRead,
  searchOlderPages: SEARCH_OLDER_PAGES,
  searchOlderMs: SEARCH_OLDER_MS,
  searchOlderHits: SEARCH_OLDER_HITS,
  searchOlderTapMs: SEARCH_OLDER_TAP_MS,
  repliesOffered: () => repliesSearchOffered(),
  endSearchWalk: (column) => endSearchWalk(/** @type {any} */ (column)),
  endFilterWalk,
  mainColumn: () => columns.main,
  sideColumn: () => columns.side,
  allCover: () => allCover(),
  repliesKey: () => repliesKey(),
  canon: () => channelStore.current(),
  currentCanon: () => channelStore.current(),
  heldNow: (canon) => heldNow(/** @type {any} */ (canon)),
  readTimeline: (path, options) => timelineReader.readTimeline(path, options),
  pageLimit: DISCORD_PAGE_LIMIT,
  newerPage,
  foldLateTimelinePage: (p, r) => foldLateTimelinePage(/** @type {any} */ (p), /** @type {any} */ (r)),
  placeFromPage: (p, c, n, h) => placeFromPage(/** @type {any} */ (p), /** @type {any} */ (c), n, /** @type {any} */ (h)),
  followPlacement,
  foldTimelinePage: (p, o, view, thread) => foldTimelinePage(/** @type {any} */ (p), o, view, /** @type {any} */ (thread)),
  heldViews: () => channelStore.current().views,
  flatViewKey: () => viewKey("flat"),
  sentNumberOf,
  setStatus,
  searchParsed,
  messageMatches,
  threadOf,
  sideMode: () => columns.side.mode,
  sideUp: () => sideColumnUp(),
  allLoaded: () => allLoaded(),
  timeOfFirst: (payload) => timeOf((/** @type {any} */ (payload).messages || [])[0], "timestamp"),
  renderRepliesSearch: () => renderRepliesSearch(),
});
const serverSearchOwner = serverSearchCopy.owner;
const gathering = serverSearchCopy.gathering;
const repliesWalks = serverSearchCopy.repliesWalks;
const threadDirectoryReadAt = threadList.directoryReadAt;
const { serverCopyOpen, searchFurtherRunning, stopRepliesWalk, repliesControlPressed, sideCardsShown, cardsFound } = serverSearchCopy;

// --- pulling the channel down to refresh it -----------------------------------------------------
//
// `#68 pull-to-refresh`. The owner found the channel hours out of date and reached for the gesture
// his thumb already makes: "especially when I swipe up on this view and it shows me something very
// stale." `4e3d850` fixed the staleness itself — the view re-reads on entry and polls while it is
// up — and that covers being stale AND WAITING. It gives no way to say "refresh, NOW".
//
// THE TWO GESTURES AT THIS END OF THE LIST ARE TOLD APART BY WHERE THE LIST RAN OUT, and the rule
// is two sentences long because it has to leave BOTH of them reachable on a channel that still has
// history above the reader — which is every channel this issue is about:
//
//   1. A FINGER ON THE GLASS SUSPENDS THE AUTOMATIC STEP BACK (`maybeLoadOlder` above). Deferred,
//      not dropped: it is taken the moment the finger lifts.
//   2. THE PULL'S TRAVEL IS MEASURED FROM WHERE THE LIST RAN OUT, never from where the finger
//      landed. An overscroll begins at the edge, so the pixels the finger spent getting to the top
//      are scrolling and only the pixels after it are a pull.
//
// Together those give the reader one continuous motion for each meaning, and neither is a mode:
//
//   * drag up through the history — the list scrolls; if the finger lifts within OLDER_TRIGGER_PX
//     of the top the deferred step fires and the walk back continues, `#65 scrollback-paging`;
//   * drag DOWN until the list runs out and keep going — the extra PULL_ARM_PX past the edge is
//     the overscroll, and that is the pull. `overscroll-behavior: contain` on #scroll-area is what
//     leaves that overscroll to this page rather than letting the browser's own pull-to-refresh
//     reload the whole application.
//
// The earlier reading of this — "judged by the scroll position at touchstart" — is what rule 2
// replaces, and it was wrong in a way no test then reached: arriving at the top of a paged channel
// fires the step back, `preservingScroll` restores the reader to a positive offset, and so
// `scrollTop === 0` at touchstart was a state a reader on a channel with history could never be
// in. The gesture existed only once the whole channel had been walked back.
//
// Rule 2 keeps what that reading got right, and for the reason it was chosen: a flick started a
// little below the top runs the list out within a single frame, so the first touchmove the page
// sees already reports zero. Anchoring at the edge means that flick has travelled nothing yet —
// judging it from where the finger IS would turn ordinary scrolling into a refresh.
//
// `keepPosition: false`, unlike Refresh and unlike the poll. This is a gesture made AT THE TOP of
// the history asking for what is new, and "keep my place" there means "stay at the oldest thing
// you have loaded", which is the opposite of the request. The button keeps its place because it
// is pressed from wherever the reader happens to be reading.
//
// ...AND PULLING UP PAST THE NEWEST MESSAGE. `#188 pull-refresh-bottom`. The same gesture at the
// other end of the list, which is the end the owner actually reads at: "I want pull UP to trigger
// refresh even IF messages were loaded in the past." The pull above answers only a reader at the
// TOP. Before anything has loaded, that is everyone — an empty list is at its top — which is why
// it seemed to work then; once a channel has loaded, the reader is parked at the newest line, a
// whole history away from it. Bouncing against the bottom did nothing, and showed nothing.
//
// ONE GESTURE, TWO EDGES, and the edge is whichever end of the list ran out under the finger. A
// list too short to scroll is at both ends at once, and there the direction decides: down is the
// top's pull, up is the bottom's. The axis test, the second-finger refusal, the threshold and the
// refusal during a step back are shared, because a second gesture beside this one would be a
// second set of rules about what counts as a pull, and the two would drift. What differs, and why:
//
//   * A PULL UP MUST BEGIN WITH THE READER PARKED AT THE NEWEST LINE — `atBottom` at touchstart,
//     the same test that decides whether a re-read follows them. The top cannot ask this (rule 1:
//     the step back makes "already at the top" unreachable on a paged channel), but nothing pages
//     in at the bottom, so the bottom can. It is what keeps a fling from refreshing: a drag that
//     starts in the history and runs the list out is a scroll however far the finger goes after
//     it. Only a reader who stopped at the bottom and pulled again is asking.
//   * ITS TRAVEL IS STILL MEASURED FROM WHERE THE LIST REALLY RAN OUT, which is rule 2 — to within
//     PULL_EDGE_SLACK_PX, not `atBottom`'s thumb-sized slack. The pixels inside that slack still
//     scroll, and pixels that scroll are not a pull.
//   * `keepPosition: true`, like the poll. The reader is AT the newest line, which is the one place
//     "keep my place" and "show me the newest" are the same instruction — and keeping it means a
//     refresh does not throw away the history they walked back through to get here.
//   * A READ ALREADY IN FLIGHT IS THE REFRESH. A poll, the stream's catch-up or the cold start's
//     re-read is reading what the reader asked for, so the release waits for it and reports it
//     rather than putting a second read on the wire behind it — whether it was already on the wire
//     when the finger landed or began while the finger was down. The top's pull is NOT offered
//     over a read in flight, as before: its release asks `loadDiscord` directly, which can only
//     queue behind that read and return at once, and it would report a refresh that had not
//     happened. The foot has to be offered there all the same. On a weak signal a channel read can
//     hang for as long as the connection stalls, which is just when the owner bounces against the
//     bottom, and a pull that showed nothing then would be "I don't get a refresh that shows in
//     the UI" all over again.
//   * WHAT IT REPORTS IS ABOUT THE VIEW IT WAS ASKED OVER, AND FROM THE RELEASE ON. A reader on a
//     slow read taps another tab, another thread or another channel; the read they pulled for is
//     then folded away for later, and the freshness the page restores belongs to the view they
//     went to. Said at the foot, that would be "Updated" at a time the reader never refreshed,
//     with "new messages" for rows that arrived with a different view. So the result is dropped
//     once the view is not the one pulled, and "Updated" needs a read that landed since the
//     release.
//   * IT SAYS WHAT IT FOUND, ITSELF. At the top the affordance gives way to the freshness pill,
//     which then says "Updated 07:15" or "Refresh failed" in the same place. From the bottom that
//     pill is a screen away, so the affordance says it, for PULL_RESULT_MS, and goes.
//
// Chrome's OWN pull-to-refresh is not a concern at either end, and must not become one: it
// reloads the whole application — the live stream, a call, the reader's place — to re-read a list
// this page can re-read by itself. `overscroll-behavior: none` on the document and `contain` on
// #scroll-area already keep every overscroll on this list: `contain` rather than `none`, so the
// platform's own stretch at the edge still shows, which is the bounce the owner described and a
// fair cue that the list has run out.
//
// The gesture itself — the touch it follows, its thresholds (PULL_ARM_PX, PULL_EDGE_SLACK_PX,
// PULL_RISE_PX, PULL_RESULT_MS, PULL_WAIT_MS) and the affordance's words (PULL_LABELS) — is
// web/src/view/channel's (`createPullToRefresh`, `#624 voice-page-modularity`); what a refresh reads is
// here.
const pullToRefresh = createPullToRefresh({
  byId: (id) => /** @type {any} */ (el(id)),
  scrollArea: () => el("scroll-area"),
  onChannelList: () => screens.screen() === "main" && screens.view() === "discord",
  inChannelView: () => screens.view() === "discord",
  atBottom: (area, slack) => (slack === undefined ? atBottom(area) : atBottom(area, slack)),
  fetchInFlight: () => channelReads.busy(),
  olderInFlight: () => olderPages().inFlight,
  takeDeferredOlder: () => takeDeferredOlder(),
  undeferOlder: () => {
    olderPages().deferred = false;
  },
  loadDiscord: (options) => timelineView.loadDiscord(options),
  newestId: () => listState().newestId,
  setStatus: (text) => setStatus(text),
  channelContextKey: () => channelContextKey(),
  freshness: () => loadState(),
  renderFreshness: () => renderChannelFreshness(),
});
const { renderPull, clearPullOffer, pullStart, pullMove, pullEnd, pullCancel } = pullToRefresh;

// A step back that `maybeLoadOlder` refused because a finger was down, and owes the reader: a
// column's `olderPages().deferred`.

// The id of the newest message the channel list is currently showing, or null. Kept per column: `listState().newestId` (`freshListState`, `#624 voice-page-modularity`).

/**
 * Settle the reader's position and the jump-newest chip after a read that replaced the list.
 *
 * ONE function for both reads, and that is the point: `loadTodo` had no version of this at all, so
 * in the one view whose entire purpose is surfacing new work a background poll could add a row off
 * screen and say nothing, while the unfiltered channel beside it raised the chip correctly. A
 * second mechanism for the filtered list would be a second set of rules about when the chip is
 * earned, and they would disagree.
 *
 * Something ARRIVED only if the newest id MOVED. A refresh that returns the same messages must not
 * raise the chip, or a background poll every 45 seconds would offer to jump the reader to a bottom
 * that has not changed since the last time they declined.
 *
 * `ownAct` is for the read that FOLLOWS something the reader just did — a dismissal, an undo. Those
 * change which row is newest without anything arriving, and a chip saying "something arrived" in
 * answer to the reader's own tap is a lie about where the row came from.
 *
 * `heldPlaceAt` is for a read that replaced rows a view switch drew from the store (`#220
 * view-switch-instant`): the time of the message the reader was on, NaN when unknown. When the read
 * does not reach back to it, the old offset would point at whatever the read put there instead.
 *
 * @param {Array<{id: string}>} messages the list as it was just read, oldest first
 * @param {{keepPosition: boolean, wasAtNewest: boolean, area: object, previousTop: number,
 *          anchorId?: string|null, anchorTop?: number, around?: ListAround | null, ownAct?: boolean,
 *          heldPlaceAt?: number}} how
 */

/**
 * Where the reader is as a read LANDS, for `settleAfterRead`: measured then, never when the read was
 * sent. `#280 scroll-stability`. The owner, 2026-10-13: "I swear I just saw an instance where it jumped
 * me to the bottom of the screen in the main message view (two column mode)... I don't think I touched
 * anything. I think it was something updating underneath." A read is on the wire for as long as the
 * server takes — seconds, behind a bridge — and every read took the reader's place as it went out:
 * one sent while the reader was on the newest line, who then scrolled up to read, threw them back to
 * the bottom when it landed, and one sent while they were further up put them back where they had been
 * before they scrolled. Measured in Chromium (tests/scroll_stability_browser.py). What was true at the
 * send is kept for one question only: `sentAtNewest`, whether the reader was then on the newest line.
 *
 * A read that would take the reader to the newest message — entering the channel over rows already up,
 * saved on this device or drawn by an earlier read — finds them where they are when it lands too: a
 * reader who was on the newest line when it went out and has scrolled away since is reading, and is
 * left there, with the jump to the newest offered for what arrived. One who was not on the newest line
 * when it went out — a pull at the head of the list, a walk back whose snapshot expired — is taken to
 * the newest as asked, as before.
 *
 * Asked once the in-flow loading line over the rows has gone (`renderChannelLoading(false)`): the rows
 * as they stand without it, which is where a browser's own scroll anchoring has held them as it came
 * and went, and where the reader was before it came.
 *
 * @param {HTMLElement} area
 * @param {boolean} keepPosition
 * @param {boolean} sentAtNewest
 */



/**
 * The cards of the Threads list on screen as a place was read: each card's id and the top of its box, fully
 * shown first, and every card's neighbours. `#612 view-stability`.
 *
 * @typedef {{marks: {id: string, top: number}[], near: Map<string, {prev: string | null, next: string | null}>}} ListAround
 */




// --- the to-do view: what you have not dealt with yet ---------------------------------------------
//
// `#50 todo-view`. A long backlog of assistant messages is a to-do list in practice, and until now
// nothing on this page could tell the ones that still want attention from the ones already handled.
//
// A SUB-TOGGLE of the channel view rather than a third tab, because it is the same list filtered.
// Turning it on reads a DIFFERENT route — `/todo`, which is the recent window minus what has been
// dealt with — rather than filtering the rows already on screen. That is deliberate: the walk-back
// cursor belongs to the unfiltered channel, and a filtered list paged by an unfiltered cursor would
// step over messages without saying so. So `#load-older` is absent in this mode, and the view is
// honestly the recent window.
//
// THE ONE THING THIS MODE HAS TO SAY OUT LOUD, from `#61 unread-status`: this read state is OURS.
// Discord shares none with a bot, so nothing here is read from the Discord app and nothing here is
// written back to it. `#inbox-note` carries the server's own sentence, quoted rather than rewritten
// here, so the page and the server cannot come to describe the posture differently.
//
// WHAT IS NOT HERE, and why. The issue asks for a SWIPE to dismiss and a PRESS-AND-HOLD to declare
// bankruptcy. Those are a gesture layer — horizontal intent disambiguated from vertical, on the one
// list this page scrolls — and they are a change of their own; this lands the acts themselves, each
// reachable by a control a keyboard can also get to, so the gestures become a second way in rather
// than the only way. It also asks for a message to leave the list when it is REPLIED to; that is
// derived state and needs a reply reference on the server's Message, which is a wire-format change.
// Both are follow-ups, and until the second one lands "dealt with" here is always DECLARED — which
// is why nothing in this file has to decide what happens when derived and declared disagree.

// Is the reader looking at the to-do list rather than the whole channel? Kept across a reload with
// the rest of what was on screen, in `UI_STATE_KEY`. `#189 restore-ui-state`. Since `#221 read-modes`
// this is Hide read, the third of the read modes: the mode being "hide", set only with it. It keeps a
// question of its own (`readModes.hiding()`) because every read path already asks it the one question
// it answers, which is whether read messages are left out of the list.
//
// The mode itself, its names (READ_MODES, READ_MODE_LABELS, READ_MODE_VERBS, READ_MODE_SAID,
// READ_MODE_TITLES), its kept copy (READ_MODE_KEY), what an old record means (`savedReadMode`) and the
// cycle (`nextReadMode`) are web/src/present/rows' (`createReadMode`, `#624 voice-page-modularity`):
// `readModes.mode()` is the mode, and `readModes.hiding()` this question.
const readModes = createReadMode({ storage: () => deviceStorage() });

// --- the read modes: Show read, Fold read, Hide read ----------------------------------------------
//
// `#432 read-mode-unify`. The owner, 2026-10-22: Fold read was "Collapse read" until now. "Collapse" is the
// size-bounded message card's word (a long message collapsed to a few lines, `;` and `:`), "fold" the
// run lines' ("… 21 read messages …"). A saved record that says "collapse" means "fold" (`savedReadMode`).
//
// `#221 read-modes`. The owner, 2026-10-08: "since we may have long threads we can introduce
// further collapsing for the read messages in between. I.e show the thread root then ' ... 45 read
// messages ... ' then the expanded next message I want to read. Maybe it could become the new
// behavior of the 'Hide read' feature. That could actually cycle between 3 modes - Show Read,
// Collapse Read, and Hide Read."
//
// So the Hide read button is a cycle, and it names the mode the list is IN, with an icon for each:
//
//   * SHOW READ. Every message, read ones greyed: the list as it always was with Hide read off.
//   * FOLD READ. Each run of read messages is one line, "… 12 read messages …", and a tap on the
//     line opens that run. Unread messages are shown, and so are two read ones: the root of the
//     thread on screen, which says what the thread is about, and the newest message, which is where
//     the conversation stands, what the composer under it answers, and what a send just put there.
//     Replies gathered under their message (`#215 reply-coalesce`) are never collapsed either.
//   * HIDE READ. Read messages are left out, as they always were with Hide read on.
//
// THE ORDER IS ASCENDING: each tap takes more of what is read away, and the third gives it all back.
// The name on the button is then always where a tap has just put the list, and a reader on Hide read
// still turns it off with one tap, as before. SHOW READ IS THE DEFAULT, as Hide read off was: the
// page does not start hiding anything nobody asked it to hide, and a record kept by an older page —
// `todo` alone — still means what it meant.
//
// WHAT IS READ is `stillToDo`'s answer and nobody else's: Fold read folds exactly the
// messages Hide read hides. A refinement of what counts as read reaches both modes from there.
//
// ENTERING A THREAD lands on its first unread message, unfolded — see `landOnFirstUnread` — in every
// mode, and the read run before it is a line only under Fold read. Show read shows what is read:
// a mode that said "Show read" while hiding messages on entry would be a button that lies, and the
// owner's root, "… 45 read messages …", first unread is Fold read's own drawing.
// The read modes' lines and places are web/src/view/channel's now (`createReadRunLines`,
// `#624 voice-page-modularity`): whether Hide read is leaving read messages out, the collapsed
// run's line, and the rows a redraw replaced. The runs themselves stay readcore's.
const readRuns = createReadRuns();

const readRunLines = createReadRunLines({
  byId: el,
  leftById: leftEl,
  inThisColumn,
  selection: () => selection(),
  threads: () => channelStore.current().threads,
  hiding: () => readModes.hiding(),
  suspended: () => readRunsSuspended(),
  threading: () => providerFacts.threading(),
  eachColumnRenderCached: () => eachColumn(renderCachedTimeline),
  openReadRun: (row) => openReadRun(/** @type {any} */ (row)),
  readRunRows: (row) => readRunRows(row),
  threadIdOf: (message) => threadOf(message),
  selectionThread: () => selection().thread,
});
const {
  hidingRead, liftingHideRead, readRunLine, readRunWords, runDaysOf, readRunCount,
  placeInReadRun, isOpenThreadRoot, openThreadRootId, foldedOneIds, liveUnpackedRow, liveCardRow,
} = readRunLines;
const foldedOnes = readRunLines.foldedOnes;


/**
 * The exact set the last dismissal cleared, so the undo restores that and nothing else.
 *
 * Not a count and not "the last N": by the time the reader presses undo, N may name a different
 * set. Held as the server reported it, which is also what makes undoing a BULK clear exact (web/src/view/
 * shell `createUndoOffer`).
 *
 * @type {import("./src/view/shell/index.ts").UndoOffer<any>}
 */
const lastDismissal = createUndoOffer();

/** The Done and put-back writes not yet answered, kept on the device. */
const inboxLedger = createInboxWriteLedger({
  storage: deviceStorageOnUse(),
  now: () => Date.now(),
  makeRoom: () => messageCache.dropMessageCache(),
  // This page load's own name, as the back stack's (`BACK_SESSION`): another page open on the device keeps
  // its own writes.
  page: BACK_SESSION,
});
// Done in Main still showed as not Done in All, loaded earlier in the session, for seconds, until
// some later read redrew it. Each view keeps its own copy of which messages are Done (`doneState().archived`)
// for when it is shown again — a view left in a column (`channelContexts`), and the right column's
// thread while two columns are put away or show their cards — and only the list the Done was made in,
// and the other column while it shows a thread (`mirrorArchived`), were told. Worse, a stale copy put
// back was later written over the store (`syncCanonDismissed`), so the Done could go out of every view.
//
// So a Done or an Undo here, and a failed one taken back, brings every copy into line with the store
// at once (`followDoneEverywhere`), which `markHeldDone` and `settleHeldDone` have already moved; the
// cards on screen, which count unread replies from the store, are drawn again then; and a view whose
// rows were drawn before is drawn again as it is shown, where they say otherwise (`rowsMissDone`).
// No read is asked for any of it.
//
// A Done or an Undo made in the right column reaches the left one through `mirrorArchived`, which
// works only while the right column shows a thread. Its thread closed, or two columns put away, while
// the write was on its way, a refusal taken back in the right column left the left column showing it
// as made, and the next write from the left saved that over the store. So the left column's own copy
// is brought into line here too then, and its rows drawn again where they say otherwise.
//
// The mirror image, found in the fourth review round: a Done or an Undo made in the left column while
// the right column's thread was put away reached the right column's copy (above), but nothing told the
// `mirrorArchived` of the act; so when the right column showed the thread again (its thread opened
// again, or two columns brought back) while the write was on its way, a refusal taken back in the left
// column left the right one showing it as made. A reply from there then wrote no Done, its message
// looking Done already, and asked the chat service to mark a message read whose Done had failed; and
// the next write from the right saved the refused state over the store. So the right column showing a
// thread is brought into line here too, after an act made in the left column.

/**
 * `copy`, a copy of which messages are Done, with `ids` brought into line with the store's
 * (`channelStore.current().dismissed`): `copy` itself where none of them differs, else a new set, since `copy`
 * may be the very set a list on screen is drawn from.
 *
 * @param {Set<string>} copy
 * @param {string[]} ids
 * @returns {Set<string>}
 */
function caughtUpDone(copy, ids) {
  const moved = ids.filter((id) => copy.has(id) !== channelStore.current().dismissed.has(id));
  if (moved.length === 0) return copy;
  const next = new Set(copy);
  for (const id of moved) {
    if (channelStore.current().dismissed.has(id)) next.add(id);
    else next.delete(id);
  }
  return next;
}

/**
 * Every copy of the read state of `ids` that the page holds out of sight, into line with the store's
 * (`channelStore.current().dismissed`), after a Done, a put back, or a failed one taken back: the views held in
 * either column, and the right column's own while it does not show a thread. Each copy is replaced
 * rather than changed in place, since one may be the very set a list on screen is drawn from. Then
 * the cards on screen, drawn from the store, are drawn again. `#306 done-stale-across-views`.
 *
 * `madeIn` is the column the Done or the Undo was made in. Made in the right column, which no longer
 * shows a thread (its thread closed, or two columns put away, while the write was on its way), the
 * left column on screen is brought into line as well, and drawn again: `mirrorArchived`, which tells it
 * while the right column shows a thread, does nothing then. Made in the left column, the right column
 * showing a thread is brought into line, and drawn again where its rows say otherwise: a thread shown
 * again since the act (its thread opened again, or two columns brought back) holds the copy brought
 * into line while it was put away, which the `mirrorArchived` of the act never changed, so the
 * `mirrorArchived` taking it back changes nothing in it.
 *
 * @param {string[]} ids
 * @param {ColumnFields} madeIn
 */
function followDoneEverywhere(ids, madeIn) {
  if (!providerFacts.threading() || channelStore.current().channel !== String(selectedChannelId())) return;
  const named = ids.map(String);
  /** @param {Set<string>} copy */
  const caughtUp = (copy) => caughtUpDone(copy, named);
  for (const held of channelContexts.values()) held.dismissed = caughtUp(held.dismissed);
  if (!sideShowing() && columnWork.active() !== columns.side && columns.side.left) {
    inColumn(columns.side, () => {
      doneState().archived = caughtUp(doneState().archived);
    });
  }
  if (madeIn === columns.side && !sideShowing()) {
    // The cards, drawn at the end of this, dress the rows of what changed among them.
    cardsDrawn.whileDressedAfter(sideColumnUp(), () => {
      inColumn(columns.main, () => {
        doneState().archived = caughtUp(doneState().archived);
        // Its rows, or under Hide read which rows it draws; else Main's roots' counts of unread replies.
        if (rowsMissDone()) renderCachedTimeline();
        else syncUnreadReplies();
      });
    });
  }
  if (madeIn !== columns.side && sideShowing()) {
    inColumn(columns.side, () => {
      doneState().archived = caughtUp(doneState().archived);
      // Its rows, or under Hide read which rows it draws. A thread draws no counts of unread replies.
      if (rowsMissDone()) renderCachedTimeline();
    });
  }
  if (sideColumnUp()) renderSideColumn();
}

/**
 * Whether the rows drawn in the column being worked on show a message's read state other than its own
 * copy (`doneState().archived`) says: a row drawn Done, or not, against it, or under Hide read a message drawn,
 * or left out, against it. Rows held while a Done or an Undo was made in another view keep the look
 * they had; their copy was brought into line at once (`followDoneEverywhere`), and this says they are
 * to be drawn again as they are shown. `#306 done-stale-across-views`.
 */
function rowsMissDone() {
  if (!providerFacts.threading() || selection().view === "threads") return false;
  const rows = [...el("discord-log").children];
  if (rows.some((row) => {
    const ids = idsOf(row);
    return ids.length > 0 && (row.getAttribute("data-archived") === "true") !== ids.every((id) => doneState().archived.has(id));
  })) return true;
  if (!hidingRead()) return false;
  const keeps = readFilterKeeps();
  const wanted = new Set(timelineRows().messages.filter(keeps).map((message) => String(message.id)));
  const drawn = new Set(rows.flatMap(idsOf));
  return wanted.size !== drawn.size || [...wanted].some((id) => !drawn.has(id));
}

/** Put back exactly what the last dismissal cleared, and the bookmark where that moved it from. */
async function undoDismissal() {
  if (lastDismissal.current() === null) {
    return;
  }
  const undoing = lastDismissal.current();
  lastDismissal.set(null);
  renderTodoControls();
  const restoring = restoreMessages(undoing.messages);
  // `#240 reading-bookmark`. Back where it was, unless something moved it since.
  if (undoing.bump) await undoBookmarkBump(undoing.bump, restoring);
  else await restoring;
}

/** Declare bankruptcy on everything currently in the list. Two taps, and it says the count. */
async function clearBacklog() {
  const rows = [...el("discord-log").children];
  if (rows.length === 0) {
    return;
  }
  // MESSAGES, not rows: what the server is about to clear is Discord messages, and a combined row
  // is more than one of them. A count of rows would promise to clear fewer than it did. In time
  // order, so the newest is last even with replies gathered above older rows (`#215 reply-coalesce`).
  const messages = listedMessages().map((message) => String(message.id));
  if (!backlogIsArmed()) {
    armBacklog();
    setStatus(`Tap again to clear ${messages.length} — undo will be offered afterwards.`);
    return;
  }
  disarmBacklog();
  // THROUGH the newest row on screen, so the server decides the boundary from its own ordering
  // rather than from a list of ids this page assembled. The boundary is included.
  //
  // ...and WITH the window this page read the list with. The server resolves `through` against a
  // window of its own, so a boundary sent without one is resolved against the server's default —
  // which, for a client reading a smaller page than the server's default, means clearing messages
  // that were never on screen. The limit is what makes "everything above this row" mean the rows
  // the reader was actually looking at.
  //
  // The boundary is the NEWEST message on screen, which on a combined bottom row is its LAST
  // constituent rather than the id the row is filed under. Sending the row's identity would stop
  // the sweep one message short and leave the trailing overflow in the queue.
  const rowsNow = [...el("discord-log").children];
  await dismissMessages({
    through: messages[messages.length - 1],
    limit: DISCORD_PAGE_LIMIT,
  }, throughBump(rowsNow[rowsNow.length - 1]));
}

/**
 * Put the list in read mode `mode`. `#221 read-modes`. `#243 filter-keeps-selection`: a read mode is
 * a filter like any other, and the whole change — the notes and controls over the list included — is
 * made inside `keepingFilterPlace`: the reader's message stays where it is on the screen, or the line
 * of the run it joins, or the nearest message left in its place.
 */
function setReadMode(mode) {
  if (!READ_MODES.includes(mode)) return;
  const wasHiding = readModes.hiding();
  const read = keepingFilterPlace(() => {
    readModes.set(mode);
    // Runs opened under Fold read are a way of looking at that list, and the reader has just
    // chosen another way: coming back to Fold read folds them all again.
    readRuns.clear();
    // Kept across a reload, as the reader left it. `#189 restore-ui-state`.
    uiState.settle();
    saveUiState();
    // The UI-state record above is the restore; this copy is only the sync's.
    readModes.keep();
    disarmBacklog();
    // An undo belongs to the act it undoes, and leaving the view is the reader moving on. Keeping
    // it would offer to restore messages into a list they are no longer looking at. Showing or
    // collapsing what is read leaves every message in the list, so the undo stays between those two.
    if (readModes.hiding() !== wasHiding) lastDismissal.set(null);
    if (!readModes.hiding()) {
      listState().backlog = 0;
    }
    renderTodoControls();
    if (providerFacts.threading()) {
      // One read mode, for both columns. `#229 desktop-two-column`.
      eachColumn(renderCachedTimeline);
      // `#404 threads-unpack-all`. And inside every unpacked block.
      cardsDrawn.forget();
      if (sideColumnUp()) renderSideColumn();
    } else if (readModes.hiding() !== wasHiding) {
      // A read of another route, which keeps the reader's place as a refresh does and is placed again
      // when it lands. What it brings is the reader's own act, not something that arrived.
      return (readModes.hiding() ? loadTodo : (options) => timelineView.loadDiscord(options))({ keepPosition: true, ownAct: true, readAs: "enter" });
    } else {
      // Show read and Fold read are the same read of the channel, drawn two ways.
      applyReadRuns();
      renderScrollTools();
    }
    return undefined;
  });
  if (read) guardQuietly(() => read)();
  requestVisibleSummaries();
}

// Pinned messages are web/src/view/channel's now (`createPinnedMessages`, `#624
// voice-page-modularity`): the held list, the sync over it, every row's chip, the filter's
// list and its toggle. The list itself is core/annotations'.
const rowDrawing = createRowDrawing();

const currentChannelId = () => String(selectedChannelId() || "");

function freshPinsView() {
  return { only: false, drawn: "" };
}

const pinnedMessagesOwner = createPinnedMessages({
  byId: el,
  leftById: leftEl,
  currentChannelId,
  activePins: () => columnWork.active().pins,
  readPins: (channel) => apiDecoded("PinsResponse", `/api/v1/channels/${encodeURIComponent(channel)}/pins`),
  writePin: (channel, id, request) => queueInboxWrite(() => apiDecoded("PinChangeResponse",
    `/api/v1/channels/${encodeURIComponent(channel)}/pins/${encodeURIComponent(id)}`,
    request ? { method: "PUT", body: request() } : { method: "DELETE" })),
  guard: (run) => guardQuietly(run)(),
  providerName: () => channelDirectory.providerName(),
  eachColumn,
  preservingScroll,
  renderChannelRows,
  renderScrollTools,
  threading: () => providerFacts.threading(),
  heldThreads: () => channelStore.current().threads,
  heldMessages: () => channelStore.current().messages,
  timelineMessages: () => timelineRows().messages,
  combineOn: () => combineSwitch.on(),
  hideEmptyOn: () => hideEmptySwitch.on(),
  scope: () => channelStore.current().scope,
  isNoise,
  renderedHtmlOf,
  headingOf,
  editedDrawn,
  upstreamUrlOf,
  glom,
  discordNode,
  whilePinned: (fn) => rowDrawing.whilePinned(fn),
  keepingFilterPlace,
  renderChannelNavigation,
  renderOlderControl: () => renderOlderControl(),
  requestVisibleSummaries,
  setStatus,
  online: () => navigator.onLine !== false,
  settlePendingFilterPlace,
  currentStoreChannel: () => channelStore.current().channel,
  reactionsDrawn,
});
const {
  pinStore, pinsView, isPinnedId, forgetPins, notePinsRevision, ensurePins,
  drawRowPin, renderPins, syncPinnedList, setPinnedOnly, togglePin,
} = pinnedMessagesOwner;

// --- messages read automatically ------------------------------------------------------------------
//
// `#196 auto-read-noise`. The owner asked for "Working…" placeholders to stop counting as unread.
// The server holds the rules and judges every message it serves against them; what this page does
// is carry that verdict through — `isNoise` — and give the owner the two acts the verdict needs:
// editing the rules, in Settings, and rescuing one message a rule caught by mistake, on its row.
//
// WHAT THIS PAGE DELIBERATELY DOES NOT HAVE is a copy of the matcher. A row is quiet because the
// server said so, the count in Settings counts the server's verdicts, and a rule the owner adds is
// applied by re-reading the channel rather than by matching text here. Two predicates would agree
// until the day one of them changed.

/** Settings' rule editor reads the channel's tally and asks it to refresh after a save. */
const noiseSettings = mountNoiseRules({
  lookup: (id) => el(id),
  request: (path, options) => api(path, options),
  guard: (run) => guardQuietly(run),
  status: (text) => setStatus(text),
  tally: () => noiseTally(),
  selectedChannel: () => selectedChannelId(),
  reloadChannel: () => timelineView.loadDiscord({ keepPosition: true, readAs: "mutation" }),
});

/**
 * How many of the LOADED messages the server calls noise, and out of how many.
 *
 * The rows on screen, or the threaded view's messages — except in the plain to-do list, where the
 * server already left the placeholders out and said how many it left, so that number is the one.
 */
function noiseTally() {
  if (!providerFacts.threading() && readModes.hiding()) {
    return { matched: todoView.state().noise || 0, total: todoView.state().window || 0 };
  }
  const loaded = providerFacts.threading()
    ? timelineRows().messages
    : [...el("discord-log").children].flatMap(rowMessages);
  return { matched: loaded.filter(isNoise).length, total: loaded.length };
}

/**
 * Rescue messages a rule caught by mistake: the server records them as not noise, and every copy
 * of them this page holds is told the same, so the row is ordinary at once rather than after the
 * next read.
 *
 * Not optimistic, unlike Done. Done is undoable and its failure puts back a row the reader can
 * see; a rescue that failed silently would leave him believing a message is safe from a rule that
 * is still hiding it.
 */
async function markNotNoise(ids) {
  const channel = selectedChannelId();
  const rescued = new Set(ids.map(String));
  if (!channel || rescued.size === 0) return;
  await api(`/api/v1/channels/${encodeURIComponent(channel)}/not-noise`, {
    method: "POST",
    body: { messages: [...rescued] },
  });
  /** @type {any[]} */
  const copies = [...channelStore.current().messages];
  // Every column's copies: the message's verdict is not one column's. `#229 desktop-two-column`.
  eachColumn(() => copies.push(...timelineRows().messages, ...[...el("discord-log").children].flatMap(rowMessages)));
  for (const message of copies) {
    if (rescued.has(String(message.id))) message.noise = false;
  }
  if (providerFacts.threading()) eachColumn(() => guardQuietly(refreshAfterInboxChange)());
  else await refreshAfterInboxChange();
  const count = rescued.size;
  setStatus(
    `${count} message${count === 1 ? "" : "s"} no longer read automatically — the rule still applies to the others.`
  );
}

// Voice Settings own the approval switch, prefix and prompt editors (#624 voice-page-modularity).
// The session's pending prompt remains call composition; handing it over records the last call in Settings.
const agentSettings = mountAgentSettings({
  lookup: (id) => el(id),
  decoded: (type, path, options) => apiDecoded(type, path, options),
  writable: () => serverFacts.tokenScope() === "write",
  showError: (text) => showError(text),
  setStatus: (text) => setStatus(text),
  addDetail: (text) => addDetail(text),
  setTimeout: (run, ms) => setTimeout(run, ms),
  clearTimeout: (timer) => clearTimeout(timer),
  clearArmedMs: () => voiceTranscript().CLEAR_ARMED_MS,
});
const agentPrefix = mountAgentPrefix({
  lookup: (id) => el(id),
  decoded: (type, path, options) => apiDecoded(type, path, options),
  writable: () => serverFacts.tokenScope() === "write",
  showError: (text) => showError(text),
  setStatus: (text) => setStatus(text),
});

// --- saved messages on this server -----------------------------------------------------------------
//
// `#208 persistent-cache`. The server can keep its own copy of the channel messages it reads, in a
// file of its own beside its store. That is NOT this browser's saved rows (`readMessageCache`, the
// reader's offline copy): it is the SERVER's file, switched by the owner and measured by the server.
// So, like the voice switch, it is drawn from client-config and redrawn from what each PUT or DELETE
// answered, never from the click.
//
//   * SHOWN ONLY WHERE THE SERVER REPORTS IT. A server whose configuration has no [message_cache]
//     table sends no status, and no server sends one to a read token, which cannot switch it. The
//     page hides it from anything but a write token as well, so a status that reached one anyway
//     shows nothing. The help entry is hidden with the group.
//   * BOTH ERASING ACTS ASK FIRST. Unticking turns saving off and erases the file; Erase empties it
//     and carries on. Each puts its question in `#message-cache-confirm`, the box goes on showing
//     what the server holds until the answer, and only the question's own button sends. Keep sends
//     nothing at all.
//   * TURNING IT ON SENDS AT ONCE: it erases nothing. The server refuses it where its own
//     configuration turns the cache off — the operator's word wins — and the line says why.

/**
 * The group, its question and its requests, and the status the server last reported, which search and
 * the channel box read (web/src/view/settings `mountServerCache`, in the words of
 * web/src/present/settings, `#624 voice-page-modularity`).
 *
 * @type {import("./src/view/settings/index.ts").ServerCache}
 */
const serverCacheBox = mountServerCache({
  lookup: (id) => el(id),
  decoded: (type, path, options) => apiDecoded(type, path, options),
  writable: () => serverFacts.tokenScope() === "write",
  renderFolds: () => settingsFolds.renderAll(),
  renderHistory: () => channelHistoryBox.render(),
});

// --- replying to a channel message -------------------------------------------------------------
//
// Drafts are per target message. Once Send is pressed the captured reply target, destination and
// text move into the outgoing queue, freeing this screen for the next reply immediately. Only
// provider message IDs reconcile a successful send with history; local IDs cannot be replied to.

// What was written in Reply to each message and not sent, kept on the device, and which of those were left
// bound for a new thread, are web/src/core/outbox's (`createReplyDrafts`, `#624 voice-page-modularity`).

/** The reply drafts, by message id. */
const replyDrafts = createReplyDrafts({ storage: deviceStorageOnUse(), makeRoom: () => messageCache.dropMessageCache() });

/**
 * The reply open in the stream and what it was opened with — its message, channel and route, its column and
 * list, whether it is docked, one put away to reopen, the focus a column took from it, its quote, the
 * replies it gathered and the row it is drawn under: one record in web/src/view/channel/replystate.ts
 * (`#624 voice-page-modularity`), which says what each is.
 *
 */
const replySession = createReplySession({
  el: (id) => /** @type {any} */ (el(id)),
  leftEl: (id) => /** @type {any} */ (leftEl(id)),
  mainColumn: /** @type {any} */ (columns.main),
  columns: /** @type {any} */ (columns),
  columnWork: /** @type {any} */ (columnWork),
  inColumn: (column, act) => inColumn(/** @type {any} */ (column), act),
  eachColumn: (act) => eachColumn(/** @type {any} */ (act)),
  inThisColumn: (fn) => inThisColumn(fn),
  screens: { screen: () => screens.screen(), view: () => screens.view() },
  selection: () => selection(),
  selectedChannelId: () => selectedChannelId(),
  channelStore: /** @type {any} */ (channelStore),
  providerFacts,
  threadOf: (message) => threadOf(message),
  heldThreadRecord: (message) => heldThreadRecord(message),
  unansweredThread: (message, record) => unansweredThread(message, record),
  channelRepliesInThread: (channel) => channelRepliesInThread(channel),
  removedUpstream: (id) => removedUpstream(id),
  threadReplies: (thread, message) => threadReplies(thread, message),
  openThreadRootId: () => openThreadRootId(),
  knownChannel: (id) => knownChannel(id),
  setStatus: (text) => setStatus(text),
  setTextIfChanged: (node, text) => setTextIfChanged(node, text),
  setHiddenIfChanged: (node, hidden) => setHiddenIfChanged(node, hidden),
  replyDrafts,
  draftLedger,
  draftQueries: {
    draftsHere: () => draftQueries.draftsHere(),
    draftHeld: (draft) => draftQueries.draftHeld(/** @type {any} */ (draft)),
    draftLive: (draft) => draftQueries.draftLive(/** @type {any} */ (draft)),
  },
  draftRound: /** @type {any} */ (createDraftRound()),
  replySight: createReplySight(),
  quoteOwner: () => /** @type {any} */ (quoteOwner),
  dictation: {
    stop: () => dictation.stop(),
    quietKeyboard: () => dictation.quietKeyboard(),
    start: () => dictation.start(),
    showKeyboard: () => dictation.showKeyboard(),
  },
  queueOutgoingMessage: (entry, column) => queueOutgoingMessage(/** @type {any} */ (entry), /** @type {any} */ (column)),
  replyMarksRead: (target, text) => replyMarksRead(/** @type {any} */ (target), text),
  quoteBlock: (quote) => quoteBlock(quote),
  fitGrowingBox: (box) => fitGrowingBox(box),
  scheduleBackSync: () => scheduleBackSync(),
  markCardBesideReply: () => voiceTranscript().markCardBesideReply(),
  gatherState: () => /** @type {any} */ (gatherState()),
  gatheredParent: () => gatheredParent(),
  putRepliesBack: () => putRepliesBack(),
  rowsChosen: () => rowsChosen(),
  renderChannelRows: () => renderChannelRows(),
  renderScrollTools: () => renderScrollTools(),
  renderOutgoingEverywhere: () => renderOutgoingEverywhere(),
  renderOutgoingMessages: () => renderOutgoingMessages(),
  outbox,
  openThread: (id, opts) => openThread(id, opts),
  closeThread: () => closeThread(),
  jumpToMessageLink: (ref) => jumpToMessageLink(/** @type {any} */ (ref)),
  focusAtEnd: (box) => focusAtEnd(box),
  focusList: (pane) => focusList(/** @type {any} */ (pane)),
  followFoot: (area) => followFoot(area),
  restoreScroll: (mark) => restoreScroll(/** @type {any} */ (mark)),
  captureScroll: () => captureScroll(),
  atBottom: (area) => atBottom(area),
  shownNode: (node) => shownNode(node),
  sideShowing: () => sideShowing(),
  sideColumnUp: () => sideColumnUp(),
  layoutShown: { columns: () => layoutShown.columns() },
  pinsView: () => pinsView(),
  channelContexts,
  rowMessages: (li) => rowMessages(li),
  idsOf: (row) => idsOf(/** @type {any} */ (row)),
  rowShown: (row) => rowShown(/** @type {any} */ (row)),
  paneRowFor: (pane, id) => paneRowFor(/** @type {any} */ (pane), id),
  nodeWithin: (area, node) => nodeWithin(area, node),
  paneColumn: (pane) => paneColumn(/** @type {any} */ (pane)),
  channelPane,
  sidePane,
  rowIn: (list, id) => rowIn(list, id),
  gatherReport: (id) => gatherReport(id),
  channelContextKey: () => channelContextKey(),
  repliesTo: (id, messages) => repliesTo(id, messages),
  focusLost: () => focusLost(),
  isComposer: (node) => isComposer(node),
});
const {
  replyState, replyRoute, replyBoundForThread, rememberDraft, draftMarker, renderDraftMarkers,
  noteDraft, inSight, renderDraftChip, openSentenceReply, pressDraftChip, cancelQuickReply,
  answerQuickReplyDiscard, quickReplyToStream, openReply, renderReplyDestination, closeReply,
  cancelReply, answerReplyDiscard, sendReply, replyPane, replyKeysHere, placeInlineReply, placeRowsInPlaceAgain,
  fitInlineRoom, reopenPendingReply, noteReplyFocusLeaving, putDockedReplyAway, replyingTo,
  markReplying, followReplyStore, anchorUnder, replySight,
} = replySession;
// Which kept drafts were left with "Start a new thread" ticked is the reply drafts' (`startsThread`).

const dictation = createDictation({
  recognition: () => /** @type {any} */ (window).SpeechRecognition || /** @type {any} */ (window).webkitSpeechRecognition || null,
  lang: () => (typeof navigator === "object" && navigator && navigator.language) || "en-US",
  box: () => el("quick-reply-text"),
  say: (text) => setTextIfChanged(el("quick-reply-state"), text),
  listening: (on) => {
    el("quick-reply").toggleAttribute("data-listening", on);
    // `#829 voice-page-dictation-polish`: the microphone badge, while it listens.
    el("quick-reply-mic").hidden = !on;
  },
  settled: () => quoteOwner.noteQuickReplyInput(),
});

const quoteOwner = createQuoting({
  byId: (id) => /** @type {any} */ (leftEl(id)),
  inputKind: () => inputKinds.kind(),
  drawReplyLabel: (button) => drawReplyLabel(button),
  selectedChannelId: () => String(selectedChannelId() || ""),
  replyRoute: (message, bound) => replyRoute(message, bound),
  readOnly: (channel) => knownChannel(channel)?.writable === false,
  replyMarksRead: (message, text) => replyMarksRead(message, text),
  queueOutgoingMessage: (send) => queueOutgoingMessage(send),
  toStream: (row, message, text) => quickReplyToStream(row, message, text),
  drafts: quickDrafts,
  noteDraft: (key, channel, written) => noteDraft("quick", key, channel, written),
  // `#821 voice-page-read-aloud-react`. The microphone stops whenever the box closes: Send, Cancel, Esc, a tap
  // elsewhere, another reply.
  closed: () => {
    dictation.stop();
    // `#846 draft-chip-honest`. A question left unanswered goes with the box, and its draft is off screen now.
    setHiddenIfChanged(leftEl("quick-reply-discard"), true);
    renderDraftChip();
  },
  opened: () => {
    setHiddenIfChanged(leftEl("quick-reply-discard"), true);
    renderDraftChip();
  },
});


const {
  noteQuoteSelection, takeQuote, placeSelectionChip, pressSelectionChip, quickReplyHolds,
  quickReplyListMoved, placeQuickReply, closeQuickReply, putDownQuickReply, sendQuickReply,
} = quoteOwner;

// --- a reply open in the stream -------------------------------------------------------------------
//
// `#277 reply-in-column` opened the reply's box at the foot of the column Reply was pressed in, in two
// columns, and kept the reply screen for one. `#410 inline-reply-editor`. The owner, 2026-10-21: "When I
// press 'reply' on a message inside a thread in two column view, I see a reply box pop up AT THE BOTTOM.
// But I STILL see the other 'inline' reply box at the bottom of the thread message series ... Why don't
// we move the 'Reply' box to also be an inline box and NOT show two at once ... gather the existing
// replies underneath the message we just pressed Reply on, and then our new Reply interface is just the
// existing inline-message editor (with Grow button) inserted in the stream ... This will get rid of the
// jarring 'jump to a different screen' effect when we currently press reply."
//
//   * NO REPLY SCREEN FROM THE CHANNEL. Reply, Quote reply, a row's Draft and a failed reply's Edit,
//     pressed on a row of Main, All, a thread (across the window or on the right), the rows unpacked
//     under the Threads view's cards or the Pinned filter, open the reply's box IN THAT LIST, in one
//     column or two, on a phone as on a desk. One box moved, not a second one: one Send, one draft, one
//     route, one Escape. Its layout is the composers': the box, Grow (`#340 composer-grow-box`, the one
//     choice for every box), Send, with Cancel and the line naming who is answered.
//   * UNDER THE MESSAGE AND ITS REPLIES. The message's replies are gathered under it by the gathering
//     All's N replies makes (`#215 reply-coalesce`, `gatherForReply`), in Main and in a thread too; on a
//     thread's root in Main, its thread's replies are drawn there as rows of the list, as the Threads view
//     unpacks them (`unpackMessages`). The box is drawn under the last of them (`inlineReplyAnchor`), or
//     under the message where none is drawn; a thread's own root has its whole thread under it already,
//     and a root unpacked under a card its block. Later messages stay below, moved down by the box.
//   * NEVER A ROW. The box stays out of the list, whose children are its message rows: web/voice.css draws
//     it under its row by anchor positioning, the row keeping room under itself as tall as the box
//     (`placeInlineReply`), so it follows the row through every redraw, fold and arrival above it. With
//     no row drawn to put it under, it is drawn at the foot of the list, where the column's box was.
//   * ONE BOX. While the reply is open the columns' own boxes, "Message the main channel…" and "Reply in
//     this thread…", are hidden, in both columns; Send and Cancel bring them back. Reply elsewhere moves
//     the one box there. Their drafts are their own: the reply's per message, a column's per channel and
//     thread.
//   * NOTHING JUMPS. The message answered keeps its place on the screen as its replies gather under it,
//     and the column moves only where the box would be under its foot, by as little as shows the whole
//     box: a row taller than that leaves room for goes up, its top a scroll up to read. Send and Cancel leave the message where it is
//     (`leaveDockedReply`); Cancel puts back what Reply gathered, and Send leaves it, so the reply joins
//     the stack where it lands.
//   * WHERE IT GOES IS THE REPLY SCREEN'S: `openReply` decides it, in the column pressed, as it always
//     did. The quote, the draft kept on Cancel and the Done a sent reply makes are the same.
//   * A REPLY THAT DOES NOT GO SAYS SO WHERE THE READER IS. Send leaves the reader where they were, and the
//     outbox row of a send that failed, which offers Retry, is down at the column's foot, unseen. So the
//     status line says it (aloud too: it is a polite live region), and the column's Newest chip reads
//     "Reply not sent" in the outbox's own orange until the reader has been down to it
//     (`reportColumnReply`).
//   * A COLUMN THAT CHANGES UNDER IT — another channel, thread or view — puts it away, its words kept as a
//     draft, which the row's Draft opens again. Two columns that go while a reply is open in one put it
//     away to be opened again in the stream, words and all, as soon as its message is drawn: at once in
//     the one column left where it is drawn there (`reopenPendingReply`).
//   * THERE IS NO REPLY SCREEN. The screen Reply used to go to in one column — `#200 reply-context`'s
//     earlier messages and thread title, `#247 reply-continuity`'s message drawn where its row was — is
//     gone: the message is its own row, where it was, with everything before it above it and its replies
//     under it, and where the reply posts is said under the box.

/** The keys' list of `column`: the left column's, or the right one's. */







// --- a reply that is sent marks what it answers Done ---------------------------------------------
// Marking what a sent reply answers Done, and the chat service's marker after it, are
// web/src/view/channel's now (`createSentRead`, `#624 voice-page-modularity`).

// --- the reply open in the stream follows the store ------------------------------------------------


// --- two columns on a desk ------------------------------------------------------------------------
// The layout of the two columns is web/src/view/channel's now (`createTwoColumns`, `#624
// voice-page-modularity`). Esc on the thread list, opening a thread from it and leaving it
// (threadListBack, openThreadFromList, leaveThreadList) stay here until entering a thread's
// move takes them, calling the layout owner's names as they were.

/**
 * Esc on the thread list, with nothing smaller open: back to what the list was chosen from
 * (`listBefore`). `#281 view-keys`. The owner, of the Threads screen it replaced: "rather ESC works".
 * Two columns, where the window still draws them, their button going back to what it went back to
 * then; else one column, on the view it showed (`oneColumnBeforeList`).
 */
function threadListBack() {
  if (columnWork.side()) return inColumn(columns.main, threadListBack);
  const before = layoutChoice.listBefore();
  if (before.layout === "two" && layoutShown.fit()) {
    layoutChoice.restoreBeforeColumns(before.columns);
    setLayout("two");
  } else {
    chooseOneColumn(oneColumnBeforeList(before));
  }
  settleFocus();
  return undefined;
}

/**
 * Open thread `id` from the thread list: across the window, one column, and Back from it is the list
 * again (`closeThread`). `#281 view-keys`. A card, the picker, and Enter on a card all come here.
 */
function openThreadFromList(id, summary = null) {
  const known = summary || channelStore.current().threads.find((held) => String(held.id) === String(id)) || null;
  // `#298 thread-card-header`. Its card moves up from the list to the top of the window.
  const from = threadListOnScreen() ? cardFlightStart(String(id)) : null;
  chooseOneColumn({ view: "thread", thread: String(id), summary: known, origin: "list" });
  flyThreadCard(leftEl("thread-card"), from, leftEl("pane-discord"));
}

/**
 * Out of the thread list: to one column — the view asked for, or the one the channel opens in — or to
 * two columns, Main coming in beside the cards as they are. `#281 view-keys`.
 *
 * @param {"one" | "list" | "two"} next
 */
function leaveThreadList(next = "one") {
  // `#269 approval-card-inline`. A send card docked under the cards goes back to the foot of the left
  // column's list, its next message, BEFORE that column's view is decided, so that a view drawn afresh
  // (All, a thread) is placed with it in. Main kept behind the list, which nothing redraws, is told as a
  // message arriving tells it (`#280 scroll-stability`): a reader on its newest line follows the card
  // down, and anyone else stays on their row and is offered the jump to it.
  //
  // ...and told BEFORE that view is decided, since a switch between Main and All keeps the place Main's
  // reader is at (`changeChannelView`). Told only after, and only where the view stayed, a reader at
  // All's foot was drawn in All on the rows they could see above the card: in Chromium 369px above All's
  // foot on a phone and 337px at a desk, Send off screen. The place read as the list came up, with the
  // card still in Main's list, is what is put back, where it still holds (`layoutShown.holdListPlace`): the card's
  // going moved a reader near the foot.
  //
  // ...and a place read in All is put back in All as well, card or no card, once the way out has shown
  // All, where every row the reader could see is. Main lacks All's replies, so a place near All's foot
  // put back in Main is at Main's foot, and the switch to All took that for a reader on the newest line
  // and went to All's foot. A reader up All is offered the jump to a card come back below them, as one
  // up Main is.
  const carded = voiceTranscript().postWatch.shown !== null;
  const follows = carded && liveFollowers().main;
  const shown = `${selection().view}|${selection().threadId}`;
  const held = layoutShown.takeListPlace();
  layoutShown.setThreadList(false);
  // `#419 threads-view-main-messages`. The main channel's box back under the left column's list.
  renderMainToggle();
  renderOrderToggle();
  placeMainCard();
  // `#262 search-load-older`. The cards' walk for the search ends with the list, unless two columns
  // keep the cards beside Main.
  if (next !== "two") endRepliesWalk();
  voiceTranscript().placePostCard();
  const followed = inColumn(columns.main, () => {
    const holds = held !== null && screens.screen() === "main" && screens.view() === "discord" &&
      selection().view === "main" && String(selectedChannelId()) === held.channel;
    if (holds && keepReaderPlace(held.place)) return held.place.pinned;
    if (follows) scrollToNewest();
    return follows;
  });
  if (next === "one") showOneColumnLeft();
  const backInAll = next === "one" && held !== null && held.view === "flat" && inColumn(columns.main, () =>
    screens.screen() === "main" && screens.view() === "discord" && selection().view === "flat" &&
    String(selectedChannelId()) === held.channel && keepReaderPlace(held.place));
  if (carded && (backInAll ? !held.place.pinned : !followed && `${selection().view}|${selection().threadId}` === shown)) {
    inColumn(columns.main, () => followIfPinned(false, 0, "discord"));
  }
  renderSearchInAll();
  refreshSearchCount();
}

// The right column's cards are web/src/view/channel's now (`createSideCards`, `#624
// voice-page-modularity`).

// --- the card the reader was held by, moved -----------------------------------------------------------
//
// `#612 view-stability`. The owner, 2026-10-27: "I just experienced a screen jump. I was in threaded view reading a
// message and it jumped off the screen. It was probably bc of some new message causing a thread to jump to the
// bottom. But we should ALWAYS prioritize view stability. Whatever I'm looking at should not jump away."
//
// The cards hold their reader by ONE card, found again by id once they are drawn again (`cardAnchor`): the
// highlighted one, the selected one, else the first showing, which is most often a card half scrolled off the
// top. A reply in that card's thread moves it to the foot, and the reader was taken after it: every card they
// were reading went, 755px at 393x852 and 245px at 1280x800 in Chromium, with the card they were looking at
// gone off the screen. So a place in the cards also notes every row on screen, cards and the replies unpacked
// under them, fully shown first, and the cards either side of each one's card. A card standing between neither
// of the cards it stood between has MOVED — re-sorted to the foot, or gone — and is not followed: the reader is
// held by the first row on screen whose card did not move, so what stayed on screen stays where it was, and
// the moved card is where the list now puts it. Only a reader on the foot follows the list (`followFoot`).
//
// UNLESS MOST OF THE SCREEN MOVED WITH IT. The rule is to keep as much of what is on screen still as can be: a
// thread unpacked and being read, its replies filling the screen, goes to the foot whole when a reply comes,
// and following it leaves every line the reader sees where it was, the reply arriving under them. So the rows
// on screen are weighed by how much of each shows: a moved card whose rows show more than those that stayed is
// followed. Main's messages among the cards (`#419 threads-view-main-messages`) belong to no card, do not
// re-sort, and are found again by id.
//
// FOR CHANGES NOBODY ASKED FOR. A place the reader chose to come back to — the card a thread was opened from,
// the place a layout had, the selected card through a sort the reader pressed — is found again by its card
// wherever it now is, as before (`placeCardAnchor`'s `follow`).

// Where the reader is in the cards, and putting them back once the cards are drawn again — held by what kept
// most of the screen still when the card held has moved — are web/src/view/channel's (`createCardPlaces`,
// with `rowNeighbours`, `neighbourStayed` and `sameCardPlace`; `#624 voice-page-modularity`).

/** The reader's place in the cards. */
const cardPlaces = createCardPlaces({
  lookup: (id) => leftEl(id),
  readable: () => sideColumnUp() && columns.side.mode === "list" && !cardsDrawn.opening(),
  atBottom: (area) => atBottom(area),
  followFoot: (area) => followFoot(area),
  litCard: () => litCard(),
  selectedCard: () => {
    const mark = markOf(sidePane);
    return mark && mark.card !== false ? mark.id : null;
  },
  finishSlide: () => finishPackSlide(),
});

/**
 * Where the reader is in the cards, or null with the cards not on screen, or being opened.
 *
 * @returns {import("./src/view/channel/index.ts").CardPlace | null}
 */
function cardAnchor() {
  return cardPlaces.anchor();
}

/**
 * Put the reader back on `anchor` in the cards; whether the reader was placed. With `follow`, a place the
 * reader chose to come back to, the card wherever it now is.
 *
 * @param {import("./src/view/channel/index.ts").CardPlace | null} anchor
 * @param {boolean} [follow]
 */
function placeCardAnchor(anchor, follow = false) {
  return cardPlaces.place(anchor, follow);
}

/**
 * Run `change` to the cards, keeping the reader's place in them. With `follow`, a change the reader asked
 * for that re-sorts the cards: the card they are on is kept wherever it goes.
 *
 * @param {() => void} change
 * @param {boolean} [follow]
 */
function keepingCardPlace(change, follow = false) {
  cardPlaces.keeping(change, follow);
}

/**
 * By channel, where the reader was in the cards when a thread was selected from them, for this
 * session: closing the thread comes back there. `#261 two-column-connectors`.
 *
 * @type {Map<string, {newest: boolean, id: string | null, offset: number}>}
 */
const sideCardPlaces = new Map();

/** Note where the reader is in the cards, before the right column shows something else. */
function rememberCardPlace() {
  const anchor = cardAnchor();
  if (anchor) sideCardPlaces.set(String(selectedChannelId()), anchor);
}

/** Show the cards in the right column, the reader placed in them as `placeOpenedCards` says. */
function openSideCards() {
  // A place held for the way back to the thread list is not one in cards opened afresh (`layoutShown.holdListCards`).
  layoutShown.takeListCards();
  // Nor the room past the foot held for a card packed in them (`#459 pack-keeps-card-still`).
  closeOverextend();
  columns.side.mode = "list";
  cardsDrawn.whileOpening(renderSideMode);
  placeOpenedCards();
}

/**
 * The cards just opened: at the foot, as a list of messages opens at its newest, unless the reader was
 * up the list when a thread was selected from it — then back on the card they were on.
 */
function placeOpenedCards() {
  const anchor = sideCardPlaces.get(String(selectedChannelId()));
  sideCardPlaces.delete(String(selectedChannelId()));
  // The foot, too, when the card the reader was on has gone.
  if (!(anchor && !anchor.newest && placeCardAnchor(anchor, true))) placeCardAnchor({ newest: true, id: null, offset: 0 });
}

/**
 * The search text over the cards, as over the Threads list's (`#129 message-search`): a card the
 * text does not match goes while the search is open. Links leaves them: a card has no links of its
 * own, and its thread's are in its messages.
 *
 * `#432 read-mode-unify`. And under Hide read a card whose thread is read goes too, its unpacked rows
 * with it (`threadIsRead`: no unread reply held or reported, and its root read), as Unread only did
 * (`#259 thread-list-unread-filter`); under Fold read a card in a run is behind its line
 * (`foldCardRuns`). The search and the read mode are ANDed, and a list they empty says why, over
 * where the cards were (`sideListState`).
 *
 * The owner asked whether the search should reach a thread's replies too. What is cheap is done: the
 * replies the page already holds for each thread are searched with the card's own words — each term
 * in one or the other — and a card found only with its replies says so, "in replies", beside its
 * facts (`markReplyMatch`). Nothing is read as the search is typed: a reply the page does not hold is
 * found once the cards' own walk further back has read it (`searchReplies`, `#262 search-load-older`).
 */
function filterSideCards() {
  // A filter choosing rows opens every run, and closing it folds them again: unpacked blocks' runs, and
  // under Fold read the runs of read threads over the cards (`foldCardRuns`, `#432 read-mode-unify`).
  // Noted whether or not the blocks are drawn again (`cardsDrawn.noteSuspended`).
  if (cardsDrawn.noteSuspended(readRunsSuspended()) && (unpackOnScreen() || readModes.mode() === "fold")) {
    cardsDrawn.forget();
    renderSideCards();
    return;
  }
  // `#606 server-side-search`. The shared language, message by message: a thread with any hit is
  // chosen, by its card's own words or by a reply the page holds.
  const node = searchField.open() ? searchParsed(searchField.query()).node : null;
  const typed = node !== null;
  const marking = searchField.open() ? searchParsed(searchField.marked()).terms : [];
  const allRows = /** @type {HTMLElement[]} */ ([...leftEl("side-cards").children]);
  const rows = allRows.filter((row) => row.hasAttribute("data-card"));
  const replyHits = typed ? threadsWithHits(node) : null;
  // `#439 search-unmatched-separators`. The runs among the cards the reader has opened: shown, as in
  // the channel's lists, but never counted among the matches the bar names.
  const openedCards = openedCardGaps(false);
  let unread = 0;
  let shown = 0;
  /** @type {Map<string, boolean>} */
  const cardShown = new Map();
  for (const row of rows) {
    const text = row.getAttribute("data-search") || "";
    const fresh = !row.hasAttribute("data-thread-read");
    const revealed = openedCards !== null && openedCards.threads.has(String(row.getAttribute("data-id")));
    // `#432 read-mode-unify`. Hide read takes a read thread out with its unpacked rows, as it takes a read
    // message out of Main; a card in a folded run is hidden behind the run's line. A card in a run
    // the reader opened from its unmatched line (`#439`) stands through either, while the search stands.
    const kept = (((!hidingRead() || fresh) && row.getAttribute("data-read-run") !== "member") || revealed);
    const own = !typed || rowMatches(row, node, text);
    const inReplies = !own && replyHits !== null && replyHits.has(String(row.getAttribute("data-id")));
    const said = own || inReplies || revealed;
    if (row.hidden === kept) row.hidden = !kept;
    setClassWord(row, "search-hidden", !said);
    markReplyMatch(row, inReplies);
    // `#249 search-highlight`. And where, once the typing pauses (`searchField.marked()`): the card's own
    // words, whatever its replies matched.
    markSearchHits(row, own || inReplies ? marking : []);
    if (fresh) unread += 1;
    if (kept && (own || inReplies)) shown += 1;
    cardShown.set(String(row.getAttribute("data-id")), kept && said);
  }
  // `#419 threads-view-main-messages`. Main's messages among the cards: the read mode chose them as they
  // were drawn (`mainRowMessages`), a folded run hides its members behind its line, and the search finds
  // them by their words.
  for (const row of allRows) {
    if (!row.hasAttribute("data-main-row")) continue;
    const revealed = openedCards !== null && inOpenedGap(row, openedCards.ids);
    const kept = row.getAttribute("data-read-run") !== "member" || revealed;
    const matched = !typed || rowMatches(row, node, row.getAttribute("data-search") || "");
    const said = matched || revealed;
    if (row.hidden === kept) row.hidden = !kept;
    setClassWord(row, "search-hidden", !said);
    markSearchHits(row, matched ? marking : []);
  }
  // `#404 threads-unpack-all`. A card's unpacked block goes where its card goes, a row hidden in a
  // collapsed run of read messages staying hidden behind its line (`#407 unpack-row-parity`).
  for (const row of allRows) {
    const thread = row.getAttribute("data-unpack-thread");
    if (thread === null) continue;
    const shownBlock = cardShown.get(thread) === true && row.getAttribute("data-read-run") !== "member";
    // ...and a block whose card heads a folded run of read threads goes behind the run's line with it.
    const folded = row.getAttribute("data-card-run") === "member";
    if (row.hidden === (shownBlock && !folded)) row.hidden = !(shownBlock && !folded);
    // `#606 server-side-search`. Unpacked under a search, a thread shows only its hits, its other
    // messages folded into lines between them (`drawUnpackGaps`), until the reader opens one.
    const hit = !typed || rowMatches(row, node, row.getAttribute("data-search")) ||
      (openedCards !== null && inOpenedGap(row, openedCards.ids));
    setClassWord(row, "search-hidden", !hit);
    markSearchHits(row, typed && hit ? marking : []);
  }
  // `#281 view-keys`. A failed read of older threads is said over the cards it left as they were.
  const state = [olderThreadsFailure(), sideListState(rows.length, unread, shown)].filter((part) => part !== "").join(" ");
  setTextIfChanged(leftEl("side-list-state"), state);
  if (leftEl("side-list-state").hidden !== (state === "")) leftEl("side-list-state").hidden = state === "";
  cardsDrawn.setCount(rows.length, shown);
  if (threadListOnScreen()) sayCardCount();
  // `#262 search-load-older`. The way to their threads' older replies, as the search and All's reach say.
  renderRepliesSearch();
  // `#439 search-unmatched-separators`. A line in each run the search leaves among the cards.
  drawCardGaps(leftEl("side-cards"));
  // `#261 two-column-connectors`. The cards shown are what the links go to.
  scheduleColumnLinks();
}

// --- unmatched runs among the thread cards ------------------------------------------------------
//
// `#439 search-unmatched-separators`. The cards are threads, and a thread is a container of
// messages: where the search leaves whole threads out between two it shows, the line counts the
// MESSAGES in them — "… 15 unmatched messages in 3 threads …" — each thread's from what the page
// knows of it (`cardMessageCount`), "at least" where a count is not exact. Main's own messages
// among the cards (`#419 threads-view-main-messages`) count as themselves, and a run holding any
// of them does not lay its whole count at the threads' door: it names no threads. A thread's
// unpacked messages (`#404 threads-unpack-all`) go with their card and are already in its count.
//
// DRAWN WHILE SEARCH WORDS CHOOSE CARDS, in both lists of cards: the right column's and the thread
// list's across the window (one list, `filterSideCards`), and the Threads list in one column
// (`applySearch`). While words are typed the read modes over the cards stand suspended with the
// messages' (`readRunsSuspended`, `hidingRead`), so a read thread the words leave out is counted
// here under the one word "unmatched", as the channel's gaps count what Links and whose messages
// leave out; Fold read's own lines over the cards (`foldCardRuns`) are the read runs', drawn only
// with no search choosing. The line is the channel gap's own (`.row-seam`, on the card after its
// run), and a tap opens its run the same way: shown, not matched, until the search changes.








/**
 * What the right column says over its cards, or "" while it shows some: none in the channel, none
 * unread under Hide read (`#432 read-mode-unify`; Unread only, `#259`, before it), or none the search
 * matches.
 *
 * @param {number} cards How many cards there are.
 * @param {number} unread How many of them have unread replies.
 * @param {number} shown How many the filters leave.
 */
function sideListState(cards, unread, shown) {
  if (cards === 0) return "No threads in this channel yet.";
  if (shown > 0) return "";
  if (hidingRead() && unread === 0) return "No unread threads.";
  // `#262 search-load-older`. How far back the search has looked, where that is the whole channel: until
  // then the way further back under this line says the day (`renderRepliesSearch`).
  const whole = allLoaded() ? " — all of this channel is loaded" : "";
  return `No threads match that search${whole}.`;
}

// `#432 read-mode-unify`. Unread only (`#259 thread-list-unread-filter`) was the cards' own filter, a
// chip and a choice kept on this device. It is the read mode now, as for every list: Hide read leaves the
// read threads out, Fold read folds them into runs (`foldCardRuns`), Show read shows them.

/** `filterSideCards`, the reader's place in the cards kept: the search typed, opened or closed. */
function refilterSideCards() {
  keepingCardPlace(filterSideCards);
}

// --- unpack every thread under its card (#404 threads-unpack-all) ----------------------------------
//
// The owner, 2026-10-21: in All, **N replies** bridges one thread at a time — the children come up
// to meet the parent, which stays put — and bridging another thread breaks the first. He wants
// every thread chained at once in one stream. So in the Threads view (Cmd+3), which already lists
// threads by last updated, **Unpack msgs** (key u; "Unpack all" until `#421 chip-states`) draws each thread's messages under its card, in
// time order, joined by the same bridge All's gathering draws (`#215 reply-coalesce`), tinted by
// the thread's colour. The read modes apply inside each block as they do in a thread: under
// Fold read a run of read rows is one grey line hanging off the spine by its own stub; under
// Hide read read rows are absent. Blocks are later siblings of their card in `#side-cards`, never
// children, so every walk that knows cards by `data-card` is untouched. A thread whose replies the
// store does not hold in full is read only as its block nears the screen, at most a few at a time,
// and only its newest page: older history stays behind opening the thread, and its block says so.

// --- each thread packed or unpacked from its card (#440 thread-card-pack-toggle) -------------------------
//
// The owner, 2026-10-22: "I want to toggle pack/unpack on different threads the same way I toggle
// collapse/expand on messages. Currently CLICK on a thread card in thread list view lights up that card
// ... and dims the colored left edge of all the other threads. That's a behavior from the interleave All
// view that really doesn't make sense in the thread view ... a 3 state toggle. Collapsed, expanded card
// but no children, expanded card and expanded children ... a little up triangle to pack those messages back
// into the card." And: "let's also go 3-way with unpacking in thread list view ... The middle state will
// be to pack read threads and unpack any with non-zero unread child messages ... ONLY in this middle state
// I would also like to render as grey the colorful left sidebar of the READ cards".
//
//   * UNPACK, THREE WAYS: Pack all, Unpack unread (a thread with any unread message unpacked, a read one
//     packed, by `threadIsRead`), Unpack all; the chip says which, and a press or u goes on to the next.
//     Kept on the server for every device (`#457 settings-server-side`). Only in Unpack unread is a read card's colour strip grey, by the very rule that
//     greys the other cards while one is highlighted (web/voice.css).
//   * EACH THREAD, THREE WAYS, in the Threads view only: a click on its card, or Enter or o on it, goes
//     collapsed (three lines) -> whole, nothing unpacked -> whole with its messages unpacked -> collapsed.
//     A summarized card (`#405`), written to fit, skips the middle state: whole and three lines are the
//     same card. The Threads view highlights no card and dims none; two columns keep `#277`'s highlight.
//   * A THREAD'S CHOICE OUTRANKS UNPACK for that thread until Unpack is pressed again, which gives every
//     thread of the channel back to it. Kept on the server with Unpack (`THREAD_CARD_STATES_KEY`, `#457 settings-server-side`).
//   * PACKED IN PLACE: the up triangle in the first unpacked row's gutter, where All's bridge has its X, or
//     ^ on a thread's card or row, packs it, its card held where it is on the screen; a thread read under
//     Unpack unread packs where it is too (`keepingCardPlace`), its card staying in the list.

// Unpack's state and each thread's own choice of its card, and how both are kept, are
// web/src/present/rows' (`createCardPacking`: UNPACK_MODES, UNPACK_MODE_NAMES, UNPACK_MODE_SAID,
// CARD_STATES, THREADS_UNPACKED_KEY, THREAD_CARD_STATES_KEY; `#624 voice-page-modularity`).
const cardPacking = createCardPacking({ storage: () => deviceStorage() });

/** Whether the cards on screen are the Threads view's, where each thread is packed or unpacked from its card. */
const cardsCycle = () => threadListOnScreen() && columns.side.mode === "list";


/**
 * The reads unpacking owes the threads — each read once, again only once its thread has grown past what
 * the read brought, `UNPACK_READS_AT_ONCE` at a time — are web/src/core/channels's (`createUnpackReads`,
 * `#624 voice-page-modularity`); the read itself, and drawing what it brought, are here.
 */
const unpackReadQueue = createUnpackReads({
  fetch: async (id) => {
    const channel = String(selectedChannelId());
    if (channelStore.current().channel !== channel) loadCanon(channel);
    const held = heldNow(channelStore.current());
    const path = withThreadQuery(
      `/api/v1/channels/${encodeURIComponent(channel)}/timeline?view=thread&limit=${DISCORD_PAGE_LIMIT}&reason=threads`, id);
    return { channel, held, payload: await timelineReader.readTimeline(path, undefined) };
  },
  land: (id, fetched) => {
    const { channel, held, payload } = /** @type {{channel: string, held: Map<string, any>, payload: any}} */ (fetched);
    if (String(selectedChannelId()) !== channel) return false;
    foldTimelinePage(payload, false, "thread", id, held);
    return true;
  },
  ended: (id) => {
    cardsDrawn.forget();
    if (sideColumnUp()) renderSideColumn();
    // `#410 inline-reply-editor`. A thread gathered under its root in Main draws what the read brought.
    const wanting = gatherState().gathered && gatherState().gathered.thread === id ? gatherState().gathered.column : null;
    if (wanting) inColumn(/** @type {any} */ (wanting), renderChannelRows);
  },
  guard: (run) => guardQuietly(run)(),
});

/**
 * The blocks read as they near the screen, through one observer made when first wanted: web/src/view/channel's
 * (`createUnpackWatch`, UNPACK_NEAR_PX; `#624 voice-page-modularity`).
 */
const unpackWatch = createUnpackWatch({ root: () => leftEl("side-scroll"), read: (id) => queueUnpackRead(id) });

/** Whether `choice`'s card says a summary rather than its root's words (`#405 root-under-summary`). */
const cardIsSummarized = (choice) => {
  if (!choice) return false;
  const { named, said } = threadWords(choice);
  // Exactly when `sideCardText` says the summary. Not "when the card's words are not the root's":
  // a split card's are not either (`#522 thread-card-max-lines`), and it says no summary.
  return Boolean(named && said);
};

/**
 * Whether unpacked blocks may be on screen: the Threads view (Cmd+3) showing cards, Unpack unpacking or a
 * thread unpacked by its own choice. Which threads are is each card's (`cardState`).
 */
const unpackOnScreen = () => cardsCycle() && (cardPacking.mode() !== "packed" ||
  cardPacking.unpackedIn(String(selectedChannelId())));























// --- the main channel's messages among the cards (#419 threads-view-main-messages) -------------------
//
// The owner, 2026-10-21: "In Cmd-3 thread list view... we don't see the main channel messages that are
// not yet threads, and we cannot send messages from this view to the main channel ... a toggle to show
// Main msgs. When that is on we should see messages to the main channel that are not (yet) threads and
// can make them threads from there ... when we have Main messages turned on, we should also have the
// Reply box (to main channel) as our implicit last card in our list."
//
//   * MAIN CHANNEL (key a; "Main messages" until `#421 chip-states`), beside Unread only and Unpack msgs, over the Threads view's cards, kept on this
//     device. Pressed, the cards' list also holds the main channel's messages that head no thread a card
//     stands for: what Main draws, less its threads' roots. Each is a message row, built and dressed by the
//     code every list's rows are (`discordNode`, `dressChannelRow`), marked `data-main-row`, and placed
//     among the cards in the list's order, a card where its thread last moved and a message where it was
//     posted. The search finds them by their words, Hide read leaves out the read ones, as it does in
//     Main, and Fold read folds them into the cards' runs (`foldCardRuns`, `#432 read-mode-unify`).
//   * MADE A THREAD WHERE IT IS. Thread or Reply on one opens the reply in the list under it (`#410
//     inline-reply-editor`), bound for a new thread where the service starts one on a reply's say-so (the
//     new-thread choice ticked; Slack threads every reply anyway), not the thread view.
//   * THE MAIN CHANNEL'S BOX IS THE LAST CARD. The left column is Main behind the list (`leftColumnToMain`),
//     so its box posts to the main channel: it is moved to the foot of the cards while Main channel is on
//     (`placeMainCard`), and back when the list goes or the chip is let go. It is the one box: a reply
//     opened anywhere hides it, and putting the reply down gives it back (`dockReply`).

/** The device's choice of Main channel among the cards: "on", or anything else for cards alone. */
const THREADS_MAIN_KEY = DEVICE_SETTINGS.threadsMain.key;

/**
 * Whether the main channel's messages stand among the cards: Main channel pressed, kept on the server
 * (`#457 settings-server-side`) and read from the device here, as the page loads; a browser refusing storage
 * reads as cards alone (`DEVICE_SETTINGS.threadsMain.refused`). `#419 threads-view-main-messages`. One owner
 * in web/src/core/settings/switches.ts (`#624 voice-page-modularity`).
 *
 * @type {import("./src/core/settings/index.ts").KeptSwitch}
 */
const mainRowsChoice = createKeptSwitch({
  setting: DEVICE_SETTINGS.threadsMain,
  deviceSettings: () => deviceSettings(),
});

/** Whether Main's messages belong on screen among the cards: pressed, and the Threads view showing cards. */
const mainRowsOnScreen = () => mainRowsChoice.on() && threadListOnScreen() && columns.side.mode === "list";

/** Main channel: offered over the Threads view's cards, as Unpack msgs is, and pressed as the reader left it. */
function renderMainToggle() {
  const button = leftEl("side-main");
  const offered = columns.side.mode === "list" && layoutShown.threadList();
  if (button.hidden === offered) button.hidden = !offered;
  const pressed = mainRowsChoice.on() ? "true" : "false";
  if (button.getAttribute("aria-pressed") !== pressed) button.setAttribute("aria-pressed", pressed);
}

/** The reader pressed Main channel, or a: kept, and the cards drawn again. */
function setMainRowsShown(on) {
  mainRowsChoice.set(on);
  mainRowsChoice.keep(on);
  renderMainToggle();
  cardsDrawn.forget();
  if (sideColumnUp()) keepingCardPlace(renderSideColumn);
  placeMainCard();
}

/** a, over the Threads view: Main's messages among the cards, or the cards alone again. Elsewhere, nothing. */
function mainRowsFromKeyboard() {
  if (!threadListOnScreen() || columns.side.mode !== "list") return false;
  setMainRowsShown(!mainRowsChoice.on());
  setStatus(mainRowsChoice.on() ? "The main channel's messages among the threads." : "The threads alone.");
  return true;
}

// --- the order of the Threads view's cards (#426 threads-sort-toggle) -----------------------------------
//
// The owner, 2026-10-22: "One toggle I DO think we should have at the top of the thread view is to order
// by thread creation vs latest update. It could be a drop down on sort order or just a toggle button ...
// Our other buttons up there are toggles so maybe start with that and if we add more sort orders make it
// a drop down." So a chip beside Main channel, saying the order the cards are in: Latest update (each
// card where its thread last moved, as they always were) or Thread created (where its first message was
// posted). A press, or d, goes on to the next order in `THREAD_ORDERS`, kept on the server (`#457 settings-server-side`). The orders
// are a table, not a flag: a third is one more entry, and the chip a menu then. Main channel's rows stand
// among the cards by when they were posted, against each card's time in the order chosen. Only over the
// Threads view's cards, as its neighbours are; two columns' cards stay by latest update.

// The orders, their words and times, the cards put in one, and the order chosen, kept on this device,
// are web/src/present/rows's (`createThreadOrder`, `#624 voice-page-modularity`).

/** The order the Threads view's cards are in. */
const threadOrders = createThreadOrder({ storage: () => deviceStorage() });

/** The order the cards on screen are drawn in: the one chosen over the Threads view's cards, else the default. */
const cardOrderOnScreen = () => (threadListOnScreen() && columns.side.mode === "list" ? threadOrders.current() : THREAD_ORDER_NAMES[0]);

/** The order chip: offered over the Threads view's cards, as Main channel is, saying the order they are in. */
function renderOrderToggle() {
  const button = leftEl("side-order");
  const offered = columns.side.mode === "list" && layoutShown.threadList();
  if (button.hidden === offered) button.hidden = !offered;
  const order = THREAD_ORDERS[threadOrders.current()];
  const next = THREAD_ORDERS[threadOrderAfter(threadOrders.current())];
  // Pressed in any order but the default, as the other chips are pressed when they change the list.
  const pressed = threadOrders.current() === THREAD_ORDER_NAMES[0] ? "false" : "true";
  if (button.getAttribute("aria-pressed") !== pressed) button.setAttribute("aria-pressed", pressed);
  const label = `Order: ${order.name}`;
  if (button.getAttribute("aria-label") !== label) button.setAttribute("aria-label", label);
  const title = `In order of ${order.name.toLowerCase()}: ${order.says}. Press for ${next.name.toLowerCase()}`;
  if (button.title !== title) button.title = title;
  // The whole name, and on a phone the short one (web/voice.css).
  setTextIfChanged(leftEl("side-order-whole"), order.name);
  setTextIfChanged(leftEl("side-order-short"), order.short);
}

/**
 * The reader pressed the order chip, or d: the next order, kept, and the cards drawn in it,
 * the selected card kept where it is on screen (`keepingCardPlace` keeps it where it shows; otherwise
 * it is brought into view).
 */
function setThreadOrder(order) {
  threadOrders.set(order);
  renderOrderToggle();
  cardsDrawn.forget();
  if (!sideColumnUp()) return;
  const mark = markOf(sidePane);
  const selected = mark && mark.card !== false ? mark.id : null;
  keepingCardPlace(renderSideColumn, true);
  const row = selected === null ? null : paneRowFor(sidePane, selected, true);
  if (!row || !rowShown(row)) return;
  const area = leftEl("side-scroll").getBoundingClientRect();
  const box = row.getBoundingClientRect();
  if ((box.top < area.top || box.bottom > area.bottom) && typeof row.scrollIntoView === "function") {
    row.scrollIntoView({ block: "nearest", inline: "nearest" });
  }
}

/** The next order after the one the cards are in. */
const nextThreadOrder = () => threadOrderAfter(threadOrders.current());

/** d, over the Threads view: the cards in the next order. Elsewhere, nothing. */
function orderFromKeyboard() {
  if (!threadListOnScreen() || columns.side.mode !== "list") return false;
  setThreadOrder(nextThreadOrder());
  setStatus(`Threads in order of ${THREAD_ORDERS[threadOrders.current()].name.toLowerCase()}.`);
  return true;
}

/**
 * The main channel's messages that head no thread a card stands for, oldest first, as Main draws them:
 * the store's copies of Main's rows (`inView`), less the roots of `carded` and anything removed upstream,
 * no empty message where those are hidden, and under Hide read only what is still to do.
 *
 * @param {Set<string>} carded the threads the cards stand for, by id
 */
function mainRowMessages(carded) {
  if (channelStore.current().channel !== String(selectedChannelId())) return [];
  const messages = channelStore.current().messages.filter((message) => {
    if (!inView(message, "main", null) || channelStore.current().removed.has(String(message.id)) || hiddenEmpty(message)) return false;
    const thread = threadOf(message);
    if (thread !== null && message.thread.is_root === true && carded.has(thread)) return false;
    return !hidingRead() || stillToDo(message, channelStore.current().dismissed) || sentFromThisPage(message);
  });
  return inTimeOrder(messages, "timestamp");
}

/**
 * Main's messages `messages`, one run between two cards, as rows of the cards' list: built and dressed
 * as every list's rows are, in the right column, whose list the keys and the mouse move them in.
 *
 * @param {Array<any>} messages
 * @param {ReplyTally} tally
 * @returns {HTMLElement[]}
 */
function mainRows(messages, tally) {
  // They offer the way into a thread, as All's do, and point at nothing (`rowDrawing.main`).
  const rows = rowDrawing.whileMain(() => glom(messages).map((group) => {
    const row = discordNode(group);
    row.setAttribute("data-main-row", "");
    row.className = `${row.className} main-row`;
    return row;
  }));
  const viewing = String(selectedChannelId());
  const pass = { tally, answered: answeredBy(rows, viewing), viewing, inMain: false, inAll: false };
  for (const row of rows) dressChannelRow(row, pass);
  return rows;
}

/**
 * The main channel's box as the last card of the list, while Main's messages are among the cards;
 * back in its place under the left column's list otherwise. The box's draft and where it posts are the
 * left column's, which is Main behind the list (`leftColumnToMain`).
 */
function placeMainCard() {
  const box = leftEl("channel-composer");
  const list = leftEl("side-list");
  // `#430 threads-main-own-sent`. And the outbox's receipts over it, as in Main: a send still on its
  // way, or one that failed, is shown where it was sent from, not in the list hidden behind the cards.
  const receipts = leftEl("outgoing-log");
  const wanted = mainRowsOnScreen();
  const there = box.parentNode === list;
  if (wanted && !there) list.append(receipts, box);
  else if (!wanted && there) {
    leftEl("pane-discord").insertBefore(box, leftEl("column-reply"));
    leftEl("pane-discord").insertBefore(receipts, box);
  }
  if (box.hasAttribute("data-main-card") !== wanted) box.toggleAttribute("data-main-card", wanted);
}

// --- runs of read threads over the cards (#432 read-mode-unify) ------------------------------------------
//
// The owner, 2026-10-22: "the thread view will respect show/hide/collapse for unread. Whether thread cards
// are packed or unpacked it will simply follow the rule that a thread card with any unread-children acts as
// an unread, and with 100% read acts as read. That does mean we would in collapsed mode show separator
// lines that say e.g. '... 3 read threads ...'". So under Fold read the cards' list folds as a list of
// messages does (`readRunPlan`, the one rule of the runs): its items are the cards, each standing with the
// rows unpacked under it, and Main channel's rows (`#419`); a run of two or more read items in a row, never
// the last, is one line on its first item, "… 3 read threads …" (and "and 2 read messages" where Main's
// rows are in it), a tap or Enter opening it (`openReadRun`). A read thread outside a run keeps its
// unpacked rows folded as a thread's are (`unpackBlock`).

/** The key the cards' opened runs are kept under, by channel (`readRuns`). */
const cardRunKey = () => `cards:${String(selectedChannelId())}`;

// A card's DOM helpers (cardRootOf, threadCardId, cardGapGroups, markCardLayout, fillCardText, ...) are
// web/src/view/channel/'s, and the Main runs among the cards (mainRunsAmong) web/src/present/rows/'s
// (`#624 voice-page-modularity`).

/** Whether an item of the cards is read: a card by its thread (`threadIsRead`), a Main row by its messages. */
const cardRunRead = (row) => (row.hasAttribute("data-card") ? row.hasAttribute("data-thread-read") : rowIsRead(row));

/** The items of the run whose line is on `head`, the line's item first: cards and Main rows, not unpacked rows. */
function cardRunRows(head) {
  const rows = [...(head.parentNode || leftEl("side-cards")).children];
  const run = [head];
  for (let at = rows.indexOf(head) + 1; at < rows.length; at += 1) {
    const row = rows[at];
    if (row.getAttribute("data-card-run") === "member") continue;
    if (row.getAttribute("data-read-run") !== "member" || row.hasAttribute("data-unpack-thread")) break;
    run.push(row);
  }
  return run;
}

/**
 * Fold the runs of read items over the cards in `list` under Fold read, and undo them otherwise: each run's
 * line on its first item, its other items hidden, and the rows unpacked under its cards marked as in the run
 * (`data-card-run`), over the folds of their own block, which come back when the run is opened.
 */
function foldCardRuns(list) {
  const rows = /** @type {HTMLElement[]} */ ([...list.children]);
  const items = rows.filter((row) => row.hasAttribute("data-card") || row.hasAttribute("data-main-row"));
  const placed = readModes.mode() === "fold" && !readRunsSuspended()
    ? readRunPlan(items, cardRunKey(), cardRunRead, cardRunIds) : new Map();
  const isItem = new Set(items);
  /** @type {{row: HTMLElement, threads: number, messages: number} | null} */
  let run = null;
  const say = () => {
    if (!run) return;
    const line = readRunLine(run.row);
    const words = cardRunText(run.threads, run.messages);
    if (line.textContent !== words) line.textContent = words;
    line.setAttribute("title", `Show ${words.replace(/^… /, "these ")}`);
  };
  for (const row of rows) {
    if (isItem.has(row)) {
      const role = (placed.get(row) || { role: null }).role;
      if (role !== "member") {
        say();
        run = role === "head" ? { row, threads: 0, messages: 0 } : null;
      }
      placeInReadRun(row, role, 0);
      if (run) {
        if (row.hasAttribute("data-card")) run.threads += 1;
        else run.messages += idsOf(row).length;
      }
      continue;
    }
    if (run) row.setAttribute("data-card-run", "member");
    else if (row.hasAttribute("data-card-run")) row.removeAttribute("data-card-run");
  }
  say();
}



// --- one card highlighted ------------------------------------------------------------------------
//
// `#277 thread-card-focus`. The owner, 2026-10-13, of the thread cards: "It is VERY busy with all the
// colored lines... Let's make it so when we highlight a thread card all the other lines grey out and we
// can see what we've highlighted. I also don't like the cards expanding. Let's leave the thread cards
// contracted. Let's CLICK to put a persistent highlight and expand on one card (NOT enter the thread).
// To enter the thread you can click the 'N reply M unread' bit let's say or press '->' to go into the
// thread."
//
// The cards are the right column's in two columns and the Threads list's in one, and both work so.
// (`#281 view-keys` made the thread list, Ctrl+3, the right column's cards across the window, in one
// column and on a phone: they work so there too, as they do on the right.)
//
//   * EVERY CARD STAYS CONTRACTED: three lines on the right, the thread's name in the list. `#257
//     summary-truncation` showed the whole of the card under the mouse or the keys, and with the mouse
//     moving over the cards that was a card growing and shrinking under it at every move.
//   * A PRESS ON A CARD HIGHLIGHTS IT — a click anywhere on it but its replies button, or Enter on it
//     with the keys or Tab — and shows it whole. It stays: the mouse moving over the cards, the keys
//     walking through them and the cards drawn again leave it where it is. Another card pressed takes
//     it, and the same one pressed again or Esc puts it out. One per channel, for the session.
//   * WHILE ONE IS HIGHLIGHTED, EVERY OTHER CARD'S EDGE AND UNREAD COUNT GO GREY, and so does every
//     other link between the columns, the highlighted card's drawn over them: the stylesheet greys the
//     cards while the highlighted one shows (a card a filter hides greys nothing), and the links are
//     greyed where they are drawn (`planColumnLinks`, web/src/view/channel/columnlinks.ts).
//   * THE WAY IN is the card's "3 replies · 2 unread", a button of its own (`cardOpener`), or →: into
//     the highlighted card's thread, or with none the card the keys are on — and on a card's own
//     button, where Tab leaves the focus, that card's (`cardForArrow`). In two columns ← comes back out
//     to the cards, the keys on the card entered, as Esc and the X come back to them; in one column ←,
//     Esc and Back come back to the Threads list, as they always did.

/**
 * By channel, the thread whose card is highlighted, for this session.
 *
 * @type {Map<string, string>}
 */
const litCards = new Map();

/** The thread whose card is highlighted in the channel on screen, or null. */
function litCard() {
  // `#440 thread-card-pack-toggle`. The Threads view highlights no card: a press there packs and unpacks.
  if (cardsCycle()) return null;
  return litCards.get(String(selectedChannelId())) || null;
}

/** The thread a card of either list stands for. */
const cardThread = (/** @type {HTMLElement} */ row) =>
  String(row.getAttribute(row.hasAttribute("data-card") ? "data-id" : "data-context-id"));

/**
 * Draw `row`, the card of thread `id`, highlighted or not: `data-highlighted` on it, which the stylesheet
 * draws, and its button saying whether the card is expanded. Written only where it changes.
 *
 * @param {HTMLElement} row
 * @param {string} id
 */
function paintLitCard(row, id) {
  const lit = litCard() === id;
  if (row.hasAttribute("data-highlighted") !== lit) {
    if (lit) row.setAttribute("data-highlighted", "true");
    else row.removeAttribute("data-highlighted");
  }
  const button = childByClass(row, row.hasAttribute("data-card") ? "side-card" : "thread-open");
  // In the Threads view, whether the card is shown whole, unpacked or not (`#440 thread-card-pack-toggle`).
  const whole = cardsCycle() && row.hasAttribute("data-card") ? row.getAttribute("data-card-state") !== "collapsed" : lit;
  const expanded = whole ? "true" : "false";
  if (button && button.getAttribute("aria-expanded") !== expanded) button.setAttribute("aria-expanded", expanded);
}

/** Draw every card of both lists as the highlight now stands, and the links between the columns with it. */
function paintLitCards() {
  for (const list of ["side-cards", "thread-list"]) {
    for (const row of /** @type {HTMLElement[]} */ ([...leftEl(list).children])) {
      if (row.hasAttribute("data-unpack-thread") || row.hasAttribute("data-main-row")) continue;
      paintLitCard(row, cardThread(row));
    }
  }
  noteLinkedSelection();
}

/**
 * A press on card `row`, of thread `id`: highlighted, or put out when it is the one highlighted. The card
 * stays where it was on the screen, and is then shown whole where it fits: a card grown past the foot of
 * its list comes up as far as shows it, and one taking the highlight from a card above it does not jump
 * up as that one shrinks.
 *
 * @param {string} id
 * @param {HTMLElement} row
 */
function toggleLitCard(id, row) {
  const channel = String(selectedChannelId());
  if (litCards.get(channel) === id) litCards.delete(channel);
  else litCards.set(channel, id);
  const pane = row.parentNode === leftEl("side-cards") ? sidePane : channelPane;
  const before = rowShown(row) ? row.getBoundingClientRect().top : null;
  paintLitCards();
  if (before === null || !row.parentNode) return;
  // `#338 column-overscroll`. In the room past the right column's foot, the room is fitted to the card as it
  // now is before the card is put back: the last card put out would otherwise be held by the end of the list
  // the browser clamped to, and dropped by what it lost.
  fitOverscroll(pane.scroller());
  pane.scroller().scrollTop += row.getBoundingClientRect().top - before;
  bringIntoView(pane, row);
  noteOverscrollReader(pane.scroller());
}

/**
 * A card's way into its thread: "3 replies · 2 unread", the owner's "N reply M unread" bit, a button of
 * its own beside the card's, which only highlights. Its parts are named for its list, by `prefix`:
 * "side-card" on the right, "thread-card" in the Threads list. With no count and nothing unread it says
 * "Open thread". A screen reader hears what it does, in the words it shows.
 *
 * @param {string} prefix
 * @param {string} replies "3 replies", "about 3 replies", or "".
 * @param {string} unread "2 unread", "2+ unread", "1 new", or "".
 * @param {() => void} open
 */
function cardOpener(prefix, replies, unread, open) {
  const button = document.createElement("button");
  button.className = `${prefix}-open`;
  button.setAttribute("type", "button");
  button.append(...cardCountParts(prefix, replies, unread, "Open thread"));
  const said = [replies, unread].filter(Boolean).join(" · ");
  button.setAttribute("aria-label", said ? `Open this thread, ${said}` : "Open thread");
  button.setAttribute("title", "Open this thread");
  sayRowShortcut(button, `${prefix}-open`);
  button.addEventListener("click", open);
  return button;
}

/**
 * What a card says of its thread's replies, "3 replies · 2 unread", as the parts it is drawn in: the
 * count, and the unread in the accent (`${prefix}-replies`, `${prefix}-unread`). The one place those
 * words and that accent are made, for the button into a card's thread (`cardOpener`) and for the card
 * at the top of an open thread (`paintThreadCard`, `#298 thread-card-header`). With neither, `empty`
 * alone, or nothing for "".
 *
 * @param {string} prefix
 * @param {string} replies "3 replies", "about 3 replies", or "".
 * @param {string} unread "2 unread", "2+ unread", "1 new", or "".
 * @param {string} [empty]
 * @returns {Node[]}
 */
function cardCountParts(prefix, replies, unread, empty = "") {
  /** @type {Node[]} */
  const parts = [];
  if (replies || (!unread && empty)) {
    const count = document.createElement("span");
    count.className = `${prefix}-replies`;
    count.textContent = replies || empty;
    parts.push(count);
  }
  if (unread) {
    const fresh = document.createElement("span");
    fresh.className = `${prefix}-unread`;
    fresh.textContent = unread;
    if (parts.length) parts.push(document.createTextNode(" · "));
    parts.push(fresh);
  }
  return parts;
}

/**
 * Into the thread of card `row`, by its replies button or →. In two columns on the right, noted as
 * entered from the cards so that ← goes back out to them (`threadOutFromKeyboard`); in one column as
 * any thread opens, its Back the way to the Threads list. From the thread list, across the window, its
 * Back, Esc and ← the way back to the list (`openThreadFromList`, `#281 view-keys`).
 *
 * `#298 thread-card-header`. On the right the keys go into the thread with it, from a click as from →:
 * the button pressed was in the cards, which have just gone, and a browser drops the focus of a control
 * that goes on the page's body, where the next key and the next redraw of Main would not find the
 * right column (`#284 redraw-keeps-pane`).
 *
 * @param {string} id
 * @param {any} summary
 * @param {HTMLElement} row
 */
function enterCard(id, summary, row) {
  if (row.parentNode !== leftEl("side-cards")) return openThread(id, summary);
  if (layoutShown.threadList()) return openThreadFromList(id, summary);
  const opened = selectSideThread(id, summary);
  columns.side.fromCards = true;
  if (sideShowing()) focusList(sidePane);
  return opened;
}

/** The right column's card of thread `id`, or null. */
function cardRowOnRight(/** @type {string | null} */ id) {
  if (id === null) return null;
  return /** @type {HTMLElement[]} */ ([...leftEl("side-cards").children]).find((row) => row.getAttribute("data-id") === id) || null;
}

/** The thread whose card is highlighted while that card shows on the right beside Main, which the links follow; else null. */
function litCardOnRight() {
  if (!columnLinksShown() || columns.side.mode !== "list") return null;
  const row = cardRowOnRight(litCard());
  return row && rowShown(row) ? String(row.getAttribute("data-id")) : null;
}

/**
 * The thread cards on screen: the right column's while two columns show them, or the thread list does
 * (`#281 view-keys`: the same cards, across the window), or the Threads list in one column. Null
 * otherwise.
 *
 * @returns {HTMLElement | null}
 */
function cardsOnScreen() {
  if (!channelOnScreen()) return null;
  if (sideColumnUp()) return columns.side.mode === "list" ? leftEl("side-cards") : null;
  return inColumn(columns.main, () => (selection().view === "threads" && !pinsView().only ? leftEl("thread-list") : null));
}

/**
 * The card → goes into, on the cards on screen — the right column's while the key is for that column,
 * or the Threads list in one column: the card whose own button holds the focus, as Tab leaves it, which
 * is the card the key was pressed on; else, with the keys on a message unpacked under a card, that card,
 * as → on a message of All goes into its thread (`#407 unpack-row-parity`); else the highlighted card
 * while it shows; else, on the right, the card the keys are on, while it is on screen. Null anywhere
 * else.
 *
 * @param {HTMLElement} target
 * @returns {HTMLElement | null}
 */
function cardForArrow(target) {
  const list = cardsOnScreen();
  const right = list === leftEl("side-cards");
  if (!list || (right && paneFor(target) !== sidePane)) return null;
  const rows = /** @type {HTMLElement[]} */ ([...list.children])
    .filter((row) => !row.hasAttribute("data-unpack-thread") && !row.hasAttribute("data-main-row") && rowShown(row));
  const held = rows.find((row) => row !== target && nodeWithin(row, target));
  if (held) return held;
  const keyed = right ? keyContext(target).row : null;
  const under = keyed ? keyed.getAttribute("data-unpack-thread") : null;
  if (under !== null) return rows.find((row) => cardThread(row) === under) || null;
  const lit = litCard();
  const chosen = lit === null ? undefined : rows.find((row) => cardThread(row) === lit);
  if (chosen) return chosen;
  if (!right) return null;
  const { row } = keyContext(target);
  return row && row.hasAttribute("data-card") && rowInView(sidePane, row) ? row : null;
}

/** Esc: the highlighted card on screen put out. Whether there was one. */
function putOutLitCard() {
  const list = cardsOnScreen();
  const lit = litCard();
  const row = list && lit !== null
    ? /** @type {HTMLElement[]} */ ([...list.children]).find((one) => cardThread(one) === lit && rowShown(one)) : undefined;
  if (!row || lit === null) return false;
  toggleLitCard(lit, row);
  return true;
}

// --- the card at the top of an open thread --------------------------------------------------------
//
// `#298 thread-card-header`. The owner, 2026-10-14: "I quite like the visual style of the thread cards
// with their summaries and their slugs and their designated colors. When I drill into a given thread
// card to see the messages inside it, I would like to KEEP that same style. I.e. in lieu of the header
// at the top ... I would like to see the SAME colorful card move up to the top of the screen and KEEP
// its connector linkage over to the main channel message on the left column. So it just moves up and
// disgorges its internal messages underneath while keeping the visual object of the thread card. We can
// still sneak in an X button somewhere for a mouse-only exit back to all-threads view."
//
//   * ONE CARD, THE LIST'S. The heading over an open thread is a card drawn by the list card's rules and
//     in its words (`sideCardText`, `briefAge`, `sideCardReplies`, `sideCardUnread`, `cardCountParts`):
//     what the thread is about, three lines of it, edged and ringed in its thread's colour and tinted as
//     a highlighted card is, then how long since it moved, its replies and how many are unread, and the
//     X. A thread nobody has answered spends no colour (`#252 thread-button-state`): it is grey until its
//     first reply. The markup is the card's skeleton, one per column, and `paintThreadCard` fills
//     whichever it is handed, so a column with a card at its top has this one.
//   * PINNED BY WHERE IT STANDS. It is in the column's heading, outside the scroller, so it cannot scroll
//     away and adds nothing inside the scroller for the reader's place (`#280 scroll-stability`) to
//     allow for; and its height is fixed, the text clamped as a card's is, so a name or a summary that
//     arrives after the thread opened moves nothing under the reader.
//   * A PRESS FOLDS IT TO A BAR (`#388 thread-header-collapse`, below), and opens it again. Its button's
//     name, and its tooltip, are the whole text whatever is drawn.
//   * IT IS JOINED TO ITS ROOT. In two columns the line from the thread's first message on Main goes to
//     it (`columnLinkPairs`, web/src/view/channel/columnlinks.ts), as it went to its card in the list.

/**
 * By the id of each card at the top of a thread, what it was last drawn from, so a pass that changes
 * nothing writes nothing to it.
 *
 * @type {Map<string, string>}
 */
const threadCardsDrawn = new Map();

/**
 * Whether thread `choice` is one nobody has answered, by the rule its grey Thread tag is drawn by
 * (`unansweredRoot`): no colour is spent on it.
 *
 * @param {import("./src/present/rows/index.ts").ThreadChoice} choice
 * @param {ReplyTally} tally
 */
function threadUnanswered(choice, tally) {
  const root = choice.root;
  if (!root || !root.thread) return false;
  const entry = tally.threads.get(choice.id);
  return unansweredRoot(root, threadReplyCount(choice.id, root), entry ? entry.held : 0,
    exactReplyCount(choice.id, root, tally));
}

/**
 * Draw `card`, the card at the top of a thread, for thread `choice`, from `tally`: what the thread is
 * about, how long since it moved, its replies and unread, and its colour — or grey, for a thread nobody
 * has answered. Its parts are the elements whose ids are the card's and `-text`, `-age` and `-count`.
 * Written only when what it would draw has changed.
 *
 * @param {HTMLElement} card
 * @param {import("./src/present/rows/index.ts").ThreadChoice | null} choice
 * @param {ReplyTally} tally
 */
function paintThreadCard(card, choice, tally) {
  if (!choice) return;
  const view = cardView(choice);
  const text = view.text;
  const age = briefAge(Number.isFinite(choice.at) ? choice.at : choice.started);
  const replies = sideCardReplies(choice, tally);
  const unread = sideCardUnread(choice, tally);
  const unanswered = threadUnanswered(choice, tally);
  const summarized = cardIsSummarized(choice);
  // `#528 thread-card-markdown-and-cap`. Waiting for its summary, it reads the Threads list again soon.
  watchPendingSummaries([choice], card.id);
  const signature = JSON.stringify([choice.id, view.mode, text, summarized, age, replies, unread, unanswered]);
  if (threadCardsDrawn.get(card.id) === signature) return;
  threadCardsDrawn.set(card.id, signature);
  if (card.getAttribute("data-thread") !== choice.id) card.setAttribute("data-thread", choice.id);
  // `#428 summary-cards-full`. As a list card's text, this card's summary, when that is what it says, is drawn whole.
  if (summarized !== card.hasAttribute("data-summarised-card")) {
    if (summarized) card.setAttribute("data-summarised-card", "");
    else card.removeAttribute("data-summarised-card");
  }
  const state = unanswered ? "start" : "open";
  if (card.getAttribute("data-thread-state") !== state) card.setAttribute("data-thread-state", state);
  const hue = String(threadHue(choice.id));
  if (unanswered) card.style.removeProperty("--thread-hue");
  else if (card.style.getPropertyValue("--thread-hue") !== hue) card.style.setProperty("--thread-hue", hue);
  const button = leftEl(`${card.id}-text`);
  let words = childByClass(button, "side-card-text");
  if (!words) {
    words = document.createElement("span");
    words.className = "side-card-text";
    button.append(words);
  }
  // `#528 thread-card-markdown-and-cap`: its root's formatted words, as a list card's.
  markCardLayout(card, view.mode, view.cut);
  fillCardText(words, view, text);
  if (button.getAttribute("title") !== text) button.setAttribute("title", text);
  setTextIfChanged(leftEl(`${card.id}-age`), age);
  // `#479 message-time-full-date`. Hovering the open thread's age says the day it last moved, whole.
  const cardLastMoved = Number.isFinite(choice.at) ? choice.at : choice.started;
  const ageNode = leftEl(`${card.id}-age`);
  const ageTitle = Number.isFinite(cardLastMoved) ? fullLocalAt(new Date(cardLastMoved)) : null;
  if (ageTitle && ageNode.getAttribute("title") !== ageTitle) ageNode.setAttribute("title", ageTitle);
  // A span, not the list card's button: the thread is open already. Never "Open thread".
  const count = leftEl(`${card.id}-count`);
  count.replaceChildren(...cardCountParts("side-card", replies, unread));
  if (count.hidden !== (!replies && !unread)) count.hidden = !replies && !unread;
  // The link to the card goes, or comes, with the thread's first reply.
  scheduleColumnLinks();
}

// --- the card at the top of a thread folded to a bar ------------------------------------------------
//
// `#388 thread-header-collapse`. The owner, 2026-10-19: "On a small mobile screen, I don't like wasting
// vertical space on the thread header at the top. I like the consistent visual language for the colored
// thread cards. But I don't always want to use the space. The same way tapping a message expands and
// contracts it I want tapping that header to collapse it to basically a thin colored bar. Just enough
// vertical height that you can see the inside color as well as the border color and so it's wide enough
// to tap on."
//
//   * A PRESS ANYWHERE ON THE CARD but its X, or a link in it, folds it to a bar of its fill and its
//     edge, and a press on the bar opens it again. Its text is the button the keys reach, which says
//     `aria-expanded` and is named by the whole text, folded or not; folded, it is the bar's press
//     target, taller than the colour it covers (web/voice.css).
//   * ONE CHOICE FOR THE DEVICE, every thread, both columns: kept as the neighbouring settings are, and
//     a card drawn for another thread is folded or not as the last one was.
//   * NOTHING UNDER IT MOVES. The card is in its column's heading, so folding it gives its height to the
//     list under it, whose top rises by that much; the list is scrolled by as much, so every row the
//     reader sees stays where it was, and a reader on the newest line stays on it (`followFoot`, and the
//     room past the right column's foot fitted again, `#338 column-overscroll`).
//   * It replaces `#298`'s press on clamped text opening the card over the list: the press has one
//     meaning on the card now, as it has on a message.

const THREAD_CARD_FOLD_KEY = DEVICE_SETTINGS.threadCardFolded.key;

/** The cards at the top of a thread, one per column, and the list each heads. */
const THREAD_CARD_LISTS = [["thread-card", "scroll-area"], ["side-thread-card", "side-scroll"]];

/**
 * Whether the cards at the top of a thread are folded to a bar: the reader's choice, kept on the device and
 * read from it here, as the page loads. ABSENT MEANS OPEN; a browser refusing storage reads as open
 * (`DEVICE_SETTINGS.threadCardFolded.refused`). One owner in web/src/core/settings/switches.ts
 * (`#624 voice-page-modularity`).
 *
 * @type {import("./src/core/settings/index.ts").KeptSwitch}
 */
const threadCardFoldChoice = createKeptSwitch({
  setting: DEVICE_SETTINGS.threadCardFolded,
  deviceSettings: () => deviceSettings(),
});

/** Draw the cards at the top of a thread folded or open, as `threadCardFoldChoice` says: written only where it changed. */
function renderThreadCardFold() {
  for (const [id] of THREAD_CARD_LISTS) {
    const card = leftEl(id);
    if (card.hasAttribute("data-folded") !== threadCardFoldChoice.on()) {
      if (threadCardFoldChoice.on()) card.setAttribute("data-folded", "");
      else card.removeAttribute("data-folded");
    }
    const button = leftEl(`${id}-text`);
    const expanded = threadCardFoldChoice.on() ? "false" : "true";
    if (button.getAttribute("aria-expanded") !== expanded) button.setAttribute("aria-expanded", expanded);
  }
}

/**
 * A click on `card`, the card at the top of a thread: folded or opened, unless it was on the X or a link
 * in it, which do their own acts, or it ended a selection of the card's text made with the mouse.
 *
 * @param {HTMLElement} card
 * @param {Event} event
 */
function threadCardPressed(card, event) {
  for (let node = /** @type {Node | null} */ (event.target); node && node !== card; node = node.parentNode) {
    const tag = String(/** @type {HTMLElement} */ (node).tagName || "").toLowerCase();
    if (tag === "a" || (tag === "button" && node !== leftEl(`${card.id}-text`))) return;
  }
  const selection = typeof getSelection === "function" ? getSelection() : null;
  if (selection && !selection.isCollapsed && nodeWithin(card, selection.anchorNode)) return;
  setThreadCardFolded(!threadCardFoldChoice.on());
}

/**
 * Fold the cards at the top of a thread to a bar, or open them: kept, and drawn, with every list a card
 * on screen heads scrolled by as far as its top moved, so the rows the reader sees stay where they were.
 * A reader exactly on the newest line is kept on it (`followFoot`); one a little above it stays where
 * they are, not pulled down the slack. In the room past the right column's foot (`#338
 * column-overscroll`) the room is fitted to the column at once: a fold leaves the reader where they are
 * in it, and an opening too, except at the room's end, which keeps its own rule — the last card's top no
 * higher than the middle of the column, which a shorter column has lower — and so lets the rows there
 * down by at most half the card's growth, in the frame the card opens rather than one after.
 *
 * @param {boolean} folded
 */
function setThreadCardFolded(folded) {
  threadCardFoldChoice.keep(folded);
  const held = THREAD_CARD_LISTS.filter(([card]) => drawnOnPage(leftEl(card))).map(([, id]) => {
    const area = leftEl(id);
    return { area, top: area.getBoundingClientRect().top, scrolled: area.scrollTop,
      foot: atBottom(area, 1) && overscrollRoom(area) === 0 };
  });
  threadCardFoldChoice.set(folded);
  renderThreadCardFold();
  // From the scroll position read before, not the one now: a list grown under a reader near its foot has
  // been clamped to its new end by the layout that measured it, and adding to that put them further up.
  for (const { area, top, scrolled, foot } of held) {
    area.scrollTop = scrolled + area.getBoundingClientRect().top - top;
    fitOverscroll(area);
    if (foot) followFoot(area);
  }
  // The line from the root meets the card, or the bar, at its middle.
  scheduleColumnLinks();
}

/** The right column's card at the top of its thread, as the rows now stand, after a pass over either column's rows. */
function paintSideThreadCard(tally = replyTally()) {
  if (!sideShowing()) return;
  paintThreadCard(leftEl("side-thread-card"), /** @type {any} */ (sideThreadChoice()), tally);
}

// --- the card moving up as its thread opens ----------------------------------------------------------
//
// `#298 thread-card-header`. The owner: "I would like to see the SAME colorful card move up to the top
// of the screen ... So it just moves up and disgorges its internal messages underneath while keeping
// the visual object of the thread card." A thread opened from its card while that card is on screen —
// its replies button, → on it, the picker or its Thread(N) on Main while the cards are up on the right,
// or the same from the thread list across the window — has its card at the top start where the card
// in the list was, and slide up to its place, while the thread's messages fade in under it.
//
//   * FIRST, LAST, INVERT, PLAY. Where the card in the list is, read before the thread is shown; where
//     the card at the top is, read once it is drawn, the thread laid out under it; the distance between
//     them written on the card at the top (`--flight-x`, `--flight-y`), and the stylesheet's animation
//     starting it there and ending it at its place. Only a transform and an opacity move, so nothing
//     under the card is laid out again and the thread is at its place — its foot, or its first unread —
//     before it fades in (`#280 scroll-stability`). Across layouts too: from the thread list's card to
//     the top of the window, the two places measured on the screen.
//   * The stylesheet says how long (`--card-flight`), and says 0 for a reader whose system asks for less
//     motion: then nothing moves and nothing fades (`cardFlightMs`). The script asks no media query.
//   * In two columns the line from the thread's root follows the card up, drawn every frame it moves.
//   * Never for a thread opened any other way — from a tag in one column, from the picker with its card
//     off screen, from one thread to another, on a reload — which has no card on screen to come from;
//     nor back down when the thread closes.

/**
 * How long the card at the top of a thread takes to move up from its card in the list, in ms: the
 * stylesheet's `--card-flight` on #screen-main, which is 0 for a reader whose system asks for less
 * motion, as `--column-glide` is "auto" then. 0 too where the stylesheet says nothing.
 */
function cardFlightMs() {
  const style = typeof window.getComputedStyle === "function" ? window.getComputedStyle(leftEl("screen-main")) : null;
  const said = style ? String(style.getPropertyValue("--card-flight")).trim() : "";
  const parsed = /^(\d+(?:\.\d+)?)(ms|s)$/.exec(said);
  return parsed ? Number(parsed[1]) * (parsed[2] === "s" ? 1000 : 1) : 0;
}

/**
 * Where thread `id`'s card is in the cards on screen — on the right, or in the thread list — while any
 * of it shows: where the card at the top of its thread starts from. Else null.
 *
 * @param {string} id
 */
function cardFlightStart(id) {
  const row = cardRowOnRight(id);
  if (!row || !rowShown(row) || !rowInView(sidePane, row)) return null;
  return row.getBoundingClientRect();
}

/**
 * The card moving up while it is, moving it, and ending it by its animations or a timer: one owner in
 * web/src/view/channel/cardflight.ts (`#624 voice-page-modularity`). The page destructures its commands
 * under their old names.
 */
const cardFlightOwner = createCardFlight({
  clock: clockOnUse(),
  ms: () => cardFlightMs(),
  drawn: (node) => drawnOnPage(node),
  linksShown: () => columnLinksShown(),
  drawLinks: () => drawColumnLinks(),
  scheduleLinks: () => scheduleColumnLinks(),
  frame: (run) => {
    if (typeof requestAnimationFrame !== "function") return false;
    requestAnimationFrame(run);
    return true;
  },
});
const { flyThreadCard, endCardFlightPart, endCardFlight } = cardFlightOwner;

/** Whether `node` and everything it is in are drawn: none of them `hidden`. */
function drawnOnPage(node) {
  for (let at = node; at; at = /** @type {HTMLElement} */ (at.parentNode)) if (at.hidden) return false;
  return true;
}

/** The right column with no thread selected: its cards, and the thread list read behind them. */
function showSideList() {
  openSideCards();
  // Not before sign-in has answered: the reopen's read reads the list behind the cards then.
  if (serverFacts.configApplied()) guardQuietly(refreshThreadDirectory)();
}

/**
 * Select thread `id` on the right, from wherever the reader asked for it: its tag on a row, a count
 * of unread replies, a card, the view picker. Drawn at once from what the page holds and read
 * behind, landing on its first unread, exactly as a thread opens in one column.
 */
function selectSideThread(id, summary = null) {
  noteDeepLinkReaderMove();
  const thread = String(id);
  const channel = String(selectedChannelId());
  if (!layoutShown.columns() || !channel) return undefined;
  // Entered from its card, `enterCard` says so once this returns. `#277 thread-card-focus`.
  columns.side.fromCards = false;
  const known = summary || channelStore.current().threads.find((held) => String(held.id) === thread) || null;
  sideThreads.set(channel, { id: thread, summary: known });
  const was = columns.side.mode;
  // `#298 thread-card-header`. Where its card is, while the cards show it, for the card at the top to
  // move up from.
  const from = was === "list" ? cardFlightStart(thread) : null;
  if (was === "list") rememberCardPlace();
  // `#410 inline-reply-editor`. A reply open under a row unpacked among the cards goes with them, its
  // words kept as a draft: it is the left column's reply (`openReply`), drawn in a list now hidden.
  if (replyState.docked && replyState.column !== columns.side && replyPane() === sidePane) putDockedReplyAway();
  columns.side.mode = "thread";
  renderSideMode();
  const opened = inColumn(columns.side, () => {
    if (was === "thread" && selection().view === "thread" && String(selection().threadId) === thread) {
      // Shown again as it was put away, two columns coming back to it: drawn again where a Done or an
      // Undo made meanwhile in one column left its rows behind its copy. `#306 done-stale-across-views`.
      if (rowsMissDone()) renderCachedTimeline();
      return undefined;
    }
    return changeChannelView("thread", thread, known);
  });
  flyThreadCard(leftEl("side-thread-card"), from, leftEl("side-pane"));
  return opened;
}

/** The X: the thread goes, and the cards come back. Its rows are held, so selecting it again is instant. */
function closeSideThread() {
  // `#277 reply-in-column`. A reply open under the thread goes with it, its words kept as a draft.
  if (replyState.column === columns.side) putDockedReplyAway();
  // `#262 search-load-older`. The thread's walk for the search goes with it: nothing else it asks
  // about changes when the cards come back.
  endSearchWalk(columns.side);
  const channel = String(selectedChannelId());
  sideThreads.delete(channel);
  if (columns.side.mode === "thread") {
    inColumn(columns.side, () => {
      dropReplyArrowTap();
      putRepliesBack();
      rememberChannelDraft();
      rememberChannelContext();
      ++loadState().generation;
    });
  }
  showSideList();
}

/**
 * Forget what the right column shows, for another channel or another credential. `reopen`, for a
 * channel the reader changed to: show that channel's thread selected there earlier in this session,
 * or its cards and the thread list read behind them.
 */
function resetSideColumn(reopen = false) {
  // `#404 threads-unpack-all`. Unpack reads were the last channel's.
  unpackReadQueue.forget();
  unpackWatch.stop();
  endSearchWalk(columns.side);
  columns.side.searchNote = null;
  // `#262 search-load-older`. And the cards' walk, and what the last one found: they were this channel's.
  endRepliesWalk();
  repliesWalks.forgetNote();
  inColumn(columns.side, () => {
    dropReplyArrowTap();
    gatherState().gathered = null;
    listState().rowsGathered = false;
    ++loadState().generation;
  });
  // Made afresh, as when it was first used: only while it is not the column worked on, whose records
  // are live.
  if (columnWork.active() !== columns.side) {
    columns.side.left = false;
    columns.side.older = freshOlderPages();
    columns.side.arrows = freshReplyArrows();
    columns.side.load = freshLoadState();
    columns.side.entry = freshEntryState();
    columns.side.pins = freshPinsView();
    columns.side.tools = freshScrollTools();
    columns.side.filter = freshFilterState();
    columns.side.list = freshListState();
    columns.side.rows = freshTimelineRows();
    columns.side.selection = freshSelection();
    columns.side.composer = freshComposerState();
    columns.side.done = freshDoneState();
    columns.side.place = freshScrollPlace();
    columns.side.gather = freshGatherState();
  }
  for (const id of ["side-log", "side-thread-list", "side-summary", "side-pinned-log", "side-outgoing-log"]) {
    leftEl(id).replaceChildren();
  }
  leftEl("side-notice").hidden = true;
  leftEl("side-older").hidden = true;
  leftEl("side-loading").hidden = true;
  cardsDrawn.forget();
  leftEl("side-cards").replaceChildren();
  if (!sideColumnUp() || !reopen) {
    columns.side.mode = "list";
    if (sideColumnUp()) openSideCards();
    return;
  }
  // `#281 view-keys`. The thread list shows cards, whatever two columns had selected on the right.
  if (layoutShown.threadList()) {
    columns.side.mode = "list";
    showSideList();
    return;
  }
  const open = sideThreads.get(String(selectedChannelId()));
  if (open) {
    columns.side.mode = "list";
    guardQuietly(() => selectSideThread(open.id, open.summary))();
  } else {
    showSideList();
  }
}

/**
 * Read the right column's thread behind its rows, or the thread list behind its cards: the poll's
 * second read, and coming back to the page. Nothing while one column is up.
 *
 * `since` is `replayLedger.started()` as it stood when the refresh asking for this began, before its
 * read of Main. `#231 two-column-polish`. A read of the thread on the right that went on the wire
 * after that is this read already: a thread selected while the poll's read of Main was out queued its
 * read behind it, and that read ran before the poll got here. Reading it again brought nothing the
 * first had not, a second request for the same rows. See `answeredSince` for which reads count.
 *
 * @param {string} reason
 * @param {number | null} [since]
 */
async function readSideColumn(reason, since = null) {
  if (!sideColumnUp()) return;
  if (columns.side.mode === "thread") {
    await inColumn(columns.side, () => (since !== null && answeredSince(since, reason)
      ? undefined : timelineView.loadDiscord({ keepPosition: true, reason, readAs: "side" })));
  } else {
    await refreshThreadDirectory();
  }
}

/**
 * The newest timeline read each column sent and that has not failed before landing a page: the view
 * it was for (`channelContextKey`), its number among `replayLedger.started()`, and whether it read the
 * view in full rather than as what changed. `#231 two-column-polish`. One still on the wire counts,
 * as the read a refresh would send would only queue behind it; if it then fails, the right column's
 * line says so (`renderSideFreshness`) and the next poll reads the view again. A chain of what
 * changed that fails after its first page keeps its record, which no refresh begun before it can
 * act on: the chain ran queued behind that refresh's read of Main, and its failure is that read's.
 *
 * @type {Map<ColumnFields, {context: string, read: number, full: boolean}>}
 */
const columnReadsSent = new Map();

/**
 * Whether a read this column sent of the view it shows now, after read number `since`, answers for
 * a read for `reason`. Only one sent after: one that left BEFORE the refresh asking may have left
 * before what that refresh reads for — an edit the stream announced, say — had happened. And only
 * one that read as much as `reason` needs: a read in full answers anything, and a read of what
 * changed answers a reason only where that reason may be read so too (`forwardCursor`) — on a
 * channel whose changes carry additions alone, an edit's read is in full, and a delta read for the
 * poll would leave the edit unread.
 */
function answeredSince(since, reason) {
  const sent = columnReadsSent.get(columnWork.active());
  return Boolean(sent) && sent.read > since && sent.context === channelContextKey() &&
    (sent.full || timelineReader.forwardCursor(reason) !== null);
}

/** Whether a message belongs to the thread the right column is showing. */
function inSideThread(message) {
  if (!sideShowing()) return false;
  return inColumn(columns.side, () => selection().view === "thread" && threadOf(message) === String(selection().threadId));
}

// --- the links between Main's roots and their cards ------------------------------------------------
//
// `#261 two-column-connectors`. The connectors between Main's roots and their cards on the right are
// web/src/view/channel's (`createColumnLinks`, `#624 voice-page-modularity`), with what is drawn and
// what it costs. The page tells it which columns show, the selection and the highlighted card; the
// names below are the page's commands to it. Nothing here scrolls for the mouse (`#1144`).

const columnLinks = createColumnLinks({
  lookup: (id) => leftEl(id),
  onTwoColumns: () => layoutShown.columns() && screens.screen() === "main" && screens.view() === "discord",
  sideMode: () => columns.side.mode,
  mainList: () => channelPane.list(),
  openThread: () => /** @type {any} */ (sideThreadChoice()),
  activePane: () => activePane(),
  sidePane: () => sidePane,
  litCard: () => litCardOnRight(),
  cardRow: (thread) => cardRowOnRight(thread),
  hue: (thread) => Number(threadHue(thread)),
});

/** Whether the links have a place: two columns, the channel on the main screen, and cards or a thread's card. */
function columnLinksShown() {
  return columnLinks.shown();
}

/** Ask for the links to be drawn again at the next animation frame. */
function scheduleColumnLinks() {
  columnLinks.schedule();
}

/** Draw the links as the page stands now (`flyThreadCard`'s frames, and the frame asked for). */
function drawColumnLinks() {
  columnLinks.draw();
}

/** The selection was drawn again (`drawSelection`): the links follow it. */
function noteLinkedSelection() {
  columnLinks.noteSelection();
}

/**
 * How a deliberate glide moves (a jump the reader asked for, never the links'): "smooth", or
 * "auto" where the stylesheet says so (`--column-glide`).
 *
 * @returns {ScrollBehavior}
 */
function glideBehavior() {
  return columnLinks.glideBehavior();
}

// --- keeping the channel view fresh -----------------------------------------------------------
//
// The channel used to be fetched exactly ONCE, on the first switch into the view, because the
// load was guarded on the log being empty. Everything after that was whatever had been true the
// first time you looked. On the owner's phone that meant a view hours out of date, presented with
// no hint that it was stale — which is worse than an empty view, because it reads as current.
//
// The first visit to each Main / Threads / All context fetches its projection, then tab switches
// restore that page's in-memory snapshot. The active context still refreshes on a timer. A
// configured adapter may additionally PUSH changes into the server; the timer is the authoritative
// fallback if that adapter or the browser's stream is interrupted. How long between those reads
// (`DISCORD_POLL_MS`) is web/src/core/sync's.

// `#195 send-resilience`. How long a read of the newest messages may take before the page stops
// waiting and says the refresh failed. Above what a slow service ordinarily takes — ten to twelve
// seconds, in the incident this was written for — and well inside the server's own fifty.
const CHANNEL_READ_TIMEOUT_MS = 20000;

// --- not asking again while the server still reads (`remote-ops-audit` W7) -----------------------
//
// Giving up on a read at `CHANNEL_READ_TIMEOUT_MS` stops the page waiting, not the server: its own read
// of the chat service runs on to `SERVER_READ_BOUND_MS`, and behind a bridge that read is a scan of the
// whole space. The page used to poll again regardless, a second scan beside the first.
//
// So once a read of a view is given up on, the POLL does not read that view again while the read given
// up on may still be running at the server: until it answers or fails, or the server's bound has passed
// since it was sent, or a live event for its channel says the service is answering. A live event that
// arrived while the page still waited means no hold at all. Nothing else is put off: a pull to refresh,
// entering a channel or a view, coming back to the page, a re-read after a change, and every read that
// fills a gap the stream reports (a reset, an edit or removal out of its replay tail) read at once, as
// before. Each of those may bring what no answer begun before it can, and the poll is the one read that
// only asks again. The read given up on is not aborted either: if its answer lands, it is used, unless
// something newer has replaced it meanwhile (`abandonedReadStands`).

/**
 * By view (`readHoldKey`), until when a read of it the page gave up on may still be running at the
 * server, and the channel it is in; the server's bound (`SERVER_READ_BOUND_MS`) and the reasons a hold
 * puts off (`HELD_REASONS`: only the poll) with it (web/src/core/sync `createReadHolds`, `#624
 * voice-page-modularity`).
 *
 * @type {import("./src/core/sync/index.ts").ReadHolds}
 */
const readHolds = createReadHolds({ now: () => Date.now() });

/**
 * How many live events have arrived for each channel: a read given up on whose answer lands after one
 * may be older than what the event brought (`abandonedReadStands`); how many of those were news, not
 * the stream's replay tail, which says the service is answering and sets no hold when that read is
 * given up on; and how many were removals (`noteLiveRemoval`), after which a placing read out as one
 * arrived is not folded (`landPlacingRead`, `#308 live-reply-in-main`). web/src/core/sync
 * `createLiveEvents` (`#624 voice-page-modularity`).
 *
 * @type {import("./src/core/sync/index.ts").LiveEvents}
 */
const liveEvents = createLiveEvents();

/**
 * Reads the page gave up on: their views' holds, what a live event does to them, and whether
 * a late answer may still be used (web/src/core/sync `createAbandonedReads`, `#624
 * voice-page-modularity`). Landing one (`landAbandonedTimelineRead`) stays below.
 *
 * @type {import("./src/core/sync/index.ts").AbandonedReads}
 */
const abandonedReads = createAbandonedReads({
  holds: readHolds,
  live: liveEvents,
  threading: () => providerFacts.threading(),
  hiding: () => readModes.hiding(),
  contextKey: () => channelContextKey(),
  selectedChannel: () => String(selectedChannelId()),
  currentStore: () => channelStore.current(),
  asked: () => readOrder.asked(),
  generation: () => loadState().generation,
});



/**
 * The poll's timer — armed after each poll, held while the page is hidden and taken up when it is shown,
 * its wait shortened for a stand-in read's quick retry — is web/src/core/sync's (`createChannelPoll`,
 * `#624 voice-page-modularity`); the poll itself (`pollDiscord`) is here.
 */
const channelPoll = createChannelPoll({
  clock: clockOnUse(),
  now: () => Date.now(),
  hidden: () => document.hidden,
  onChannel: () => screens.view() === "discord",
  take: (delay) => standInRetries.take(delay),
  poll: () => pollDiscord(),
});
/**
 * The channel's reads on the wire: one at a time across both columns, the view a timeline read out is for
 * (`channelContextKey`; `#220 view-switch-instant`), and the read queued behind it by the column it is for
 * (`#229`): one owner in web/src/core/sync/reads.ts (`#624 voice-page-modularity`).
 *
 * @type {import("./src/core/sync/index.ts").ChannelReads<ColumnFields>}
 */
const channelReads = createChannelReads();
// Every requested replacement gets a generation, including one that arrives while another request
// is in flight. Only the newest generation may draw. The newest blocked request is queued so a
// channel switch cannot be swallowed by a refresh of the channel the reader just left.
// Kept per column: `loadState().generation` (`freshLoadState`, `#624 voice-page-modularity`).
/**
 * How many timeline reads have gone on the wire, the newest of them that landed (and that could answer
 * an edit or a removal), that count when the live stream last attached, and the replayed events
 * waiting on a read that may not have seen them (web/src/core/sync `createReplayLedger`, `#43
 * replay-burst-double-read`, `#203 incremental-refresh`, `#624 voice-page-modularity`).
 *
 * @type {import("./src/core/sync/index.ts").ReplayLedger}
 */
const replayLedger = createReplayLedger();

/**
 * The channel's timeline as the page reads and draws it (web/src/view/channel
 * `createTimelineView`, `#624 voice-page-modularity`): the refresh, the step back, the store
 * drawn with no request, and All read behind Main for its counts. The page's hands — drawing,
 * settling, the other column — are passed in; the column is captured across each wait there.
 *
 * @type {import("./src/view/channel/index.ts").TimelineView}
 */
const timelineView = createTimelineView({
  el, selection, timelineRows, olderPages, loadState, listState, entryState, doneState,
  selectedChannelId, channelContextKey, channelStore, providerFacts, messageCache,
  restoreSavedBookmark, noteServerThreadHuesIn, knownChannel, viewKey,
  reader: timelineReader, timelineDeltaPages: TIMELINE_DELTA_PAGES, isCursorRefusal,
  abandonedReads, channelReads, queueDiscordLoad: (options) => queueDiscordLoad(options), finishDiscordLoad: (column) => finishDiscordLoad(column),
  replayLedger, columnReadsSent, landAbandonedTimelineRead: (payload, read) => landAbandonedTimelineRead(payload, /** @type {any} */ (read)),
  queuePlaceUnplaced, heldMessage: (id) => liveStream.heldMessage(id), removedUpstream: (id) => removedUpstream(id), prepareSpeech: () => readAloudOwner().prepareSpeech(), guardQuietly: (fn) => guardQuietly(fn),
  settleReplays: (read, answer) => settleReplays(read, answer), setStatus,
  liveSeen: (channel) => liveEvents.seen(channel), liveNews: (channel) => liveEvents.news(channel),
  readsAsked: () => readOrder.asked(), readOrderNow: () => readOrder.now(), sentNumberOf, refreshFailed, within,
  clock: clockOnUse(), discordNode, threadCards, glom, idsOf, rowMessages, hiddenEmpty,
  hidingRead, readFilterKeeps, gatheredParent, observeOutgoingMessages: (messages) => observeOutgoingMessages(messages), noteArchived,
  noteFreshRead, noteChannelReadFailure, renderChannelLoading, backgroundRead,
  renderChannelNavigation, renderChannelSeam, renderOlderControl: () => renderOlderControl(), renderScrollTools,
  renderTodoControls, renderChannelRows, setChannelFreshness, renderOutgoingMessages: () => renderOutgoingMessages(),
  channelSummary, channelName, clearChannelScreen, holdingReplyArrows, holdingScrollTools,
  renderChannelFreshness, preservingScroll, columnWork, catchUpHeldView, catchUpFromAll, syncUnreadReplies, followInOtherColumn, keepEntryLanding,
  entryLandingMark, payThreadEntry, saveChannelScope, requestVisibleSummaries,
  seamsAfterMidnight, applySearch, columnSteps: () => /** @type {any} */ (columnSteps()), inColumn, inOtherColumn,
  replyTally, unseenReplies, followPlacement,
  unplacedCount: () => unplacedArrivals.count(), unplacedNext: () => unplacedArrivals.next(),
  heldUnansweredRoot, copyLedger, removalRecord, removedAfterRead, ownPostStands,
  foldReadState, noteReplyGaps, noteServerPageBuild, renderColumns,
  threadPresenceNote: (hasThreads, inThread) => threadPresence.note(hasThreads, inThread),
  screenBelongsToToken, readModes, readLedger, api, currentDiscordLoad: (generation, channel, readingTodo) => currentDiscordLoad(generation, channel, readingTodo),
  get noChannelWords() { return NO_CHANNEL_TO_READ; }, notePinsRevision, noteBookmark, plainReach, todoView,
  noiseRenderCount: () => noiseSettings.renderCount(), foldedOneIds, foldedOnes, readRuns, setReadMode,
  setJumpNewest, scrollToNewest, keepFilterHold, inThisColumn: (fn) => inThisColumn(fn), pinsView, searchOffered: () => searchOffered(),
  pullToRefresh, heldFilterRow, scrollAnchor, followsNewest, visibleList, noiseSettings,
  combineOn: () => combineSwitch.on(), columns, sideShowing, openThreadRootId,
  isOpenThreadRoot, readRunWords, placeInReadRun, runDaysOf, renderReplyLinks, leftEl,
  cardRunKey, cardRunRows, screensScreen: () => screens.screen(), searchField, authorChosen: () => authorFilters.chosen(), searchTyped, stillToDo, cardsDrawnForget: () => cardsDrawn.forget(), renderSideColumn, keepSelection,
  syncCanonDismissed, olderTriggerPx: OLDER_TRIGGER_PX, threadPresence, openThreadName,
  captureScroll, restoreScroll, screensView: () => screens.view(), renderJumpNewest,
});
const { loadCanon, drawProjection, walkCursorOwed, heldViewRows, drawHeldRows, queueReadAllForCounts, readAllForCounts, rowsAsDrawn, applyTimelinePage, renderCachedTimeline, mutationAwaitsRead, loadTimeline, foldLateTimelinePage, loadOlderTimeline, foldLiveMessage, inView, projectView, holdingMessage, foldTimelinePage, foldTimelineDelta, loadTodo, loadOlder, maybeLoadOlder, takeDeferredOlder, appendChannelRow, appendToColumns, applyReadRuns, readRunPlan, readRunRows, cycleReadMode, readRunsSuspended, rowIsRead, openReadRun, renderReadModeControl, applyNewestPage, channelReadPosition, settleAfterRead, landedReadPosition, todoSummary } = timelineView;
/**
 * Walking back for older rows, and for a search (web/src/view/channel `mountOlderWalks`,
 * `#624 voice-page-modularity`): the step-back control, the search's bounded walk, the server
 * search and the cards' replies walk rendering. The walk record stays on the column.
 *
 * @type {import("./src/view/channel/index.ts").OlderWalks}
 */
const olderWalks = mountOlderWalks({
  el, leftEl, selection, selectedChannelId, channelContextKey, viewKey, olderPages,
  channelStore, providerFacts, readModes, searchField, searchIdentity, searchTyped,
  filterKey, linkKindsHidden: () => linkKinds.hidden(), authorChoosing,
  pinsOnly: () => pinsView().only, readRunsSuspended, screensView: () => screens.view(),
  screensScreen: () => screens.screen(), rowDay, threadPresenceAny: () => threadPresence.any(),
  endFilterWalk, readerActed, pullFingerDown: () => pullToRefresh.fingerDown(), sideCardsShown,
  layoutColumnsShown: () => layoutShown.columns(), lastPaneCurrent: () => lastPane.current(),
  sidePaneValue: sidePane, serverSearchOwner, serverFactsTokenScope: () => serverFacts.tokenScope(),
  searchFurtherRunning, serverCopyOpen, gathering, repliesWalks, historyReadToStart,
  api, channelName, channelDirectoryChannels: () => channelDirectory.channels(),
  jumpToMessageLink, messageBody, markSearchHits, searchParsed, heldMessagesById,
  keepingCardPlace, walkCursorOwed: timelineView.walkCursorOwed, loadOlder: timelineView.loadOlder,
  guardQuietly: (fn) => guardQuietly(fn), renderJumpNewest, applySearch, preservingScroll, setStatus,
  columnWork, columns, sideShowing, inColumn, columnSteps: () => /** @type {any} */ (columnSteps()), filterChanges,
  replyJumpCurrent: () => replyJumpTicket.current(), idsOf, rowMessages, plainReach, replyTally, providerOfChannel, channelDirectoryName: () => channelDirectory.providerName(),
});
const { renderOlderControl, olderControlPressed, searchOlder, searchOlderFromField, searchReplies, searchOffered, searchKey, endSearchWalks, endSearchWalk, stopSearchWalk, renderSearchOlderNote, historyReach, searchFurtherPressed, chooseSearchAllChannels, renderRepliesSearch, renderServerSearch, scheduleServerSearch, channelViewWords, oldestLoadedMs, serviceWords, repliesFloor, repliesSearchOffered, allCover, allLoaded, repliesKey, endRepliesWalk } = olderWalks;

/**
 * The gaps a search opens in the cards and in an unpacked thread's block (web/src/view/channel
 * `createCardGaps`, `#624 voice-page-modularity`).
 *
 * @type {import("./src/view/channel/index.ts").CardGaps}
 */
const cardGaps = createCardGaps({
  leftEl, selectedChannelId, searchKey, rowShown, placeSeam, gapWords, cardGapWords,
  keepingCardPlace, filterSideCards, applySearch, searchField, searchTyped, cardThread, idsOf,
  openedCards: (key, create) => openedGaps.cards(key, create),
});
const { openedCardGaps, unpackBlocks, drawUnpackGaps, openUnpackGap, drawCardGaps, openCardGap, markReplyMatch } = cardGaps;
/**
 * A thread unpacked under its card (web/src/view/channel `createUnpackBlock`,
 * `#624 voice-page-modularity`).
 *
 * @type {import("./src/view/channel/index.ts").UnpackBlock}
 */
const unpackBlockOwner = createUnpackBlock({
  channelStore, selectedChannelId, selection, viewKey, discordNode, glom, dressChannelRow,
  answeredBy, readRunPlan, placeInReadRun, foldedOneIds, unpackFilter, unpackHeldMessages,
  unpackMessages, unpackReadSignature, isThreadRoot, cardView, cardSplit, rootInward,
  continuationOf, messageLook, readModes, readRunsSuspended: timelineView.readRunsSuspended,
  rowIsRead: timelineView.rowIsRead, idsOf,
  hideEmptyOn: () => hideEmptySwitch.on(), combineOn: () => combineSwitch.on(),
  hidingRead, readRunsOpened: (key) => readRuns.opened(key),
  readAloudNow: () => { const owner = readAloudOwner(); return { id: owner.nowPlaying ? owner.nowPlaying.id : null, pending: owner.pendingRead }; },
  unpackReadQueue, unpackWatch: (row, id) => unpackWatch.watch(row, id),
  threadHue, bridgePiece,
  packButton: (id) => packControls.packButton(id),
});
const { unpackBlock, unpackNeedsRead, queueUnpackRead, redrawUnpackBlock, unpackSignature } = unpackBlockOwner;
/**
 * The threads column's cards packed and unpacked (web/src/view/channel `createPackControls`,
 * `#624 voice-page-modularity`).
 *
 * @type {import("./src/view/channel/index.ts").PackControls}
 */
const packControls = createPackControls({
  leftEl, selectedChannelId, cardPacking, cardView, cardsCycle,
  keepingCardPlace, keepSelection, setStatus,
  threadHue, readAloudNow: () => ({ readingMode: readAloudOwner().readingMode }),
  readCardAloud: (row) => readAloudOwner().readCardAloud(row), toggleLitCard, sayRowShortcut,
  fullLocalAt, columnsSideMode: () => columns.side.mode, layoutThreadList: () => layoutShown.threadList(),
  cardsDrawnForget: () => cardsDrawn.forget(), sideColumnUp, keepingCardStill, renderSideColumn,
  markOf, sidePaneValue: sidePane, litCard, cardOpener,
  guard: (fn) => guardQuietly(fn), enterCard, paintLitCard,
  markReadingCard: (row) => readAloudOwner().markReadingCard(row),
  linkedMarkDrawOn: (row) => linkedMark.drawOn(row), searchable, threadName, keyContext,
});
const { renderUnpackToggle, setUnpackMode, setCardState, cycleCardState, packThread, packFromKeyboard, unpackFromKeyboard, cardRow, sideCard, countSideCard, cardState, packButton } = packControls;
/**
 * Remember the newest read requested while another channel read is in flight.
 *
 * `#203 incremental-refresh`. The one queued read stands for every trigger that asked meanwhile,
 * so it carries EVERY reason among them, and `forwardCursor` makes it a delta only if each could
 * be one: a stream reset's full read must not be narrowed to a delta because a live message
 * arrived behind it. Not a ranking: which of two reasons reads more depends on what the channel's
 * deltas carry, which is learned only when the queued read runs — on a channel whose deltas carry
 * additions only, a poll reads in full while a replay with no edit waiting reads a delta, and
 * before any delta has landed it can be the other way round. The newest request's other options
 * win, as they always have — except `quiet`, kept from either: a read behind rows a view switch
 * drew stays one without the loading line over them when a poll queues behind it (`#220
 * view-switch-instant`; a list with nothing in it shows the line whatever was asked).
 *
 * @param {{keepPosition?: boolean, quiet?: boolean, ownAct?: boolean, reason?: string | (string | undefined)[], readAs?: string}} [options]
 */






// --- a hidden page -------------------------------------------------------------------------------
//
// `#226 page-energy-profile`. The owner keeps this page open as an installed app, on a desk and on
// a phone, and asked that it not drain either. Measured with the window minimized, so that the page
// really was hidden: the poll kept its forty-five seconds, because a timer re-armed from a fetch's
// continuation is not one Chromium's background throttling slows; every live message was re-read
// and every row rebuilt, forced layouts and all, for a page nobody could see; and the stream's
// keep-alive, every fifteen seconds, is what keeps a phone's radio from ever going idle.
//
// So, while the page is hidden:
//
//   * THE POLL WAITS. One armed, or come due, resumes when the page is shown: a fresh wait, or now
//     when its time passed while hidden.
//   * A LIVE MESSAGE GOES INTO THE STORE AND NO FURTHER. No row, no read. An edit or a removal waits
//     for a read the same way. The agent of a call still hears it (`relayToAgent`): a call is what
//     the phone in a pocket is most likely to be doing.
//   * AFTER `LIVE_HIDDEN_CLOSE_MS` WITHOUT A CALL, THE STREAM IS LET GO, with the last event it saw.
//     During a call it stays: that is how the agent hears the channel.
//
// Coming back pays for all of it at once (`onVisibility`): what the store took in is drawn from the
// store as a read would have drawn it, the newest line followed or the reader's place kept and what
// arrived counted; then one read of what changed; then the stream, resumed from its last event, so
// what it replays is held already and nothing between is skipped. The pill says "Updated" rather
// than "Live · updated" from the stream's going until it is back, which is the only thing a reader
// can see of this, and only in the moment after a return.

// What arrived while hidden, the timer that lets the stream go after `LIVE_HIDDEN_CLOSE_MS` with no call
// listening, and the stream let go to resume: one owner in web/src/core/sync/hidden.ts (`#624
// voice-page-modularity`).
const hiddenStream = createHiddenStream({
  clock: clockOnUse(),
  follow: { channel: () => liveFollow.channel(), lastEventId: () => liveFollow.lastEventId(), generation: () => liveFollow.generation() },
  hidden: () => document.hidden,
  callListening: () => Boolean(voiceCall().session.socket),
  stop: () => stopChannelStream(),
});





// --- resuming an earlier conversation ------------------------------------------------------------
//
// `#46 conversation-replay`. The vendor documents no way to resume a conversation once the socket
// closes — the initiation message and the signed-URL endpoint both take an `agent_id` and neither
// accepts a `conversation_id`. But the EFFECT can be rebuilt from this end: the server holds the
// transcript, so a new call can be handed the earlier exchange as text.
//
// THIS IS A RECONSTRUCTION AND THE SCREEN MUST NEVER SAY OTHERWISE. That is the whole risk of the
// feature, and it is why there are six states here rather than an on/off light: resuming can be
// off, armed, PARTIAL, FAILED, or one of the two that send nothing — an earlier conversation that
// held nothing to replay, and one the budget could not fit a single line of. Each is a different
// sentence, and the last two matter most because they look identical from `included` alone and
// mean opposite things about what the reader said earlier.
//
// A failed replay never aborts the call. It degrades to a fresh one and says so — the point of a
// call is the call, and a lost reconstruction is not a reason to refuse to connect.

const RESUME_KEY = "vibe-talk.voice.resume";

/**
 * Resuming, as far as this page holds it: whether the server permits it, the conversation a new call
 * would resume from, what the last attempt actually did, the reader's switch kept under `RESUME_KEY`,
 * the clause under the large control and the fetch of a new call's payload. One owner in
 * web/src/core/call/resume.ts (`#624 voice-page-modularity`), which says what each outcome means.
 *
 * @type {import("./src/core/call/index.ts").CallResume}
 */
const callResume = createCallResume({
  key: RESUME_KEY,
  storage: () => deviceStorage(),
  api: (path) => api(path),
  addDetail: (text) => addDetail(text),
  render: () => renderResumeState(),
});
const {
  wanted: resumeWanted,
  persist: persistResume,
  armed: resumeArmed,
  note: resumeNote,
  fetchPayload: fetchResume,
} = callResume;

/**
 * What Settings says under Resuming: ONE SHORT LINE per state.
 *
 * `#332 settings-voice-agent-section`. "Keep each section's state line short; long explanations
 * belong in help." So each line names its state and the one number that matters — how many earlier
 * lines did not fit — and stops; why a state happens and what would change it (the two budget
 * settings, the operator's switch) is the Resuming entry in Help, one `?` away. The page suite holds
 * every line here to a budget, so the paragraphs cannot creep back one clause at a time. Shortened,
 * not merged: the six states are still six different sentences, for the reasons above.
 */
/** Settings reads the voice owner's recap state without taking ownership of its outcome. */
const resumePreference = mountResumePreference({
  lookup: (id) => el(id),
  read: {
    get allowed() { return callResume.allowed(); },
    get last() { return callResume.last(); },
    get conversationId() { return callResume.conversationId(); },
    get replayUnavailable() { return voiceTranscript().replayUnavailable; },
    wanted: () => resumeWanted(),
  },
  renderControls: () => voiceCall().renderControls(),
});

function renderResumeState() {
  resumePreference.render();
}

// --- the live channel stream -------------------------------------------------------------------
//
// `#44 live-push`. Until now this view only ever learned about a message by asking: a timer above
// re-read the channel every forty-five seconds and that was the whole of "live". The server can
// now TELL us, over `GET /api/v1/channels/{id}/stream`, and two decisions behind that are worth
// restating where the code that depends on them lives (the full argument is in src/live.rs):
//
//   * INGESTION MAY BE POLLING OR ADAPTER PUSH. The server reports which transport is configured;
//     an open browser stream proves only that this page can hear the server, not that an external
//     adapter is healthy. Settings keeps that distinction visible.
//   * THIS PAGE KEEPS THE CONVERSATION SOCKET. The server relays nothing to ElevenLabs, so an
//     arriving message reaches the agent only while this tab is open. That is a real limitation
//     and it is stated on the Settings screen instead of being discovered.
//
// NOT `EventSource`, which would be the obvious tool. It cannot carry an Authorization header, and
// the alternative — the token in the query string — puts a bearer credential in a URL, which is
// the exact thing `/api/v1/signed-url`'s `no-store` and this page's `redact()` exist to prevent. A
// URL is logged by every proxy it passes and lands in the browser's own history. So: `fetch`, a
// reader over the response body, and about thirty lines of SSE parsing.

// Where the reconnect delay lives (`LIVE_RETRY_MS`), so the suite can walk the page past it, and how
// long a read waits for the stream to attach (`LIVE_ATTACH_WAIT_MS`), are web/src/core/sync's.


// How often the server reads the channel, how it delivers live updates, and whether the selected provider can
// move its own read cursor are the provider facts' (`ProviderFacts`, `#624 voice-page-modularity`).
// The scope the server saw on the saved token (`read`, `write`, or null from an older server that does not
// say) is the server facts' (`ServerFacts.tokenScope`).
/**
 * The stream the page follows — which channel, its generation, the last event id it saw, whether it is
 * attached and whether that attach resumed, the reads waiting for it, and the loop that reads it and
 * retries: web/src/core/sync's (`createChannelFollow`, `#624 voice-page-modularity`). The request and
 * what each frame means are the page's.
 */
const liveFollow = createChannelFollow({
  clock: clockOnUse(),
  token: () => token(),
  delivery: () => providerFacts.liveDelivery(),
  // NOT `EventSource`: see the head of this section.
  open: (path, headers) => fetch(path, { headers }),
  decoder: () => new TextDecoder(),
  frame: (frame) => onStreamFrame(frame),
  attached: () => {
    // The server subscribed this stream before it answered, so its replay tail is everything a read
    // begun from here on will see.
    replayLedger.noteAttached();
    renderLiveState();
  },
  changed: () => renderLiveState(),
  dropped: (why) => addDetail(`live stream dropped: ${why}`),
  guard: (run) => guardQuietly(run)(),
});

const liveState = createLiveState({
  deviceStorage: () => deviceStorage(), el, setStatus, renderChannelFreshness,
  connectionLive: () => connectionReadouts.live(), liveFollow,
});
const { RELAY_MAX_CHARS, RELAY_PREAMBLE, RELAY_FENCE_OPEN, RELAY_FENCE_CLOSE, RELAY_MODES, RELAY_COALESCE_MS, RELAY_KEY, relayMode, renderReadNew, chooseReadNew, renderLiveState, stopChannelStream, whenLiveAttached, startChannelStream } = liveState;

// One SSE frame, as fields (`parseEventFrame`), is web/src/core/sync's, with the reading
// (`readEventStream`, `#624 voice-page-modularity`).


// Whether a re-read for live edits is owed or running, and the loop that waits out a read in flight and
// reads again for edits arriving during it: web/src/core/sync/coalesce.ts (`#624 voice-page-modularity`).
const mutationRefresh = createMutationRefresh({
  clock: clockOnUse(),
  busy: () => channelReads.busy(),
  read: async () => {
    const since = replayLedger.started();
    await timelineView.loadDiscord({ keepPosition: true, reason: "mutation" });


    // An edit may be in the thread on the right. `#229 desktop-two-column`.
    await readSideColumn("mutation", since);
  },
  launch: (run) => refreshQuietly(run)(),
});
/**
 * Keeping the channel on screen current (web/src/view/channel `createChannelRefresh`,
 * `#624 voice-page-modularity`): the read queue's release, the poll, the hidden page.
 * Reads asked for go through this script's own `loadDiscord` name above.
 *
 * @type {import("./src/view/channel/index.ts").ChannelRefresh}
 */
const channelRefresh = createChannelRefresh({
  channelReads: /** @type {any} */ (channelReads), channelPoll, hiddenStream: /** @type {any} */ (hiddenStream), columnWork, columns, inColumn,
  loadDiscord: (options) => timelineView.loadDiscord(options), loadTimeline,
  foldLateTimelinePage, renderChannelLoading, queuePlaceUnplaced,
  loadStateGeneration: () => loadState().generation, selectedChannelId,
  readModesHiding: () => readModes.hiding(), refreshQuietly,
  configApplied: () => serverFacts.configApplied(), signIn: () => signIn(),
  replayStarted: () => replayLedger.started(), replayWaiting: () => replayLedger.waiting(),
  channelContextKey, readSideColumn, screensView: () => screens.view(),
  discordPollMs: DISCORD_POLL_MS, startChannelStream,
  renderChannelNavigation, applySearch,
  el, channelStore, viewKey, channelReadPosition: timelineView.channelReadPosition,
  landedReadPosition: timelineView.landedReadPosition, settleAfterRead: timelineView.settleAfterRead,
  applyTimelinePage, foldTimelineDelta, holdingScrollTools, noteFreshRead, saveChannelScope,
  renderScrollTools, requestVisibleSummaries, followInOtherColumn, catchUpFromAll, catchUpHeldView,
  readFilterKeeps, hiddenEmpty, hidingRead, timelineRows, syncUnreadReplies, inOtherColumn,
  providerFacts, liveFollow, token,
});
const { queueDiscordLoad, finishDiscordLoad, currentDiscordLoad, stopDiscordPolling, scheduleDiscordPoll, pollDiscord, pauseWhileHidden, resumeLiveStream, resumePolling, catchUpHiddenArrivals, landAbandonedTimelineRead } = channelRefresh;
/**
 * The live stream's frames as the page takes them (web/src/view/channel `createLiveStream`,
 * `#624 voice-page-modularity`).
 *
 * @type {import("./src/view/channel/index.ts").LiveStream}
 */
const liveStream = createLiveStream({
  channelStore, selectedChannelId, providerFacts,
  loadDiscord: (options) => timelineView.loadDiscord(options), refreshQuietly, setStatus,
  noteLiveEvent: (channel, fromTail) => abandonedReads.noteLiveEvent(channel, fromTail),
  contractIs: (name, value) => VibeTalkContract.is(/** @type {any} */ (name), value),
  watchAllAttached: () => watchAll.attached(),
  voiceCanSendText: () => voiceCall().canSendText(), relayMode,
  reportUnrelayed: (messages, reason) => voiceCall().reportUnrelayed(/** @type {any} */ (messages), /** @type {any} */ (reason)),
  refreshServerSettings: async () => { await refreshServerSettings(); }, onSettingsFrame: (frame) => onSettingsFrame(frame), hiddenNow: () => document.hidden,
  noteHiddenMessage: () => hiddenStream.noteMessage(),
  relayToAgent: (message, selfPosted, replayed, arrival) => voiceCall().relayToAgent(message, selfPosted, replayed, arrival),
  refreshAfterMutation: () => refreshAfterLiveMutation(), noteChannelReadFailure,
  foldLiveMessage: timelineView.foldLiveMessage, appendChannelRow: timelineView.appendChannelRow,
  appendToColumns: timelineView.appendToColumns, renderOutgoingEverywhere: () => renderOutgoingEverywhere(), noteServerThreadHuesIn: (messages, summaries, channel, provisional) => noteServerThreadHuesIn(messages, summaries, channel, provisional),
  threadOf, loadCanon,
  replayLedger,
  replaysWaiting: () => replayLedger.waiting(), readLedgerEpoch: () => readLedger.epoch(),
  buildCopyOn, noteRemoved, liveFollow, noteOutgoingEcho: (id) => noteOutgoingEcho(id), settleFromServer: (id) => settleFromServer(id), noteSelfAuthor,
  loadState, renderChannelFreshness, channelReads, columns, queueDiscordLoad,
  noteStartsRead, hiddenStream: { noteMessage: () => hiddenStream.noteMessage(), noteMutation: () => hiddenStream.noteMutation() },
  followReplyStore, followPlacement, inSideThread, inColumn, el, idsOf, renderOutgoingMessages: () => renderOutgoingMessages(), ownPosts, liveEvents, entryLandingHolds, sideShowing, hiddenEmpty, listStateNewestId: (id) => { listState().newestId = id; }, scrollToNewest, setJumpNewest, renderScrollTools, requestVisibleSummaries, saveChannelScope, mutationRefresh,
  screens, followsNewest,
});
const { onStreamFrame, receiveLiveMessage, heldMessage, placesHeld, settleReplays, liveFollowers, refreshAfterLiveMutation } = liveStream;
/**
 * Loading what a previous page left unsent (web/src/core/outbox `createOutboxLoads`,
 * `#624 voice-page-modularity`). Entries are stored as `saved.ts` decoded them; this layer filters no fields.
 *
 * @type {import("./src/core/outbox/index.ts").OutboxLoads}
 */
const outboxLoads = createOutboxLoads({
  store: outbox, clock: clockOnUse(), blocker: (entry) => outboxDispatch.blocker(entry),
  credential: () => outboxDispatch.credential(), noteQueuedUnder: (id) => outboxDispatch.noteQueuedUnder(id), keepsOwnPosts: storeKeepsOwnPosts,
  selectedChannel: () => String(selectedChannelId()),
  storedIn: (channel) => (channelStore.current().channel === channel
    ? new Set(channelStore.current().messages.map((held) => String(held.id))) : null),
  schedule: (entry) => outboxDispatch.schedule(entry),
  retryPending: (id) => outboxDispatch.retryPending(id),
  testKey: (id) => outboxDispatch.testKey(id),
  observations: outgoingObservations,
  onChange: () => renderOutgoingEverywhere(),
  now: () => Date.now(),
});
const { persist: persistOutgoingMessages, load: loadOutgoingMessages, observe: observeOutgoingMessages, noteEcho: noteOutgoingEcho, settleFromServer, receiptRetires: sentReceiptRetires, armSentReceipt } = outboxLoads;
/**
 * Every unconfirmed send in the outbox, asked of the server once per load (`settleFromServer`): one owner
 * in web/src/core/outbox/unconfirmed.ts (`#624 voice-page-modularity`).
 *
 * @type {import("./src/core/outbox/index.ts").UnconfirmedSends}
 */
const unconfirmedSends = createUnconfirmedSends({
  ids: () => outbox.ids(),
  settle: (id) => settleFromServer(id),
});
/**
 * Sending what the outbox holds (web/src/core/outbox `createOutboxDispatch`,
 * `#624 voice-page-modularity`): queueing in the order written, placing, the POST, what became
 * of a lost answer, and every automatic retry. Placing a reply's target stays here
 * (`placeReplyTarget`), settled from the channel store and timeline reads.
 *
 * @type {import("./src/core/outbox/index.ts").OutboxDispatch}
 */
const outboxDispatch = createOutboxDispatch({
  store: outbox, observations: outgoingObservations, now: () => Date.now(), clock: clockOnUse(),
  cancellation: { create: () => browserPorts().cancellation.create() },
  random: () => Math.random(), api, token, identity: () => tokenFingerprint(token()),
  online: () => navigator.onLine !== false,
  channelWritable: (channel) => knownChannel(channel)?.writable !== false,
  originView: () => selection().view, persist: () => persistOutgoingMessages(),
  renderOutgoing: () => renderOutgoingEverywhere(), renderRows: () => eachColumn(renderChannelRows),
  placeReplyTarget: timelineView.placeReplyTarget,
  // Per channel, by its own provider where several are described (`#195 send-resilience`): one
  // owner in web/src/core/channels/providerfacts.ts (`channelPostsOnce`, `#624 voice-page-modularity`).
  postsOnce: (channel) => channelPostsOnce(channel, providerOfChannel, channelDirectory.providers().size, channelDirectory.capabilities()),
  serviceOf: (channel) => chatServiceOf(channel), readableFailure, readableCause, refusalWords, redact,
  threadOf, noteOwnSent: (ids) => ownPosts.noteSent(ids), armSentReceipt,
  selectedChannel: () => String(selectedChannelId()),
  inDiscord: () => screens.view() === "discord",
  followsNewest: (side) => side
    ? sideShowing() && inColumn(columns.side, () => followsNewest(el("scroll-area")))
    : followsNewest(el("scroll-area")),
  noteSelfAuthor, noteStartsRead, appendPart: (part) => appendToColumns(/** @type {any} */ (part)),
  scrollToNewest: (side) => { if (side) inColumn(columns.side, scrollToNewest); else scrollToNewest(); },
  markSentRead: (entry) => guardQuietly(markSentRead)(entry),
  reportColumnReply, columnReplies, quietly: (run) => guardQuietly(run)(),
});
const { queue: queueOutgoingMessage, retry: retryOutgoingMessage, schedule: scheduleOutgoingMessage, blocker: outgoingBlocker, credential: currentCredential, retriesItself, editable: editableSend, waitingItsTurn, resumeRetries: resumeOutgoingRetries, stop: stopOutgoingSends } = outboxDispatch;
// The browser's own word that the connection is back. Guarded because a page loaded without a
// full window — some harnesses build one by hand — has no `window.addEventListener`.
if (typeof window.addEventListener === "function") {
  window.addEventListener("online", () => outboxComposition.resumeOnOnline());
}
/**
 * Taking a send back (web/src/view/channel `createSendTakeBack`, `#624 voice-page-modularity`):
 * Edit, Dismiss and its Undo, putting a dismissed send back, and the dismissed pop-out's listeners.
 *
 * @type {import("./src/view/channel/index.ts").SendTakeBack}
 */
const sendTakeBack = createSendTakeBack({
  el, outbox, drop: (id) => outboxDispatch.drop(id),
  editable: (entry) => outboxDispatch.editable(/** @type {any} */ (entry)), waitingItsTurn: (entry) => outboxDispatch.waitingItsTurn(/** @type {any} */ (entry)),
  retryOf: (id) => outboxDispatch.retryOf(id),
  restoreHold: (entry, hold) => outboxDispatch.hold(/** @type {any} */ (entry), hold),
  identity: () => tokenFingerprint(token()), online: () => navigator.onLine !== false,
  fromSaved: (saved) => outgoingFromSaved(saved), unsentText, underDraft, snippet: (text) => sendSnippet(text),
  selectedChannelId, composerKey: () => composerState().key,
  activeColumn: () => columnWork.active(), mainColumn: columns.main, sideColumn: columns.side,
  sideShowing, inColumn,
  channelDrafts: { get: (key) => channelDrafts.get(key), set: (key, text) => channelDrafts.set(key, text), delete: (key) => channelDrafts.delete(key) },
  saveChannelDrafts: () => saveChannelDrafts(), noteDraft, openThread,
  fitBox: (box) => fitGrowingBox(box), focusAtEnd, setStatus,
  replyDrafts: { get: (id) => replyDrafts.get(id), set: (id, text) => replyDrafts.set(id, text), delete: (id) => replyDrafts.delete(id), save: () => replyDrafts.save(), setStartsThread: (id, starts) => replyDrafts.setStartsThread(id, starts) },
  renderDraftMarkers, idsOf, rowMessages, heldMessage, heldThreadRecord, openReply,
  dismissed: dismissedSends, dismissedView: dismissedSendsView, undo: sendUndo, nextUndoAt,
  persist: () => persistOutgoingMessages(), renderOutgoing: () => renderOutgoingEverywhere(),
  channelLabel, view: () => screens.view(),
});
const { edit: editOutgoingMessage, dismiss: dismissOutgoingMessage, endUndo: endSendUndo, renderUndo: renderSendUndo, putBack: putBackDismissedSend, forget: forgetDismissedSends, load: loadDismissedSends } = sendTakeBack;
/**
 * The outbox's own rows (web/src/view/channel `createOutboxRows`, `#624 voice-page-modularity`).
 *
 * @type {import("./src/view/channel/index.ts").OutboxRows}
 */
const outboxRows = createOutboxRows({
  el, leftEl, selectedChannelId,
  outgoingThreadOf: (entry) => outgoingThreadOf(entry),
  entries: () => /** @type {any} */ (outbox.values()),
  removeEntry: (id) => outbox.remove(id),
  forgetObservations: (id) => { outgoingObservations.delete(id); },
  receiptRetires: /** @type {any} */ (sentReceiptRetires), blockerOf: (entry) => /** @type {any} */ (outgoingBlocker(/** @type {any} */ (entry))), inPlaceDrawn: outgoingInPlaceDrawn,
  retryOf: (id) => outboxDispatch.retryOf(id), blockerRemoved: (entry) => entry.channel === String(selectedChannelId()) && Boolean(entry.replyTo && removedUpstream(entry.replyTo)),
  editableOf: (entry) => editableSend(/** @type {any} */ (entry)), canRetry: (entry) => retriesItself(/** @type {any} */ (entry)),
  snippetOf: (entry) => sendSnippet(unsentText(entry)),
  sideCardsShown, rowMessages,
  channelStore: { current: () => channelStore.current() }, threadHue: (id) => /** @type {any} */ (threadHue(id)), openThread,
  renderBodyInto, textBody, storageOkay: () => outbox.storageOkay(),
  knownChannel, retryOutgoingMessage, editOutgoingMessage, dismissOutgoingMessage,
  threadCards: (summaries) => threadCards(summaries), pendingThreadTimes, sentFromThisPage, ownSentAt: (id) => ownPosts.sentAt(id), retryKnown: (id) => outboxDispatch.retryPending(id),
  renderSideCards: () => renderSideCards(), renderChannelRows,
  pinsOnly: () => pinsView().only,
  searchable,
  listState: () => listState(), renderReplyCounts, columnWork,
  roomWatch: outgoingRoomWatch, roomAnchors: outgoingRoomAnchors, selection,
  serviceOf: (entry) => (typeof chatServiceOf === "function" && chatServiceOf(entry.channel)) || "upstream",
  nowMs: () => Date.now(), dismissedDays: DISMISSED_SENDS_DAYS,
  unsentText: (entry) => unsentText(entry),
  anchorUnder, columns, eachColumn, edit: (id) => editOutgoingMessage(id), guardQuietly,
  inThisColumn, now: () => Date.now(), pendingThreads, persist: () => persistOutgoingMessages(),
  replyState: /** @type {any} */ (replyState), retry: (id) => retryOutgoingMessage(id), shownNode, sideColumnUp, timelineRows,
});
const { outgoingInView, renderOutgoingMessages, renderOutgoingEverywhere, withPendingThreads, outgoingReplyKey, fitOutgoingRoom } = outboxRows;
/**
 * Sending from the channel's message box (web/src/view/composer `createChannelSend`,
 * `#624 voice-page-modularity`).
 *
 * @type {import("./src/view/composer/index.ts").ChannelSend}
 */
const channelSend = createChannelSend({
  composerState, composerKeys: /** @type {any} */ (composerKeys), el: /** @type {any} */ (el), selectedChannelId, selectionThreadId: () => selection().threadId,
  setHiddenIfChanged: (node, hidden) => setHiddenIfChanged(node, hidden),
  knownWritable: (channel) => knownChannel(channel)?.writable !== false,
  rootThisStarts, queueOutgoingMessage,
  channelDrafts: { get: (key) => channelDrafts.get(key), set: (key, text) => channelDrafts.set(key, text), delete: (key) => channelDrafts.delete(key), save: () => channelDrafts.save() },
  storageOkay: () => outbox.storageOkay(), noteDraft, fitBox: (box) => fitGrowingBox(box),
  draftRefusedWords: CHANNEL_DRAFT_REFUSED, scrollToNewest, renderChannelNavigation, renderDraftChip,
});
const { sendChannelMessage, cancelChannelMessage, answerChannelDiscard, rememberChannelDraft, restoreChannelComposer, saveChannelDrafts, channelComposerKey, onComposerKey } = channelSend;
/**
 * Which drafts are held, and whether one is live (web/src/core/outbox `createDraftQueries`,
 * `#624 voice-page-modularity`). Writing the ledger stays with each writer.
 *
 * @type {import("./src/core/outbox/index.ts").DraftQueries}
 */
const draftQueries = createDraftQueries({
  ledger: draftLedger,
  composerDraft: (key) => channelDrafts.get(key), composerDraftKeys: () => channelDrafts.keys(),
  replyDraft: (key) => replyDrafts.get(key), replyDraftIds: () => replyDrafts.ids(),
  quickDraft: (key) => quickDrafts.get(key), quickDraftKeys: () => quickDrafts.keys(),
  quickDraftMessage: (key) => quoteOwner.quickDraftMessage(key),
  storeNow: () => ({ channel: channelStore.current().channel, removed: channelStore.current().removed, messages: channelStore.current().messages }),
  selectedChannelId,
});
const { draftHeld, draftLive, draftsHere } = draftQueries;











/**
 * One read merges a replayed event the store cannot answer for, and the rest of its burst.
 *
 * The server had an event from the replay tail before the stream attached, so any read begun after
 * the attach has seen it, even a reply in a thread the view does not show. The adapter's catch-up
 * is replayed too but may arrive after every read the page has made, so it is only known to be
 * seen by a read begun after it arrived. `arrivedAfter` is the read count at that point. When such
 * a read has landed there is nothing to do. Otherwise the event waits under `key` for the next read
 * to land, and `settleReplays` decides then; with none on the wire and the channel on screen, it
 * starts one. The voice pane starts none: entering the channel reads it, and that read answers for
 * everything waiting. A LIVE frame is not asked about: it is newer than any read on the wire, so it
 * always re-reads, as `refreshAfterLiveMutation` explains.
 *
 * @param {string} key the id of a new message, which a landed page holding it answers for; an
 *   edit or removal is keyed by its event and id, which no page's message id can match, because
 *   a page's copy says nothing about which edit it has seen
 * @param {string} channel
 * @param {number} arrivedAfter
 */


// `canSendText` and `sendClientEvent` used to be defined a second time HERE, shadowing the pair
// near `startCapture`. They are declared once now, where the long note about the send path already
// lived; the relay below calls exactly the same function it was already getting.

/**
 * The name the channel picker shows for a snowflake, or the snowflake itself for one no longer on
 * the list (an outbox entry may outlive its channel). From the list, not the picker's children:
 * those are service groups once there are several. `#355 provider-grouped-channels`.
 */
function channelLabel(channelId) {
  const channel = knownChannel(channelId);
  return channel ? channelName(channel) : String(channelId);
}

// --- the stream of every channel, while a call is open ------------------------------------------
//
// `#337 voice-relay-silent`. The owner was in a long call with Live messages set to read every new
// message out in full, a message arrived in a channel he watches — his phone's own notification
// showed it — and the call heard nothing. The page followed ONE stream, the channel on screen's
// (`startChannelStream`), so a message anywhere else could never reach the agent. A call now also
// follows `GET /api/v1/live/stream`, which carries every channel on the list, and relays from it as
// from the channel's own: the ledger keeps a message both streams bring to one turn.
//
// Only during a call, because only a call hears it (`#226 page-energy-profile` keeps the page from
// holding open what nobody uses). It resumes from its cursor after a drop, so what arrived meanwhile
// is relayed, not lost; a reset, which says the gap cannot be filled, is said in the transcript. A
// server that predates it answers 404, and the connection details say that only the channel on
// screen is read out.

/**
 * The stream of every channel: its generation, the cursor of the last event it delivered, whether its
 * response body is open and whether that attach resumed from a cursor, whether the server can stream
 * every channel at all, the handle that lets the attach now open go, and the loop that reads it and
 * retries, as the channel stream's are (`liveFollow`): one owner in web/src/core/sync/watchall.ts
 * (`#624 voice-page-modularity`). The request and what each frame means are the page's.
 *
 * @type {import("./src/core/sync/index.ts").WatchAll}
 */
const watchAll = createWatchAll({
  clock: clockOnUse(),
  token: () => token(),
  delivery: () => providerFacts.liveDelivery(),
  // Where the browser can, a call that has ended lets its connection go at once rather than at the
  // stream's next keep-alive, so a phone on HTTP/1.1 does not spend one of its few connections to this
  // server on a stream nobody reads.
  abortable: () => (typeof AbortController === "function" ? new AbortController() : null),
  open: (path, headers, abort) => fetch(path, abort ? { headers, signal: abort.signal } : { headers }),
  decoder: () => new TextDecoder(),
  frame: (frame) => onWatchAllFrame(frame),
  dropped: (why) => addDetail(`the stream of every channel dropped: ${why}`),
  unsupported: () => addDetail("this server cannot stream every channel at once, so a call reads out new messages only " +
    "from the channel on screen"),
  guard: (run) => guardQuietly(run)(),
});

/** Follow every watched channel for the call that has just connected. */
function startWatchAll() {
  watchAll.start();
}

/** Stop following every channel. Idempotent. */
function stopWatchAll() {
  watchAll.stop();
}

/**
 * One frame of the stream of every channel, its cursor (the cursor over every channel) already taken
 * down by `watchAll`, and dropped there on a reset. A message is relayed as one from the channel's own
 * stream is, under the same guards and the same ledger.
 *
 * @returns {"reset" | undefined} `reset` when the server said it cannot fill the gap
 */
function onWatchAllFrame(frame) {
  if (frame.event === "reset") {
    refreshServerSettings().catch(() => {});
    if (voiceCall().canSendText() && relayMode() !== "off") voiceCall().reportUnrelayed([], "gap");
    return "reset";
  }
  if (frame.event === "settings") {
    onSettingsFrame(frame);
    return undefined;
  }
  if (frame.event !== "message" || !frame.data) return undefined;
  let payload = null;
  try {
    payload = JSON.parse(frame.data);
  } catch (_error) {
    return undefined;
  }
  if (!VibeTalkContract.is("LiveMessageEvent", payload)) return undefined;
  if (payload.self_posted) noteSelfAuthor(payload.message.author_id);
  if (payload.self_posted && typeof payload.client_key === "string") settleFromServer(payload.client_key);
  voiceCall().relayToAgent(payload.message, payload.self_posted, payload.replayed, {
    fromTail: payload.from_tail === true,
    resumed: watchAll.resumed(),
  });
  return undefined;
}

// --- what to call a channel ----------------------------------------------------------------------
//
// `#39 channel-alias`. A name of the OWNER's own for a channel, because saying which channel he
// means has to be possible out loud: `1532416065114607829` is unsayable, and a configured label
// like "build noise" is not what anyone says either.
//
// THREE things this deliberately is not, each of them said on the screen as well as here:
//
//   * It is NOT a rename in Discord. The server keeps the name and never tells Discord; the
//     channel keeps whatever it is called there. The server states that on every answer and this
//     page shows THAT sentence rather than writing its own — same rule `#50 todo-view` follows
//     with `read_state_notice`, and for the same reason: a second copy of a policy is a second
//     thing that can go stale.
//   * It is NOT visible to anyone outside this deployment.
//   * It is NOT the agent's to choose. The agent is HANDED it — `list_channels` and the digest
//     header carry it — and the server offers no tool that sets one. This editor is the only way
//     in, and it is behind the write token, which is the operator's.
//
// ONE RULE, IN ONE PLACE: the alias when there is one, the configured label otherwise. Every
// render of a channel's name on this page goes through `channelName`, so the picker, the head of
// the channel and the head of the to-do list cannot come to disagree about what it is called.

/**
 * The name to show for a channel, from anything the server calls a channel.
 *
 * Trimmed and emptiness-checked rather than a bare `alias || label`: the server refuses a blank
 * one, so an empty string arriving here means something upstream is wrong, and a picker with a
 * blank entry in it is worse than one showing the configured label.
 */
function channelName(channel) {
  if (!channel) {
    return "this channel";
  }
  const alias = typeof channel.alias === "string" ? channel.alias.trim() : "";
  return alias || String(channel.label || channel.id || "this channel");
}

/**
 * What the server last said the channels and their services were — the channels on the list, the
 * configured ones taken off it, each provider's description, the deployment-wide capabilities, whether
 * the bridge registers and lists channels, and the chat service's own name: web/src/core/channels's
 * (`createChannelDirectory`, `#624 voice-page-modularity`), with why each is kept.
 */
const channelDirectory = createChannelDirectory();

const NO_CHANNEL_TO_READ =
  "no channel to read — your channel list is empty. " +
  "Add one, or show a hidden one again, in Settings.";

/** The chat service as a noun phrase: its configured name, or a generic one. */
function chatServiceName() {
  return channelDirectory.providerName() || "the chat service";
}

/** Remember every provider's description, and the accounts each one speaks as. */
function learnProviders(providers) {
  whoIsWho.learnProviderAccounts(channelDirectory.learnProviders(providers));
}

/** The provider description for the channel `id`, or null when the server did not say. */
function providerOfChannel(id) {
  return channelDirectory.providerOf(id);
}

// --- channels grouped by service -----------------------------------------------------------------
//
// `#355 provider-grouped-channels`. One server may read several chat services, and a flat list of
// their channels hides which is which: two called "general" are one name. So a channel picker
// groups its channels under a header per service, in the order the server configures them, once
// two or more services have channels. With one, or with a server that describes no services, the
// list is exactly the flat one it always was. The closed picker shows the channel's name alone;
// the open list carries the headers, so the bar keeps its width on a phone.

/** The header a group shows: the service's name, with its key where two services share it. */
function serviceLabel(key) {
  const described = channelDirectory.providers().get(key);
  const name = described && typeof described.name === "string" && described.name.trim()
    ? described.name.trim()
    : String(key);
  const shared = [...channelDirectory.providers().values()].filter(
    (other) => other !== described && typeof other.name === "string" && other.name.trim() === name
  ).length > 0;
  return shared ? `${name} (${key})` : name;
}

/** The header of the group of channels whose service this server no longer describes. */
const UNDESCRIBED_SERVICE = "Not connected";

/**
 * `channels` by service: one group per described service that has channels, in the server's
 * order, then the channels of no described service (an added channel whose bridge is no longer
 * configured), never dropped.
 *
 * @returns {{provider: string | null, heading: string, channels: any[]}[]}
 */
function channelGroups(channels = channelDirectory.channels()) {
  const groups = [...channelDirectory.providers().keys()].map((key) => ({
    provider: /** @type {string | null} */ (key),
    heading: serviceLabel(key),
    channels: channels.filter((channel) => channel.provider === key),
  }));
  const rest = channels.filter(
    (channel) => typeof channel.provider !== "string" || !channelDirectory.providers().has(channel.provider)
  );
  groups.push({ provider: null, heading: UNDESCRIBED_SERVICE, channels: rest });
  return groups.filter((group) => group.channels.length > 0);
}

/** Whether the pickers show service groups: two or more services have channels. */
function channelsGrouped() {
  return channelGroups().length >= 2;
}

/**
 * The channels in the order the bar's picker shows them: grouped by service when it is, else the
 * server's own order. Alt+↑ and Alt+↓ step through this order, and a picker with nothing to keep
 * opens on its first.
 */
function channelsInPickerOrder() {
  return channelsGrouped() ? channelGroups().flatMap((group) => group.channels) : channelDirectory.channels();
}

/** Settings' channel box owns its service choice, editor and hidden list (#624 voice-page-modularity). */
const channelSettings = mountChannelSettings({
  lookup: (id) => el(id),
  directory: channelDirectory,
  allChannels: ALL_CHANNELS,
  undescribedService: UNDESCRIBED_SERVICE,
  tokenScope: () => serverFacts.tokenScope(),
  selectedChannelId: () => selectedChannelId(),
  serviceLabel: (key) => serviceLabel(key),
  chatServiceName: () => chatServiceName(),
  channelName: (channel) => channelName(channel),
  pickerName: (name) => pickerName(name),
  browser: () => channelDirectoryBox,
  upstream: () => upstreamClearBox,
  history: () => channelHistoryBox,
  identities: () => identityList,
  folds: () => settingsFolds,
  request: (path, options) => api(path, options),
  guard: (run) => guardQuietly(run),
  adoptChannelList: (payload) => adoptChannelList(payload),
  changeSelectedChannel: () => changeSelectedChannel(),
  forgetChannelScopes: (matches) => messageCache.forgetChannelScopes(matches),
  saveCacheChannels: () => messageCache.saveCacheChannels(),
  fillReadingSelect: () => fillChannelSelect("discord-channel"),
  restateChannelSeam: () => restateChannelSeam(),
  setStatus: (text) => setStatus(text),
});

/**
 * Act on the selected channel's own provider: its threads, its read cursor, its live delivery.
 *
 * Called before anything reads the new channel, because whether to read a timeline or a flat
 * history is exactly the kind of decision that must not be made with another service's answer.
 */
function applyChannelProvider() {
  const provider = providerOfChannel(selectedChannelId());
  providerFacts.fromProvider(provider, channelDirectory.capabilities());
  renderLiveState();
}

// `sentenceStart` is web/src/foundation's (`#624 voice-page-modularity`).

/**
 * Settings' channel browser (`#19 channel-browser`, `#371 channel-search-hash`): its search, its pages,
 * the count of a slow read, and one tap to add. web/src/view/settings `mountChannelDirectory`, in the
 * words of web/src/present/settings (`#624 voice-page-modularity`); see "browsing channels".
 *
 * @type {import("./src/view/settings/index.ts").ChannelDirectory}
 */
const channelDirectoryBox = mountChannelDirectory({
  lookup: (id) => el(id),
  request: (path, options) => api(path, options),
  known: (id) => knownChannel(id) !== null,
  showPanel: (which) => channelSettings.showChannelPanel(which),
  serviceName: () => channelSettings.settingsServiceName(),
  provider: () => (channelSettings.settingsServiceDescription() ? channelSettings.service() : null),
  guard: (run) => guardQuietly(run),
  adoptChannelList: (payload) => adoptChannelList(payload),
  pointSettingsAt: (id) => channelSettings.pointSettingsAt(id),
  renderChannelBox: () => channelSettings.renderChannelBox(),
  channelName: (channel) => channelName(channel),
  now: () => Date.now(),
  setTimeout: (run, ms) => setTimeout(run, ms),
  clearTimeout: (timer) => clearTimeout(timer),
});

/**
 * A channel's name in a picker: its name without a leading '#'. `#371 channel-search-hash`.
 *
 * A native <select> finds an option by the first letters of its text as they are typed, so a Slack
 * channel listed as `#team-builds` was never reached by typing `team`. The '#' says only that it is
 * a Slack channel, which the picker's service header (or Settings' Service row) says already.
 * Everywhere else a channel keeps its name as written.
 */
function pickerName(name) {
  return name.replace(/^#+\s*/, "") || name;
}

function knownChannel(id) {
  return channelDirectory.known(id);
}

/**
 * The channel to open on: the one this window was on, which a reload of it comes back to, and for a
 * brand-new window, which has none, the one chosen last in any window. `#265 window-channel-memory`.
 * One the server no longer lists gives way to its first, in `fillChannelSelect`, as it always has.
 */
function storedActiveChannel() {
  const own = windowItem(ACTIVE_CHANNEL_KEY);
  if (own) return own;
  try {
    return deviceStorage().getItem(ACTIVE_CHANNEL_KEY) || "";
  } catch (_error) {
    return "";
  }
}

/**
 * Keep the channel on the bar as this window's. `chosen`, when the reader changed to it: then it is
 * also the device's, the one a brand-new window opens on. A channel only put on the bar — the one a
 * new window opened on, a list redrawn — is this window's alone, so another window's choice is not
 * taken back by a window that merely reloaded. `#265 window-channel-memory`.
 */
function rememberActiveChannel(chosen = false) {
  const id = selectedChannelId();
  keepWindowItem(ACTIVE_CHANNEL_KEY, id || null);
  if (!chosen) return;
  try {
    if (id) deviceStorage().setItem(ACTIVE_CHANNEL_KEY, id);
    else deviceStorage().removeItem(ACTIVE_CHANNEL_KEY);
  } catch (_error) {
    // Switching spaces still works when this browser cannot retain the preference.
  }
}

/**
 * Redraw one channel picker from the directory's channels (`channelDirectory`), keeping it on the
 * channel it was showing.
 *
 * A rename is a redraw, NOT a change of channel. Rebuilding the options moves a `select` to its
 * first entry unless the value is put back, and doing that to `#discord-channel` would silently
 * move the Discord view to a different channel because a name changed somewhere else.
 */
function fillChannelSelect(id) {
  if (id === "settings-channel") return channelSettings.fillSettingsSelect();
  const select = el(id);
  // Restore only an ID still present in the server's current channel list. Settings has its own
  // picker and must not change the remembered reading destination merely by editing a channel.
  // The bar's picker shows the selection (`navigation`), so its channel is the selection's.
  const chosen = selectedChannelId() || storedActiveChannel();
  select.replaceChildren();
  const optionFor = (channel) => {
    const option = document.createElement("option");
    option.value = channel.id;
    option.textContent = pickerName(channelName(channel));
    return option;
  };
  // `#355 provider-grouped-channels`. The bar's picker groups its channels by service once two or
  // more have channels; each group is an <optgroup>, so the open list has a header per service.
  const grouped = channelsGrouped();
  if (grouped) {
    for (const group of channelGroups()) {
      const header = document.createElement("optgroup");
      // The attribute, not the property of that name: reading or writing a `label` property is
      // what the server's `#39 channel-alias` guard counts, as a channel named past its alias.
      header.setAttribute("label", group.heading);
      header.append(...group.channels.map(optionFor));
      select.append(header);
    }
  } else {
    for (const channel of channelDirectory.channels()) select.append(optionFor(channel));
  }
  const order = grouped ? channelsInPickerOrder() : channelDirectory.channels();
  const showing = order.some((channel) => String(channel.id) === String(chosen))
      ? chosen
      : order.length > 0
        ? order[0].id
        : "";
  // The bar's picker is redrawn onto the selection, which this keeps on its channel or moves to the
  // first: through its owner, whose picker then shows it.
  selectionOwner().selectChannel(String(showing));
  rememberActiveChannel();
}

/** Take the server's whole answer about the list: both pickers, the hidden list, the cache. */
function adoptChannelList(payload) {
  if (payload && Array.isArray(payload.channels)) channelDirectory.setChannels(payload.channels);
  if (payload && Array.isArray(payload.hidden_channels)) channelDirectory.setHidden(payload.hidden_channels);
  messageCache.saveCacheChannels();
  channelSettings.fillServiceSelect();
  fillChannelSelect("discord-channel");
  fillChannelSelect("settings-channel");
  channelSettings.renderHiddenChannels();
}

// --- browsing channels ---------------------------------------------------------------------
//
// `#19 channel-browser`. Pasting a link works, but only for someone who already has the link. The
// bridge can list the channels its account sees, so the page offers that list, searchable, with
// one tap to add. Adding goes through exactly the same `POST /api/v1/channels` as the form above,
// so a browsed channel is registered, probed and stored the same way a pasted one is.

// The browser itself (opening, searching, pages, adding) is web/src/view/settings `mountChannelDirectory`
// (`channelDirectoryBox`, `#624 voice-page-modularity`).

/**
 * Clear unread in the channel box, for the picker's channel or All channels: drawn, read again as
 * Settings opens, its Save, and "Mark read now".
 *
 * @type {import("./src/view/settings/index.ts").UpstreamClearBox}
 */
const upstreamClearBox = mountUpstreamClear({
  lookup: (id) => el(id),
  decoded: (type, path, options) => apiDecoded(type, path, options),
  offered: (id) => channelSettings.upstreamClearOffered(id),
  allChannels: () => channelSettings.upstreamClearAllChannels(),
  allChannelsChosen: () => channelSettings.allChannelsChosen(),
  serviceOf: (id) => chatServiceOf(id),
  channelCount: () => channelSettings.settingsChannels().length,
  nameOf: (id) => channelName(knownChannel(id)),
  setTimeout: (run, ms) => setTimeout(run, ms),
  clearTimeout: (timer) => clearTimeout(timer),
});

// --- how much of the channel's history the server keeps --------------------------------------------
//
// `#602 history-settings-and-limits`. The channel box says how much of the channel the server's message
// cache holds, how its walk into older messages goes, and offers the channel's own mode and "Read older
// messages next": web/src/view/settings `mountChannelHistory`, in the words of web/src/present/settings
// (`#624 voice-page-modularity`). The choice is the server's; nothing is kept on this device.

/**
 * The history part of the channel box: drawn for the picker's channel, read again while Settings
 * shows it, and its Save and "Read older messages next".
 *
 * @type {import("./src/view/settings/index.ts").ChannelHistoryBox}
 */
const channelHistoryBox = mountChannelHistory({
  lookup: (id) => el(id),
  decoded: (type, path, options) => apiDecoded(type, path, options),
  allChannelsChosen: () => channelSettings.allChannelsChosen(),
  knownChannel: (id) => knownChannel(id),
  cacheOpen: () => serverCacheBox.open(),
  writable: () => serverFacts.tokenScope() === "write",
  setTimeout: (run, ms) => setTimeout(run, ms),
  clearTimeout: (timer) => clearTimeout(timer),
});

/**
 * Rewrite the line at the head of the channel, which is already on screen saying the old name.
 *
 * From what is on the screen rather than by re-reading: a rename is not a reason to spend a
 * request, and a seam left standing with the previous name is the most confident possible way of
 * being wrong. It does nothing before the first read, when there is no seam to correct.
 */
function restateChannelSeam() {
  if (el("channel-summary").children.length === 0) {
    return;
  }
  if (readModes.hiding()) {
    renderChannelSeam(todoSummary());
    return;
  }
  renderChannelSeam(
    channelSummary(
      el("discord-log").children.length,
      olderPager.loadedIsWhole(olderPages().moreAbove),
      channelName(knownChannel(selectedChannelId()))
    )
  );
}

// --- sign-in ---------------------------------------------------------------------------------



// --- the way out to the agent's own configuration -----------------------------------------------
//
// Everything about HOW THE AGENT BEHAVES is owned by the selected provider. The provider trait
// supplies the human name and, where applicable, a direct settings URL and instance id. The page
// neither recognizes vendor names nor constructs vendor URLs.

/**
 * The conversational voice as the selected provider describes it in client-config: one owner in
 * web/src/core/call/provider.ts (`#624 voice-page-modularity`).
 *
 * @type {import("./src/core/call/index.ts").VoiceProvider}
 */
const voiceProvider = createVoiceProvider();

/**
 * What the server's client-config answer puts into force: one owner in
 * web/src/view/settings/clientconfig.ts (`createClientConfig`, `#624 voice-page-modularity`).
 * The page destructures its act under its old name.
 */
const { apply: applyClientConfig } = createClientConfig({
  el: (id) => el(id),
  setTitle: (title) => { document.title = title; },
  readAloud: () => readAloudOwner(),
  providerFacts, channelDirectory, serverFacts, whoIsWho,
  learnProviders: (providers) => learnProviders(providers),
  noteSelfAuthor: (id) => noteSelfAuthor(id),
  voiceProvider,
  voiceCall: () => voiceCall(),
  settingsReadouts,
  channelSettings,
  fillChannelSelect: (id) => fillChannelSelect(id),
  restoreChannelComposer: () => restoreChannelComposer(),
  summaryRows,
  setSummaryMode: (on) => setSummaryMode(on),
  storedSummarizeCollapsed: () => storedSummarizeCollapsed(),
  renderScrollTools: () => renderScrollTools(),
  applyChannelProvider: () => applyChannelProvider(),
  upstreamClearBox,
  callResume,
  voiceTranscript: () => voiceTranscript(),
  speechHoldFrom: (hold) => speechHoldFrom(hold),
  renderResumeState: () => renderResumeState(),
  noiseSettings, agentSettings, agentPrefix, serverCacheBox,
  seedServerThreadHues: (id) => seedServerThreadHues(id),
  selectedChannelId: () => selectedChannelId(),
  startChannelStream: (id) => startChannelStream(id),
  settingsSync: () => settingsSync(),
  messageCache,
  uiState,
  saveUiState: () => saveUiState(),
  keyHints,
});




/**
 * Prove the saved token actually works, then show the main interface.
 *
 * A refusal (401/403) is the one failure that means "sign in again", and only that one sends the
 * owner back to the sign-in screen. Anything else — server down, network gone — leaves the token
 * alone and lands on the main screen with the error visible, because bouncing to a sign-in form
 * would blame the wrong thing.
 */


/**
 * Sign-in — the token proved, saved and forgotten: one owner in web/src/view/settings/signin.ts
 * (`createSignIn`, `#624 voice-page-modularity`). The page destructures its acts under their old
 * names; the Save button's own words and timer stay with its owner (`saveTokenButton`, below).
 */
const { signIn, saveToken, forgetToken, setTokenState, noTokenSentence } = createSignIn({
  el: (id) => el(id),
  field: (id) => /** @type {{value: string}} */ (/** @type {unknown} */ (el(id))),
  token: () => token(),
  signedOutByRename: () => signedOutByRename(),
  config: () => apiDecoded("ClientConfigResponse", "/api/v1/client-config"),
  applyClientConfig: (config) => applyClientConfig(config),
  forgetMessages: () => forgetMessages(),
  showError: (text) => showError(text),
  setStatus: (text) => setStatus(text),
  refusalSentence: (error) => refusalSentence(error),
  showScreen: (name) => showScreen(name),
  loadFreshAt: () => loadState().freshAt,
  setChannelFreshness: (state) => setChannelFreshness(state),
  payReopenRead: () => payReopenRead(),
  channelContextKey: () => channelContextKey(),
  threading: () => providerFacts.threading(),
  reconcileChannelSnapshot: (drawn) => reconcileChannelSnapshot(drawn),
  clearError: () => clearError(),
  reopenSavedView: () => reopenSavedView(),
  setState: (state) => setState(state),
  voiceTranscript: () => voiceTranscript(),
  startSettingsSync: () => startSettingsSync(),
  settleUnconfirmedSends: () => unconfirmedSends.settleOnce(),
  credentialStore: () => credentialStore(),
  stopOutgoingSends: () => stopOutgoingSends(),
  markSave: (label, state, busy) => markSave(label, state, busy),
  revertSaveLater: (run, ms) => saveTokenButton.revertLater(run, ms),
  settingsSync: () => settingsSync(),
  deepLinks: { signOut: () => deepLinks.signOut(), noteWritten: (address) => deepLinks.noteWritten(address) },
  clearLinkedMark: () => linkedMark.clear(),
  forgetLinkJumps: () => linkJumps.forget(),
  backHistory: () => backHistory(),
  location: () => (typeof window === "object" ? window.location : null),
  stopChannelStream: () => stopChannelStream(),
});

// A banner that says "saved" while the button looks untouched leaves two questions open at once:
// did the click register, and did the save work? So the button itself changes — immediately on
// click, and again on the result — and the line under it states what is stored right now.
/**
 * The Save button's words, look and state, and the timer putting it back to SAVE_LABEL after a save: one
 * owner in web/src/view/settings/savetoken.ts (`#624 voice-page-modularity`).
 *
 * @type {import("./src/view/settings/index.ts").SaveTokenButton}
 */
const saveTokenButton = mountSaveTokenButton({
  lookup: (id) => el(id),
  setTimeout: (run, ms) => setTimeout(run, ms),
  clearTimeout: (timer) => clearTimeout(timer),
});
const { markSave } = saveTokenButton;



// Pairing this device from a link (`#369 pairing-link`) is `devicePairing`'s, near the top of this file.



// --- settings kept on the server (`#457 settings-server-side`) ----------------------------------
//
// Every setting outside Settings' Screen and keys section lives on the server, shared by all the
// owner's devices; that one section — text size, reading width, the control bar, typing and the
// keyboard — stays on this device and says "(client side)" beside its title. The full inventory
// and its reasons are in `src/client_settings.rs`; web/src/core/settings/keys.ts names the synced
// keys, and web/src/core/settings/sync.ts says how the device's replica and the server's document
// are kept in agreement.

/**
 * The settings sync's laziness and small acts: one owner in web/src/view/settings/syncglue.ts
 * (`createSettingsSyncGlue`, `#624 voice-page-modularity`). The reconciler and its view are made
 * on first use, as they were; the page destructures their acts under their old names.
 */
const settingsSyncGlue = createSettingsSyncGlue({
  createSync: (apply, status) => createSettingsSync({
    storage: deviceStorage(),
    clock: browserPorts().clock,
    decoded: apiDecoded,
    token,
    tokenScope: () => serverFacts.tokenScope(),
    apply,
    status,
    landed: summariesSettingLanded,
  }),
  mountView: (sync) => mountSettingsSyncView({
    lookup: (id) => el(id),
    sync,
    guard: (run) => guardQuietly(run),
    keys: {
      get READ_MODE_KEY() { return READ_MODE_KEY; },
      get MARK_OWN_KEY() { return MARK_OWN_KEY; },
      get HIDE_EMPTY_KEY() { return HIDE_EMPTY_KEY; },
      get COMBINE_KEY() { return COMBINE_KEY; },
      get RICH_HEADERS_KEY() { return RICH_HEADERS_KEY; },
      get SUMMARIZE_COLLAPSED_KEY() { return SUMMARIZE_COLLAPSED_KEY; },
      get IN_APP_LINKS_KEY() { return IN_APP_LINKS_KEY; },
      get OPEN_IN_VIEW_KEY() { return OPEN_IN_VIEW_KEY; },
      get COLLAPSED_LINES_KEY() { return COLLAPSED_LINES_KEY; },
      get CARD_MAX_LINES_KEY() { return CARD_MAX_LINES_KEY; },
      get READ_SPEED_KEY() { return READ_SPEED_KEY; },
      get READ_AUDIO_SOURCE_KEY() { return readAloudOwner().READ_AUDIO_SOURCE_KEY; },
      get WIT_VOICE_KEY() { return readAloudOwner().WIT_VOICE_KEY; },
      get QUOTE_READ_PERCENT_KEY() { return QUOTE_READ_PERCENT_KEY; },
      get RELAY_KEY() { return RELAY_KEY; },
      get LINK_KINDS_KEY() { return LINK_KINDS_KEY; },
      get LINK_RAW_KEY() { return LINK_RAW_KEY; },
      get THREADS_UNPACKED_KEY() { return THREADS_UNPACKED_KEY; },
      get THREAD_CARD_STATES_KEY() { return THREAD_CARD_STATES_KEY; },
      get THREADS_MAIN_KEY() { return THREADS_MAIN_KEY; },
      get THREAD_ORDER_KEY() { return THREAD_ORDER_KEY; },
      get THREAD_CARD_FOLD_KEY() { return THREAD_CARD_FOLD_KEY; },
      get RESUME_KEY() { return RESUME_KEY; },
    },
    get microphone() { return microphoneSettings; },
    get prompts() { return cannedPrompts; },
    resumeWanted: () => resumeWanted(),
    replaceIdentityChoices: (choices) => whoIsWho.replaceChoices(choices),
    renderChannelRows: () => eachColumn(renderChannelRows),
    renderIdentities: () => identityList.render(),
    readMode: (value) => setReadMode(value),
    markOwnRead: () => {
      ownReadSwitch.apply(ownReadSwitch.stored());
    },
    hideEmptyMessages: () => {
      hideEmptySwitch.apply(hideEmptySwitch.stored());
    },
    combineMessages: () => {
      combineSwitch.apply(combineSwitch.stored());
    },
    richHeaders: () => {
      applyRichHeaders(storedRichHeaders());
    },
    summarizeCollapsed: () => {
      applySummarizeCollapsed(storedSummarizeCollapsed());
    },
    inAppLinks: () => {
      inAppLinks.apply(inAppLinks.stored());
      redrawMessageLinks();
    },
    openInView: () => {
      renderOpenInView();
    },
    collapsedLines: () => {
      collapsedLines.apply(collapsedLines.stored());
      refreshElisionLines();
    },
    cardMaxLines: () => {
      keepingCardPlace(() => cardMaxLines.apply(cardMaxLines.stored()));
    },
    readSpeed: () => {
      readAloudOwner().applyReadSpeed(readAloudOwner().storedReadSpeed());
    },
    readAudioSource: () => {
      readAloudOwner().setReadAudioSource(readAloudOwner().storedReadAudioSource(), false);
    },
    witVoice: () => {
      readAloudOwner().setWitVoice(readAloudOwner().storedWitVoice(), false);
    },
    quoteReadPercent: () => {
      quoteReadPercent.apply(quoteReadPercent.stored());
    },
    relay: () => {
      renderReadNew();
    },
    linkKinds: () => {
      linkKinds.reload();
      renderLinkKinds();
    },
    linkRaw: () => {
      keepingFilterPlace(() => {
        linkKinds.reloadRaw();
        renderLinkKinds();
        if (searchField.open() && searchField.links()) eachColumn(renderScrollTools);
      });
    },
    threadsUnpacked: () => {
      cardPacking.reloadMode();
      renderUnpackToggle();
      cardsDrawn.forget();
      if (sideColumnUp()) renderSideColumn();
    },
    threadCardStates: () => {
      cardPacking.reloadChoices();
      cardsDrawn.forget();
      if (sideColumnUp()) renderSideColumn();
    },
    threadsMain: () => {
      setMainRowsShown(mainRowsChoice.stored());
    },
    threadOrder: () => {
      setThreadOrder(threadOrders.stored());
    },
    threadCardFold: () => {
      setThreadCardFolded(threadCardFoldChoice.stored());
    },
  }),
});
const {
  settingsSync, settingsSyncView, applySyncedSetting, renderSettingsSyncState,
  refreshServerSettings, onSettingsFrame, startSettingsSync,
} = settingsSyncGlue;

// --- wiring ------------------------------------------------------------------------------------

// The ordered boot (`#624 voice-page-modularity`, plan step 17): every mount below is tracked
// with the application as it is mounted, in this order, and the final sequence runs as its start
// acts, in theirs. `dispose()` takes the tracked mounts down in reverse order; production does
// not call it yet — the owners above the wiring (the pairing offer, the settings owners) join it
// as their bundles land. One boot in web/src/app/application.ts (`createApplication`).
const application = createApplication({
  mounts: [],
  starts: [
    // The LOAD-TIME sentence only, which is why this act uses `noTokenSentence()` and the causes
    // below it do not: `forgetToken` and an empty Save are causes the owner just supplied
    // themselves, and blaming the rename for either would be a wrong explanation of something
    // they already know.
    () => setTokenState(token() ? "token saved in this browser" : noTokenSentence()),
    // With no token there is nothing to report YET, and firing a toast at page load to say so
    // would put a message over a screen whose entire subject is the thing it is asking for. The
    // sign-in screen says it in its own body; see `setTokenState`. `#63 status-line-placement`.
    () => {
      if (token() && devicePairing.code() === null) {
        setStatus("Checking your token…");
      }
    },
    // Before the first screen is shown, so the bar is in its home from the first frame rather
    // than visibly jumping out of the header once script catches up. `#58 control-bar`.
    () => barPlacement.set(barPlacement.stored()),
    // The read mode as the reader left it, before the first row is drawn or read. `#189
    // restore-ui-state`, and `#221 read-modes`: a record an older page kept says only Hide read
    // or not.
    () => {
      if (uiState.saved()) {
        readModes.set(savedReadMode(uiState.saved()));
        // As in `setReadMode`: the record just read is the restore either way.
        readModes.keep();
      }
    },
    () => {
      showView("voice");
      renderEmptyState();
      voiceCall().renderControls();
    },
    // No token means no interface to show yet: the sign-in screen is the whole page until there
    // is one. With a token, prove it before showing the main screen — unless this device saved
    // the channel for that same token, in which case the application is up in this task, with
    // the saved rows in it, and the proof arrives behind it. A refusal still ends on the sign-in
    // screen, with every saved row gone. `#18 offline-message-cache`.
    //
    // ...and up where the reader left it: on the channel, in their view, at their message. The
    // view's one read waits for the proof, and for the live stream it starts (`payReopenRead`).
    // `#189 restore-ui-state`.
    () => {
      if (devicePairing.code() !== null) {
        // `#369 pairing-link`. The person decides, with one tap: sign in with the link, or not now.
        devicePairing.offer();
      } else if (token()) {
        if (hydrateFromCache()) {
          showScreen("main");
          reopenSavedView();
        }
        // `#625 signin-flash`. ...and with no saved shell to show, NO screen until the proof
        // arrives: the sign-in form is served hidden and stays so, and "Checking your token…"
        // says what is happening. A form asking for a token the reader has already given, up for
        // a round trip on every reload, read as being signed out. `signIn` shows the main
        // screen, or this one after a refusal.
        guardQuietly(signIn)();
      } else {
        showScreen("signin");
      }
    },
    // `#304 back-gesture-in-app`. Last, with every part of the page defined: the history kept in
    // step with what is open, from the screen the page opened on.
    () => startBackStack(),
    // `#462 webapp-deep-links`. Open where a link says, once sign-in gives it a channel to open.
    () => guardQuietly(applyDeepLink)(),
  ],
});

// `#616 composer-no-autofill`. The token field starts EMPTY, even with a token saved, and is emptied again
// once a token has signed in. A password field holding the token, in one form with every message box, is
// what made a phone's password manager take a send for a sign-in and offer to save it (see #signin-form
// in web/voice.html). The saved token lives in localStorage only, and Save with the field empty tries it
// again. The form is never submitted; should something submit it, nothing happens. Sign-in's and pairing's listeners,
// a second pairing link opened in this tab included: one owner in web/src/view/settings/signincontrols.ts (`#624
// voice-page-modularity`), mounted here, in the page's order; its `unmount` waits for the ordered boot's teardown.
application.track(mountSignInControls({
  lookup: (id) => el(id),
  saveToken: guardQuietly(saveToken),
  forgetToken: () => forgetToken(),
  redeemPairingInput: guardQuietly(() => devicePairing.redeemInput()),
  redeemPairing: guardQuietly(() => devicePairing.redeem()),
  declinePairing: () => devicePairing.decline(),
  takePairingCode: () => takePairingCode(),
  pairingOpened: (code) => devicePairing.opened(code),
}));

// Settings' switches and choices, each restored as kept before it is heard, since the controls carry the live truth
// `start()` and the rest read: the microphone's checkboxes, the audio devices (`#619 voice-bluetooth-output`), where
// the bar sits (`#58 control-bar`), where Slack links open (`#481 slack-app-deep-link`), the read-new button and
// select (`#44 live-push`, `#126 read-new-selector`), and resuming (`#46 conversation-replay`). One owner in
// web/src/view/settings/switches.ts (`#624 voice-page-modularity`), mounted here, in the page's order; its `unmount`
// waits for the ordered boot's teardown.
application.track(mountSettingsSwitches({
  lookup: (id) => el(id),
  storedMicSettings: () => microphoneSettings.stored(),
  micSettingsChanged: () => microphoneSettings.changed(),
  refreshAudioDevices: () => audioDevices.refresh(),
  audioDevicesChanged: () => audioDevices.changed(),
  renderAudioRoute: () => audioDevices.renderRoute(),
  mediaDevices: () => navigator.mediaDevices,
  storedBarPlacement: () => barPlacement.stored(),
  barPlacementChanged: () => barPlacement.changed(),
  storedSlackOpen: () => storedSlackOpen(),
  slackOpenChanged: () => settingsReadouts.slackOpenChanged(),
  // Least-to-most talking, wrapping back to off, so the whole range is reachable with taps and without a menu.
  cycleReadNew: () => chooseReadNew(RELAY_MODES[(RELAY_MODES.indexOf(relayMode()) + 1) % RELAY_MODES.length]),
  chooseReadNew: (mode) => chooseReadNew(mode),
  renderReadNew: () => renderReadNew(),
  renderLiveState: () => renderLiveState(),
  resumeWanted: () => resumeWanted(),
  resumeToggled: () => {
    // `#331 resume-on-websocket`. Disabled where this server cannot resume, so nothing can change it
    // there; and were something to, the choice is not taken from a switch that is not in force.
    if (!callResume.allowed()) {
      renderResumeState();
      return;
    }
    const on = el("resume-toggle").checked;
    persistResume(on);
    // Deliberately NOT clearing the last outcome (`callResume.last()`): everything the screen says
    // about resuming is already gated on the switch, so an outcome from before it was flipped cannot
    // be read out under it.
    // Clearing here as well would be a second place that decides the same thing.
    renderResumeState();
    setStatus(
      on
        ? "The next call will be told what was already said."
        : "The next call will start the agent from nothing."
    );
  },
}));

// Every setting restored as kept before anything is drawn: the column and the type, the line counts (`#425
// collapsed-lines`), the read-aloud speed, the switches that shape the lists, and the message box's Enter. The order
// is said once, in web/src/view/settings/restore.ts (`#624 voice-page-modularity`); each setting's owner applies it.
restoreStoredSettings({
  readingWidth: () => readingWidthControl.apply(storedReadingWidth()),
  textSize: () => textSize.apply(storedMsgScale()),
  collapsedLines: () => collapsedLines.apply(collapsedLines.stored()),
  cardMaxLines: () => cardMaxLines.apply(cardMaxLines.stored(), false),
  quoteReadPercent: () => quoteReadPercent.apply(quoteReadPercent.stored()),
  readSpeed: () => readAloudOwner().applyReadSpeed(readAloudOwner().storedReadSpeed()),
  ownRead: () => ownReadSwitch.apply(ownReadSwitch.stored()),
  combine: () => combineSwitch.apply(combineSwitch.stored()),
  hideEmpty: () => hideEmptySwitch.apply(hideEmptySwitch.stored()),
  richHeaders: () => applyRichHeaders(storedRichHeaders()),
  inAppLinks: () => inAppLinks.apply(inAppLinks.stored()),
  openInView: () => renderOpenInView(),
  summarizeCollapsed: () => applySummarizeCollapsed(storedSummarizeCollapsed()),
  enterSends: () => enterSends.apply(enterSends.stored()),
});
// `#340 composer-grow-box`. The grow switches drawn as kept and heard, three boxes fitted as they are typed in, and
// Send kept in view as a box grows. One owner in web/src/view/composer/growlisteners.ts (`#624
// voice-page-modularity`), mounted here, in the page's order; its `unmount` waits for the ordered boot's teardown.
application.track(mountComposerGrowListeners({
  lookup: (id) => leftEl(id),
  storedComposerGrow: () => storedComposerGrow(),
  renderComposerGrow: (on) => renderComposerGrow(on),
  applyComposerGrow: (on, grow) => applyComposerGrow(on, grow),
  fitGrowingBox: (field) => fitGrowingBox(field),
  keepGrowingBoxInView: (field) => keepGrowingBoxInView(field),
  resizeObserver: () => window.ResizeObserver,
}));
// The screen's sliders and their steps, the page-build notice's reload (`#529 page-build-stale-notice`: only on the
// reader's press), and the reading column's grip. One owner in web/src/view/settings/screencontrols.ts (`#624
// voice-page-modularity`), mounted here, in the page's order; its `unmount` waits for the ordered boot's teardown.
application.track(mountScreenControls({
  lookup: (id) => el(id),
  readingWidthChanged: (value) => readingWidthControl.changed(value),
  textSizeChanged: (value) => textSize.changed(value),
  stepTextSize: (direction) => textSize.step(direction),
  collapsedLinesChanged: (value) => collapsedLinesChanged(value),
  stepCollapsedLines: (direction) => stepCollapsedLines(direction),
  cardMaxLinesChanged: (value) => cardMaxLinesChanged(value),
  stepCardMaxLines: (direction) => stepCardMaxLines(direction),
  reloadPage: () => window.location.reload(),
  showPageBuild: () => settingsReadouts.pageBuild(),
  quoteReadPercentChanged: (value) => quoteReadPercentChanged(value),
  stepQuoteReadPercent: (direction) => stepQuoteReadPercent(direction),
  gripDown: (event) => readingWidthControl.gripDown(event),
  gripMove: (event) => readingWidthControl.gripMove(event),
  gripUp: () => readingWidthControl.gripUp(),
  gripKey: (event) => readingWidthControl.gripKey(event),
}));
// The two columns' separator: its drag, its keys and its double-click reset (`#1186
// two-column-splitter`). One owner in web/src/view/channel/splitter.ts (`#624
// voice-page-modularity`), mounted here, in the page's order.
application.track(mountColumnSplitterControls({
  lookup: (id) => leftEl(id),
  down: (event) => columnSplitter.down(event),
  move: (event) => columnSplitter.move(event),
  up: () => columnSplitter.up(),
  key: (event) => columnSplitter.key(event),
  doubleClick: (event) => columnSplitter.doubleClick(event),
}));

// The page's last document-level listeners (`#54 resume-recovery`): the page
// going away or coming back, and a pointer down outside a row's menu.
// One owner in web/src/view/shell/documentlisteners.ts (`#624 voice-page-modularity`), mounted here,
// in the page's order; its `unmount` waits for the ordered boot's teardown.
application.track(mountDocumentListeners({
  onVisibility: () => onVisibility(),
  closeRowMenuOutside: (event) => closeRowMenuOutside(event),
}));
// The keys' and the selection's listeners (`#224 keyboard-shortcuts`, `#344 search-shortcut`, `#296
// copy-highlighted-message`, `#239 single-selection`): every key heard in one place, the chords that act whatever
// holds the focus heard first, a copy with nothing selected, the single-key switch (restored first, so the keys are
// said as they will act), and the clicks, the focus and the pointer that move the one selection. One owner in
// web/src/view/shell/keylisteners.ts (`#624 voice-page-modularity`), mounted here, in the page's order; the page
// never takes them down, and its `unmount` waits for the ordered boot to have a teardown.
application.track(mountKeyListeners({
  onPageKey: (event) => keyTiming.around(event, () => keyDispatch.onPageKey(event)),
  onEverywhereKey: (event) => keyDispatch.onEverywhereKey(event),
  onPageBeforeCopy: (event) => highlightCopy.onPageBeforeCopy(event),
  onPageCopy: (event) => highlightCopy.onPageCopy(event),
  restoreSingleKeys: () => singleKeys.restore(),
  renderKeyHints: () => keyHints.render(),
  applySingleKeys: (on) => singleKeys.apply(on),
  singleKeysBox: () => /** @type {HTMLInputElement} */ (el("single-keys")),
  keyPanes: () => KEY_PANES,
  noteClickedRow: (pane, node) => noteClickedRow(pane, node),
  onPaneFocus: (pane, event) => onPaneFocus(pane, event),
  onPointerMove: (event) => onPointerMove(event),
  onPointerDown: (event) => onPointerDown(event),
}));
// The quoted words' and the quick-reply box's listeners: words selected in one message make its Reply a Quote reply
// (`#267 quote-reply`); the selection chip and its twin in the paused read's strip, pressed without taking the
// selection or the focus from the words they quote (`#726 vt-selection-chip`, `#819
// voice-page-quoted-reply-and-draft-chip`); the floating box (`#727 vt-selection-quick-reply`): its keys, Send and
// Cancel and its question (`#846 draft-chip-honest`), its microphone (`#829 voice-page-dictation-polish`), the Draft
// chip and React (`#821 voice-page-read-aloud-react`), a tap outside (which puts what was written back under its
// message: `putDownQuickReply`), and the lists and the window that move it. One owner in
// web/src/view/composer/quickreply.ts (`#624 voice-page-modularity`), mounted here, in the page's order; its
// `unmount` waits for the ordered boot's teardown.
application.track(mountQuickReplyListeners({
  byId: (id) => el(id),
  leftById: (id) => leftEl(id),
  noteQuoteSelection: () => noteQuoteSelection(),
  pressSelectionChip: () => pressSelectionChip(),
  sendQuickReply: () => guardQuietly(sendQuickReply)(),
  cancelQuickReply: () => cancelQuickReply(),
  answerQuickReplyDiscard: (discard) => answerQuickReplyDiscard(discard),
  closeQuickReply: () => closeQuickReply(),
  onBoxKey: (event) => onComposerKey(event, guardQuietly(sendQuickReply), el("quick-reply-text")),
  watchBoxInsertions: (box) => watchBoxInsertions(box),
  noteQuickReplyInput: () => quoteOwner.noteQuickReplyInput(),
  showKeyboard: () => dictation.showKeyboard(),
  stopDictation: () => dictation.stop(),
  pressDraftChip: () => guardQuietly(pressDraftChip)(),
  reactToHeldSentence: () => readAloudOwner().reactToHeldSentence(),
  quickReplyOpen: () => quoteOwner.quickReplyOpen(),
  putDownQuickReply: () => putDownQuickReply(),
  quickReplyListMoved: (target) => quickReplyListMoved(target),
  visualViewport: () => window.visualViewport || null,
  placeQuickReply: () => placeQuickReply(),
  selectionHeld: () => Boolean(quoteOwner.held()),
  placeSelectionChip: () => placeSelectionChip(),
}));
// The page's own listeners at the document and the window: a message's time worded as the pointer or the focus
// reaches it (`#260 timestamp-hover`) and its day said on a tap or Enter, captured ahead of the row's own press
// (`#479 message-time-full-date`); and the last moments a phone promises this page before it may reclaim it,
// hidden or being unloaded, when the reader's place (`#189 restore-ui-state`) and then a turn still being said
// (`#330 voice-long-turn-drop`) are written down. One owner in web/src/view/shell/pagelisteners.ts (`#624
// voice-page-modularity`), mounted here, in the page's order; its `unmount` waits for the ordered boot's teardown.
application.track(mountPageListeners({
  wordDayTitle: (event) => freshenDayTitle(event),
  onTimeTap: (event) => onTimeTap(event),
  onTimeKey: (event) => onTimeKey(event),
  hidden: () => document.visibilityState === "hidden",
  rememberUiState: () => {
    holdChannelPlace();
    saveUiState();
  },
  saveUnsettledTurns: () => voiceCall().saveUnsettledTurns(),
}));

/** Take the pace popover down. Safe to call when it is already down. */
function closeSpeedPopover() {
  el("speed-popover").hidden = true;
  el("read-speed").setAttribute("aria-expanded", "false");
  scheduleBackSync();
}

// The reading preferences' switches: the read-aloud pace's popover, Your own messages, and the switches that shape
// how messages are drawn and sent. One owner in web/src/view/settings/preferenceswitches.ts (`#624
// voice-page-modularity`), mounted here, in the page's order; its `unmount` waits for the ordered boot's teardown.
application.track(mountPreferenceSwitches({
  lookup: (id) => el(id),
  toggleSpeedPopover: () => {
    const open = el("speed-popover").hidden;
    el("speed-popover").hidden = !open;
    el("read-speed").setAttribute("aria-expanded", open ? "true" : "false");
    // `#304 back-gesture-in-app`. An open popover is a level, with an entry of its own.
    scheduleBackSync();
  },
  applyOwnRead: guardQuietly(ownReadApply.apply),
  undoOwnRead: guardQuietly(ownReadApply.undo),
  markOwnReadChanged: messagePreferences.markOwnRead.changed,
  hideEmptyChanged: messagePreferences.hideEmpty.changed,
  combineChanged: messagePreferences.combine.changed,
  richHeadersChanged: messagePreferences.richHeaders.changed,
  inAppLinksChanged: messagePreferences.inAppLinks.changed,
  openInViewChanged: messagePreferences.openInView.changed,
  summariesChanged: messagePreferences.summaries.changed,
  enterSendsChanged: (on) => enterSends.apply(on),
}));

// `#240 reading-bookmark`. The ways back: to the bookmark, to where the reader was reading (the left column's chip, as
// b is), and back to a reply (web/src/view/channel/readingjumps.ts).
application.track(mountReadingJumps({
  lookup: (id) => el(id),
  leftLookup: (id) => leftEl(id),
  jumpToBookmark: () => guardQuietly(jumpToBookmark)(),
  jumpToReading: () => readingJump.jump(),
  backToReply: () => backToReply(),
}));

// The read-aloud controls (`#567 pace-stepper`, `#376 read-aloud-blocks`, `#442 read-strip-order`, `#434
// read-aloud-pause`, `#451 read-strip-followups`, `#627 scrubber-thumb-arrows`, `#886 voice-page-autoplay`): the pace's
// steps and box, the strip's arrows, grip and Autoplay, the window resizing under it, Read and Stop under the list.
// One owner in web/src/view/voice/readcontrols.ts (`#624 voice-page-modularity`), mounted here, in the page's order;
// its `unmount` waits for the ordered boot's teardown. Read and Stop leave reading mode by `setReadingMode` and
// `leaveReadingMode`, below.
application.track(mountReadControls({
  byId: (id) => el(id),
  win: () => (typeof window.addEventListener === "function" ? window : null),
  stepReadSpeed: (direction) => readAloudOwner().stepReadSpeed(direction),
  commitReadSpeedBox: () => readAloudOwner().commitReadSpeedBox(),
  stepReading: (delta) => readAloudOwner().stepReading(delta),
  stepSection: (delta) => readAloudOwner().stepSection(delta),
  onReadGripDown: (event) => readAloudOwner().onReadGripDown(event),
  onReadGripMove: (event) => readAloudOwner().onReadGripMove(event),
  onReadGripUp: () => readAloudOwner().onReadGripUp(),
  onReadGripKey: (event) => readAloudOwner().onReadGripKey(event),
  moveReadSteer: (offset) => readAloudOwner().moveReadSteer(offset),
  placeReadSteer: () => readAloudOwner().placeReadSteer(),
  autoplayOn: () => autoplay.on(),
  setAutoplay: (on) => autoplay.setOn(on),
  forgetAutoplayHalt: () => autoplay.halt(null),
  readInProgress: () => readAloudOwner().readInProgress(),
  toggleReadingPaused: () => readAloudOwner().toggleReadingPaused(),
  pendingRead: () => readAloudOwner().pendingRead,
  readingMode: () => readAloudOwner().readingMode,
  setReadingMode: (on) => setReadingMode(on),
  leaveReadingMode: () => leaveReadingMode(),
}));

/**
 * `#442 read-strip-order`. Stop: the read ends AND reading mode is left, in one tap, by the path
 * the Read button has always left it by. After #434 made Read the Pause of a read in progress, the
 * only ways out were a tap on the row being read or the media keys' stop, which a reader holding a
 * phone in a car does not find. The button pressed goes as the mode does, so focus goes to Read.
 */
function leaveReadingMode() {
  const from = document.activeElement;
  if (readAloudOwner().readingMode) setReadingMode(false);
  else readAloudOwner().stopReading();
  if (from && from === el("read-aloud-stop")) {
    const read = el("read-aloud");
    if (!read.hidden && typeof read.focus === "function") read.focus();
  }
}

/** Turn reading mode on or off. The one path in and out of it, whichever control asked. */
function setReadingMode(on) {
  readAloudOwner().readingMode = on;
  // Turning it OFF stops what is playing. Leaving audio running under a reader who has just said
  // "not this any more" is the kind of thing that gets a tab muted and never opened again.
  if (!readAloudOwner().readingMode) {
    readAloudOwner().stopReading();
  }
  readAloudOwner().setReadState(readAloudOwner().readingMode ? "ready" : "idle");
  if (readAloudOwner().readingMode) {
    // A failed conversation banner belongs to the call that produced it. Leaving it over the
    // independent device-speech mode makes a working Read control look broken; if device speech
    // itself is unavailable, replace it immediately with that actionable reason.
    clearError();
    const speechProblem = readAloudOwner().readAloudPlayback === "browser" ? readAloudOwner().browserSpeechProblem() : "";
    if (speechProblem) {
      readAloudOwner().setReadState("failed");
      showError(speechProblem);
    }
    // AHEAD OF THE TAP. This is the request that makes every later tap cheap, and it is issued the
    // moment the mode is entered rather than when a message is chosen, because the reader is
    // already looking at the text and has not decided yet which line they want.
    guardQuietly(readAloudOwner().prepareSpeech)();
  } else {
    readAloudOwner().forgetPreparedSpeech();
  }
  setStatus(
    readAloudOwner().readingMode
      ? readAloudOwner().readAloudPlayback === "browser"
        ? readAloudOwner().browserSpeechProblem() || `${readAloudOwner().readAloudLabel}: tap a message to hear it, and it archives when it finishes.`
        : "reading mode: tap a message to hear it, and it archives when it finishes."
      : "reading mode off."
  );
  voiceCall().renderControls();
  renderChannelRows();
}
// The call's controls: Talk, Hang up, the speaker, clearing the view, forgetting the stored conversations, and a
// failed start tried again. One owner in web/src/view/voice/callcontrols.ts (`#624 voice-page-modularity`), mounted
// here, in the page's order; its `unmount` waits for the ordered boot's teardown.
application.track(mountCallControls({
  byId: (id) => el(id),
  onTalk: () => voiceCall().onTalk(),
  hangUp: () => voiceCall().stop(),
  // `#886 voice-page-autoplay`. Hang up ends the going on: a read in progress, heard to its end, reads nothing after it.
  haltAutoplay: () =>
    autoplay.halt(readAloudOwner().nowPlaying !== null ? readAloudOwner().nowPlaying.id : readAloudOwner().pendingRead),
  clearView: () => voiceTranscript().onClear(),
  // `guardQuietly`, not `guard`: erasing a stored record must never be able to hang up a live call.
  forgetConversations: () => guardQuietly(voiceTranscript().forgetConversations)(),
  toggleSpeaker: () => voiceCall().setSpeakerOff(!voiceCall().session.speakerOff),
  retryStart: () => voiceCall().retryStart(),
}));
// `#43 typed-input`, rehoused by `#59 text-entry-button` and then by `#241 voice-composer`: the
// composer, in three listeners and no logic. Everything they call is a named function above, which
// is exactly why moving the control from the dock to the bar and from the bar to the transcript did
// not move the send path with it.
//
// `guardQuietly`, NOT `guard`. `guard()` calls `teardown()`, so a send that failed would hang up on
// the owner — which is the one thing a failed message must never do. The conversation a first send
// starts is guarded on its own, inside `sendTyped`, exactly as Talk's is.
// Send, the box's draft restored and kept as it is written, a tap that takes the box into view (`#241
// voice-composer`), and each canned prompt's field restored and kept and its button (`#60 canned-prompt-buttons`).
// One owner in web/src/view/voice/callcomposer.ts (`#624 voice-page-modularity`), mounted here, in the page's order;
// its `unmount` waits for the ordered boot's teardown.
application.track(mountCallComposer({
  lookup: (id) => el(id),
  sendTyped: guardQuietly(voiceCall().sendTyped),
  storedComposeDraft: () => voiceCall().storedComposeDraft(),
  composerFocused: () => {
    if (voiceCall().writingToAgent()) voiceCall().keepComposerInView();
  },
  composeInput: () => {
    voiceCall().rememberComposeDraft();
    voiceCall().noteComposing();
  },
  cannedPrompts: CANNED_PROMPTS,
  storedCannedPrompts: () => cannedPrompts.stored(),
  cannedPromptsChanged: () => cannedPrompts.changed(),
  pressCannedPrompt: (entry) => {
    // Shut FIRST, and whether or not the send goes. Choosing from a menu closes it — and when the
    // send is refused the reason appears on the status line at the far end of the screen, which an
    // open tray would be standing in front of.
    setPromptsOpen(false);
    if (voiceCall().sendUserMessage(cannedPrompts.promptFor(entry))) {
      // AFTER the send, and only if it went: `sendUserMessage` says "Sent.", which is true of any
      // message. This says WHICH question was asked, because the button carries five letters.
      setStatus(entry.said);
    }
  },
}));
// The prompts tray's and the action menu's openers, each a plain toggle, and what they show stated once at load
// (`#820 voice-page-action-menu`, `#880 voice-page-menu-placement`, `#886 voice-page-autoplay`). One owner in
// web/src/view/shell/menuopeners.ts (`#624 voice-page-modularity`), mounted here, in the page's order; its `unmount`
// waits for the ordered boot's teardown.
application.track(mountMenuOpeners({
  lookup: (id) => el(id),
  togglePrompts: () => setPromptsOpen(!promptsTray.open()),
  toggleActionMenu: () => setActionMenuOpen(!actionMenu.open()),
  shutActionMenu: () => setActionMenuOpen(false),
  renderReaderRow: () => readerRow.render(),
  renderAutoplay: () => autoplay.render(),
  shutPrompts: () => setPromptsOpen(false),
}));
// The call's box (web/src/view/composer/boxlisteners.ts, `#624 voice-page-modularity`). `#225 enter-to-send`: one
// rule for every message box; see `onComposerKey`. A textarea since `#241 voice-composer`, as the channel's box is, so
// with Enter to send off an Enter is a new line. `paste-never-sends`: every message box, a paste only ever puts text in.
application.track(mountMessageBox({
  box: el("compose-text"),
  // `#1144 scrollback-typing-jump`: typed at the foot, the keyboard's jump is made whole after the
  // browser reveals the caret, so the whole composer stands on the keys.
  typed: () => {
    const box = el("compose-text");
    if (atBottom(growingBoxList(box))) screenKeyboard.noteComposerInput(box);
  },
  onKey: (event) => onComposerKey(event, guardQuietly(voiceCall().sendTyped), el("compose-text")),
  watchBoxInsertions: (box) => watchBoxInsertions(box),
}));
// The page's own buttons beside the call: the banner's, the status line's and the error's dismissals, the post card's
// Send, Cancel and More, Settings opened and closed, reading on the device, and the read-aloud source and voice. One
// owner in web/src/view/voice/pagebuttons.ts (`#624 voice-page-modularity`), mounted here, in the page's order; the
// acts its owners take the press for are handed as they were taken here, at load.
application.track(mountPageButtons({
  lookup: (id) => el(id),
  dismissBanner,
  dismissStatus,
  sendPostProposal: voiceTranscript().sendPostProposal,
  cancelPostProposal: voiceTranscript().cancelPostProposal,
  // `#269 approval-card-inline`. More opens a long draft in place, and Less folds it again. Measured
  // before, so a reader at the newest line still sees the buttons after the card grows upward. Docked
  // under the thread list (`#281 view-keys`), the same for the left column's list behind the cards, and
  // `showPostProposal` keeps a reader at the foot of the cards on it.
  togglePostDraft: () => {
    if (!voiceTranscript().postWatch.shown) return;
    const pinned = atBottom(leftEl("scroll-area"));
    voiceTranscript().postWatch.unfolded = !voiceTranscript().postWatch.unfolded;
    voiceTranscript().showPostProposal(voiceTranscript().postWatch.shown);
    if (screens.screen() === "main") followIfPinned(pinned, 0, screens.view());
  },
  openSettings: () => showScreen("settings"),
  clearError,
  readOnDevice: readAloudOwner().readOnDevice,
  toggleAudioSource: () =>
    readAloudOwner().setReadAudioSource(readAloudOwner().readAudioSource === "device" ? (readAloudOwner().witReadAloud ? "wit" : "agent") : "device"),
  setWitVoice: (voice) => readAloudOwner().setWitVoice(voice),
  setReadAudioSource: (source) => readAloudOwner().setReadAudioSource(source),
  closeSettings,
}));

// `#85 voice-desktop-review`. The paragraphs that used to stand between the reader and every switch
// now live on their own screen, and each `?` on the settings screen opens the matching entry.
//
// ONE DELEGATED HANDLER over `data-help`, not a listener per button. The alternative is an id per
// `?` — and this page's suite refuses ids that are not declared in web/voice.html, so a new
// settings group would mean inventing one there purely to hang a listener on. The attribute names
// the entry, the entry's id is `help-<that>`, and adding a group is one button plus one article.
// Help's ways in and out (web/src/view/settings/helplinks.ts, `#624 voice-page-modularity`).
application.track(mountHelpLinks({
  lookup: (id) => el(id),
  closeHelp: () => closeHelp(),
  showHelp: (topic) => showHelp(topic),
}));
// What the page kept of its sends, read back, and the listeners over it: the replies' drafts (`#51 reply-view`), this
// device's own posts before the outbox (`#500 sent-receipts-pile-up`), the Done writes a page closed before the server
// had them, sent now, when the connection comes back, and whenever a write of this page's is answered (`#892
// voice-page-optimistic-done`), and the sends dismissed in the last days (`#193 compose-draft-loss`) with their Undo
// and Forget, and the bookmark's Undo (`#420 bookmark-clear`). One owner in web/src/view/channel/keptsends.ts.
application.track(mountKeptSends({
  lookup: (id) => el(id),
  loadReplyDrafts: () => replyDrafts.load(),
  loadOwnPosts: () => loadOwnPosts(),
  loadOutgoingMessages: () => loadOutgoingMessages(),
  sendLeftOverInboxWrites: () => sendLeftOverInboxWrites(),
  leaveInbox: () => inboxLedger.leave(),
  loadDismissedSends: () => loadDismissedSends(),
  renderDismissedSends: () => dismissedSendsView.render(),
  undoSendDismiss: () => {
    const offer = sendUndo.current();
    if (offer) putBackDismissedSend(offer.id);
  },
  undoBookmarkRemoval: () => undoBookmarkRemoval(),
  forgetDismissedSends: () => forgetDismissedSends(),
}));
channelDrafts.load();
// `#819 voice-page-quoted-reply-and-draft-chip`. The quick replies' drafts, and where every draft is.
quickDrafts.load();
draftLedger.load();
// What the ledger kept of drafts sent, emptied or overwritten elsewhere since is let go of.
draftLedger.prune(draftHeld);
// The channel's view chips, the thread's Back, a thread chosen, and the picker touched, the cue that the reader wants
// the full list of threads. One owner in web/src/view/channel/viewcontrols.ts (`#624 voice-page-modularity`), mounted
// here, in the page's order; its `unmount` waits for the ordered boot's teardown.
application.track(mountChannelViewControls({
  lookup: (id) => el(id),
  changeView: (view) => guardQuietly(() => changeChannelView(view))(),
  closeThread: () => guardQuietly(closeThread)(),
  selectThread: () => guardQuietly(onThreadSelect)(),
  refreshThreadDirectory: () => guardQuietly(refreshThreadDirectory)(),
}));
// The channel's box, and a thread's, which is the same box posting somewhere else: what is written kept as it is
// written, Send, its keys, and what is pasted (web/src/view/composer/boxlisteners.ts).
application.track(mountMessageBox({
  box: el("channel-compose-text"),
  remember: () => rememberChannelDraft(),
  // `#1144 scrollback-typing-jump`, as the call's box above.
  typed: () => {
    const box = el("channel-compose-text");
    if (atBottom(growingBoxList(box))) screenKeyboard.noteComposerInput(box);
  },
  sendButton: el("channel-send"),
  send: guardQuietly(sendChannelMessage),
  // `#1135 raw-message-page`: the row under the box, its Cancel and the question Cancel asks.
  cancelButton: el("channel-cancel"),
  discardYes: el("channel-discard-yes"),
  discardNo: el("channel-discard-no"),
  cancel: () => cancelChannelMessage(),
  answerDiscard: (discard) => answerChannelDiscard(discard),
  onKey: (event) => onComposerKey(event, guardQuietly(sendChannelMessage), el("channel-compose-text")),
  watchBoxInsertions: (box) => watchBoxInsertions(box),
}));
// The thread's back gesture and the right column's controls (`#304 back-gesture-in-app`, `#229 desktop-two-column`,
// `#388 thread-header-collapse`, `#281 view-keys`, `#262 search-load-older`, `#404`, `#419`, `#426`): a swipe right out
// of a thread, the layout button, the X, the card at the top of a thread, the card's flight ending, and the Threads
// view's chips. One owner in web/src/view/channel/sidecontrols.ts (`#624 voice-page-modularity`), mounted here, in the
// page's order; its `unmount` waits for the ordered boot's teardown.
application.track(mountSideControls({
  byId: (id) => el(id),
  leftById: (id) => leftEl(id),
  inThread: () => selection().view === "thread",
  backGesture: () => entryState().backGesture,
  setBackGesture: (start) => { entryState().backGesture = start; },
  claimClick: () => rowGestures.claimClick(),
  closeThread: () => guardQuietly(closeThread)(),
  toggleColumns: () => toggleColumns(),
  closeSide: () => closeSideByX(),
  threadCardIds: THREAD_CARD_LISTS.map(([id]) => id),
  threadCardPressed: (card, event) => threadCardPressed(card, event),
  endCardFlightPart: (node, event) => endCardFlightPart(node, event),
  loadOlderThreads: () => guardQuietly(loadOlderThreads)(),
  searchReplies: () => guardQuietly(repliesControlPressed)(),
  nextUnpack: () => setUnpackMode(cardPacking.nextMode()),
  toggleMainRows: () => setMainRowsShown(!mainRowsChoice.on()),
  nextOrder: () => setThreadOrder(nextThreadOrder()),
}));
// `#388 thread-header-collapse`. The card at the top of a thread drawn as the device last chose before anything is shown.
renderThreadCardFold();
renderUnpackToggle();
renderMainToggle();
renderOrderToggle();
inColumn(columns.side, () => {
  // The right column's twin of the channel's box, each of its acts done in that column.
  const sideBox = el("channel-compose-text");
  mountMessageBox({
    box: sideBox,
    remember: inThisColumn(rememberChannelDraft),
    // `#1144 scrollback-typing-jump`, as the left column's box, in this column.
    typed: inThisColumn(() => {
      if (atBottom(growingBoxList(sideBox))) screenKeyboard.noteComposerInput(sideBox);
    }),
    sendButton: el("channel-send"),
    send: inThisColumn(guardQuietly(sendChannelMessage)),
    cancelButton: el("channel-cancel"),
    discardYes: el("channel-discard-yes"),
    discardNo: el("channel-discard-no"),
    cancel: inThisColumn(cancelChannelMessage),
    answerDiscard: inThisColumn(answerChannelDiscard),
    onKey: inThisColumn((event) => onComposerKey(event, guardQuietly(sendChannelMessage), sideBox)),
    watchBoxInsertions: (box) => watchBoxInsertions(box),
  });
  // This list's own controls (web/src/view/channel/sidelistcontrols.ts), each act done in this column.
  mountSideListControls({
    lookup: (id) => el(id),
    loadOlder: inThisColumn(guardQuietly(olderControlPressed)),
    clearBacklog: inThisColumn(guardQuietly(clearBacklog)),
    // `#338 column-overscroll`. The reader's own way to the newest line lands on the foot, as End does.
    jumpNewest: inThisColumn(() => {
      closeOverscroll();
      scrollToNewest();
    }),
    // What the left column's scroll listener does, for this list: the jump, the way back to a reply,
    // the step back at the top, and the summaries of what has come into view.
    listScrolled: inThisColumn(() => {
      if (!sideShowing()) return;
      renderJumpNewest();
      if (replyArrows().back !== null) {
        const reply = replyReturnRow();
        if (reply === null || rowOnScreen(reply)) {
          replyArrows().back = null;
          renderScrollTools();
        }
      }
      maybeLoadOlder();
      requestVisibleSummaries();
    }),
  });
});
// The window's width decides one column or two, through the stylesheet: asked again once per frame of
// a resize, never per scroll (web/src/view/dom `createOncePerFrame`).
const columnsResize = createOncePerFrame(() => {
  renderColumns();
  // `#340 composer-grow-box`. A window widened to a desk's draws a growing box, to be measured.
  fitGrowingBoxes();
  // `#846 draft-chip-honest`. One column or two shows other boxes.
  renderDraftChip();
}, () => (typeof requestAnimationFrame === "function" ? requestAnimationFrame : null));
// --- the on-screen keyboard ------------------------------------------------------------------------
//
// `#429 keyboard-hides-bar`. The owner, writing in the channel view on a phone: the open keyboard, the
// big Read and Pace tile and the control bar above it left too little of the screen to read the message
// being answered, and nobody writing a message is being read to at the same time — "hide the entire
// bar, both rows of it, until the keyboard is dismissed again". So while the on-screen keyboard is up
// the page is the list and the box being typed in, and both rows come back as the keyboard goes down or
// the field loses the focus.
//
//   * A FOCUSED FIELD IS NOT A KEYBOARD. A desk, or a phone with a keyboard plugged in, focuses the same
//     field with no keyboard on the screen. What only an on-screen keyboard does is take height from the
//     visual viewport, so "up" is both: a field the reader types in has the focus (the message box, a
//     reply in the stream, the search), AND the visible height is well short of the tallest this window
//     has been at its width, or `navigator.virtualKeyboard` reports a keyboard with a height, where the
//     browser has it. The page does not set that API's `overlaysContent`, which would undo
//     `interactive-widget=resizes-content`, and some browsers report no geometry without it; the
//     viewport is the signal every phone browser gives.
//   * THE TALLEST, NOT THE LAYOUT VIEWPORT. With `interactive-widget=resizes-content` (web/voice.html),
//     Chrome on Android shrinks the layout viewport with the keyboard, so `innerHeight` shrinks with the
//     visual viewport and comparing the two sees nothing; Safari leaves the layout alone and shrinks the
//     visual viewport only. The tallest height seen at this width is the screen without a keyboard on
//     both, and a turn of the phone, which changes the width, starts it again. The visual viewport is
//     reckoned at the page's own scale, so a pinch zoom is not a keyboard; the threshold clears a
//     browser's own toolbars sliding away, which change the height by well under it.
//   * WHAT GOES IS THE STYLESHEET'S. This sets `data-keyboard` on the root element and nothing else;
//     web/voice.css takes the bars away only in the channel view and only where the pointer is coarse,
//     and never the box, its Send or its Grow, which are in the list. The call's view keeps its dock
//     over the keyboard, as `#241 voice-composer` laid it out to.
//   * THE SOUND GOES ON. A reading already playing as the keyboard comes up keeps playing — the reader
//     asked for it and is only answering something — and a call keeps going; the tile shrinks to a slim
//     Stop, or Hang up, instead of going, so the way out stays one tap away. The pace popover, whose
//     button has gone, is closed.
//   * THE READER'S PLACE is kept by the keepers the dock already has: the list grows at its foot as the
//     bars go, its scroll position left as it was (a reader at the foot stays there), and as they come
//     back the dock-growth keeper of `#280 scroll-stability` holds a reader at the foot on it.

// Whether the keyboard is up and the stylesheet told, the composer's foot brought to the keyboard's top in
// the jump's own moment, and the readout of the numbers behind it: one owner in
// web/src/view/composer/keyboard.ts (`#624 voice-page-modularity`), with the window, the document and
// `navigator.virtualKeyboard` handed to it here. The listeners below are the page's.
const screenKeyboard = createOnScreenKeyboard({
  clock: clockOnUse(),
  lookup: (id) => leftEl(id),
  el: (id) => el(id),
  viewport: () => window.visualViewport || null,
  innerHeight: () => window.innerHeight,
  scrollY: () => window.scrollY,
  root: () => document.documentElement,
  activeElement: () => document.activeElement,
  virtualKeyboard: () => navigator.virtualKeyboard,
  computedStyle: (node) => (typeof window.getComputedStyle === "function" ? window.getComputedStyle(node) : null),
  closeSpeedPopover: () => closeSpeedPopover(),
});
const { noteKeyboard, noteKeyboardReader, settleOnScroll, showKeyboardDebug, renderKeyboardDebug } = screenKeyboard;
screenKeyboardRoom = screenKeyboard;


// --- the keyboard's numbers, on screen -----------------------------------------------------------------
//
// `#610 mobile-keyboard-compose`. Where a phone's keyboard leaves the message box depends on numbers only the
// phone has: what its browser says the window, the visual viewport and its pan are, and where the box and
// its list are drawn. No phone is driven where this page is tested, so Settings › Screen and keys ›
// Typing › "Keyboard layout debug" draws them over the top of the page, live, for a screenshot to carry
// back. Off by default, kept on this device alone.

/** What was stored. ABSENT MEANS OFF. */
function storedKeyboardDebug() {
  // A browser refusing storage reads as off (`DEVICE_SETTINGS.keyboardDebug.refused`).
  return deviceSettings().get(DEVICE_SETTINGS.keyboardDebug);
}

/** The reader's choice, kept on this device. @param {boolean} on */
function applyKeyboardDebug(on) {
  try {
    deviceSettings().keep(DEVICE_SETTINGS.keyboardDebug, on);
  } catch (_error) {
    // A browser that refuses storage still draws it for this session.
  }
  showKeyboardDebug(on);
}

// --- what moved a list, kept for the log -------------------------------------------------------------------
//
// `#1144 scrollback-typing-jump`. A list found somewhere the reader never put it says nothing about what
// moved it, and the numbers that would are only on the phone it happened on. Settings › Screen and keys ›
// Typing › "Scroll trace" keeps the last minute of each list's scrolls, with the code that asked for each
// (web/src/view/channel/scrolltrace.ts), and "Send scroll trace" beside it posts that minute to the
// server's log (`POST /api/v1/scroll-trace`). Off by default, kept on this device alone, and nothing is
// sent until Send is pressed.

/**
 * The first row the reader could see in `area`, named for the trace: its first message's id, and how
 * far below the list's top it stands.
 *
 * @param {HTMLElement} area
 * @returns {import("./src/view/channel/index.ts").ScrollTraceAnchor | null}
 */
function scrollTraceAnchor(area) {
  const row = scrollAnchor(area);
  if (!row) return null;
  const ids = idsOf(row);
  return { id: ids[0] || row.id || "", offset: row.getBoundingClientRect().top - area.getBoundingClientRect().top };
}

const scrollTrace = createScrollTrace({
  lookup: (id) => leftEl(id),
  activeElement: () => document.activeElement,
  viewport: () => window.visualViewport || null,
  anchor: (area) => scrollTraceAnchor(area),
  elementPrototype: () => Element.prototype,
  now: () => performance.now(),
});

/** What was stored. ABSENT MEANS OFF. */
function storedScrollTrace() {
  // A browser refusing storage reads as off (`DEVICE_SETTINGS.scrollTrace.refused`).
  return deviceSettings().get(DEVICE_SETTINGS.scrollTrace);
}

/** The reader's choice, kept on this device, its switch set to match. @param {boolean} on */
function applyScrollTrace(on) {
  try {
    deviceSettings().keep(DEVICE_SETTINGS.scrollTrace, on);
  } catch (_error) {
    // A browser that refuses storage still traces for this session.
  }
  el("scroll-trace").checked = on;
  if (on) scrollTrace.start();
  else scrollTrace.stop();
}

/** Post the minute the trace holds to the server's log, and say on the status line how it went. */
async function sendScrollTrace() {
  try {
    // `report` says the trace as one JSON text; `api` carries a body it writes as JSON itself, as it
    // carries the call-end record's.
    await api("/api/v1/scroll-trace", { method: "POST", body: JSON.parse(scrollTrace.report()) });
    setStatus("Scroll trace sent.");
  } catch (_error) {
    setStatus("Scroll trace could not be sent.");
  }
}

el("scroll-trace").addEventListener("change", () => applyScrollTrace(el("scroll-trace").checked));
el("scroll-trace-send").addEventListener("click", () => {
  void sendScrollTrace();
});
// As stored, so a reload with the trace on loses none of the minute it is on for.
applyScrollTrace(storedScrollTrace());

// Drawing the readout and its switch, and keeping it redrawn while it is drawn, are the keyboard owner's
// (`showKeyboardDebug`, `renderKeyboardDebug`, above).

// The keyboard's and the columns' listeners (`#429 keyboard-hides-bar`, `#575`/`#610
// mobile-keyboard-compose`, `#340 composer-grow-box`, `#846 draft-chip-honest`): the window resized,
// the visual viewport over the keyboard, the lists scrolled in the jump's moment, the reader's touch,
// the debug readout's switch, the keyboard's geometry, and the focus once its tap is over — then the
// first measure. One owner in web/src/view/composer/keyboardlisteners.ts (`#624
// voice-page-modularity`), mounted here, in the page's order; its `unmount` waits for the ordered
// boot's teardown.
application.track(mountKeyboardListeners({
  lookup: (id) => el(id),
  leftLookup: (id) => leftEl(id),
  windowTarget: () => (typeof window.addEventListener === "function" ? window : null),
  visualViewport: () => (window.visualViewport && typeof window.visualViewport.addEventListener === "function" ? window.visualViewport : null),
  virtualKeyboard: () => (navigator.virtualKeyboard && typeof navigator.virtualKeyboard.addEventListener === "function" ? navigator.virtualKeyboard : null),
  columnsResizeAsk: () => columnsResize.ask(),
  noteKeyboard: () => noteKeyboard(),
  noteKeyboardReader: () => noteKeyboardReader(),
  settleOnScroll: () => settleOnScroll(),
  applyKeyboardDebug: (on) => applyKeyboardDebug(on),
  keyboardDebugChecked: () => el("keyboard-debug").checked,
  showKeyboardDebug: () => showKeyboardDebug(storedKeyboardDebug()),
  afterThisTask: (act) => { setTimeout(act, 0); },
}));
// Before the first channel read, so the very first list of rows is already filtered rather
// than appearing unfiltered for a frame and then rearranging under the reader.
whoIsWho.load();
identityList.render();
renderChannelRows();
for (const [key, box] of NEWLINE_KEYS) wireNewlineKey(leftEl(key), leftEl(box));
// `#846 draft-chip-honest`. A reply open in the stream scrolls on and off screen with its list: the chip
// follows, once a frame.
// Redrawn only when the reply crosses its list's edge (`replySight`), not on every frame of a scroll.
const draftChipFrame = createOncePerFrame(() => {
  const now = replyState.docked && inSight(el("reply-text"), replyPane().scroller());
  if (!replySight.crossed(now)) return;
  renderDraftChip();
}, () => (typeof requestAnimationFrame === "function" ? requestAnimationFrame : null));
// The reply box's listeners (`#410 inline-reply-editor`, `#846 draft-chip-honest`, `#225 enter-to-send`,
// `#185 reply-new-thread`, `#314 reply-focus-flake`): Cancel and its discard question, the lists' scrolls,
// Send and the box's keys, what is pasted in, the tick and the words kept as they are typed, the focus
// leaving, and the box's height fitted again. One owner in web/src/view/channel/replybox.ts (`#624
// voice-page-modularity`), mounted here, in the page's order; its `unmount` waits for the ordered boot's
// teardown.
application.track(mountReplyBox({
  lookup: (id) => el(id),
  leftLookup: (id) => leftEl(id),
  canWatchSizes: () => typeof window.ResizeObserver === "function",
  sizeWatcher: (report) => new window.ResizeObserver(() => report()),
  cancelReply: () => cancelReply(),
  answerReplyDiscard: (discard) => answerReplyDiscard(discard),
  draftChipAsk: () => {
    if (replyState.docked) draftChipFrame.ask();
  },
  sendReply: guardQuietly(sendReply),
  onBoxKey: (event) => onComposerKey(event, guardQuietly(sendReply), el("reply-text")),
  watchBoxInsertions: (box) => watchBoxInsertions(box),
  rememberDraft: () => rememberDraft(),
  renderReplyDestination: () => renderReplyDestination(),
  noteFocusLeaving: (event) => noteReplyFocusLeaving(event),
  noteFocusIn: () => {
    replyState.focusTaken = false;
  },
  fitInlineRoom: () => fitInlineRoom(),
  pressViewSwitch: () => pressViewSwitch(),
}));

/** The view switch, pressed: rotate Voice -> Channel -> Documents -> Voice (`documents-view-client`). */
function pressViewSwitch() {
  // The selector owns the rotation and, through `chooseMajorView`, each view's
  // entry work: entering the channel re-reads it (keeping the position on a
  // return, as the two-way switch did), and any other view stops its polling.
  documentsSelector.cycle();
}

/** Read the channel just entered, and keep reading it while it stays on screen. */
function readEnteredChannel(keepPosition) {
  return guardQuietly(async () => {
    try {
      // A return finds the channel's store as it was left, so it asks only for what changed.
      const since = replayLedger.started();
      await timelineView.loadDiscord(keepPosition ? { keepPosition: true, reason: "visible" } : { reason: "enter" });
      // ...and the right column's thread, or its list of threads. `#229 desktop-two-column`.
      await readSideColumn(keepPosition ? "visible" : "enter", since);
    } finally {
      // Armed after a failure too: offline is exactly when the reader needs the page to try again.
      if (screens.view() === "discord") {
        scheduleDiscordPoll();
      }
    }
  })();
}
// Both wrapped in a lambda rather than passed straight through: a DOM listener is handed the
// EVENT as its first argument, and `loadDiscord`'s first argument is its options object. Reading
// `keepPosition` off a MouseEvent happens to answer false, which is the right answer by accident
// and stops being right the moment another option is added.
//
// Refresh keeps your place. It is a re-read of the channel you are already looking at, and being
// thrown to the bottom of it is the same defect as a background poll doing so — the fact that you
// asked for fresh messages is not a request to stop reading the one in front of you. A reader who
// was at the bottom still follows, which is the case where "keep my place" and "show me the
// newest" are the same instruction.
// Changing channel is not a re-read; it is a different history, and its bottom is where to start.
// The walk back resets with it: a cursor from one channel means nothing in another, and carrying
// one across would ask the server to step back from a message that is not there.
function changeSelectedChannel() {
  // From the left column, whichever column the key that asked for it was in. `#229 desktop-two-column`.
  if (columnWork.side()) return inColumn(columns.main, changeSelectedChannel);
  noteDeepLinkReaderMove();
  // `#277 reply-in-column`. A reply open in a column answers a message of the channel being left.
  putDockedReplyAway();
  // `#215 reply-coalesce`. Gathered replies belong to the channel they were gathered in, whose rows
  // are about to go — and so does a tap on an arrow still waiting to jump, which would otherwise walk
  // this channel's history for a message of the last one.
  gatherState().gathered = null;
  listState().rowsGathered = false;
  dropReplyArrowTap();
  applyChannelProvider();
  // This window's, and the one a new window opens on. `#265 window-channel-memory`.
  rememberActiveChannel(true);
  rememberChannelDraft();
  // `#205 channel-view-memory`. Another channel opens in the view the reader chose there, and in ITS
  // default — All where it has threads — when they chose none: a choice made in the last channel is
  // that channel's, and it waits there for the reader to come back. The first read is the view's.
  uiState.settle();
  openChannelView();
  threadPresence.forget();
  timelineRows().messages = [];
  timelineRows().threads = [];
  channelContexts.clear();
  // `#396 thread-color-memory`. The channel's own colours, as this device last drew them.
  restoreThreadHues(selectedChannelId());
  // `#427 server-thread-hues`: and offered to the server, as at sign-in — issued before this
  // channel's first read below, not awaited by it.
  void seedServerThreadHues(selectedChannelId());
  el("thread-list").replaceChildren();
  restoreChannelComposer();
  olderPages().moreAbove = false;
  olderPages().cursor = null;
  channelReads.payCursor();
  listState().newestId = null;
  el("discord-log").replaceChildren();
  // The summary goes with the rows it is about. It is written only inside `loadDiscord`, which
  // THROWS when the read fails — so leaving it standing means a failed change of channel shows
  // the previous channel's name and a count of messages that are no longer on the screen, which
  // is the most confident possible way of being wrong.
  el("channel-summary").replaceChildren();
  renderOlderControl();
  loadState().freshAt = 0;
  loadState().freshness = "fresh";
  // `#18 offline-message-cache`. The new channel as this device last saw it, until the read below
  // answers — which then merges into those rows rather than replacing them.
  const saved = hydrateChannelScope();
  renderChannelFreshness();
  if (saved) scrollToNewest();
  saveUiState();
  // `#206 pin-message`. Under the Pinned filter the new channel's pins are what the reader is about
  // to look at, so they are read now rather than after its first read reports their revision.
  if (pinsView().only) {
    ensurePins();
    renderChannelRows();
  }
  // `#229 desktop-two-column`. The right column shows this channel's thread selected earlier in
  // the session, or its cards; and a channel without threads is one column.
  resetSideColumn(true);
  renderColumns();
  // A stream follows ONE channel, and a cursor from the old one means nothing in the new one —
  // the same reason the walk-back cursor is dropped two lines above.
  startChannelStream(selectedChannelId());
  return timelineView.loadDiscord(saved && screens.view() === "discord" ? { keepPosition: true, reason: "enter" } : { reason: "enter" });
}
// The channel box's listeners (`#349 channel-settings-all`, `#355 provider-grouped-channels`, `#232
// upstream-mark-read`, `#602 history-settings-and-limits`, `#39 channel-alias`, `#208
// persistent-cache`): the pickers (a channel chosen is the selection's first, before anything that
// reads it hears of the change; Ctrl+K's word after the change has started, `#224`), Clear unread and
// History, Rename and Add and Browse, Remove and the alias, the noise rules, the voice agent's
// editors and prefix, and the message cache. One owner in
// web/src/view/settings/channelboxlisteners.ts (`#624 voice-page-modularity`), mounted here, in the
// page's order; its `unmount` waits for the ordered boot's teardown. The guarded acts are evaluated
// here, at mount, as they were.
application.track(mountChannelBoxListeners({
  lookup: (id) => el(id),
  selectChannel: () => selectionOwner().selectChannel(el("discord-channel").value),
  changeSelectedChannel: guardQuietly(changeSelectedChannel),
  channelPicked: () => channelPicked(),
  chooseSettingsService: () => channelSettings.chooseSettingsService(el("settings-service").value),
  changeSettingsChannel: () => {
    channelSettings.showChannelPanel(null);
    // Everything in this box is about the channel the picker is on, or about all of them
    // (`#349 channel-settings-all`).
    channelSettings.renderChannelBox();
  },
  renderClearChoice: () => upstreamClearBox.renderChoice(),
  saveClear: () => upstreamClearBox.save(),
  renderHistoryChoice: () => channelHistoryBox.renderChoice(),
  saveHistory: () => channelHistoryBox.save(),
  prioritizeHistory: () => channelHistoryBox.prioritize(),
  pushClear: () => upstreamClearBox.push(),
  openRename: () => {
    channelSettings.showChannelPanel("rename-fields");
    el("channel-alias").focus();
  },
  cancelRename: () => {
    // Back to what the server says, not to what was typed. Cancel means the typing did not happen.
    channelSettings.renderAliasEditor();
    channelSettings.showChannelPanel(null);
  },
  openAddChannel: () => {
    channelSettings.showChannelPanel("add-channel-fields");
    el("new-channel-id").focus();
  },
  cancelAddChannel: () => {
    el("new-channel-id").value = "";
    el("new-channel-label").value = "";
    el("new-channel-writable").checked = false;
    el("add-channel-state").textContent = "";
    channelSettings.showChannelPanel(null);
  },
  addChannel: guardQuietly(channelSettings.addChannel),
  openBrowse: guardQuietly(() => channelDirectoryBox.open()),
  closeBrowse: () => channelSettings.showChannelPanel(null),
  scheduleDirectorySearch: () => channelDirectoryBox.scheduleSearch(),
  directorySearchKey: (event) => {
    if (event && event.key === "Enter") {
      if (typeof event.preventDefault === "function") event.preventDefault();
      guardQuietly(() => channelDirectoryBox.search())();
    }
  },
  directoryMore: guardQuietly(() => channelDirectoryBox.load(true)),
  directoryRetry: guardQuietly(() => channelDirectoryBox.retry()),
  removeChannel: guardQuietly(channelSettings.removeChannel),
  saveAlias: guardQuietly(channelSettings.saveAlias),
  addNoiseRule: guardQuietly(noiseSettings.add),
  saveSendSetting: guardQuietly(agentSettings.saveSendSetting),
  renderPrompt: () => agentSettings.renderPrompt(),
  saveInstructions: guardQuietly(() => agentSettings.savePrompt("instructions", el("voice-instructions").value)),
  saveSystemPrompt: guardQuietly(() => agentSettings.savePrompt("system_prompt", el("voice-system-prompt").value)),
  restoreSystemPrompt: guardQuietly(agentSettings.restoreSystemPrompt),
  renderPrefix: () => agentPrefix.render(),
  savePrefix: guardQuietly(() => agentPrefix.save(false)),
  resetPrefix: guardQuietly(() => agentPrefix.save(true)),
  cacheSwitched: guardQuietly(() => serverCacheBox.switched()),
  askErase: () => serverCacheBox.ask("erase"),
  answerEraseYes: guardQuietly(() => serverCacheBox.answer(true)),
  answerEraseNo: guardQuietly(() => serverCacheBox.answer(false)),
  noiseRuleKey: (event) => {
    if (event.key === "Enter") guardQuietly(noiseSettings.add)();
  },
  clearAlias: guardQuietly(channelSettings.clearChannelAlias),
}));
// The list's tools (`#65`, `#262 search-load-older`, `#605`-`#608 server-side-search`, `#221 read-modes`, `#564
// message-summary-toggle`, `#820 voice-page-action-menu`, `#880 voice-page-menu-placement`): the ways further back,
// the search's ways wider, the action menu's items closing it after their own acts, and the chips over the list. One
// owner in web/src/view/channel/listcontrols.ts (`#624 voice-page-modularity`), mounted here, in the page's order; its
// `unmount` waits for the ordered boot's teardown.
application.track(mountListControls({
  byId: (id) => el(id),
  leftById: (id) => leftEl(id),
  loadOlder: () => guardQuietly(olderControlPressed)(),
  searchInAll: () => guardQuietly(searchInAll)(),
  serverMore: () => guardQuietly(() => serverSearchOwner.more())(),
  searchAllChannels: (on) => chooseSearchAllChannels(on),
  searchFurther: () => guardQuietly(searchFurtherPressed)(),
  loadOlderTurns: () => guardQuietly(voiceTranscript().loadOlderTurns)(),
  setAllFolded: (folded) => setAllFolded(folded),
  cycleReadMode: () => cycleReadMode(),
  clearBacklog: () => guardQuietly(clearBacklog)(),
  undoDismissal: () => guardQuietly(undoDismissal)(),
  toggleSummaries: () => applySummarizeCollapsed(!storedSummarizeCollapsed()),
  readerHidden: () => readerRow.hidden(),
  setReaderHidden: (hidden) => readerRow.setHidden(hidden),
  autoplayOn: () => autoplay.on(),
  setAutoplay: (on) => autoplay.setOn(on),
  closeSpeedPopover: () => closeSpeedPopover(),
  focusInActionMenu: () => nodeWithin(el("action-menu"), document.activeElement),
  closeActionMenu: () => setActionMenuOpen(false),
  jumpToNewest: () => scrollKeys.jumpToNewest(),
}));
// The search's and the filter chips' listeners (`#129 message-search`, `#206 pin-message`, `#211`/`#212
// link-filter`, `#251 author-filter`, `#266 link-filter-kinds`, `#620 link-url-chip`): the glass and the filters
// as toggles, each kind of link, and the search field's typing, Enter and Escape. One owner in
// web/src/view/channel/filterchips.ts (`#624 voice-page-modularity`), mounted here, in the page's order; its
// `unmount` waits for the ordered boot's teardown.
application.track(mountFilterChips({
  byId: (id) => el(id),
  searchOpen: () => searchField.open(),
  setSearchOpen: (open) => setSearchOpen(open),
  pinnedOnly: () => pinsView().only,
  setPinnedOnly: (on) => setPinnedOnly(on),
  linksOnly: () => searchField.links(),
  setLinksOnly: (on) => setLinksOnly(on),
  cycleAuthorFilter: () => cycleAuthorFilter(),
  linkKindHidden: (kind) => linkKinds.hidden().has(kind),
  setLinkKindShown: (kind, shown) => setLinkKindShown(kind, shown),
  linksRaw: () => linkKinds.raw(),
  setLinksRaw: (raw) => setLinksRaw(raw),
  keepingFilterPlace: (change) => keepingFilterPlace(change),
  liftingHideRead: (change) => liftingHideRead(change),
  setQuery: (text) => searchField.setQuery(text),
  query: () => searchField.query(),
  setMarked: (text) => searchField.setMarked(text),
  searchTyped: (text) => searchTyped(text),
  renderEveryColumnsTools: () => eachColumn(renderScrollTools),
  sideColumnUp: () => sideColumnUp(),
  refilterSideCards: () => refilterSideCards(),
  markSearchSoon: () => markSearchSoon(),
  markSearchNow: () => markSearchNow(),
  searchOlderFromField: () => searchOlderFromField(),
  settleFocus: () => settleFocus(),
  swallow: (event) => swallow(event),
}));

// Watching the lists move (`#207 scrollback-jump`, `#245 done-feedback`, `#204 reply-arrow`, `#49 cached-summaries`,
// `#340 composer-grow-box`, `#261 two-column-connectors`, `#229 desktop-two-column`, `#338 column-overscroll`):
// what a scroll of Main's list asks for, and what a change of size of either list asks for once the browser has
// finished telling of it. One owner in web/src/view/channel/listwatch.ts (`#624 voice-page-modularity`), mounted
// here, in the page's order; its `unmount` waits for the ordered boot's teardown.
/**
 * Watching sizes, as the page offers it to the mounts that watch the lists: a ResizeObserver, where the browser
 * has one (web/src/view/dom `SizeWatching`).
 *
 * @type {import("./src/view/dom/index.ts").SizeWatching}
 */
const sizeWatching = {
  canWatchSizes: () => typeof window.ResizeObserver === "function",
  sizeWatcher: (report) => new window.ResizeObserver(() => report()),
};
application.track(mountListWatch({
  ...sizeWatching,
  byId: (id) => el(id),
  leftById: (id) => leftEl(id),
  afterThisTask: (act) => { setTimeout(act, 0); },
  placeChipsOverComposer: () => placeChipsOverComposer(),
  renderJumpNewest: () => renderJumpNewest(),
  replyBackOffered: () => replyArrows().back !== null,
  replyReturnRow: () => replyReturnRow(),
  dropReplyBack: () => { replyArrows().back = null; },
  rowOnScreen: (row) => rowOnScreen(row),
  renderScrollTools: () => renderScrollTools(),
  maybeLoadOlder: () => maybeLoadOlder(),
  maybeLoadOlderTurns: () => voiceTranscript().maybeLoadOlderTurns(),
  requestVisibleSummaries: () => requestVisibleSummaries(),
  fitGrowingBoxes: () => fitGrowingBoxes(),
  scheduleColumnLinks: () => scheduleColumnLinks(),
  fitOverscroll: (area) => fitOverscroll(area),
  sideShowing: () => sideShowing(),
  renderSideJumpNewest: () => inColumn(columns.side, renderJumpNewest),
}));
/**
 * How much stands under the main screen's lists, for the dock-growth keepers below: the dock, and the
 * send card while it is docked under the thread list (`#269 approval-card-inline` x `#281 view-keys`).
 * Off the main screen the card is docked too, but there the lists are not drawn and nothing is kept.
 */
function heightUnderLists() {
  const card = leftEl("post-confirm");
  const docked = screens.screen() === "main" && !card.hidden && card.getAttribute("data-place") === "dock";
  return (el("dock").offsetHeight || 0) + (docked ? card.offsetHeight || 0 : 0);
}

/**
 * What the dock-growth keepers of Main's list and of the right column (below) noted at their last reports:
 * each list's height and `heightUnderLists`, and Main's reader's distance from its foot as a send card came
 * up docked under the thread list (`cardCameUnderLists`). One owner in web/src/view/channel/underlists.ts
 * (`#624 voice-page-modularity`).
 */
const underLists = createUnderLists();

/**
 * `#280 scroll-stability` x `#281 view-keys`. The send card moved into the dock under the thread list
 * or out of it, changing `heightUnderLists` by `px`: told to Main's keeper as no growth at all, since
 * the card left the foot of Main's list, or came back to it, in the same move (`placePostCard`). The
 * right column's keeper is not told: its cards never held the card, and a move into the dock is the
 * dock growing under them, except where the change of layout that moves it puts the cards back where
 * the reader had them in the thread list (`cardsPlacedUnderLists`).
 *
 * @param {number} px
 */
function cardMovedUnderLists(px) {
  underLists.cardMoved(px);
}

/**
 * `#280 scroll-stability` x `#281 view-keys`. The send card came up docked under the thread list, Main's
 * reader `gap` px from the foot of its list just before (`placePostCard`). Told to Main's keeper, which
 * judges whether that reader was at the foot by `gap` at its next report, rather than by how near the
 * foot they are left: the box at the list's foot shrank in the same layout as the dock grew.
 *
 * @param {number} gap
 */
function cardCameUnderLists(gap) {
  underLists.cardCame(gap);
}

/**
 * `#280 scroll-stability` x `#269 approval-card-inline` x `#281 view-keys`. The cards were just put back
 * where the reader had them in the thread list, on the way back to it from two columns (`moveLayout`,
 * `layoutShown.holdListCards`), in their column as it now stands, with everything now under it: told to the right
 * column's keeper, which measures what grows under the cards from there. The send card that change moved
 * into the dock under the list, and the picker's new name if it wrapped the dock, are not the dock growing
 * under a reader at the foot of two columns' cards. Counted as growth, they took anyone within the slack
 * plus the card's height of the list's foot for a reader at it: in Chromium at a desk, the long draft's
 * card 246px tall, a reader put back 160px up the list's cards was pulled to its foot.
 */
function cardsPlacedUnderLists() {
  underLists.noteSide({ list: leftEl("side-scroll").clientHeight, dock: heightUnderLists() });
}
// The keepers of the lists' readers (`#218 desktop-dock`, `#229 desktop-two-column`, `#269 approval-card-inline` x
// `#281 view-keys`, `#280 scroll-stability`, `#338 column-overscroll`, `#214 reply-arrow`): a reader on the newest
// line kept on it as the dock grows under Main's list or the right column's, the right column's reader kept on its
// newest line as what is in it changes size, and the reply arrows measured again as layout changes. One owner in
// web/src/view/channel/listkeepers.ts (`#624 voice-page-modularity`), mounted here, in the page's order; its
// `unmount` waits for the ordered boot's teardown.
const listKeepersMount = mountListKeepers({
  ...sizeWatching,
  el: (id) => el(id),
  leftEl: (id) => leftEl(id),
  screens,
  underLists,
  cardsDrawn,
  columns,
  layoutShown: { columns: () => layoutShown.columns() },
  bottomSlackPx: BOTTOM_SLACK_PX,
  heightUnderLists: () => heightUnderLists(),
  quickReplyHolds: (area) => quickReplyHolds(area),
  placeQuickReply: () => placeQuickReply(),
  entryLandingHolds: () => entryLandingHolds(),
  entryState: () => entryState(),
  placeEntryLanding: (row, first) => placeEntryLanding(row, first),
  keepFilterHold: () => keepFilterHold(),
  heldFilterRow: () => heldFilterRow(),
  searchWalkAnchored: () => {
    const walk = columnWork.active().searchWalk;
    return Boolean(walk && walk.anchored);
  },
  voiceCall: () => voiceCall(),
  keepWrittenBox: (area, seen, now) => keepWrittenBox(area, seen, now),
  writingIn: (area) => boxBeingWritten(area) !== null,
  footTop: (area) => footTop(area),
  atBottom: (...args) => atBottom(...args),
  followFoot: (area) => followFoot(area),
  sideColumnUp: () => sideColumnUp(),
  sideShowing: () => sideShowing(),
  inColumn: (column, act) => inColumn(column, act),
  fitOverextend: (area) => fitOverextend(area),
  holdOverscrollReader: (area) => holdOverscrollReader(area),
  channelContextKey: () => channelContextKey(),
  pinsView: () => pinsView(),
  measureReplyArrows: () => measureReplyArrows(),
});
mainFollowState = () => listKeepersMount.followingMain();
application.track(listKeepersMount);
// The lists' gestures and scrolls (`#68 pull-to-refresh`, `#188 pull-refresh-bottom`, `#261 two-column-connectors`,
// `#338 column-overscroll`, `#460 pack-gap-slide`): the pull on Main's list, each column's scroll moving the links'
// ends, the wheel at the right column's foot, the reader's own wheel or key ending a slide, and the room past the
// foot following the scroll, every listener on a list passive. One owner in web/src/view/channel/listgestures.ts
// (`#624 voice-page-modularity`), mounted here, in the page's order; its `unmount` waits for the ordered boot's
// teardown.
application.track(mountListGestures({
  byId: (id) => el(id),
  leftById: (id) => leftEl(id),
  pullStart: (event) => pullStart(event),
  pullMove: (event) => pullMove(event),
  pullEnd: () => guardQuietly(pullEnd)(),
  pullCancel: () => pullCancel(),
  scheduleColumnLinks: () => scheduleColumnLinks(),
  footWheel: (event) => footRooms.wheel(event),
  footScrolled: (timeStamp) => footRooms.scrolled(timeStamp),
  finishPackSlide: () => finishPackSlide(),
}));

/**
 * Like `guard`, but for things that are not a call: it reports, and it does NOT tear down a live
 * conversation. Reading the channel list must never be able to hang up on the owner.
 */
function guardQuietly(fn) {
  return (...args) =>
    Promise.resolve(fn(...args)).catch((error) => {
      showError(error.message);
    });
}

/**
 * Like `guardQuietly`, for a re-read of the channel nobody asked for: the poll, the live feed's
 * catch-up. `#195 send-resilience`.
 *
 * Its failure is already said over the rows it is about, by the freshness pill — "Refresh failed ·
 * showing messages from 10:17". The banner on top of that was the same news a second time, in the
 * server's diagnostic words, back on every poll for as long as the service stayed slow, and it
 * read as though something the reader did had failed. The banner stays for what the reader asked
 * for, and for a failure with no rows on screen to carry it — reworded, never raw.
 */
function refreshQuietly(fn) {
  return (...args) =>
    Promise.resolve(fn(...args)).catch((error) => {
      if (error && error.shownInPill) return;
      showError(readableChannelFailure(error));
    });
}

// The boot, mounted and started in the page's order (`createApplication`, above): the mounts
// were tracked as the wiring mounted them, and the start acts are the first drawing and sign-in.
application.mount();
application.start();
