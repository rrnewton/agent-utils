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
/** `#1321 after-done-setting`. What e does after marking Done: "stay", "next" or "unread". */
const AFTER_DONE_KEY = DEVICE_SETTINGS.afterDone.key;
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
const { loadState, scrollPlace, scrollTools, selection, timelineRows, composerState, viewKey } = createColumnStateAccess(columnWork);
const { el, leftEl } = createElementLookup(columnWork);

/**
 * The page's way to the browser and the server: web/src/app's now (`createServerAccess`, `#624`),
 * made here, before its first use; everything it holds is still made on first use, as the `var`s were.
 */
const serverAccess = createServerAccess({
  readRefused: () => messageCache.dropMessageCache(),
  noteAsked: () => readOrder.noteAsked(),
  chatServiceOf: (channel) => chatServiceOf(channel),
  selectedChannelId: () => selectedChannelId(),
});
const { browserPorts, deviceStorage, deviceStorageOnUse, clockOnUse, windowStorage, deviceSettings, credentialStore, token, signedOutByRename, theServer, api, apiDecoded, within } = serverAccess;


/**
 * `#349 channel-settings-all`. The value of Settings' channel picker's first choice, All channels:
 * not an id any chat service gives a channel. Up here because the picker is filled during start-up.
 */
const ALL_CHANNELS = "*";


/** The channel chosen to read: web/src/view/shell's now (`createChannelSelection`, `#624`). */
const channelSelection = createChannelSelection({
  createNavigation,
  picker: () => el("discord-channel"),
  channels: () => channelDirectory.channels(),
});
const { selectionOwner, selectedChannelId, chooseChannel } = channelSelection;



/** Which deployment's origin and host, and redacting its token: web/src/view/shell's now (`createDeploymentWords`, `#624`). */
const deploymentWords = createDeploymentWords({
  token: () => token(),
  tokenField: () => el("api-token"),
});
const { deploymentOrigin, deploymentHost, redact } = deploymentWords;


for (const id of ["signin-server-url", "settings-server-url"]) {
  el(id).textContent = deploymentOrigin();
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
    if (pinned) scrollKeeping.toFoot(area);
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
    if (pinned) scrollKeeping.toFoot(area);
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
/** The page's quiet guards: web/src/app/guards.ts (`createPageGuards`, #624). */
const pageGuards = createPageGuards({
  showError,
  readableChannelFailure: (error) => readableChannelFailure(error),
});
const { guardQuietly, refreshQuietly } = pageGuards;

// The state dot (`setState`): one of idle, working, live, unresponsive, ended, suspended, error, which
// web/voice.css colours. `suspended` is `#54 resume-recovery`: the socket died while the page was in the
// background, neither `ended` (the reader chose to hang up) nor `error` (something is broken). `unresponsive`
// is `voice-unresponsive-signal`: the socket is open and the call may recover, but the service has stopped
// producing anything audible or readable.


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
    get discussOpen() { return discussView.isOpen(); },
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
    get openCallMicrophone() { return /** @type {any} */ (openCallMicrophone); },
    get echoCancellationRefused() { return voiceTranscript().echoCancellationRefused; },
    get browserPorts() { return browserPorts; },
    get applyCallOutput() { return /** @type {any} */ (applyCallOutput); },
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
  intoView: (element, block, inline) => scrollKeeping.intoView(element, block, inline),
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
    // `#1388` landing 1: read-aloud's marks restate an open summary's strike.
    get dismissMessages() {
      const raw = dismissMessages;
      return (body) => {
        const run = raw(body);
        void Promise.resolve(run).then(refreshDiscussStrikes, refreshDiscussStrikes);
        return run;
      };
    },
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
  cycleView: () => documentsSelector.cycle(),
  loadDiscord: (options) => timelineView.loadDiscord(options),
  readSideColumn: (reason, since) => readSideColumn(reason, since),
  scheduleDiscordPoll: () => scheduleDiscordPoll(),
  putDockedReplyAway: () => putDockedReplyAway(),
  gatherState: () => gatherState(),
  listState: () => listState(),
  dropReplyArrowTap: () => dropReplyArrowTap(),
  applyChannelProvider: () => applyChannelProvider(),
  rememberActiveChannel: (chosen) => rememberActiveChannel(chosen),
  rememberChannelDraft: () => rememberChannelDraft(),
  openChannelView: () => openChannelView(),
  forgetThreadPresence: () => threadPresence.forget(),
  timelineRows: () => timelineRows(),
  clearChannelContexts: () => channelContexts.clear(),
  restoreThreadHues: (channel) => restoreThreadHues(/** @type {any} */ (channel)),
  seedServerThreadHues: (channel) => seedServerThreadHues(/** @type {any} */ (channel)),
  restoreChannelComposer: () => restoreChannelComposer(),
  olderPages: () => olderPages(),
  payReadCursor: () => channelReads.payCursor(),
  renderOlderControl: () => renderOlderControl(),
  loadState: () => loadState(),
  hydrateChannelScope: () => hydrateChannelScope(),
  renderChannelFreshness: () => renderChannelFreshness(),
  saveUiState: () => saveUiState(),
  pinsOnly: () => pinsView().only,
  ensurePins: () => ensurePins(),
  renderChannelRows: () => renderChannelRows(),
  resetSideColumn: (reopen) => resetSideColumn(reopen),
  renderColumns: () => renderColumns(),
  startChannelStream: (channel) => startChannelStream(/** @type {any} */ (channel)),
  viewScroll: () => viewScroll,
  atBottom: (area) => atBottom(area),
  viewMark: (name) => viewMarks.get(name),
  setViewMark: (name, mark) => viewMarks.set(name, mark),
  holdChannelPlace: () => holdChannelPlace(),
  placePostCard: () => voiceTranscript().placePostCard(),
  syncDocumentsSelector: () => documentsSelector.sync(),
  syncDiscussView: () => discussView.sync(),
  forgetJumpNewest: (name) => { jumpNewestWanted[name] = false; },
  dismissStatus: () => dismissStatus(),
  readAloudOwner: () => readAloudOwner(),
  renderCallControls: () => voiceCall().renderControls(),
  renderControlBar: () => renderControlBar(),
  renderChannelNavigation: () => renderChannelNavigation(),
  placeChipsOverComposer: () => placeChipsOverComposer(),
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
  noteJumpAnchor: (row, id) => jumpAnchor.note(/** @type {HTMLElement} */ (row), id),
  cardsCycle: () => cardsCycle(),
  rowShown: (row) => rowShown(/** @type {any} */ (row)),
  visibleList: () => visibleList(),
  rowOnScreen: (row) => rowOnScreen(row),
  eachColumn: (act) => eachColumn(act),
  openReadRun: (row) => openReadRun(/** @type {any} */ (row)),
  setSearchOpen: (open) => setSearchOpen(open),
  finishPackSlide: () => finishPackSlide(),
  headOfList: (area, row) => headOfList(area, row),
  toPosition: (area, top) => scrollKeeping.toPosition(area, top),
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
  putBackOnSpot, pressViewSwitch, readEnteredChannel, changeSelectedChannel, showView,
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
    // `#1224 mention-jump-in-view`. The reader put the mark out: the jump's hold on its row goes too.
    jumpAnchor.clear();
    const focus = deepLinks.focus();
    if (!focus || String(focus.message) !== linked.id || String(focus.channel) !== linked.channel) return;
    deepLinks.setFocus(null);
    syncDeepLinkAddress();
  },
});

// `#1224 mention-jump-in-view`. The row a jump landed on, held as the scroll anchor while what
// arrives after it changes height above it (web/src/view/channel `createJumpAnchor`): asked first
// by the scroll keeping's capture once mu-stable's shared mark lands, placed again by the lists'
// keepers, and the trace told through mu-scroll's `noteJumpLost` (`#1208`) where a landing is
// pushed off screen inside its window with no hand in it.
/** @type {import("./src/view/channel/index.ts").JumpAnchor} */
const jumpAnchor = createJumpAnchor({
  el: (id) => el(id),
  contextKey: () => channelContextKey(),
  rowById: (id) => rowHolding(id),
  listPlaceTop: (area) => listPlaceTop(area),
  now: () => performance.now(),
  clock: {
    setTimeout: (run, ms) => setTimeout(run, ms),
    clearTimeout: (handle) => clearTimeout(/** @type {any} */ (handle)),
  },
  nudge: (area, delta) => scrollKeeping.nudge(area, delta),
  noteJumpLost: (id) => scrollTrace.noteJumpLost(id),
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
  applyMarkReadOnReply: (on) => markReadOnReplySwitch.apply(on),
  applyMarkReadQuoteGuard: (on) => markReadQuoteGuardSwitch.apply(on),
  applyMarkReadFoldGuard: (on) => markReadFoldGuardSwitch.apply(on),
  applyMarkReadNonquote: (on) => markReadNonquoteSwitch.apply(on),
  applyMarkReadFinalGuard: (on) => markReadFinalGuardSwitch.apply(on),
  applyMarkReadFirstGuard: (on) => markReadFirstGuardSwitch.apply(on),
  applyMarkReadFirstPercent: (value) => markReadFirstPercent.apply(value),
  applyMarkReadLongWords: (value) => markReadLongWords.apply(value),
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
        guardQuietly(() => timelineView.loadDiscord({ keepPosition: true, ownAct: true, readAs: "mutation" }))();
      }
  },
  applyCombineMessages: (on) => combineSwitch.apply(on),
  regroupChannelRows: () => eachColumn(regroupChannelRows),
  chooseOpenInView: (view) => chooseOpenInView(view),
  storedOpenInView: () => storedOpenInView(),
  chooseAfterDone: (choice) => chooseAfterDone(choice),
  storedAfterDone: () => storedAfterDone(),
});

/** Which view Open in channel lands in, and the links redrawn for it: web/src/view/channel/openinview.ts (`createOpenInView`, #624). */
const openInViewOwner = createOpenInView({
  threading: () => providerFacts.threading(),
  eachColumn: (act) => eachColumn(act),
  el,
  renderCachedTimeline: () => renderCachedTimeline(),
  view: () => selection().view,
  threadCards: (threads) => threadCards(threads),
  threads: () => timelineRows().threads,
  renderSideColumn: () => renderSideColumn(),
  regroupChannelRows: (redraw) => regroupChannelRows(redraw),
  storedChoice: () => deviceSettings().get(DEVICE_SETTINGS.openInView),
  keepChoice: (view) => deviceSettings().keep(DEVICE_SETTINGS.openInView, view),
  renderChoice: () => messagePreferences.openInView.render(),
});
const { redrawMessageLinks, storedOpenInView, renderOpenInView, chooseOpenInView } = openInViewOwner;

/**
 * What e does after marking Done (`#1321 after-done-setting`): kept in Settings, read at each
 * press, so a synced change from another device applies without a reload. Choosing it also
 * says the keys again, since the list of keys words e by this choice.
 */
const storedAfterDone = () => deviceSettings().get(DEVICE_SETTINGS.afterDone);
const renderAfterDone = () => messagePreferences.afterDone.render();
const chooseAfterDone = (choice) => {
  try {
    deviceSettings().keep(DEVICE_SETTINGS.afterDone, choice);
  } catch (_error) {
    // A browser that refuses storage keeps the shipped default.
  }
  renderAfterDone();
  keyHints.render();
};




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
  intoView: (element, block, inline) => scrollKeeping.intoView(element, block, inline),
  nudge: (scroller, delta) => scrollKeeping.nudge(scroller, delta),
  toPosition: (scroller, top) => scrollKeeping.toPosition(scroller, top),
  toFoot: (scroller) => scrollKeeping.toFoot(scroller),
  api: (path, options) => api(path, options),
  byId: (id) => el(id),
  // Per-document starred/hidden state (`#1265 documents-star-hide`): kept in
  // device storage under a synced key, so the settings sync mirrors it.
  storage: () => deviceStorageOnUse(),
  closeActionMenu: () => setActionMenuOpen(false),
  readDocument: (doc, from) => readAloudOwner().readDocument(doc, from),
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
  // `#1505`: the reader opening or closing re-decides the control pane,
  // which is the document read's transport while the reader stands.
  onLevelChange: () => { shellNavigation.scheduleBackSync(); voiceCall().renderControls(); },
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

// --- discuss (`catchup-side-agent`) -------------------------------------------------------------
//
// The Discuss panel: a one-shot catch-up over the channel or thread on screen, in
// web/src/view/catchup. The page's whole share is this construction, tracking its
// controls with the boot, and `sync()` from `showView`; its scope, request and
// drawing are the view's. The Done acts it marks through are made below and read
// only inside these thunks, as the Documents view's are.

/** `#1388` landing 1. Restate an open summary's strike after any mark settles. */
function refreshDiscussStrikes() {
  try { discussView.refreshStrikes(); } catch { /* no panel yet */ }
}

/** The mounted Discuss view. */
const discussView = createDiscussView({
  byId: (id) => el(id),
  api: (path, options) => api(path, options),
  // No direct LLM summarizer, no Discuss: client-config's `discuss_available`,
  // held with the other server facts and read here only when drawn or pressed.
  discussAvailable: () => serverFacts.discussAvailable(),
  enterSends: () => enterSends.current(),
  selectedChannelId: () => String(selectedChannelId() ?? ""),
  selection: () => selection(),
  serviceName: (channel) => chatServiceOf(channel),
  channelLabel: (id) => channelLabel(id),
  // `discuss-transcript-persist`: a jump keeps the panel open (it overlays the
  // list); the list moves behind it, and the panel's state is untouched.
  onJump: (ref) => {
    void shellNavigation.jumpToMessageLink(ref);
  },
  dismissMessages: (body) => dismissMessages(body),
  restoreMessages: (ids) => restoreMessages(ids),
  // `#1388` landing 1: the strike reads the channel store's read (Done) set.
  isReadNow: (id) => channelStore.current().dismissed.has(String(id)),
  showError: (text) => showError(text),
  setStatus: (text) => setStatus(text),
  // `#1344 discuss-panel-resize`. The panel's kept layout, on this device as the
  // two-column split is kept; the stylesheet decides where each applies.
  storedWidth: () => deviceSettings().get(DEVICE_SETTINGS.discussWidth),
  keepWidth: (px) => deviceSettings().keep(DEVICE_SETTINGS.discussWidth, px),
  clampWidth: (value) => clampDiscussWidth(value),
  defaultWidth: DEFAULT_DISCUSS_WIDTH,
  storedSheetFull: () => deviceSettings().get(DEVICE_SETTINGS.discussSheetFull),
  keepSheetFull: (full) => deviceSettings().keep(DEVICE_SETTINGS.discussSheetFull, full),
  // `#1354 discuss-layout-modes`. The panel's mode, on this device with the rest.
  storedMode: () => deviceSettings().get(DEVICE_SETTINGS.discussMode),
  keepMode: (mode) => deviceSettings().keep(DEVICE_SETTINGS.discussMode, mode),
  // `#1497 discuss-fold-state-persist`. The saved summary's open sections, on
  // this device with the rest; a new summary's keep replaces the entry whole.
  storedFolds: () => deviceSettings().get(DEVICE_SETTINGS.discussFolds),
  keepFolds: (state) => deviceSettings().keep(DEVICE_SETTINGS.discussFolds, state),
  // `#1501 discuss-read-aloud-parity`. The summary read through the shared
  // owner, as a document is; its stop scopes to its own key alone, as the
  // Documents stop beside it does.
  readDocument: (doc, from) => readAloudOwner().readDocument(doc, from),
  stopSpeaking: (key) => {
    const owner = readAloudOwner();
    const id = `doc:${key}`;
    const playing = owner.nowPlaying;
    if ((playing && playing.kind === "document" && playing.id === id) || owner.pendingRead === id) {
      owner.stopReading();
    }
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
const { showScreen, closeSettings } = screenShowing;

// Where Back on Help goes — Settings, which is where Help is opened from, unless `?` opened it
// (`keysFromKeyboard`) — and where Settings' way out goes are the screens' (`Screens.helpReturn`,
// `Screens.beforeSettings`), put back as each screen is arrived at (`Screens.arrive`).



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
  intoView: (element, block, inline) => scrollKeeping.intoView(element, block, inline),
  lookup: (id) => el(id),
  closeDismissed: () => dismissedSendsView.close(false),
});

// Open Help, optionally at one entry (`#85 voice-desktop-review`): web/src/view/settings
// `mountHelpScreen`, beside the folds below (`#624 voice-page-modularity`).

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
  intoView: (element, block, inline) => scrollKeeping.intoView(element, block, inline),
  lookup: (id) => el(id),
  byId: (id) => document.getElementById(id),
});

/**
 * Help's way in: the screen shown, opened at one entry (`mountHelpScreen`, web/src/view/settings).
 *
 * @type {import("./src/view/settings/index.ts").HelpScreen}
 */
const helpScreen = mountHelpScreen({
  showScreen: (name) => showScreen(name),
  openAt: (slug) => helpFolds.openAt(slug),
});
const { showHelp } = helpScreen;

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
// `#1251 scroll-single-owner`: the owner's mark for the channel place a view switch took,
// per view left. Restoring the saved number alone throws the reader by whatever grew or
// shrank above them while they were away, as native anchoring used to absorb silently.
const viewMarks = new Map();

/**
 * What the selected channel's chat service can do — threads, its own read cursor, how its live updates are
 * delivered and how often the server reads it: one owner in web/src/core/channels/providerfacts.ts (`#624
 * voice-page-modularity`).
 */
const providerFacts = createProviderFacts();
/**
 * What the server last said the channels and their services were — the channels on the list, the
 * configured ones taken off it, each provider's description, the deployment-wide capabilities, whether
 * the bridge registers and lists channels, and the chat service's own name: web/src/core/channels's
 * (`createChannelDirectory`, `#624 voice-page-modularity`), with why each is kept.
 */
const channelDirectory = createChannelDirectory();


/** Naming and grouping channels: web/src/view/shell's now (`createChannelDirectoryView`, `#624`). */
const directoryView = createChannelDirectoryView({
  directory: () => channelDirectory, activeChannelKey: ACTIVE_CHANNEL_KEY,
  el: (id) => el(id), selectedChannelId: () => selectedChannelId(),
  selectionOwner: () => selectionOwner(), renderLiveState: () => renderLiveState(),
  keepWindowItem: (key, value) => keepWindowItem(key, value), windowItem: (key) => windowItem(key),
  deviceStorage: () => deviceStorage(),
  fromProvider: (provider, capabilities) => providerFacts.fromProvider(/** @type {any} */ (provider), /** @type {any} */ (capabilities)),
  saveCacheChannels: () => messageCache.saveCacheChannels(),
  learnProviderAccounts: (providers) => whoIsWho.learnProviderAccounts(providers),
  boxes: () => channelSettings,
});
const { NO_CHANNEL_TO_READ, UNDESCRIBED_SERVICE, channelName, chatServiceName, learnProviders, providerOfChannel, serviceLabel, channelGroups, channelsGrouped, channelsInPickerOrder, applyChannelProvider, pickerName, knownChannel, storedActiveChannel, rememberActiveChannel, fillChannelSelect, adoptChannelList } = directoryView;
/**
 * Whether the selected channel holds any thread at all, from its last history page (or yes while a thread is
 * open): one owner in web/src/core/channels/threadpresence.ts (`#624 voice-page-modularity`).
 *
 * @type {import("./src/core/channels/index.ts").ThreadPresence}
 */
const threadPresence = createThreadPresence();


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
  intoView: (element, block, inline) => scrollKeeping.intoView(/** @type {any} */ (element), block, inline),
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
  atBottom: (area, slack) => (slack === undefined ? atBottom(area) : atBottom(area, slack)),
  restSlackPx: FOLLOW_REST_PX,
  quickReplyHolds: (area) => quickReplyHolds(area),
  endSearchWalks: () => endSearchWalks(),
  scrollToNewest: () => scrollToNewest(),
  nudge: (area, delta) => scrollKeeping.nudge(area, delta),
  toPosition: (area, top) => scrollKeeping.toPosition(area, top),
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
  invalidateDock: () => listKeepersMount.invalidateDock(),
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
  selectionIn: (area) => textSelectionIn(area),
  restoreSelectionIn: (area, selection) => restoreTextSelectionIn(area, selection),
  restoreChannelComposer: () => restoreChannelComposer(),
  renderOlderControl: () => renderOlderControl(),
  restoreScroll: (position) => restoreScroll(/** @type {any} */ (position)),
  renderCachedTimeline: () => renderCachedTimeline(),
  syncUnreadReplies: () => syncUnreadReplies(),
  renderChannelFreshness: () => renderChannelFreshness(),
  payThreadEntry: () => payThreadEntry(),
  rowsMissDone: () => rowsMissDone(),
  scrollToNewest: () => scrollToNewest(),
  nudge: (area, delta) => scrollKeeping.nudge(area, delta),
  toPosition: (area, top) => scrollKeeping.toPosition(area, top),
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
  nudge: (area, delta) => scrollKeeping.nudge(area, delta),
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
  nudge: (area, delta) => scrollKeeping.nudge(area, delta),
  glideTo: (area, top, behavior) => scrollKeeping.glideTo(area, top, behavior),
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
  listState: () => listState(), listedMessages: () => listedMessages(),
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
  timeOf: (m, field) => timeOf(m, field),
  token: () => token(), tokenFingerprint: (t) => tokenFingerprint(t),
  undoLabel: (d) => undoLabel(d), undoName: (text) => undoName(text),
  voiceTranscript: () => voiceTranscript(), writeInBatches: (route, ids, landed) => writeInBatches(route, ids, landed),
});
const { doneState, noteArchived, dismissalName, drawUndoLabel, renderTodoControls, backlogIsArmed, disarmBacklog, armBacklog, toggleArchived, markRowDone, forgetDoneFlash, drawDoneFlash, drawDoneFlashes, unflashIds, unflashDone, flashDone, refreshAfterInboxChange, renderDoneChange, putBackRowsOff, queueInboxWrite, inboxWriteOwner, inboxPost, sendLeftOverInboxWrites, noteInboxWrite, settleInboxWrite, noteDoneSaving, doneSaved, dismissMessages, dealtWithWords, markReadUpstream, restoreMessages, mirrorArchived, doneSaving } = doneAct;

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
  nudge: (area, delta) => scrollKeeping.nudge(area, delta),
  toPosition: (area, top) => scrollKeeping.toPosition(area, top),
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
const nextUndoAt = undoStamps.next;

// Showing the call's, the channel's or the Documents view: web/src/view/shell/navigation.ts
// (`createPageNavigation`, `#624`), destructured as `showView` above.

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
 * voice-page-modularity`). What the column shows and its last card are the owner's, from the page acts handed in below.
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
  el: (id) => el(id),
  leftEl: (id) => leftEl(id),
  layoutColumns: () => layoutShown.columns(),
  layoutThreadList: () => layoutShown.threadList(),
  screens: () => screens,
  sideMode: () => columns.side.mode,
  readMode: () => readModes.mode(),
  sideContext: () => inColumn(columns.side, () => [channelContextKey(), pinsView().only, searchKey()]),
  inSideColumn: (act) => inColumn(columns.side, act),
  shownChannelList: () => shownChannelList(),
  sideUp: () => sideColumnUp(),
  toPosition: (area, top) => scrollKeeping.toPosition(area, top),
  finishPackSlide: () => finishPackSlide(),
  cardRow: (id) => cardRow(id),
  rowShown: (row) => rowShown(row),
  itemAfterBlock: (row) => itemAfterBlock(row),
  slideKey: (item) => slideKey(item),
  slideIntoPlace: (row, was) => slideIntoPlace(row, was),
  paddingBottom: (area) => (typeof window.getComputedStyle === "function"
    ? parseFloat(window.getComputedStyle(area).paddingBottom) || 0 : 0),
  pageStep: (area) => pageStep(area),
  keyboardRoom: (area) => (screenKeyboardRoom ? screenKeyboardRoom.roomIn(area) : 0),
  toFoot: (area) => scrollKeeping.toFoot(area),
  nudge: (area, delta) => scrollKeeping.nudge(area, delta),
  wheelBy: (area, px) => scrollKeeping.wheelBy(area, px),
});
const {
  overscrollRoom, overextendRoom, footGap, footTop, closeOverscroll, waitOverscrollIdle, openOverscroll, fitOverscroll,
  noteOverscrollReader, holdOverscrollReader, followFoot, toFoot, closeOverextend, sizeOverextend, fitOverextend, keepingCardStill,
} = footRooms;




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
/** Keeping the reader's text selection across rebuilds: web/src/view/channel's now (`mountTextSelection`, `#624`). */
const textSelectionMount = mountTextSelection({
  el: (id) => el(id),
  idsOf: (row) => idsOf(row),
});
const { textSelectionInNow, textSelectionIn, restoreTextSelectionIn } = textSelectionMount;


const scrollKeeping = createScrollKeeping({
  el: (id) => el(id),
  restSlackPx: FOLLOW_REST_PX,
  selectionIn: (area) => textSelectionIn(area),
  restoreSelectionIn: (area, selection) => restoreTextSelectionIn(area, selection),
  onMain: () => screens.screen() === "main",
  currentView: () => screens.view(),
  channelView: () => selection().view,
  pinnedOnly: () => pinsView().only,
  heldFilterRow: () => heldFilterRow(),
  jumpAnchorRow: () => jumpAnchor.row(),
  searchWalk: () => columnWork.active().searchWalk,
  atBottom: (area) => atBottom(area),
  scrollToNewest: () => scrollToNewest(),
  renderJumpNewest: () => renderJumpNewest(),
  setJumpNewest: (on, view, turns) => setJumpNewest(on, view, turns),
});
const {
  visibleList, shownChannelList, scrollAnchor, preservingScroll, preservingCollapse, holdingReader, captureScroll, restoreScroll, followIfPinned,
} = scrollKeeping;
// The dom layer's focus reveal is the owner's write too, handed in now the owner stands (`#1251`).
setFocusReveal((element, block, inline) => scrollKeeping.intoView(element, block, inline));

/** The invitation only makes sense while there is nothing else in the transcript. */
const renderEmptyState = () => {
  el("empty-state").hidden = el("transcript").children.length > 0;
};

/** Saying a message's day on the status line: web/src/view/channel's now (`createDayTap`, `#624`). */
const dayTap = createDayTap({ setStatus: (text) => setStatus(text) });
const { sayTimeDay, onTimeTap, onTimeKey } = dayTap;


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
  intoView: (element, block, inline) => scrollKeeping.intoView(element, block, inline),
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
  nudge: (area, delta) => scrollKeeping.nudge(area, delta),
  preservingCollapse,
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
  bringIntoView: (pane, row) => bringIntoView(/** @type {any} */ (pane), row),
  takeRow: (pane, row, focus) => takeRow(/** @type {any} */ (pane), row, focus),
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
  setJumpNewest, renderJumpNewest, renderJumpUnread, jumpToUnread,
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

// The search bar opened and closed: web/src/view/channel/searchopen.ts
// (`createSearchOpenControl`, #624). Closing clears the query and filters inside
// the filter-kept place, as before.
const searchOpenControl = createSearchOpenControl({
  keepingFilterPlace,
  searchField,
  markSearchSoon,
  el,
  setLinksOnly: (on) => setLinksOnly(on),
  setPinnedOnly: (on) => setPinnedOnly(on),
  setAuthorFilter: (author) => setAuthorFilter(author),
  hidingRead: () => hidingRead(),
  holdingReader,
  renderControlBar: () => renderControlBar(),
  renderChannelFreshness: () => renderChannelFreshness(),
  eachColumn,
  renderCachedTimeline: () => renderCachedTimeline(),
  renderScrollTools: () => renderScrollTools(),
  sideColumnUp,
  refilterSideCards: () => refilterSideCards(),
});
const { setSearchOpen, showSearchOpen } = searchOpenControl;

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
  scrollKeeping.toPosition(area, rect.height <= view ? bottom - view : top);
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
  nudge: (area, delta) => scrollKeeping.nudge(area, delta),
  toPosition: (area, top) => scrollKeeping.toPosition(area, top),
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

// Whether nothing holds the focus (`focusLost`) is web/src/view/dom's (`#624 voice-page-modularity`).

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
  nudge: (area, delta) => scrollKeeping.nudge(area, delta),
  toPosition: (area, top) => scrollKeeping.toPosition(area, top),
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
  preservingCollapse: (area, row, head, foot, act) => preservingCollapse(area, row, head, foot, act),
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
  stepReading: (delta) => readAloudOwner().stepReading(delta),
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
  afterDone: () => storedAfterDone(),
  deviceSettings: () => deviceSettings(),
  keyTimingSetting: () => deviceSettings().get(DEVICE_SETTINGS.keyTiming),
  locationSearch: () => window.location.search || "",
  toggleDocumentOutline: () => {
    if (screens.view() !== "docs" || !documentsView.reading()) return false;
    documentsView.toggleOutline();
    return true;
  },
  // `#1386 discuss-mod8-toggle`: Mod+8 over the current view; no view changes.
  toggleDiscuss: () => {
    discussView.toggle();
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


// --- what a slow or failing chat service means, in words ------------------------------------------
//
// `#195 send-resilience`. The server's sentence for a provider failure is a diagnosis —
// `HTTP 502 chat_error: Google Chat returned HTTP 502: {"message":"… timed out …"}` — which is
// right in a log and wrong on a phone, where it was all the reader saw of a refresh that did not
// land, in a red banner that came back every poll for as long as the service stayed slow. These say
// what it means instead: the provider's own words without the JSON around them, and a timeout —
// most of what a slow service does — as "slow to answer". A network failure and a refusal are
// already sentences, and keep them.

// Words for a slow service and a failed channel read: web/src/view/channel/servicewords.ts
// (`createServiceWords`, #624), over core/api's `readableFailure`.
const serviceWordsOwner = createServiceWords({
  readableFailure,
  providerOfChannel,
  selectedChannelId,
  chatServiceName,
});
const { chatServiceOf, readableChannelFailure } = serviceWordsOwner;
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
/**
 * The call's audio devices: what a label says a device is, which capture device a call opens,
 * whether its output can be chosen, opening its microphone and aiming its playback (web/src/core/call
 * `createCallDevices`, beside the route it writes).
 *
 * @type {import("./src/core/call/index.ts").CallDevices}
 */
const callDevices = createCallDevices({
  route: {
    begin: () => audioRoute.begin(),
    noteInput: (label) => audioRoute.noteInput(label),
    settleInput: (label) => audioRoute.settleInput(label),
    noteSink: (sink) => audioRoute.noteSink(sink),
    noteOutput: (label) => audioRoute.noteOutput(label),
  },
  openMicrophone: (constraints) => browserPorts().microphone.open(/** @type {any} */ (constraints)),
  constraintsFor: (mic) => audioConstraints(/** @type {any} */ (mic)),
  mediaDevices: () => navigator.mediaDevices,
  preferences: () => audioDevices.preferences(),
  audioContextClass: () => browserPorts().audio.contextClass(),
});
const { deviceLabel, classifyAudioDevice, pickCallInput, outputSinkSupported, openCallMicrophone, applyCallOutput } = callDevices;

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

// The devices themselves — deviceLabel, classifyAudioDevice, pickCallInput, outputSinkSupported,
// openCallMicrophone and applyCallOutput — are web/src/core/call's now (`createCallDevices`, made
// above with the route it writes, `#624 voice-page-modularity`).

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

// What a read says of the server's page build, a summary landing, and the collapsed-lines chord
// are web/src/view/channel's now (`createCardRefresh`, beside `createCardLines`, `#624
// voice-page-modularity`), made below with the card lines it stands beside.

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

/** Draw every Mark read on reply control as the switches now stand. */
function renderMarkReadSettings() {
  messagePreferences.markRead.render(
    markReadOnReplySwitch.on(), markReadQuoteGuardSwitch.on(), markReadFoldGuardSwitch.on(),
    markReadNonquoteSwitch.on(), markReadFinalGuardSwitch.on(), markReadFirstGuardSwitch.on());
}

/** Mark read on reply itself: Yes unless the reader said No. Synced. */
const markReadOnReplySwitch = createKeptSwitch({
  setting: DEVICE_SETTINGS.markReadOnReply, deviceSettings: () => deviceSettings(), initial: true,
  render: () => renderMarkReadSettings(),
});
/** Hold a reply back when its quote covers under the share. Synced. */
const markReadQuoteGuardSwitch = createKeptSwitch({
  setting: DEVICE_SETTINGS.markReadQuoteGuard, deviceSettings: () => deviceSettings(), initial: false,
  render: () => renderMarkReadSettings(),
});
/** Hold a reply back sent in Fold read. Synced. */
const markReadFoldGuardSwitch = createKeptSwitch({
  setting: DEVICE_SETTINGS.markReadFoldGuard, deviceSettings: () => deviceSettings(), initial: true,
  render: () => renderMarkReadSettings(),
});
/** Whether a reply that quotes nothing marks what it answers. Synced. */
const markReadNonquoteSwitch = createKeptSwitch({
  setting: DEVICE_SETTINGS.markReadNonquote, deviceSettings: () => deviceSettings(), initial: true,
  render: () => renderMarkReadSettings(),
});
/** Hold a quoted reply back when its quote ends before the final paragraph. Synced. */
const markReadFinalGuardSwitch = createKeptSwitch({
  setting: DEVICE_SETTINGS.markReadFinalGuard, deviceSettings: () => deviceSettings(), initial: true,
  render: () => renderMarkReadSettings(),
});
/** Hold a quoted reply back when its quote sits in the first share of a long message. Synced. */
const markReadFirstGuardSwitch = createKeptSwitch({
  setting: DEVICE_SETTINGS.markReadFirstGuard, deviceSettings: () => deviceSettings(), initial: true,
  render: () => renderMarkReadSettings(),
});
/** The first share of a long message, in percent (25 until the reader says otherwise). Synced. */
const markReadFirstPercent = createMarkReadFirstPercent({
  deviceSettings: () => deviceSettings(),
  render: (n) => messagePreferences.markRead.renderFirstPercent(n),
});
/** How many words make a message long (300 until the reader says otherwise). Synced. */
const markReadLongWords = createMarkReadLongWords({
  deviceSettings: () => deviceSettings(),
  render: (n) => messagePreferences.markRead.renderLongWords(n),
});


/**
 * The card lines' page-side acts: what a read says of the server's page build, a summary landing
 * owed a re-read, and the collapsed-lines chord (web/src/view/channel `createCardRefresh`).
 *
 * @type {import("./src/view/channel/index.ts").CardRefresh}
 */
const cardRefresh = createCardRefresh({
  notePageBuild: (build) => serverFacts.notePageBuild(build),
  pageBuildChanged: () => settingsReadouts.pageBuild(),
  view: () => screens.view(),
  selectedChannelId: () => selectedChannelId(),
  forgetThreadRead: (channel) => threadDirectoryReadAt.delete(channel),
  refreshThreadDirectory: () => refreshThreadDirectory(),
  renderThreadHeading: () => renderThreadHeading(),
  renderSideColumn: () => renderSideColumn(),
  selectionView: () => selection().view,
  renderReplyLinks: () => renderReplyLinks(),
  isTextField: (target) => isTextField(/** @type {any} */ (target)),
  collapsedCurrent: () => collapsedLines.current(),
  collapsedChanged: (n) => collapsedLinesChanged(n),
  setStatus: (text) => setStatus(text),
});
const { noteServerPageBuild, summaryWatchFired, collapsedLinesFromKeyboard } = cardRefresh;

// Thread card lines and how much a quote must cover are web/src/view/channel's now
// (`createCardLines`, `#624 voice-page-modularity`): where a card stops, its head and
// continuation, summaries on their way, and what a reply marks read. K and the share stay
// their settings owners', instantiated above and handed in.
const cardLinesOwner = createCardLines({
  cardMaxLines,
  quoteReadPercent,
  markReadOnReply: markReadOnReplySwitch,
  markReadQuoteGuard: markReadQuoteGuardSwitch,
  markReadFoldGuard: markReadFoldGuardSwitch,
  markReadNonquote: markReadNonquoteSwitch,
  markReadFinalGuard: markReadFinalGuardSwitch,
  markReadFirstGuard: markReadFirstGuardSwitch,
  markReadFirstPercent,
  markReadLongWords,
  readMode: { mode: () => readModes.mode() },
  bodies: () => messageBodies,
  rawTextOf,
  headingOf,
  threadNames: () => serverFacts.threadNames(),
  cardIsSummarized: (choice) => cardIsSummarized(choice),
  pendingInto: (thread) => pendingInto(thread),
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
  keepingCardPlace: (change, follow) => keepingCardPlace(change, follow),
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

// The collapsed-lines chord (Ctrl+( / Ctrl+)) is the card refresh's above (`collapsedLinesFromKeyboard`).

/**
 * The screen values as stored, read where the page restores its settings (web/src/view/settings
 * `createStoredScreen`, beside the reading width's owner).
 *
 * @type {import("./src/view/settings/index.ts").StoredScreen}
 */
const storedScreen = createStoredScreen({
  msgScale: () => deviceSettings().get(DEVICE_SETTINGS.msgScale),
  readingWidth: () => deviceSettings().get(DEVICE_SETTINGS.readingWidth),
});
const { storedMsgScale, storedReadingWidth } = storedScreen;

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

/**
 * The page going hidden and coming back: what a hidden page owes on return, and the refresh a
 * long absence earns (web/src/view/shell `createPageVisibility`, beside the document listeners).
 *
 * @type {import("./src/view/shell/index.ts").PageVisibility}
 */
const pageVisibility = createPageVisibility({
  syncScreenWakeLock: () => voiceCall().syncScreenWakeLock(),
  hidden: () => document.visibilityState === "hidden",
  hide: (at) => awayClock.hide(at),
  callSocketOpen: () => Boolean(voiceCall().session.socket),
  noteHiddenDuringCall: () => { voiceCall().hiddenDuringCall = true; },
  pauseWhileHidden: () => pauseWhileHidden(),
  noteVisibleAt: (at) => { voiceCall().visibleAt = at; },
  awayMs: (at) => awayClock.show(at),
  resumeOutgoingRetries: () => resumeOutgoingRetries(),
  catchUpHiddenArrivals: () => catchUpHiddenArrivals(),
  replyArrowsOwed: () => Boolean(replyArrows().owed),
  measureReplyArrows: () => measureReplyArrows(),
  inOtherColumn: (act) => inOtherColumn(act),
  takeStreamResume: () => hiddenStream.takeResume(),
  pollPaused: () => channelPoll.paused(),
  pollDueAt: () => channelPoll.dueAt(),
  visibleRefreshMs: VISIBLE_REFRESH_MS,
  view: () => screens.view(),
  threading: () => providerFacts.threading(),
  configApplied: () => serverFacts.configApplied(),
  selectedChannelId: () => selectedChannelId(),
  refreshQuietly: (run) => refreshQuietly(run),
  refreshAfterLiveMutation: () => refreshAfterLiveMutation(),
  replayStarted: () => replayLedger.started(),
  loadDiscordVisible: () => timelineView.loadDiscord({ keepPosition: true, reason: "visible" }),
  readSideColumn: (reason, since) => readSideColumn(reason, /** @type {any} */ (since)),
  resumeLiveStream: (resume) => resumeLiveStream(/** @type {any} */ (resume)),
  resumePolling: (mode) => resumePolling(mode),
  now: () => Date.now(),
});
const { onVisibility } = pageVisibility;
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



// `mergeIntoCanon` and `upsertThreadSummary` are web/src/core/channels's, and take the store they
// fold into (`#624 voice-page-modularity`).





/** The delta pages `foldTimelineDelta` found changed nothing. `#226 page-energy-profile`. */



/** Saving the channel's scope, drawing it back, and clearing it: web/src/view/channel's now (`createChannelScope`, `#624`). */
const channelScope = createChannelScope({
  providerFacts, channelStore, channelDirectory, messageCache,
  selectedChannelId: () => selectedChannelId(), selection: () => selection(),
  columnWork, columns: () => columns,
  timelineRows: () => timelineRows(), doneState: () => doneState(),
  loadState: () => loadState(), listState: () => listState(), olderPages: () => olderPages(),
  readModes: () => readModes,
  bookmarkToSave: (channel) => bookmarkToSave(channel),
  listedMessages: () => listedMessages(),
  plainReach: () => plainReach,
  loadCanon: (channel) => loadCanon(/** @type {any} */ (channel)),
  inheritAllCover: (view, threadId, thread) => inheritAllCover(view, /** @type {any} */ (threadId), /** @type {any} */ (thread)),
  drawProjection: () => drawProjection(),
  knownChannel: (id) => knownChannel(id), channelName: (channel) => channelName(channel),
  restoreSavedBookmark: (key, bookmark) => restoreSavedBookmark(key, bookmark),
  el: (id) => el(id),
  applyNewestPage: (payload, saved) => applyNewestPage(/** @type {any} */ (payload), saved),
  olderPager: () => olderPager,
  channelSummary: (loaded, whole, name) => channelSummary(loaded, whole, name),
  renderChannelSeam: (summary) => renderChannelSeam(summary),
  setChannelFreshness: (state) => setChannelFreshness(/** @type {any} */ (state)),
  renderOutgoingMessages: () => renderOutgoingMessages(),
  learnProviders: (providers) => learnProviders(providers),
  providerOfChannel: (id) => providerOfChannel(id),
  fillChannelSelect: (id) => fillChannelSelect(id),
  whoIsWho: () => whoIsWho, noteSelfAuthor: (id) => noteSelfAuthor(id),
  channelSettings: () => channelSettings,
  openChannelView: () => openChannelView(), restoreChannelComposer: () => restoreChannelComposer(),
  inColumn: (column, act) => inColumn(/** @type {any} */ (column), act),
  readAloudOwner: () => readAloudOwner(),
  channelContexts, channelReads: () => channelReads,
  forgetPins: () => forgetPins(), readingBookmark: () => readingBookmark,
  renderOlderControl: () => renderOlderControl(),
  screenIdentity, token: () => token(), sideThreads,
  resetSideColumn: () => resetSideColumn(), forgetBookmarks: () => forgetBookmarks(),
  putDockedReplyAway: () => putDockedReplyAway(), inboxLedger: () => inboxLedger,
  uiState: () => uiState, readableChannelFailure: (error) => readableChannelFailure(error), isSlowFailure: (error) => isSlowFailure(/** @type {any} */ (error)),
  channelContextKey: () => channelContextKey(), threadPresence,
  voiceCall: () => voiceCall(), renderColumns: () => renderColumns(), saveUiState: () => saveUiState(),
});
const { syncCanonDismissed, saveChannelScope, hydrateChannelScope, hydrateFromCache, clearChannelScreen, screenBelongsToToken, forgetMessages, noteChannelReadFailure, reconcileChannelSnapshot } = channelScope;

/** How old what is on screen is, said over the list: web/src/view/channel/freshness.ts (`createFreshness`, #624). */
const freshnessOwner = createFreshness({
  loadState,
  side: () => columnWork.side(),
  el,
  leftEl,
  stamp,
  liveAttached: () => liveFollow.attached(),
  liveDelivery: () => providerFacts.liveDelivery(),
  staleWords,
  chatServiceOf,
  selectedChannelId,
  view: () => screens.view(),
  searchOpen: () => searchField.open(),
  columnsShown: () => layoutShown.columns(),
  sideColumn: () => columns.side,
  mainColumn: () => columns.main,
  inColumn,
  sideColumnUp,
  threadListFailure: (channel) => threadList.threadListFailure(channel),
  threadListLandedAt: (channel) => threadList.threadListLandedAt(channel),
  setTextIfChanged,
  holdingReader,
  keepingCardPlace: (act) => keepingCardPlace(act),
});
const { setChannelFreshness, renderChannelFreshness, renderSideFreshness } = freshnessOwner;




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
  documentsQueryValue: (name) => documentsQueryValue(name),
  documentsDeepLinkKey: () => documentsDeepLinkKey(),
  showView: (name) => showView(name),
  showDocuments: (key) => { void documentsView.show(key); },
  reopenReadOwed: () => reopenReadOwed,
  replayStarted: () => replayLedger.started(),
  readEnteredChannel: (fresh) => readEnteredChannel(fresh),
  freshAt: () => loadState().freshAt,
  whenLiveAttached: (pay) => whenLiveAttached(pay),
});
const { windowItem, keepWindowItem, defaultChannelView, threadStillKnown, openingChannelView, openChannelView, holdChannelPlace, savedThreadSummary, viewOnScreen, saveUiState, returnToSavedPlace, reopenSavedView, payReopenRead } = openingViewOwner;

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
  pendingThreadTimes: () => pendingThreadTimes(),
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
  openThreadFromList: (id, summary) => openThreadFromList(id, summary),
  selectSideThread: (id, summary) => selectSideThread(id, summary),
  closeSideThread: () => closeSideThread(),
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
  cardOpener: (prefix, replies, unread, open) => cardOpener(prefix, replies, unread, open),
  toggleLitCard: (id, row) => toggleLitCard(id, row),
  paintLitCard: (row, id) => paintLitCard(row, id),
  enterCard: (id, summary, row) => enterCard(id, summary, row),
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

/** Saying a read is on the wire: web/src/view/channel's now (`createChannelLoading`, `#624`). */
const channelLoading = createChannelLoading({
  el: (id) => el(id), loadState: () => loadState(),
  renderChannelFreshness: () => renderChannelFreshness(), selection: () => selection(),
});
const { renderChannelLoading, backgroundRead } = channelLoading;


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

/**
 * Where each copy of a message in the store stands in the order of reads (`sentNumberOf`), the copy it
 * was put together on, and whether it says the message as it was posted (web/src/core/channels
 * `createCopyOrder`, `#310 stale-live-read-flicker`, which says what each means and why): the facts
 * private to the ledger, reached only by its named commands. Here where the maps it replaces stood.
 */
const copyLedger = createCopyOrder();


/** What reads say about read state, and where copies stand: web/src/view/channel's now (`createReadState`, `#624`). */
const readState = createReadState({
  channelStore, readLedger, copyLedger, readOrder,
  queueReadAllForCounts: () => queueReadAllForCounts(),
});
const { atOrAfter, foldReadState, noteReplyGaps, readSpeaksFor, sentEpochOf, sentNumberOf, orderCopy, buildCopyOn, orderCopiesRead, copyOlderThan } = readState;

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

/** Done on held messages at once, and settling the write: web/src/view/channel's now (`createHeldDone`, `#624`). */
const heldDone = createHeldDone({
  readLedger, providerFacts, channelStore,
  selectedChannelId: () => selectedChannelId(), selection: () => selection(),
  pinsView: () => pinsView(), stillToDo: (message, done) => stillToDo(message, done),
  inboxPost: (route, body) => inboxPost(route, /** @type {any} */ (body)),
});
const { markHeldDone, doneSettlesHere, settleHeldDone, heldDonePart, writeInBatches } = heldDone;



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

/** Noting a landed read and standing by own posts: web/src/view/channel's now (`createFreshRead`, `#624`). */
const freshRead = createFreshRead({
  timelineReader, standInRetries, ownPosts,
  loadState: () => loadState(), channelPoll: () => channelPoll,
  scheduleDiscordPoll: (ms) => scheduleDiscordPoll(ms),
  setChannelFreshness: (state) => setChannelFreshness(/** @type {any} */ (state)),
  notePinsRevision: (revision) => notePinsRevision(revision),
  noteBookmark: (payload) => noteBookmark(payload),
  providerOfChannel: (id) => providerOfChannel(id),
  providerFacts, selectedChannelId: () => selectedChannelId(),
});
const { noteFreshRead, sentFromThisPage, loadOwnPosts, ownPostStands, storeKeepsOwnPosts } = freshRead;

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

/** Which thread each send in the outbox goes into: web/src/view/channel's now (`createPendingSends`, `#624`). */
const pendingSends = createPendingSends({ pendingThreads });
const { outgoingThreadOf, pendingInto, pendingThreadTimes } = pendingSends;





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

/** Showing rich headers: web/src/view/channel's now (`createRichHeaders`, `#624`). */
const richHeaders = createRichHeaders({
  renderRichHeaders: (shown) => messagePreferences.richHeaders.render(shown),
  deviceSettings: () => deviceSettings(),
  richHeadersSetting: DEVICE_SETTINGS.richHeaders,
});
const { applyRichHeaders, storedRichHeaders } = richHeaders;






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
  // `#1251 scroll-single-owner`: every Main-list redraw is held by the owner — except under a
  // quick-reply hold, whose own holder places the box (`placeQuickReply`).
  if (columnWork.side() || quickReplyHolds(el("scroll-area")) || filterChanges.changing() || currentFilterHold() !== null) renderChannelRowsNow();
  else scrollKeeping.holdingRows(() => renderChannelRowsNow());
}

function renderChannelRowsNow() {
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
    // The linked message's mark is drawn as a row is built; a row kept or put back by a view
    // (`#1229 keyed-row-patching`) is not built again, so the pass draws the mark on it here.
    linkedMark.drawOn(/** @type {any} */ (row));
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
  fromGatheredThread,
  preservingScroll,
  nudge: (area, delta) => scrollKeeping.nudge(area, delta),
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
  stillToDo: (message, done) => stillToDo(message, done),
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


/** A message's details sheet: web/src/view/channel's now (`createMessageDetails`, `#624`). */
const messageDetails = createMessageDetails({
  bookmarkAbove: (row) => bookmarkAbove(row),
  toggleBookmarkAt: (row) => toggleBookmarkAt(row),
  inThisColumn: (act) => inThisColumn(act),
});
const { toggleMessageDetails } = messageDetails;


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
  request: (path, options) => /** @type {any} */ (api(path, options)),
  setStatus: (text) => setStatus(text),
  selectedChannel: () => selectedChannelId(),
  reloadChannel: (options) => timelineView.loadDiscord(options),
  guard: (run) => guardQuietly(run),
  setTimeout: (run, ms) => setTimeout(run, ms),
  clearTimeout: (timer) => clearTimeout(timer),
});

/** Whether the reader's own messages start read, and what is still to do: web/src/view/channel's now (`createStartsRead`, `#624`). */
const startsReadOwner = createStartsRead({
  ownReadSwitch, whoIsWho: () => whoIsWho,
  messageChannel: (message) => messageChannel(message),
  providerFacts, heldMessage: (id) => heldMessage(id), readLedger,
  selectedChannelId: () => selectedChannelId(), channelStore,
  doneState: () => doneState(), hiddenEmpty: (message) => hiddenEmpty(message),
});
const { startsRead, noteStartsRead, stillToDo } = startsReadOwner;



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
  toggleDetails: (row, messages) => toggleMessageDetails(row, /** @type {any} */ (messages)),
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
  markNotNoise: (ids) => markNotNoise(ids),
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
  cardsOnScreen: () => cardsOnScreen(),
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
// Putting the list in a read mode is web/src/view/channel's now (`createReadModeAct`, `#624
// voice-page-modularity`): the mode's owner made here through present/rows' factory, handed in.
const readModeAct = createReadModeAct({
  makeReadModes: () => createReadMode({ storage: () => deviceStorage() }),
  keepingFilterPlace: (change) => keepingFilterPlace(change),
  clearReadRuns: () => readRuns.clear(),
  settleUiState: () => uiState.settle(),
  saveUiState: () => saveUiState(),
  disarmBacklog: () => disarmBacklog(),
  clearLastDismissal: () => lastDismissal.set(null),
  clearBacklogCount: () => { listState().backlog = 0; },
  renderTodoControls: () => renderTodoControls(),
  threading: () => providerFacts.threading(),
  eachColumnRenderCached: () => eachColumn(renderCachedTimeline),
  forgetCards: () => cardsDrawn.forget(),
  sideColumnUp: () => sideColumnUp(),
  renderSideColumn: () => renderSideColumn(),
  loadDiscordEnter: () => timelineView.loadDiscord({ keepPosition: true, ownAct: true, readAs: "enter" }),
  applyReadRuns: () => applyReadRuns(),
  renderScrollTools: () => renderScrollTools(),
  guardQuietly: (run) => guardQuietly(run),
  requestVisibleSummaries: () => requestVisibleSummaries(),
});
const { readModes, setReadMode } = readModeAct;

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
// The read state's acts beside Done's own are web/src/view/channel's now (`createReadActs`,
// `#624 voice-page-modularity`): the read-run lines' construction, the dismissal offer and
// inbox ledger, every copy of the Done state kept in line with the store's, and the
// dismissal's Undo and the backlog's clear.
const readActs = createReadActs({
  byId: el,
  leftById: leftEl,
  inThisColumn,
  selection: () => selection(),
  threads: () => channelStore.current().threads,
  readHiding: () => readModes.hiding(),
  readRunsSuspended: () => readRunsSuspended(),
  threading: () => providerFacts.threading(),
  eachColumnRenderCached: () => eachColumn(renderCachedTimeline),
  openReadRun: (row) => openReadRun(/** @type {any} */ (row)),
  peelReadRun: (row, edge) => peelReadRun(/** @type {any} */ (row), edge),
  readRunRows: (row) => readRunRows(row),
  threadIdOf: (message) => threadOf(message),
  selectionThread: () => selection().thread,
  makeDismissalOffer: () => createUndoOffer(),
  makeInboxLedger: () => createInboxWriteLedger({
    storage: deviceStorageOnUse(),
    now: () => Date.now(),
    makeRoom: () => messageCache.dropMessageCache(),
    // This page load's own name, as the back stack's (`BACK_SESSION`): another page open on the device keeps
    // its own writes.
    page: BACK_SESSION,
  }),
  storeDismissed: () => channelStore.current().dismissed,
  storeChannel: () => channelStore.current().channel,
  selectedChannelId: () => selectedChannelId(),
  heldDoneCopies: () => channelContexts.values(),
  sideShowing: () => sideShowing(),
  activeColumn: () => columnWork.active(),
  sideColumn: () => columns.side,
  mainColumn: () => columns.main,
  sideLeft: () => Boolean(columns.side.left),
  inColumn: (column, act) => inColumn(/** @type {any} */ (column), act),
  doneState: () => doneState(),
  cardsWhileDressedAfter: (up, act) => cardsDrawn.whileDressedAfter(up, act),
  sideColumnUp: () => sideColumnUp(),
  renderCachedTimeline: () => renderCachedTimeline(),
  syncUnreadReplies: () => syncUnreadReplies(),
  renderSideColumn: () => renderSideColumn(),
  el: (id) => el(id),
  idsOf: (row) => idsOf(/** @type {any} */ (row)),
  readFilterKeeps: () => readFilterKeeps(),
  timelineMessages: () => timelineRows().messages,
  renderTodoControls: () => renderTodoControls(),
  // `#1388` landing 1: a channel mark restates an open summary's strike.
  restoreMessages: (messages) => {
    const run = restoreMessages(messages);
    void Promise.resolve(run).then(refreshDiscussStrikes, refreshDiscussStrikes);
    return run;
  },
  undoBookmarkBump: (bump, restoring) => undoBookmarkBump(bump, restoring),
  listedMessages: () => listedMessages(),
  backlogIsArmed: () => backlogIsArmed(),
  armBacklog: () => armBacklog(),
  disarmBacklog: () => disarmBacklog(),
  setStatus: (text) => setStatus(text),
  // `#1388` landing 1: a channel mark restates an open summary's strike.
  dismissMessages: (body, bump) => {
    const run = dismissMessages(body, /** @type {any} */ (bump));
    void Promise.resolve(run).then(refreshDiscussStrikes, refreshDiscussStrikes);
    return run;
  },
  throughBump: (row) => throughBump(/** @type {any} */ (row)),
});
const { readRuns, readRunLines, lastDismissal, inboxLedger, caughtUpDone, followDoneEverywhere, rowsMissDone, undoDismissal, clearBacklog } = readActs;
const {
  hidingRead, liftingHideRead, readRunLine, readRunWords, runDaysOf, readRunCount,
  placeInReadRun, placeReadRunPeel, isOpenThreadRoot, openThreadRootId, foldedOneIds, liveUnpackedRow, liveCardRow,
} = readRunLines;
const foldedOnes = readRunLines.foldedOnes;


// Pinned messages are web/src/view/channel's now (`createPinnedMessages`, `#624
// voice-page-modularity`): the held list, the sync over it, every row's chip, the filter's
// list and its toggle. The list itself is core/annotations'.
const rowDrawing = createRowDrawing();

const currentChannelId = () => String(selectedChannelId() || "");


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
  request: (path, options) => /** @type {any} */ (api(path, options)),
  guard: (run) => guardQuietly(run),
  status: (text) => setStatus(text),
  tally: () => noiseTally(),
  selectedChannel: () => selectedChannelId(),
  reloadChannel: () => timelineView.loadDiscord({ keepPosition: true, readAs: "mutation" }),
});

/** What the server calls noise, tallied and rescued: web/src/view/channel's now (`createNoiseActs`, `#624`). */
const noiseActs = createNoiseActs({
  providerFacts, readModes: () => readModes,
  timelineRows: () => timelineRows(), el: (id) => el(id),
  rowMessages: (row) => rowMessages(/** @type {any} */ (row)),
  selectedChannelId: () => selectedChannelId(),
  api: (path, options) => api(path, /** @type {any} */ (options)),
  channelStore, eachColumn: (act) => eachColumn(act),
  guardQuietly: (fn) => guardQuietly(fn),
  refreshAfterInboxChange: () => refreshAfterInboxChange(),
  setStatus: (text) => setStatus(text),
});
const { noiseTally, markNotNoise } = noiseActs;


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
  nudge: (scroller, delta) => scrollKeeping.nudge(scroller, delta),
  intoView: (element, block) => scrollKeeping.intoView(element, block),
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
  toStream: (row, message, text, opts) => quickReplyToStream(row, message, text, opts),
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
  quickReplyListMoved, placeQuickReply, closeQuickReply, putDownQuickReply, dockQuickReply, sendQuickReply,
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
// voice-page-modularity`).

// Leaving the thread list, and the card the reader was held by, are web/src/view/channel's now
// (`createCardPlaceActs`, `#624 voice-page-modularity`): Esc on the list, opening a thread from
// it, leaving it, and the reader's place in the cards. The scroll acts are handed in as the
// exact calls the script made.
const cardPlaceActs = createCardPlaceActs({
  inSideColumn: () => columnWork.side(),
  inMainColumn: (act) => inColumn(columns.main, act),
  listBefore: () => layoutChoice.listBefore(),
  columnsFit: () => layoutShown.fit(),
  restoreBeforeColumns: (cols) => layoutChoice.restoreBeforeColumns(/** @type {any} */ (cols)),
  setLayout: (layout) => setLayout(/** @type {any} */ (layout)),
  chooseOneColumn: (place) => chooseOneColumn(/** @type {any} */ (place)),
  oneColumnBeforeList: (before) => oneColumnBeforeList(before),
  settleFocus: () => settleFocus(),
  threadSummary: (id) => channelStore.current().threads.find((held) => String(held.id) === String(id)) || null,
  threadListOnScreen: () => threadListOnScreen(),
  cardFlightStart: (id) => cardFlightStart(id),
  flyThreadCard: (card, from, pane) => flyThreadCard(card, from, pane),
  leftById: leftEl,
  postCardShown: () => voiceTranscript().postWatch.shown !== null,
  mainFollows: () => liveFollowers().main,
  selectionKey: () => `${selection().view}|${selection().threadId}`,
  takeListPlace: () => layoutShown.takeListPlace(),
  setThreadList: (on) => layoutShown.setThreadList(on),
  renderMainToggle: () => renderMainToggle(),
  renderOrderToggle: () => renderOrderToggle(),
  placeMainCard: () => placeMainCard(),
  endRepliesWalk: () => endRepliesWalk(),
  placePostCard: () => voiceTranscript().placePostCard(),
  placeHolds: (held, view) => screens.screen() === "main" && screens.view() === "discord" && selection().view === view && String(selectedChannelId()) === held.channel,
  keepReaderPlace: (place) => keepReaderPlace(/** @type {any} */ (place)),
  scrollToNewest: () => scrollToNewest(),
  showOneColumnLeft: () => showOneColumnLeft(),
  followIfPinned: () => followIfPinned(false, 0, "discord"),
  renderSearchInAll: () => renderSearchInAll(),
  refreshSearchCount: () => refreshSearchCount(),
  makeCardPlaces: () => createCardPlaces({
    lookup: (id) => leftEl(id),
    readable: () => sideColumnUp() && columns.side.mode === "list" && !cardsDrawn.opening(),
    atBottom: (area) => atBottom(area),
    followFoot: (area) => followFoot(area),
    nudge: (area, delta) => scrollKeeping.nudge(area, delta),
    toPosition: (area, top) => scrollKeeping.toPosition(area, top),
    litCard: () => litCard(),
    selectedCard: () => {
      const mark = markOf(sidePane);
      return mark && mark.card !== false ? mark.id : null;
    },
    finishSlide: () => finishPackSlide(),
  }),
  selectedChannelId: () => selectedChannelId(),
  takeListCards: () => layoutShown.takeListCards(),
  closeOverextend: () => closeOverextend(),
  setSideListMode: () => { columns.side.mode = "list"; },
  whileOpeningCards: (act) => cardsDrawn.whileOpening(act),
  renderSideMode: () => renderSideMode(),
});
const { threadListBack, openThreadFromList, leaveThreadList, cardAnchor, placeCardAnchor, keepingCardPlace, rememberCardPlace, openSideCards, placeOpenedCards } = cardPlaceActs;

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
// The search over the cards, the runs of read threads over them, and the links between Main's
// roots and their cards are web/src/view/channel's now (`createCardRuns`, `#624
// voice-page-modularity`). The read mode is only ever read, through the handed-in question.
const cardRuns = createCardRuns({
  leftById: leftEl,
  noteSuspendedRuns: () => cardsDrawn.noteSuspended(readRunsSuspended()),
  unpackOnScreen: () => unpackOnScreen(),
  readMode: () => readModes.mode(),
  readRunsSuspended: () => readRunsSuspended(),
  forgetCards: () => cardsDrawn.forget(),
  renderSideCards: () => renderSideCards(),
  searchOpen: () => searchField.open(),
  searchNode: () => searchParsed(searchField.query()).node,
  searchTerms: () => searchParsed(searchField.marked()).terms,
  threadsWithHits: (node) => threadsWithHits(/** @type {any} */ (node)),
  openedCardGaps: (create) => openedCardGaps(create),
  hidingRead: () => hidingRead(),
  rowMatches: (row, node, text) => rowMatches(row, /** @type {any} */ (node), text),
  markReplyMatch: (row, inReplies) => markReplyMatch(row, inReplies),
  markSearchHits: (row, terms) => markSearchHits(row, /** @type {any} */ (terms)),
  olderThreadsFailure: () => olderThreadsFailure(),
  allLoaded: () => allLoaded(),
  setCardsCount: (cards, shown) => cardsDrawn.setCount(cards, shown),
  threadListOnScreen: () => threadListOnScreen(),
  sayCardCount: () => sayCardCount(),
  renderRepliesSearch: () => renderRepliesSearch(),
  drawCardGaps: (list) => drawCardGaps(list),
  inOpenedGap: (row, ids) => inOpenedGap(row, /** @type {any} */ (ids)),
  selectedChannelId: () => selectedChannelId(),
  rowIsRead: (row) => rowIsRead(row),
  readRunPlan: (items, key, isRead, ids) => /** @type {any} */ (readRunPlan(/** @type {any} */ (items), key, /** @type {any} */ (isRead), /** @type {any} */ (ids))),
  readRunLine: (row) => readRunLine(row),
  placeInReadRun: (row, role, count) => placeInReadRun(row, /** @type {any} */ (role), count),
  placeReadRunPeel: (row, count) => placeReadRunPeel(row, count),
  keepingCardPlace: (change) => keepingCardPlace(change),
  makeColumnLinksDeps: () => ({
    onTwoColumns: () => layoutShown.columns() && screens.screen() === "main" && screens.view() === "discord",
    sideMode: () => columns.side.mode,
    mainList: () => channelPane.list(),
    openThread: () => /** @type {any} */ (sideThreadChoice()),
    activePane: () => activePane(),
    sidePane: () => sidePane,
    litCard: () => litCardOnRight(),
    cardRow: (thread) => cardRowOnRight(thread),
    hue: (thread) => Number(threadHue(thread)),
  }),
});
const { columnLinks, cardRunKey, filterSideCards, sideListState, refilterSideCards, cardRunRows, foldCardRuns, columnLinksShown, scheduleColumnLinks, drawColumnLinks, noteLinkedSelection, glideBehavior } = cardRuns;

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








// `#432 read-mode-unify`. Unread only (`#259 thread-list-unread-filter`) was the cards' own filter, a
// chip and a choice kept on this device. It is the read mode now, as for every list: Hide read leaves the
// read threads out, Fold read folds them into runs (`foldCardRuns`), Show read shows them.

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

// The main channel's messages among the cards, and the order of the Threads view's cards, are
// web/src/view/channel's now (`createMainCards`, `#624 voice-page-modularity`): both kept
// choices' setting objects and storage handed in untouched.
const mainCards = createMainCards({
  intoView: (element, block, inline) => scrollKeeping.intoView(element, block, inline),
  makeMainChoice: () => createKeptSwitch({
    setting: DEVICE_SETTINGS.threadsMain,
    deviceSettings: () => deviceSettings(),
  }),
  threadsMainKey: DEVICE_SETTINGS.threadsMain.key,
  storage: () => deviceStorage(),
  leftById: leftEl,
  threadListOnScreen: () => threadListOnScreen(),
  sideMode: () => columns.side.mode,
  threadListShown: () => layoutShown.threadList(),
  forgetCards: () => cardsDrawn.forget(),
  sideColumnUp: () => sideColumnUp(),
  keepingCardPlace: (change, follow) => keepingCardPlace(change, follow),
  renderSideColumn: () => renderSideColumn(),
  setStatus: (text) => setStatus(text),
  selectedCardId: () => {
    const mark = markOf(sidePane);
    return mark && mark.card !== false ? mark.id : null;
  },
  paneRowForId: (id) => paneRowFor(sidePane, id, true),
  rowShown: (row) => rowShown(/** @type {any} */ (row)),
  storeChannel: () => channelStore.current().channel,
  storeMessages: () => channelStore.current().messages,
  storeRemoved: () => channelStore.current().removed,
  storeDismissed: () => channelStore.current().dismissed,
  selectedChannelId: () => selectedChannelId(),
  inView: (message, view, threadId) => inView(/** @type {any} */ (message), /** @type {any} */ (view), threadId),
  hiddenEmpty: (message) => hiddenEmpty(/** @type {any} */ (message)),
  hidingRead: () => hidingRead(),
  stillToDo: (message, dismissed) => stillToDo(/** @type {any} */ (message), /** @type {any} */ (dismissed)),
  sentFromThisPage: (message) => sentFromThisPage(/** @type {any} */ (message)),
  whileMainRows: (act) => rowDrawing.whileMain(act),
  glom: (messages) => glom(/** @type {any} */ (messages)),
  discordNode: (group) => discordNode(/** @type {any} */ (group)),
  answeredBy: (rows, viewing) => answeredBy(/** @type {any} */ (rows), viewing),
  dressChannelRow: (row, pass) => dressChannelRow(row, /** @type {any} */ (pass)),
});
const { THREADS_MAIN_KEY, mainRowsChoice, threadOrders, mainRowsOnScreen, renderMainToggle, setMainRowsShown, mainRowsFromKeyboard, cardOrderOnScreen, renderOrderToggle, setThreadOrder, nextThreadOrder, orderFromKeyboard, mainRowMessages, mainRows, placeMainCard } = mainCards;

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

// A card's DOM helpers (cardRootOf, threadCardId, cardGapGroups, markCardLayout, fillCardText, ...) are
// web/src/view/channel/'s, and the Main runs among the cards (mainRunsAmong) web/src/present/rows/'s
// (`#624 voice-page-modularity`). The runs of read threads over the cards are `createCardRuns`' (above).



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

// By channel, the thread whose card is highlighted, for this session, and the cards' acts over
// it are web/src/view/channel's now (`createLitCards`, `#624 voice-page-modularity`). Entering a
// thread stays the page's: `enterCard` only calls its entry acts, handed in below.
const litCardsOwner = createLitCards({
  cardsCycle: () => cardsCycle(),
  selectedChannelId: () => selectedChannelId(),
  leftById: leftEl,
  noteLinkedSelection: () => noteLinkedSelection(),
  rowShown: (row) => rowShown(/** @type {any} */ (row)),
  sidePane: () => sidePane,
  channelPane: () => channelPane,
  fitOverscroll: (area) => fitOverscroll(area),
  nudge: (scroller, delta) => scrollKeeping.nudge(scroller, delta),
  bringIntoView: (pane, row) => bringIntoView(/** @type {any} */ (pane), row),
  noteOverscrollReader: (area) => noteOverscrollReader(area),
  sayRowShortcut: (button, className) => sayRowShortcut(button, className),
  openThread: (id, summary) => openThread(id, /** @type {any} */ (summary)),
  openThreadFromList: (id, summary) => openThreadFromList(id, /** @type {any} */ (summary)),
  selectSideThread: (id, summary) => selectSideThread(id, /** @type {any} */ (summary)),
  markSideFromCards: () => { columns.side.fromCards = true; },
  threadListShown: () => layoutShown.threadList(),
  sideShowing: () => sideShowing(),
  focusList: (pane) => focusList(/** @type {any} */ (pane)),
  columnLinksShown: () => columnLinksShown(),
  sideMode: () => columns.side.mode,
  channelOnScreen: () => channelOnScreen(),
  sideColumnUp: () => sideColumnUp(),
  inMainColumn: (act) => inColumn(columns.main, act),
  selectionView: () => selection().view,
  pinsOnly: () => pinsView().only,
  paneFor: (target) => paneFor(/** @type {any} */ (target)),
  nodeWithin: (root, node) => nodeWithin(root, node),
  keyRow: (target) => keyContext(/** @type {any} */ (target)).row,
  rowInView: (pane, row) => rowInView(/** @type {any} */ (pane), row),
});
const { litCard, cardThread, paintLitCard, paintLitCards, toggleLitCard, cardOpener, cardCountParts, enterCard, cardRowOnRight, litCardOnRight, cardsOnScreen, cardForArrow, putOutLitCard } = litCardsOwner;

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

// The card at the top of an open thread, and its fold to a bar, are web/src/view/channel's now
// (`createThreadCardHead`, `#624 voice-page-modularity`): what each card was last drawn from,
// its drawing, and the reader's fold choice kept on the device — the setting's own key and
// stored form, handed in untouched.
const threadCardHead = createThreadCardHead({
  makeFoldChoice: () => createKeptSwitch({
    setting: DEVICE_SETTINGS.threadCardFolded,
    deviceSettings: () => deviceSettings(),
  }),
  foldKey: DEVICE_SETTINGS.threadCardFolded.key,
  leftById: leftEl,
  unansweredRoot: (root, count, held, exact) => unansweredRoot(/** @type {any} */ (root), /** @type {any} */ (count), held, /** @type {any} */ (exact)),
  threadReplyCount: (id, root) => threadReplyCount(id, /** @type {any} */ (root)),
  exactReplyCount: (id, root, tally) => exactReplyCount(id, /** @type {any} */ (root), tally),
  cardView: (choice) => cardView(choice),
  briefAge: (at) => briefAge(at),
  sideCardReplies: (choice, tally) => sideCardReplies(choice, tally),
  sideCardUnread: (choice, tally) => sideCardUnread(choice, tally),
  cardIsSummarized: (choice) => cardIsSummarized(choice),
  watchPendingSummaries: (choices, cardId) => watchPendingSummaries(/** @type {any} */ (choices), cardId),
  threadHue: (id) => threadHue(id),
  markCardLayout: (card, mode, cut) => markCardLayout(card, mode, /** @type {any} */ (cut)),
  fillCardText: (words, view, text) => fillCardText(words, /** @type {any} */ (view), text),
  setTextIfChanged: (node, text) => setTextIfChanged(node, text),
  fullLocalAt: (when) => fullLocalAt(when),
  cardCountParts: (prefix, replies, unread) => cardCountParts(prefix, replies, unread),
  scheduleColumnLinks: () => scheduleColumnLinks(),
  drawnOnPage: (node) => drawnOnPage(node),
  atBottom: (area, slack) => atBottom(area, slack),
  overscrollRoom: (area) => overscrollRoom(area),
  fitOverscroll: (area) => fitOverscroll(area),
  followFoot: (area) => followFoot(area),
  toPosition: (area, top) => scrollKeeping.toPosition(area, top),
  sideShowing: () => sideShowing(),
  sideThreadChoice: () => /** @type {any} */ (sideThreadChoice()),
  replyTally: () => replyTally(),
});
const { threadCardsDrawn, THREAD_CARD_FOLD_KEY, THREAD_CARD_LISTS, threadCardFoldChoice, threadUnanswered, paintThreadCard, renderThreadCardFold, threadCardPressed, setThreadCardFolded, paintSideThreadCard } = threadCardHead;

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

// The card moving up as its thread opens, and the right column's thread life, are
// web/src/view/channel's now (`createSideThreadActs`, `#624 voice-page-modularity`): where the
// card at the top starts from, selecting and closing the right column's thread, forgetting the
// column, and reading it behind its rows. The flight owner is made by cardflight's factory,
// handed in as a maker with the page's acts.
const sideThreadActs = createSideThreadActs({
  leftById: leftEl,
  makeFlight: (ms, drawn) => createCardFlight({
    clock: clockOnUse(),
    ms,
    drawn: (node) => drawn(/** @type {any} */ (node)),
    linksShown: () => columnLinksShown(),
    drawLinks: () => drawColumnLinks(),
    scheduleLinks: () => scheduleColumnLinks(),
    frame: (run) => {
      if (typeof requestAnimationFrame !== "function") return false;
      requestAnimationFrame(run);
      return true;
    },
  }),
  cardRowOnRight: (id) => cardRowOnRight(id),
  rowShown: (row) => rowShown(/** @type {any} */ (row)),
  rowInView: (pane, row) => rowInView(/** @type {any} */ (pane), row),
  sidePane: () => sidePane,
  openSideCards: () => openSideCards(),
  configApplied: () => serverFacts.configApplied(),
  guardQuietly: (run) => guardQuietly(run),
  refreshThreadDirectory: () => refreshThreadDirectory(),
  noteDeepLinkReaderMove: () => noteDeepLinkReaderMove(),
  selectedChannelId: () => selectedChannelId(),
  columnsShown: () => layoutShown.columns(),
  threadSummary: (id) => channelStore.current().threads.find((held) => String(held.id) === String(id)) || null,
  sideThreads: () => sideThreads,
  sideMode: () => columns.side.mode,
  setSideMode: (mode) => { columns.side.mode = /** @type {any} */ (mode); },
  setSideFromCards: (on) => { columns.side.fromCards = on; },
  replyDockedAway: () => Boolean(replyState.docked && replyState.column !== columns.side && replyPane() === sidePane),
  replyInSide: () => replyState.column === columns.side,
  putDockedReplyAway: () => putDockedReplyAway(),
  renderSideMode: () => renderSideMode(),
  inSideColumn: (act) => inColumn(columns.side, act),
  selectionView: () => selection().view,
  selectionThreadId: () => selection().threadId,
  rowsMissDone: () => rowsMissDone(),
  renderCachedTimeline: () => renderCachedTimeline(),
  changeChannelView: (view, thread, known) => changeChannelView(/** @type {any} */ (view), thread, /** @type {any} */ (known)),
  endSearchWalk: () => endSearchWalk(columns.side),
  dropReplyArrowTap: () => dropReplyArrowTap(),
  putRepliesBack: () => putRepliesBack(),
  rememberChannelDraft: () => rememberChannelDraft(),
  rememberChannelContext: () => rememberChannelContext(),
  bumpLoadGeneration: () => { ++loadState().generation; },
  clearGathered: () => { gatherState().gathered = null; listState().rowsGathered = false; },
  forgetUnpackReads: () => unpackReadQueue.forget(),
  stopUnpackWatch: () => unpackWatch.stop(),
  endRepliesWalk: () => endRepliesWalk(),
  forgetRepliesNote: () => repliesWalks.forgetNote(),
  clearSideSearchNote: () => { columns.side.searchNote = null; },
  rememberCardPlace: () => rememberCardPlace(),
  activeIsSide: () => columnWork.active() === columns.side,
  resetSideRecords: () => {
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
  },
  forgetCards: () => cardsDrawn.forget(),
  sideColumnUp: () => sideColumnUp(),
  threadListShown: () => layoutShown.threadList(),
  loadSideDiscord: (options) => timelineView.loadDiscord(/** @type {any} */ (options)),
  channelContextKey: () => channelContextKey(),
  activeColumn: () => columnWork.active(),
  forwardCursor: (reason) => timelineReader.forwardCursor(reason),
  sideShowing: () => sideShowing(),
});
const { columnReadsSent, cardFlightMs, cardFlightStart, drawnOnPage, showSideList, selectSideThread, closeSideThread, resetSideColumn, readSideColumn, answeredSince, inSideThread } = sideThreadActs;
const cardFlightOwner = sideThreadActs.flight;
const { flyThreadCard, endCardFlightPart, endCardFlight } = cardFlightOwner;

// --- the links between Main's roots and their cards ------------------------------------------------
//
// `#261 two-column-connectors`. The connectors between Main's roots and their cards on the right are
// web/src/view/channel's (`createColumnLinks`, `#624 voice-page-modularity`), with what is drawn and
// what it costs. The page tells it which columns show, the selection and the highlighted card; the
// names below are the page's commands to it. Nothing here scrolls for the mouse (`#1144`).

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
  replayLedger, columnReadsSent: /** @type {any} */ (columnReadsSent), landAbandonedTimelineRead: (payload, read) => landAbandonedTimelineRead(payload, /** @type {any} */ (read)),
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
  screenBelongsToToken, readModes, readLedger, api: /** @type {any} */ (api), currentDiscordLoad: (generation, channel, readingTodo) => currentDiscordLoad(generation, channel, readingTodo),
  get noChannelWords() { return NO_CHANNEL_TO_READ; }, notePinsRevision, noteBookmark, plainReach,
  noiseRenderCount: () => noiseSettings.renderCount(), foldedOneIds, foldedOnes, readRuns, setReadMode,
  collapseRevealed: (rows) => { for (const row of rows) { const entry = /** @type {any} */ (row).foldEntry; if (entry) setFolded(entry, true); } },
  setJumpNewest, scrollToNewest, keepFilterHold, inThisColumn: (fn) => inThisColumn(fn), pinsView, searchOffered: () => searchOffered(),
  pullToRefresh, heldFilterRow, jumpAnchorRow: () => jumpAnchor.row(), scrollAnchor, followsNewest, visibleList, noiseSettings,
  selectionIn: (area) => textSelectionIn(area),
  restoreSelectionIn: (area, selection) => restoreTextSelectionIn(area, selection),
  combineOn: () => combineSwitch.on(), columns, sideShowing, openThreadRootId,
  isOpenThreadRoot, readRunWords, placeInReadRun, runDaysOf, renderReplyLinks, leftEl,
  cardRunKey, cardRunRows, screensScreen: () => screens.screen(), searchField, authorChosen: () => authorFilters.chosen(), searchTyped, stillToDo, cardsDrawnForget: () => cardsDrawn.forget(), renderSideColumn, keepSelection,
  syncCanonDismissed, olderTriggerPx: OLDER_TRIGGER_PX, threadPresence, openThreadName,
  captureScroll, restoreScroll, screensView: () => screens.view(), renderJumpNewest,
  nudge: (area, delta) => scrollKeeping.nudge(area, delta),
  toPosition: (area, top) => scrollKeeping.toPosition(area, top),
});
const { loadCanon, drawProjection, walkCursorOwed, heldViewRows, drawHeldRows, queueReadAllForCounts, readAllForCounts, rowsAsDrawn, applyTimelinePage, renderCachedTimeline, mutationAwaitsRead, loadTimeline, foldLateTimelinePage, loadOlderTimeline, foldLiveMessage, inView, projectView, holdingMessage, foldTimelinePage, foldTimelineDelta, loadOlder, maybeLoadOlder, takeDeferredOlder, appendChannelRow, appendToColumns, applyReadRuns, readRunPlan, readRunRows, cycleReadMode, readRunsSuspended, rowIsRead, openReadRun, peelReadRun, renderReadModeControl, applyNewestPage, channelReadPosition, settleAfterRead, landedReadPosition } = timelineView;
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
  guardQuietly: (fn) => guardQuietly(fn), renderJumpNewest, applySearch, preservingScroll, toPosition: (area, top) => scrollKeeping.toPosition(area, top), setStatus,
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

const renderResumeState = () => resumePreference.render();

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
  appendToColumns: timelineView.appendToColumns, preservingScroll: (act) => preservingScroll(act), renderOutgoingEverywhere: () => renderOutgoingEverywhere(), noteServerThreadHuesIn: (messages, summaries, channel, provisional) => noteServerThreadHuesIn(messages, summaries, channel, provisional),
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
  noteSelfAuthor, noteStartsRead: (parts) => noteStartsRead(/** @type {any} */ (parts)), appendPart: (part) => appendToColumns(/** @type {any} */ (part)),
  scrollToNewest: (side) => { if (side) inColumn(columns.side, scrollToNewest); else scrollToNewest(); },
  markSentRead: (entry) => guardQuietly(/** @type {any} */ (markSentRead))(entry),
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
  channelLabel: (id) => channelLabel(id), view: () => screens.view(),
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
  rootThisStarts: (channel, threadId) => (markReadOnReplySwitch.on() ? rootThisStarts(channel, threadId) : null), queueOutgoingMessage,
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

/** A channel's picker name and the seam at its head restated: web/src/view/channel/channelhead.ts (`createChannelHead`, #624). */
const channelHeadOwner = createChannelHead({
  knownChannel,
  channelName,
  el,
  readModes: () => readModes,
  renderChannelSeam,
  channelSummary,
  olderPager: () => olderPager,
  olderPages: () => olderPages(),
  selectedChannelId,
});
const { channelLabel, restateChannelSeam } = channelHeadOwner;

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

/** What one frame of that stream means: web/src/view/voice's now (`createWatchFrame`, `#624`). */
const watchFrameOwner = createWatchFrame({
  watchAll: () => watchAll,
  refreshServerSettings: () => refreshServerSettings(),
  voiceCall: () => voiceCall(),
  relayMode: () => relayMode(),
  onSettingsFrame: (frame) => onSettingsFrame(frame),
  noteSelfAuthor: (id) => noteSelfAuthor(id),
  settleFromServer: (key) => settleFromServer(key),
});
const { startWatchAll, stopWatchAll, onWatchAllFrame } = watchFrameOwner;


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
  request: (path, options) => /** @type {any} */ (api(path, options)),
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
 * Settings' channel browser (`#19 channel-browser`, `#371 channel-search-hash`): its search, its pages,
 * the count of a slow read, and one tap to add. web/src/view/settings `mountChannelDirectory`, in the
 * words of web/src/present/settings (`#624 voice-page-modularity`); see "browsing channels".
 *
 * @type {import("./src/view/settings/index.ts").ChannelDirectory}
 */
const channelDirectoryBox = mountChannelDirectory({
  lookup: (id) => el(id),
  request: (path, options) => /** @type {any} */ (api(path, options)),
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
  applyClientConfig: (config) => { applyClientConfig(config); discussView.sync(); },
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
      get AFTER_DONE_KEY() { return AFTER_DONE_KEY; },
      get COLLAPSED_LINES_KEY() { return COLLAPSED_LINES_KEY; },
      get CARD_MAX_LINES_KEY() { return CARD_MAX_LINES_KEY; },
      get READ_SPEED_KEY() { return READ_SPEED_KEY; },
      get READ_AUDIO_SOURCE_KEY() { return readAloudOwner().READ_AUDIO_SOURCE_KEY; },
      get WIT_VOICE_KEY() { return readAloudOwner().WIT_VOICE_KEY; },
      get QUOTE_READ_PERCENT_KEY() { return QUOTE_READ_PERCENT_KEY; },
      get MARK_READ_ON_REPLY_KEY() { return DEVICE_SETTINGS.markReadOnReply.key; },
      get MARK_READ_QUOTE_GUARD_KEY() { return DEVICE_SETTINGS.markReadQuoteGuard.key; },
      get MARK_READ_FOLD_GUARD_KEY() { return DEVICE_SETTINGS.markReadFoldGuard.key; },
      get MARK_READ_NONQUOTE_KEY() { return DEVICE_SETTINGS.markReadNonquote.key; },
      get MARK_READ_FINAL_GUARD_KEY() { return DEVICE_SETTINGS.markReadFinalGuard.key; },
      get MARK_READ_FIRST_GUARD_KEY() { return DEVICE_SETTINGS.markReadFirstGuard.key; },
      get MARK_READ_FIRST_PERCENT_KEY() { return DEVICE_SETTINGS.markReadFirstPercent.key; },
      get MARK_READ_LONG_WORDS_KEY() { return DEVICE_SETTINGS.markReadLongWords.key; },
      get RELAY_KEY() { return RELAY_KEY; },
      get LINK_KINDS_KEY() { return LINK_KINDS_KEY; },
      get LINK_RAW_KEY() { return LINK_RAW_KEY; },
      get THREADS_UNPACKED_KEY() { return THREADS_UNPACKED_KEY; },
      get THREAD_CARD_STATES_KEY() { return THREAD_CARD_STATES_KEY; },
      get DOC_STATES_KEY() { return DOC_STATES_KEY; },
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
    afterDone: () => {
      renderAfterDone();
      keyHints.render();
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
    markRead: () => {
      markReadOnReplySwitch.apply(markReadOnReplySwitch.stored());
      markReadQuoteGuardSwitch.apply(markReadQuoteGuardSwitch.stored());
      markReadFoldGuardSwitch.apply(markReadFoldGuardSwitch.stored());
      markReadNonquoteSwitch.apply(markReadNonquoteSwitch.stored());
      markReadFinalGuardSwitch.apply(markReadFinalGuardSwitch.stored());
      markReadFirstGuardSwitch.apply(markReadFirstGuardSwitch.stored());
      markReadFirstPercent.apply(markReadFirstPercent.stored());
      markReadLongWords.apply(markReadLongWords.stored());
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
    docStates: () => {
      documentsView.reloadDocStates();
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
  markRead: () => {
    markReadOnReplySwitch.apply(markReadOnReplySwitch.stored());
    markReadQuoteGuardSwitch.apply(markReadQuoteGuardSwitch.stored());
    markReadFoldGuardSwitch.apply(markReadFoldGuardSwitch.stored());
    markReadNonquoteSwitch.apply(markReadNonquoteSwitch.stored());
    markReadFinalGuardSwitch.apply(markReadFinalGuardSwitch.stored());
    markReadFirstGuardSwitch.apply(markReadFirstGuardSwitch.stored());
    markReadFirstPercent.apply(markReadFirstPercent.stored());
    markReadLongWords.apply(markReadLongWords.stored());
    renderMarkReadSettings();
  },
  readSpeed: () => readAloudOwner().applyReadSpeed(readAloudOwner().storedReadSpeed()),
  ownRead: () => ownReadSwitch.apply(ownReadSwitch.stored()),
  combine: () => combineSwitch.apply(combineSwitch.stored()),
  hideEmpty: () => hideEmptySwitch.apply(hideEmptySwitch.stored()),
  richHeaders: () => applyRichHeaders(storedRichHeaders()),
  inAppLinks: () => inAppLinks.apply(inAppLinks.stored()),
  openInView: () => renderOpenInView(),
  afterDone: () => renderAfterDone(),
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
  markReadFirstPercentChanged: (value) => messagePreferences.markRead.firstPercentChanged(value),
  stepMarkReadFirstPercent: (direction) => messagePreferences.markRead.stepFirstPercent(direction),
  markReadLongWordsChanged: (value) => messagePreferences.markRead.longWordsChanged(value),
  stepMarkReadLongWords: (direction) => messagePreferences.markRead.stepLongWords(direction),
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
// The kept text selection's document listeners were mounted with it, where they have always
// been registered; only their teardown is recorded here, in the page's order.
application.track(textSelectionMount);
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
  dockQuickReply: () => dockQuickReply(),
  onBoxKey: (event) => onComposerKey(event, guardQuietly(sendQuickReply), el("quick-reply-text")),
  watchBoxInsertions: (box) => watchBoxInsertions(box),
  noteQuickReplyInput: () => quoteOwner.noteQuickReplyInput(),
  showKeyboard: () => dictation.showKeyboard(),
  stopDictation: () => dictation.stop(),
  pressDraftChip: () => guardQuietly(pressDraftChip)(),
  reactToHeldSentence: () => readAloudOwner().reactToHeldSentence(),
  quickReplyOpen: () => quoteOwner.quickReplyOpen(),
  putDownQuickReply: () => putDownQuickReply(),
  claimClick: () => rowGestures.claimClick(),
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

/** The pace popover down and read-aloud reading mode on and off: web/src/view/voice/readingacts.ts (`createReadingModeActs`, #624). */
const readingActsOwner = createReadingModeActs({
  el,
  scheduleBackSync,
  readAloudOwner,
  clearError,
  showError,
  guardQuietly,
  setStatus,
  renderControls: () => voiceCall().renderControls(),
  renderChannelRows,
});
const { closeSpeedPopover, leaveReadingMode, setReadingMode } = readingActsOwner;

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
  afterDoneChanged: messagePreferences.afterDone.changed,
  summariesChanged: messagePreferences.summaries.changed,
  enterSendsChanged: (on) => enterSends.apply(on),
  markReadChanged: messagePreferences.markRead.changed,
  markReadQuoteGuardChanged: messagePreferences.markRead.quoteGuardChanged,
  markReadFoldGuardChanged: messagePreferences.markRead.foldGuardChanged,
  markReadNonquoteChanged: messagePreferences.markRead.nonquoteChanged,
  markReadFinalGuardChanged: messagePreferences.markRead.finalGuardChanged,
  markReadFirstGuardChanged: messagePreferences.markRead.firstGuardChanged,
}));

// `#240 reading-bookmark`. The ways back: to the bookmark, to where the reader was reading (the left column's chip, as
// b is), and back to a reply (web/src/view/channel/readingjumps.ts).
application.track(mountReadingJumps({
  lookup: (id) => el(id),
  leftLookup: (id) => leftEl(id),
  jumpToBookmark: () => guardQuietly(jumpToBookmark)(),
  jumpToUnread: () => jumpToUnread(),
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
// Discuss (`catchup-side-agent`): the bar's button and its panel's controls, in
// web/src/view/catchup, tracked here in the page's order.
application.track(discussView.mountControls());
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
  nudge: (area, delta) => scrollKeeping.nudge(area, delta),
  toFoot: (area) => scrollKeeping.toFoot(area),
  // `ios-keyboard-send`. The Send at the keyboard's top, and each box's own Send it presses:
  // every Send id stands once in the document, so the left column's lookup finds each.
  cornerSend: () => leftEl("keyboard-send"),
  normalSend: (field) => {
    const sendId = KEYBOARD_SEND_PARTS[field.id];
    return sendId ? leftEl(sendId) : null;
  },
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

/** The keyboard layout debug choice: web/src/view/composer's now (`createKeyboardDebug`, `#624`). */
const keyboardDebug = createKeyboardDebug({
  deviceSettings: () => deviceSettings(),
  keyboardDebugSetting: DEVICE_SETTINGS.keyboardDebug,
  showKeyboardDebug: (on) => showKeyboardDebug(on),
});
const { storedKeyboardDebug, applyKeyboardDebug } = keyboardDebug;


// --- what moved a list, kept for the log -------------------------------------------------------------------
//
// `#1144 scrollback-typing-jump`. A list found somewhere the reader never put it says nothing about what
// moved it, and the numbers that would are only on the phone it happened on. Settings › Screen and keys ›
// Typing › "Scroll trace" keeps the last minute of each list's scrolls, with the code that asked for each
// (web/src/view/channel/scrolltrace.ts), and "Send scroll trace" beside it posts that minute to the
// server's log (`POST /api/v1/scroll-trace`). Off by default, kept on this device alone, and nothing is
// sent until Send is pressed or the trace itself sends its minute over a shift nobody made (`#1208`).

/** The trace's anchor, its stored choice and sending what it holds: web/src/view/channel/scrolltrace.ts (`createScrollTraceControls`, #624). */
const scrollTraceControls = createScrollTraceControls({
  scrollAnchor,
  idsOf,
  storedTrace: () => deviceSettings().get(DEVICE_SETTINGS.scrollTrace),
  keepTrace: (on) => deviceSettings().keep(DEVICE_SETTINGS.scrollTrace, on),
  el,
  scrollTrace: () => scrollTrace,
  api: (path, options) => /** @type {any} */ (api(path, options)),
  setStatus,
});
const { scrollTraceAnchor, storedScrollTrace, applyScrollTrace, sendScrollTrace } = scrollTraceControls;

const scrollTrace = createScrollTrace({
  lookup: (id) => leftEl(id),
  activeElement: () => document.activeElement,
  viewport: () => window.visualViewport || null,
  anchor: (area) => scrollTraceAnchor(area),
  elementPrototype: () => Element.prototype,
  now: () => performance.now(),
  // `#1208 scroll-trace-layout-shift`: what else moved what the reader was looking at.
  containers: () => ["discord-log", "pinned-log", "outgoing-log", "transcript", "thread-list", "side-log"]
    .map((id) => leftEl(id)).filter((node) => node),
  documentTarget: () => document,
  selection: () => {
    const sel = typeof getSelection === "function" ? getSelection() : null;
    if (!sel) return null;
    const node = sel.anchorNode;
    const near = /** @type {Element | null} */ (node && (node.nodeType === 1 ? node : node.parentElement));
    const row = near && typeof near.closest === "function" ? near.closest("[data-id], [data-context-id]") : null;
    return {
      collapsed: sel.isCollapsed,
      length: String(sel).length,
      desc: row ? (row.getAttribute("data-id") || row.getAttribute("data-context-id") || row.id || "") : (near ? near.id || near.tagName || "" : ""),
    };
  },
  autoSend: (text) => { void sendScrollTrace(text); },
});




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
  pressCornerSend: () => screenKeyboard.pressCornerSend(),
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

// The view switch pressed, reading a channel just entered, and changing channel:
// web/src/view/shell/navigation.ts (`createPageNavigation`, `#624`), destructured
// under their old names above.
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
  intoView: (element, block, inline) => scrollKeeping.intoView(element, block, inline),
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
 * What the dock-growth keepers of Main's list and of the right column (below) noted at their last reports:
 * each list's height and `heightUnderLists`, and Main's reader's distance from its foot as a send card came
 * up docked under the thread list (`cardCameUnderLists`). One owner in web/src/view/channel/underlists.ts
 * (`#624 voice-page-modularity`), destructured under their old names.
 */
const underLists = createUnderLists({
  el,
  leftEl,
  screens: () => screens,
});
const { heightUnderLists, cardMovedUnderLists, cardCameUnderLists, cardsPlacedUnderLists } = underLists;
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
  layoutShown: { columns: () => layoutShown.columns(), threadList: () => layoutShown.threadList() },
  bottomSlackPx: BOTTOM_SLACK_PX,
  heightUnderLists: () => heightUnderLists(),
  quickReplyHolds: (area) => quickReplyHolds(area),
  placeQuickReply: () => placeQuickReply(),
  entryLandingHolds: () => entryLandingHolds(),
  entryState: () => entryState(),
  placeEntryLanding: (row, first) => placeEntryLanding(row, first),
  keepFilterHold: () => keepFilterHold(),
  heldFilterRow: () => heldFilterRow(),
  keepJumpAnchor: () => jumpAnchor.place(),
  jumpAnchorHolds: () => jumpAnchor.row() !== null,
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
  toFoot: (area) => scrollKeeping.toFoot(area),
  toPosition: (area, top) => scrollKeeping.toPosition(area, top),
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



// The boot, mounted and started in the page's order (`createApplication`, above): the mounts
// were tracked as the wiring mounted them, and the start acts are the first drawing and sign-in.
application.mount();
application.start();
