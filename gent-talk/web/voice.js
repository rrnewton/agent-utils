"use strict";
// gent-talk voice page.
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

const TOKEN_KEY = "gent-talk.token"; // shared with the main app on purpose.
const MIC_SETTINGS_KEY = "gent-talk.voice.mic";
const WIDTH_KEY = "gent-talk.voice.width";

const el = (id) => document.getElementById(id);

// ONE status line, above the control pane. It used to be two — a word under the header and a
// sentence at the foot — which is how the closed state managed to announce itself three times in
// three vocabularies. The sentence is the line; the state is the dot beside it.
const setStatus = (text) => {
  el("status").textContent = redact(text);
};

/**
 * One of: idle, working, live, ended, suspended, error. web/voice.css colours the dot from this.
 *
 * `suspended` is `#54 resume-recovery`: the socket died while the page was in the background. It
 * is deliberately neither `ended` (which says the reader chose to hang up) nor `error` (which says
 * something is broken), because it is neither, and calling it either one is the defect.
 */
const setState = (name) => {
  el("status-line").setAttribute("data-state", name);
};

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
  const secrets = [localStorage.getItem(TOKEN_KEY) || "", el("api-token").value || ""];
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
// three screens so it is visible on whichever one is up.
function showError(text) {
  const box = el("error");
  box.textContent = redact(text);
  box.hidden = false;
  setState("error");
}

function clearError() {
  const box = el("error");
  box.textContent = "";
  box.hidden = true;
}

const session = {
  socket: null,
  connected: false, // true only between `onopen` and teardown.
  // `#54 resume-recovery`. Set by `onerror`, read by `onclose`. The socket reports a failure and a
  // close as two separate events, in that order, and only the CLOSE knows whether the page was in
  // the background — so the error cannot be the thing that decides what to tell the reader.
  failed: false,
  muted: false,
  // The agent's VOICE is silenced; the agent is not. Its replies keep arriving and keep being
  // written into the transcript — see `handle`, where only the audio frames are dropped.
  speakerOff: false,
  audio: null, // AudioContext
  stream: null, // MediaStream
  node: null, // ScriptProcessorNode capturing the microphone
  source: null,
  playAt: 0, // next start time on the audio clock
  playing: [], // scheduled AudioBufferSourceNodes, so an interruption can cancel them
  outputRate: 16000,
};

const token = () => localStorage.getItem(TOKEN_KEY) || "";

// --- screens ----------------------------------------------------------------------------------
//
// Three screens, no router and no framework: they are three sections stacked in one grid cell and
// `hidden` picks. The explanatory text that earns no space once you know what the page is lives on
// the sign-in screen; the connection details and the knobs live in settings; the main screen is
// the transcript and the two controls, and nothing else.

const SCREENS = ["signin", "main", "settings", "reply"];

/** What the header calls each screen that is a DESTINATION rather than the app itself. */
const SCREEN_TITLES = { settings: "Settings", reply: "Reply" };

let screenBeforeSettings = "signin";
let currentScreen = "signin";

function showScreen(name) {
  for (const screen of SCREENS) {
    el(`screen-${screen}`).hidden = screen !== name;
  }
  const main = name === "main";
  // Settings and Reply are both destinations: the header becomes a title bar with a way back, and
  // the controls that act on the screen you have left are absent.
  const destination = Object.prototype.hasOwnProperty.call(SCREEN_TITLES, name);
  // The view switch belongs to the main screen; anywhere else it would switch between two things
  // neither of which is on the screen.
  el("view-switch").hidden = !main;
  el("open-settings").hidden = destination;
  // Two separate ways back, because they go to two different places — and because
  // scripts/screenshots.py drives #close-settings by name.
  el("close-settings").hidden = name !== "settings";
  el("close-reply").hidden = name !== "reply";
  el("topbar-title").hidden = !destination;
  if (destination) {
    el("topbar-title").textContent = SCREEN_TITLES[name];
  }
  // The control pane is a grid ROW inside the dock. Hiding it collapses the row, so the body grows
  // to fill the frame rather than leaving a band of empty pane under a sign-in form. The status
  // line above it is NOT hidden: a sign-in failure has to be able to say so.
  el("control-pane").hidden = !main;
  if (!main) {
    disarmClear();
  }
  if (name !== "settings") {
    screenBeforeSettings = name;
  }
  currentScreen = name;
}

let currentView = "voice";

/**
 * The call, or the channel. One switch with two positions, and the word on it names the position
 * it is IN — not a pair of labels one of which has been styled into looking disabled.
 *
 * Deliberately touches NOTHING in `session`. Switching views during a call must not disturb the
 * call: no socket close, no track stop, no re-acquire, no change to the mute state.
 */
function showView(name) {
  currentView = name;
  el("pane-voice").hidden = name !== "voice";
  el("pane-discord").hidden = name !== "discord";
  const discord = name === "discord";
  el("view-switch").setAttribute("aria-checked", discord ? "true" : "false");
  el("view-switch-label").textContent = discord ? "Discord" : "Voice";
  // Both panes share ONE scroll container, so a switch swaps the content out from under a
  // scrollTop that belonged to the other pane. Landing anywhere but the newest message is
  // wrong for both views, and it is badly wrong for Discord: arriving at the TOP of a long
  // channel means scrolling past everything already read to reach the thing you opened it
  // for. Doing this here rather than in loadDiscord() is deliberate — the load only runs on
  // the FIRST switch, so a fix that lived there would leave every later switch at the top.
  scrollToNewest();
  // Entering a view lands on its newest message, so any offer to jump there has been taken.
  // Leaving the flag set would show a chip pointing at where the reader already is.
  jumpNewestWanted[name] = false;
  // The chips belong to the list you are looking at, and you are now looking at a different one.
  renderScrollTools();
}

// --- connection details -------------------------------------------------------------------------
//
// Shown once in a banner that takes itself away, and permanently reachable from settings. Kept in
// a variable rather than read back out of the DOM, so appending to it cannot pick up a redaction
// marker and re-redact it.

const BANNER_DISMISS_MS = 8000;
let connectionDetail = "";
let bannerTimer = null;

function renderDetail() {
  const text = redact(connectionDetail);
  el("connection-detail").textContent = text;
  el("settings-detail").textContent = text;
}

function showDetail(text) {
  connectionDetail = String(text);
  renderDetail();
  el("connection-banner").hidden = false;
  if (bannerTimer !== null) {
    clearTimeout(bannerTimer);
  }
  bannerTimer = setTimeout(dismissBanner, BANNER_DISMISS_MS);
}

function addDetail(text) {
  connectionDetail = `${connectionDetail} · ${text}`;
  renderDetail();
}

function dismissBanner() {
  if (bannerTimer !== null) {
    clearTimeout(bannerTimer);
    bannerTimer = null;
  }
  el("connection-banner").hidden = true;
}

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
  area.scrollTop = area.scrollHeight;
  setJumpNewest(false);
}

/**
 * Was the reader already parked on the newest message?
 *
 * A browser clamps scrollTop to scrollHeight - clientHeight, so "at the bottom" is never
 * scrollTop === scrollHeight however hard `scrollToNewest` pushes. The slack absorbs that, plus
 * sub-pixel layout and a thumb that stopped a few pixels short — at which point the reader still
 * means "I am at the bottom", and the page should still follow.
 */
const BOTTOM_SLACK_PX = 24;

function atBottom(area, slack = BOTTOM_SLACK_PX) {
  return area.scrollHeight - area.scrollTop - (area.clientHeight || 0) <= slack;
}

/** The list the reader is actually looking at. Both panes share one scrolling element. */
function visibleList() {
  return el(currentView === "discord" ? "discord-log" : "transcript");
}

/**
 * The message the reader's eye is on: the first one not already scrolled off the top.
 *
 * This is the anchor everything below is measured against. A browser has its own scroll anchoring,
 * and it is exactly the wrong tool here — it is defeated by a mutation ABOVE the viewport, which
 * is precisely what collapsing a message the reader has already scrolled past is.
 */
function scrollAnchor(area) {
  const edge = area.getBoundingClientRect().top;
  const items = visibleList().children;
  for (const li of items) {
    if (li.getBoundingClientRect().bottom > edge) {
      return li;
    }
  }
  return items.length > 0 ? items[items.length - 1] : null;
}

/**
 * Run a mutation and leave the reader looking at the same thing afterwards.
 *
 * Capture the anchor's offset, mutate, measure it again, and move the scroll position by the
 * difference. Explicit, because the alternative is trusting browser scroll anchoring, and the
 * whole point is that it does not hold for a change made above the viewport.
 *
 * A reader who was already at the bottom is the one exception: for them "the same thing" IS the
 * newest line, so the view follows it.
 */
function preservingScroll(mutate) {
  const mark = captureScroll();
  mutate();
  restoreScroll(mark);
}

/**
 * Where the reader is: WHICH message their eye is on, and how far down the viewport it sits.
 *
 * Split out of `preservingScroll` so that `#51 reply-view` can use the same mechanism across a
 * screen change, where the two halves are separated by everything the reader does on the reply
 * screen. One mechanism, two callers: an anchor is strictly better than a saved `scrollTop` here
 * as well, because hiding an element is allowed to reset its scroll position to zero — and a
 * restore expressed as a DELTA against the anchor is correct whether the browser did that or not.
 */
function captureScroll() {
  const area = el("scroll-area");
  const anchor = scrollAnchor(area);
  return {
    pinned: atBottom(area),
    anchor,
    top: anchor ? anchor.getBoundingClientRect().top : 0,
  };
}

/** Put the reader back where `captureScroll` found them. */
function restoreScroll(mark) {
  const area = el("scroll-area");
  if (mark.pinned) {
    scrollToNewest();
    return;
  }
  // A list that was replaced while we were away has taken the anchor with it, and there is nothing
  // left to measure against; leaving the position alone beats jumping somewhere arbitrary.
  if (!mark.anchor || !mark.anchor.parentNode) {
    return;
  }
  area.scrollTop += mark.anchor.getBoundingClientRect().top - mark.top;
}

/**
 * A turn arrived. Follow it only if the reader had not scrolled away.
 *
 * `pinned` is measured BEFORE the append, because appending is itself what changes the answer.
 * When it is false the scroll position is left exactly alone and the "Newest" chip appears —
 * silently leaving the reader in place would be the other half of the same defect, since they
 * would have no way of knowing something had arrived at all.
 */
function followIfPinned(pinned) {
  if (pinned) {
    scrollToNewest();
    return;
  }
  // Named explicitly: this is only ever reached from `line()` and `seam()`, which append to the
  // transcript. Letting it default to `currentView` would raise a chip over the channel list.
  setJumpNewest(true, "voice");
}

/** The invitation only makes sense while there is nothing else in the transcript. */
function renderEmptyState() {
  el("empty-state").hidden = el("transcript").children.length > 0;
}

/** Wall-clock, as two numbers. A surface used as a debugging record has to say when. */
function stamp() {
  const now = new Date();
  const pad = (n) => String(n).padStart(2, "0");
  return `${pad(now.getHours())}:${pad(now.getMinutes())}`;
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
//   * COLLAPSED SIZE IS MEASURED IN LINES — three, by `-webkit-line-clamp` in web/voice.css. What
//     the reader is being protected from is a wall of text, and a wall is a number of lines. The
//     character count below decides only WHETHER to fold.
//   * A LONG MESSAGE ARRIVES ALREADY FOLDED. Folding it after the reader has seen it is a jump in
//     the list for no reason they asked for, which is the same class of defect as the scroll one.
//   * A SHORT MESSAGE GETS NO CONTROL AT ALL. A fold button that would reveal nothing, on every
//     line of the list, is chrome charging rent.
//   * THE STATE IS NOT PERSISTED across a reload. The voice transcript does not survive one today
//     and the channel view re-reads from the server, so there is nothing yet to persist it
//     against; worth revisiting if the transcript ever starts to survive a reload.
//   * COLLAPSE ALL IS ONE ACTION, not a preference. It puts the list back the way it arrived.
//
// 280 characters is about four lines at phone width — comfortably past the three the fold shows, so
// a folded message always has something behind it.
const COLLAPSE_OVER_CHARS = 280;

const FOLD_MORE = "More";
const FOLD_LESS = "Less";

// Every message that has a fold control, in the order it was rendered. Kept rather than re-queried
// so that "collapse all" is one pass over what exists; `liveFolds` drops the entries whose <li> has
// left the document, which is what a cleared transcript and a re-read channel both do.
const foldables = [];

function liveFolds() {
  for (let i = foldables.length - 1; i >= 0; i -= 1) {
    if (!foldables[i].li.parentNode) {
      foldables.splice(i, 1);
    }
  }
  return foldables;
}

function setFolded(entry, folded) {
  entry.li.setAttribute("data-collapsed", folded ? "true" : "false");
  entry.body.className = folded ? "body clamped" : "body";
  entry.fold.textContent = folded ? FOLD_MORE : FOLD_LESS;
  entry.fold.setAttribute("aria-expanded", folded ? "false" : "true");
}

const isFolded = (entry) => entry.li.getAttribute("data-collapsed") === "true";

/**
 * Fold this message if it is long enough to be worth folding.
 *
 * Called from both message lists with the pieces each of them has already built, so the control,
 * the class and the attribute are identical in the transcript and in the channel. Returns the
 * button, or null when the message is short and gets none.
 */
function foldable(li, meta, body, text) {
  if (String(text === null || text === undefined ? "" : text).length <= COLLAPSE_OVER_CHARS) {
    return null;
  }
  const fold = document.createElement("button");
  fold.className = "fold";
  fold.setAttribute("type", "button");
  const entry = { li, body, fold };
  setFolded(entry, true);
  // Toggling changes the height of something that may be far above the viewport, which is the one
  // case the browser's own scroll anchoring does not cover.
  fold.addEventListener("click", () => {
    preservingScroll(() => setFolded(entry, !isFolded(entry)));
    renderScrollTools();
  });
  meta.append(fold);
  foldables.push(entry);
  return fold;
}

// --- the two chips over the list ----------------------------------------------------------------
//
// Both are ABSENT unless there is something for them to do, for the same reason Hang up is absent
// when there is no call: a control that is always there and usually inert teaches the eye to skip
// the corner it lives in.

// PER LIST, not per page. Both lists live in one #scroll-area, so a single flag would raise the
// chip for a voice turn while the reader is looking at the channel — offering to jump them to the
// bottom of a list nothing arrived in — and would leave a channel arrival with no chip at all.
// The chip belongs to the list it is about.
const jumpNewestWanted = { voice: false, discord: false };

function setJumpNewest(wanted, view = currentView) {
  jumpNewestWanted[view] = wanted;
  renderScrollTools();
}

/** The folds in the list actually on screen. Collapse all acts on what you are looking at. */
function visibleFolds() {
  const list = visibleList();
  return liveFolds().filter((entry) => entry.li.parentNode === list);
}

function renderScrollTools() {
  el("jump-newest").hidden = !jumpNewestWanted[currentView];
  // "Collapse all" appears only once something IS expanded. Everything arrives folded, so until
  // the reader opens one there is nothing for it to collapse.
  el("collapse-all").hidden = !visibleFolds().some((entry) => !isFolded(entry));
}

function collapseAll() {
  preservingScroll(() => {
    for (const entry of visibleFolds()) {
      setFolded(entry, true);
    }
  });
  renderScrollTools();
}

/**
 * One turn.
 *
 * `mine` and `theirs` differ in side, colour and corner — three signals at once — because the two
 * speakers used to be told apart by nothing but a small grey word.
 */
function line(who, text) {
  const mine = who === "you";
  // Measured BEFORE the append: appending is what changes the answer.
  const pinned = atBottom(el("scroll-area"));
  const li = document.createElement("li");
  li.className = mine ? "mine" : "theirs";
  const meta = document.createElement("div");
  meta.className = "meta";
  const author = document.createElement("span");
  author.className = "who";
  author.textContent = who;
  const at = document.createElement("span");
  at.className = "at";
  at.textContent = stamp();
  meta.append(author, at);
  const body = document.createElement("div");
  body.className = "body";
  body.textContent = text; // untrusted text: never innerHTML.
  li.append(meta, body);
  el("transcript").append(li);
  foldable(li, meta, body, text);
  renderEmptyState();
  followIfPinned(pinned);
  return li;
}

/**
 * A boundary the PAGE drew, not something anybody said.
 *
 * These exist because a single unbroken list of messages is itself a claim — that it is all one
 * conversation — and that claim is sometimes false.
 *
 * What goes on the SURFACE is a thin rule carrying two or three words. The version before this one
 * printed four sentences here, about what a vendor does and does not document, and the control
 * pane cut them off mid-word; being cut off was the screen reporting that they did not belong on
 * it. The sentences are still available — inside the same element, one tap away on a phone and
 * named by `title` on a pointer device — and they are not standing on the screen.
 */
function seam(label, detail) {
  const pinned = atBottom(el("scroll-area"));
  const li = document.createElement("li");
  li.className = "seam";
  const details = document.createElement("details");
  const summary = document.createElement("summary");
  summary.className = "seam-summary";
  summary.setAttribute("title", detail);
  const word = document.createElement("span");
  word.className = "seam-label";
  word.textContent = label;
  const mark = document.createElement("span");
  mark.className = "seam-info";
  mark.textContent = "i";
  summary.append(word, mark);
  const body = document.createElement("p");
  body.className = "seam-detail";
  body.textContent = detail;
  // Opening it must not push the explanation underneath the dock, which is exactly where a
  // disclosure at the bottom of a scrolled list ends up if nothing moves.
  details.addEventListener("toggle", () => {
    if (details.open) {
      li.scrollIntoView({ block: "end" });
    }
  });
  details.append(summary, body);
  li.append(details);
  el("transcript").append(li);
  renderEmptyState();
  followIfPinned(pinned);
  return li;
}

// True from the moment a conversation opens until the end has been announced in the transcript.
let conversationOpen = false;

/**
 * Say, in the transcript itself, that the conversation ended.
 *
 * This is the honesty fix for the hang-up behaviour. ElevenLabs' Conversational AI WebSocket has
 * no documented way to resume a conversation after the socket closes: the initiation message and
 * the signed-URL endpoint both take an `agent_id` and neither accepts a `conversation_id`, and the
 * REST conversations API is transcript retrieval, not resumption. (Checked 2026-08-19 against the
 * vendor's Agent WebSocket API reference and get-signed-url reference. The docs do not say resume
 * is impossible; they provide no field through which it could be requested.) So the honest thing
 * is to mark the break rather than to promise a resume, and mute — which never closes the socket —
 * is the control that actually preserves context.
 */
// What the seam says INSIDE its disclosure, per cause. The LABEL is the same three words in every
// case, deliberately: the boundary is the same boundary — the agent below it has never seen
// anything above it — however the socket happened to close. Only the explanation differs, because
// only the cause differs, and each of these is a couple of clauses because the suite measures
// them. `#54 resume-recovery`.
const SEAM_DETAILS = {
  ended:
    "Anything below this line goes to an agent that has never seen anything above it. " +
    "Mute, not Hang up, keeps its memory.",
  suspended:
    "The call dropped while this page was in the background. Resuming starts a NEW " +
    "conversation: the agent below this line remembers nothing above it.",
  failed:
    "The connection dropped. Anything below this line goes to an agent that has never seen " +
    "anything above it.",
};

function seamDetailFor(cause) {
  return SEAM_DETAILS[cause] || SEAM_DETAILS.ended;
}

function noteConversationEnded(cause = "ended") {
  if (!conversationOpen) {
    return;
  }
  conversationOpen = false;
  // From here the large control is a different offer — a NEW call, from nothing — and it says so.
  hasEnded = true;
  // ...and if the drop was a suspension, the offer has a different WORD on it, because "Start a
  // new call" reads as an invitation to begin something and this is an invitation to carry on.
  hasSuspended = cause === "suspended";
  renderControls();
  // Two facts, and only two: what the line means, and which control would have avoided it. The
  // version before this one was fifty-seven words of vendor archaeology one tap inside a
  // disclosure, which is not shorter than a paragraph — it is a paragraph nobody opens. The rest
  // of it (why a hang-up loses the context, and that the lines above are still your own record)
  // lives in Settings under "What the controls do", which is where the long form belongs.
  //
  // Drawn at the moment of the DROP, not when Resume is tapped: the boundary is where the
  // conversation actually broke, and marking it later would put the reader's own next turn on the
  // wrong side of it.
  seam("new conversation", seamDetailFor(cause));
}

// --- clearing the transcript ---------------------------------------------------------------
//
// This control moved out of the header and into the pane, under a thumb. In the header it sat
// beside a notice calling the transcript the only surviving record, which invited exactly the
// reading it must not have — that it destroys something irreplaceable. In the pane it is honestly
// grouped with the other things you DO, and it is the one destructive thing there.
//
// So it asks twice. The first tap arms it and changes both its word and its colour; the second,
// within a few seconds, clears. A control that erases the record on one accidental tap is not made
// safe by a label.

const CLEAR_ARMED_MS = 4000;
let clearArmedTimer = null;

function disarmClear() {
  if (clearArmedTimer !== null) {
    clearTimeout(clearArmedTimer);
    clearArmedTimer = null;
  }
  el("clear-view").className = "control control-mini";
  el("clear-view-label").textContent = "Clear";
}

function armClear() {
  el("clear-view").className = "control control-mini armed";
  el("clear-view-label").textContent = "Sure?";
  if (clearArmedTimer !== null) {
    clearTimeout(clearArmedTimer);
  }
  clearArmedTimer = setTimeout(disarmClear, CLEAR_ARMED_MS);
}

const clearIsArmed = () => clearArmedTimer !== null;

/**
 * Clear the SCREEN. Nothing else.
 *
 * The ambiguous middle — the screen empties and the agent carries on with everything the operator
 * thought they had removed — is the one outcome this must not have. So during a call the very
 * first thing back on the empty screen is a seam saying so, in two words, with the sentence one
 * tap inside it. Ending the conversation is a different and heavier action with its own control.
 */
function onClear() {
  if (!clearIsArmed()) {
    armClear();
    setStatus("Tap Clear again to empty the transcript.");
    return;
  }
  disarmClear();
  el("transcript").replaceChildren();
  renderEmptyState();
  renderScrollTools(); // the folds went with the lines they were attached to.
  if (session.socket) {
    seam(
      "view cleared",
      "The screen was emptied; nothing else was. The agent still has everything said before " +
        "this point. Hang up is what ends the call."
    );
    setStatus("Transcript cleared. The agent has not forgotten anything.");
  } else {
    setStatus("Transcript cleared.");
  }
}

// --- the server ------------------------------------------------------------------------------

/**
 * One request to gent-talk, with the token, and one error taxonomy out of it.
 *
 * The server answers with a real taxonomy — 503 `elevenlabs_not_configured` names the exact
 * setting that is missing, 502 `elevenlabs_error` carries the vendor's status and message,
 * including things only the vendor knows, such as an API key that lacks the `convai_write`
 * permission. Flattening that to "could not start" would throw away the only sentence that says
 * what to fix, so all of it is passed through.
 */
async function api(path, options) {
  const headers = { Authorization: `Bearer ${token()}` };
  const init = { method: (options && options.method) || "GET", headers };
  // Only when there IS one. A GET with a Content-Type and no body is a request that says it is
  // carrying JSON and is not, which some proxies treat as a malformed request rather than as a
  // harmless extra header.
  if (options && options.body !== undefined) {
    headers["Content-Type"] = "application/json";
    init.body = JSON.stringify(options.body);
  }
  const response = await fetch(path, init);
  const text = await response.text();
  let payload = null;
  try {
    payload = text ? JSON.parse(text) : null;
  } catch (_error) {
    throw new Error(`gent-talk returned non-JSON (HTTP ${response.status})`);
  }
  if (!response.ok) {
    const detail = payload && payload.detail ? payload.detail : "(no detail)";
    const code = payload && payload.error ? payload.error : "error";
    const error = new Error(`HTTP ${response.status} ${code}: ${detail}`);
    // Only these two mean "your token is wrong". Everything else means the server is unhappy for
    // a reason that signing in again will not fix, and bouncing the owner back to the sign-in
    // screen for those would be a lie about which thing is broken.
    error.refused = response.status === 401 || response.status === 403;
    throw error;
  }
  return payload;
}

async function mintSignedUrl() {
  if (!token()) {
    throw new Error("no API token saved on this phone yet");
  }
  const payload = await api("/api/v1/signed-url");
  if (!payload || !payload.signed_url) {
    throw new Error("gent-talk answered without a signed URL");
  }
  return payload;
}

// --- audio helpers ---------------------------------------------------------------------------

function base64ToBytes(b64) {
  const binary = atob(b64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

function bytesToBase64(bytes) {
  let binary = "";
  const chunk = 0x8000; // apply() has an argument-count limit; chunk to stay under it.
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode.apply(null, bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

// Linear resample to 16 kHz, which is the input format the agent expects. Doing this explicitly
// beats asking for an AudioContext at 16 kHz and hoping: a browser is allowed to give you a
// different rate, and the failure is silent and sounds like a chipmunk.
function downsampleTo16k(input, inputRate) {
  if (inputRate === 16000) {
    return input;
  }
  const ratio = inputRate / 16000;
  const out = new Float32Array(Math.floor(input.length / ratio));
  for (let i = 0; i < out.length; i += 1) {
    const at = i * ratio;
    const low = Math.floor(at);
    const high = Math.min(low + 1, input.length - 1);
    const frac = at - low;
    out[i] = input[low] * (1 - frac) + input[high] * frac;
  }
  return out;
}

function floatToPcm16(samples) {
  const out = new Int16Array(samples.length);
  for (let i = 0; i < samples.length; i += 1) {
    const clamped = Math.max(-1, Math.min(1, samples[i]));
    out[i] = clamped < 0 ? clamped * 0x8000 : clamped * 0x7fff;
  }
  return out;
}

// "pcm_16000" -> 16000. Anything else (ulaw, mp3) is refused rather than played as noise.
function outputRateFrom(format) {
  if (!format) {
    return 16000; // the documented default for this websocket.
  }
  const match = /^pcm_(\d+)$/.exec(format);
  if (!match) {
    throw new Error(
      `the agent negotiated audio format "${format}", which this page cannot decode. ` +
        "Set the agent's output format to PCM (pcm_16000) in the ElevenLabs dashboard."
    );
  }
  return Number(match[1]);
}

function playPcm(b64) {
  const bytes = base64ToBytes(b64);
  const pcm = new Int16Array(bytes.buffer, bytes.byteOffset, Math.floor(bytes.byteLength / 2));
  const buffer = session.audio.createBuffer(1, pcm.length, session.outputRate);
  const channel = buffer.getChannelData(0);
  for (let i = 0; i < pcm.length; i += 1) {
    channel[i] = pcm[i] / 0x8000;
  }
  const node = session.audio.createBufferSource();
  node.buffer = buffer;
  node.connect(session.audio.destination);
  const now = session.audio.currentTime;
  session.playAt = Math.max(session.playAt, now + 0.05);
  node.start(session.playAt);
  session.playAt += buffer.duration;
  session.playing.push(node);
  node.onended = () => {
    session.playing = session.playing.filter((n) => n !== node);
  };
}

function stopPlayback() {
  for (const node of session.playing) {
    try {
      node.stop();
    } catch (_error) {
      // already finished; nothing to do.
    }
  }
  session.playing = [];
  session.playAt = 0;
}

// --- microphone settings ------------------------------------------------------------------------

// Three knobs on the audio the browser hands us. They are exposed because each one has a failure
// mode the owner has actually hit, and none of them is guessable from the outside: the agent
// answering its own voice on speakerphone, background noise reaching the transcriber, and — the
// awkward one — automatic gain amplifying a silent room until a phone being set down crosses the
// speech threshold and gets transcribed as a word.
const MIC_TOGGLES = [
  ["mic-echo-cancellation", "echoCancellation"],
  ["mic-noise-suppression", "noiseSuppression"],
  ["mic-auto-gain", "autoGainControl"],
];

const DEFAULT_MIC_SETTINGS = {
  echoCancellation: true, // stated explicitly today.
  noiseSuppression: true, // stated explicitly today.
  autoGainControl: true, // NOT stated today; see `audioConstraints`.
};

/** What the checkboxes say right now. The DOM is the live truth; storage only carries it across reloads. */
function micSettings() {
  const settings = {};
  for (const [id, key] of MIC_TOGGLES) {
    settings[key] = el(id).checked === true;
  }
  return settings;
}

/** What was stored, defaulted per key. A corrupt or partial entry falls back to today's behaviour. */
function storedMicSettings() {
  const settings = { ...DEFAULT_MIC_SETTINGS };
  let stored = null;
  try {
    stored = JSON.parse(localStorage.getItem(MIC_SETTINGS_KEY) || "null");
  } catch (_error) {
    stored = null;
  }
  if (stored && typeof stored === "object") {
    for (const key of Object.keys(settings)) {
      if (typeof stored[key] === "boolean") {
        settings[key] = stored[key];
      }
    }
  }
  return settings;
}

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

function persistMicSettings(settings) {
  const encoded = JSON.stringify(settings);
  try {
    localStorage.setItem(MIC_SETTINGS_KEY, encoded);
  } catch (_error) {
    return false;
  }
  // Read it back rather than assuming, exactly as saving the token does: private browsing accepts
  // setItem and stores nothing, and "saved" would then be a lie found out only after a reload.
  return localStorage.getItem(MIC_SETTINGS_KEY) === encoded;
}

// A toggle flipped mid-call does NOT reach the open microphone — the constraints were read when
// the stream opened. Saying "saved" and stopping there would leave a control that looks like it
// did something; so the line under the toggles states which call the change lands on.
function micSettingsChanged() {
  const saved = persistMicSettings(micSettings());
  const when = session.stream
    ? "The microphone is already open, so this does NOT change the call in progress — hang up " +
      "and start again to apply it."
    : "It applies the next time you start talking.";
  el("mic-settings-state").textContent = saved
    ? `Saved. ${when}`
    : "This browser refused to store the setting, so it will be forgotten when you reload " +
      `(private browsing does this). ${when}`;
}

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

const MIN_READING_CH = 45;
const MAX_READING_CH = 120;
const DEFAULT_READING_CH = 72;

/**
 * A width this page will actually apply.
 *
 * Storage is not a trusted input: it is shared with whatever else runs on this origin, it survives
 * a version of the page that had different limits, and a hand-edited entry is a thing people do.
 * A stored 4000 must become a column, not a window with no margins, so the value is clamped on the
 * way IN as well as on the way out — and anything that is not a number at all falls back to the
 * default rather than to NaN, which CSS would ignore silently.
 */
function clampReadingWidth(value) {
  // A blank entry is ABSENT, not zero. `Number("")` is 0 and `Number(null)` is 0, and taking
  // either at its word would clamp an empty storage slot to the narrowest column the page allows
  // — a reader who had never touched the control would find it already moved.
  const text = typeof value === "string" ? value.trim() : value;
  if (text === "" || text === null || text === undefined) {
    return DEFAULT_READING_CH;
  }
  const ch = Number(text);
  if (!Number.isFinite(ch)) {
    return DEFAULT_READING_CH;
  }
  return Math.min(MAX_READING_CH, Math.max(MIN_READING_CH, Math.round(ch)));
}

/** What was stored, clamped. A missing or corrupt entry is the default, never an error. */
function storedReadingWidth() {
  const stored = localStorage.getItem(WIDTH_KEY);
  return clampReadingWidth(stored === null ? DEFAULT_READING_CH : stored);
}

// The width currently in force, kept here rather than read back out of the DOM: reading a custom
// property back gives you the string that was written, and a drag would then be parsing its own
// output forty times a second.
let readingWidth = DEFAULT_READING_CH;

/**
 * Put a width on the page. Returns the width that was actually applied, which is the clamped one.
 *
 * The custom property goes on `document.documentElement` because the two things it has to reach —
 * the panes inside #screen-main and the control pane inside the dock — have no common ancestor
 * below the root.
 */
function applyReadingWidth(value) {
  readingWidth = clampReadingWidth(value);
  document.documentElement.style.setProperty("--reading-width", `${readingWidth}ch`);
  el("reading-width").value = String(readingWidth);
  el("width-grip").setAttribute("aria-valuenow", String(readingWidth));
  el("width-grip").setAttribute("aria-valuetext", `${readingWidth} characters`);
  return readingWidth;
}

/**
 * Store it, and READ IT BACK rather than assuming — the same check `persistMicSettings` makes, for
 * the same reason: private browsing accepts setItem and stores nothing, so "saved" would be a lie
 * discovered only after a reload.
 */
function persistReadingWidth(ch) {
  const encoded = String(ch);
  try {
    localStorage.setItem(WIDTH_KEY, encoded);
  } catch (_error) {
    return false;
  }
  return localStorage.getItem(WIDTH_KEY) === encoded;
}

/** Apply a new width and say, in the settings screen, whether it will survive a reload. */
function readingWidthChanged(value) {
  const ch = applyReadingWidth(value);
  el("reading-width-state").textContent = persistReadingWidth(ch)
    ? `Saved — a column ${ch} characters wide.`
    : "This browser refused to store the reading width, so it will be forgotten when you " +
      `reload (private browsing does this). The column is ${ch} characters wide until then.`;
}

// A drag, in three events. Pointer events rather than mouse events so a trackpad, a pen and a
// desktop touchscreen all work through one path; the regime that shows the handle at all is the
// stylesheet's business, not this code's.
let widthDrag = null;

/**
 * How many pixels one character is, measured from the column that is actually on screen rather
 * than assumed. `1ch` is the width of a zero in the current font, which is neither a constant nor
 * knowable from here — but the pane IS `max-width: var(--reading-width)`, so its rendered width
 * divided by the width in force is the conversion factor, whatever the font turns out to be.
 */
function pixelsPerCh() {
  const measured = el("pane-voice").getBoundingClientRect().width;
  if (measured > 0 && readingWidth > 0) {
    return measured / readingWidth;
  }
  return 8; // nothing laid out yet; a plausible figure beats dividing by zero.
}

function onGripDown(event) {
  const box = el("screen-main").getBoundingClientRect();
  // The column is centred, so the handle's distance from the middle is HALF the width. Captured
  // once at the start of the drag: re-measuring mid-drag would feed the drag its own output.
  widthDrag = { centre: box.left + box.width / 2, perCh: pixelsPerCh() };
  const grip = el("width-grip");
  if (event && event.pointerId !== undefined && grip.setPointerCapture) {
    // So the drag survives the pointer leaving the nine-pixel handle, which it does immediately.
    grip.setPointerCapture(event.pointerId);
  }
}

function onGripMove(event) {
  if (!widthDrag || !event) {
    return;
  }
  applyReadingWidth(((event.clientX - widthDrag.centre) * 2) / widthDrag.perCh);
}

function onGripUp() {
  if (!widthDrag) {
    return;
  }
  widthDrag = null;
  // Stored at the END of the drag, not on every frame: forty writes a second to localStorage is a
  // synchronous disk hit per frame, and only the width the reader stopped at is a decision.
  readingWidthChanged(readingWidth);
}

// A separator with `tabindex` is only a control if the arrow keys move it. Shift for a bigger
// step, Home/End for the ends, which is what a range input does and therefore what a reader who
// has used one expects.
const GRIP_KEYS = {
  ArrowLeft: () => readingWidth - 1,
  ArrowRight: () => readingWidth + 1,
  ArrowDown: () => readingWidth - 1,
  ArrowUp: () => readingWidth + 1,
  PageDown: () => readingWidth - 10,
  PageUp: () => readingWidth + 10,
  Home: () => MIN_READING_CH,
  End: () => MAX_READING_CH,
};

function onGripKey(event) {
  const next = event && GRIP_KEYS[event.key];
  if (!next) {
    return;
  }
  if (event.preventDefault) {
    event.preventDefault(); // PageUp and the arrows would otherwise scroll the list behind it.
  }
  readingWidthChanged(next());
}

// --- a call that was suspended, not lost ------------------------------------------------------
//
// `#54 resume-recovery`. Put the phone in your pocket mid-call and iOS suspends the page; the
// WebSocket dies, and this page used to greet you on your return with a red panel saying the
// connection to the voice agent had FAILED. Nothing had failed. You switched apps.
//
// The fix is a classification, not a reconnection. Three things can end a socket and they are
// three different sentences:
//
//   * the page was backgrounded         -> Paused, and the large control offers to Resume
//   * something really broke            -> the error panel, unchanged, because it is real
//   * the call ended                    -> what the page already said
//
// NOTHING AUTO-RECONNECTS. Coming back to a phone that has quietly reopened the microphone and
// started a conversation nobody asked for is a worse outcome than the banner this replaces, and
// the issue rules it out. Resume is a tap, and it runs the ordinary `start()` — which mints a
// FRESH signed URL every time, so the "expired credential" case the issue worries about cannot
// arise: the page has never reused one.

// How long after the page comes back a close still counts as part of the suspension. iOS commonly
// delivers the close on the way BACK rather than while hidden, so a window with no grace at all
// would miss the exact case this exists for. Too wide and a genuine drop moments after a tab
// switch gets excused, which is why the suite carries a negative control for a VISIBLE failure.
const SUSPENSION_GRACE_MS = 2500;

// `onerror` arms this; `onclose` cancels it. Short, because it is only bridging the gap between
// two events the browser fires back to back.
const FAILURE_REPORT_MS = 250;
let failureTimer = null;

let hiddenDuringCall = false;
let visibleAt = 0;

// Short on purpose: `#status` is one line with `white-space: nowrap` and an ellipsis, so anything
// longer than about forty characters is simply not readable on a phone.
const SUSPENDED_STATUS = "Paused — the app was in the background.";

function onVisibility() {
  if (document.visibilityState === "hidden") {
    // Only a call can be suspended. Hiding an idle page is not an event.
    if (session.socket) {
      hiddenDuringCall = true;
    }
    return;
  }
  visibleAt = Date.now();
}

function wasSuspended() {
  return (
    hiddenDuringCall &&
    (document.visibilityState === "hidden" || Date.now() - visibleAt < SUSPENSION_GRACE_MS)
  );
}

function reportFailure() {
  failureTimer = null;
  showError(
    "The connection to the voice agent failed. The signed URL was minted, so gent-talk and " +
      "your token are fine; the failure is between this browser and ElevenLabs."
  );
  setStatus("The connection to the voice agent failed.");
}

// --- the call ----------------------------------------------------------------------------------

async function start() {
  if (session.socket) {
    setStatus("already connected");
    return;
  }
  clearError();
  // A new call is a clean slate for all three of these. `hasSuspended` in particular: leaving it
  // set would keep the large control reading "Resume" during and after the call it started.
  session.failed = false;
  hiddenDuringCall = false;
  hasSuspended = false;
  setState("working");
  setStatus("Asking gent-talk for a signed URL…");
  const minted = await mintSignedUrl();
  showDetail(
    `agent ${minted.agent_id}; signed URL valid for about ${Math.round(
      (minted.valid_for_seconds || 900) / 60
    )} minutes`
  );

  setStatus("Asking for the microphone…");
  // Read HERE, at the moment the stream opens — which is why a toggle flipped mid-call cannot
  // reach this one, and why `micSettingsChanged` says so out loud.
  session.stream = await navigator.mediaDevices.getUserMedia(audioConstraints(micSettings()));
  session.audio = new (window.AudioContext || window.webkitAudioContext)();
  await session.audio.resume(); // iOS starts it suspended until a gesture.

  setState("working");
  const socket = new WebSocket(minted.signed_url);
  session.socket = socket;
  session.muted = false;
  renderControls();

  socket.onopen = () => {
    socket.send(JSON.stringify({ type: "conversation_initiation_client_data" }));
    session.connected = true;
    conversationOpen = true;
    setState("live");
    setStatus("Connected — say something.");
    renderControls();
    startCapture(socket);
  };

  socket.onmessage = (event) => {
    let message = null;
    try {
      message = JSON.parse(event.data);
    } catch (_error) {
      return; // a frame we do not understand is not a reason to tear the call down.
    }
    // `handle` throws for a format this page cannot decode, and `onmessage` is not inside
    // `guard()` — it is called by the browser, not by us — so an unhandled throw here would land
    // in the console, which is exactly where this page's failures used to go to die. Against the
    // in-page wire fake that could not happen; against a real socket it is one negotiated setting
    // away.
    try {
      handle(socket, message);
    } catch (error) {
      showError(error.message);
      setStatus("the agent sent something this page cannot handle");
    }
  };

  socket.onerror = () => {
    // NOT `showError` any more. `onerror` fires BEFORE the close, and only the close knows whether
    // this browser had backgrounded the page — which is the difference between "something is
    // broken" and "you switched apps". So this records the fact and arms a short timer; the close
    // cancels it and classifies. An error with no close following still reaches the screen, which
    // is what the timer is for: silence would be the old bug in a new coat.
    session.failed = true;
    if (failureTimer !== null) {
      clearTimeout(failureTimer);
    }
    failureTimer = setTimeout(reportFailure, FAILURE_REPORT_MS);
  };

  socket.onclose = (event) => {
    // THE one classifier. Three outcomes, and they are three different things to say:
    //
    //   suspended  the page was in the background — recoverable, and no failure happened
    //   failed     something really went wrong between this browser and ElevenLabs
    //   ended      the call is over, either because it was hung up or because it ran out
    //
    // The old code had two of these and reported the first as the second.
    if (failureTimer !== null) {
      clearTimeout(failureTimer);
      failureTimer = null;
    }
    const cause = wasSuspended() ? "suspended" : session.failed ? "failed" : "ended";
    if (cause === "failed") {
      reportFailure();
    } else {
      setState(cause);
      // A signed-URL failure also shows up here, as an immediate close, so the page has to say
      // something — but "code 1005" is not something. It says what happened in words; the number
      // goes where numbers belong, in the connection details on the settings screen.
      setStatus(cause === "suspended" ? SUSPENDED_STATUS : closeReason(event.code));
    }
    // The banner is about a conversation that no longer exists, and the code is the one thing on
    // this page that must not be read as user-facing. Put the banner away FIRST, then record the
    // number where numbers belong: the connection details on the settings screen.
    dismissBanner();
    addDetail(`closed with code ${event.code}${event.reason ? `: ${event.reason}` : ""}`);
    teardown();
    noteConversationEnded(cause);
  };
}

/**
 * A close code, in plain words.
 *
 * The owner's screen read "conversation closed (code 1005)". 1005 means the connection gave no
 * reason at all, so the page was reporting the ABSENCE of information as though it were
 * information, in a vocabulary only a WebSocket implementer has. Say what happened, or say
 * nothing; the number is still recorded in the connection details for whoever is debugging.
 */
function closeReason(code) {
  if (code === 1000) {
    return "Call ended.";
  }
  if (code === 1001 || code === 1005 || code === 1006) {
    return "The call ended — the connection dropped.";
  }
  return "The call ended unexpectedly. Settings has the details.";
}

function handle(socket, message) {
  switch (message.type) {
    case "conversation_initiation_metadata": {
      const meta = message.conversation_initiation_metadata_event || {};
      session.outputRate = outputRateFrom(meta.agent_output_audio_format);
      addDetail(
        `conversation ${meta.conversation_id || "?"} · agent audio ${
          meta.agent_output_audio_format || "pcm_16000"
        }`
      );
      break;
    }
    case "audio":
      // Sound off silences the agent's VOICE, not the agent: the frame is dropped here, and the
      // `agent_response` case below still writes what it said into the transcript.
      if (!session.speakerOff) {
        playPcm((message.audio_event || {}).audio_base_64);
      }
      break;
    case "agent_response":
      // "assistant", not "agent". This page's sibling view is a channel full of CODING agents
      // posting under their own names, and a transcript that labels the voice as "agent" invites
      // the reader to think one of those is talking. Only the displayed word changes: `line()`
      // still tells the two speakers apart by `who === "you"`, so side, tint and corner are
      // untouched.
      line("assistant", (message.agent_response_event || {}).agent_response || "");
      break;
    case "user_transcript":
      line("you", (message.user_transcription_event || {}).user_transcript || "");
      break;
    case "interruption":
      stopPlayback();
      break;
    case "ping":
      socket.send(
        JSON.stringify({ type: "pong", event_id: (message.ping_event || {}).event_id })
      );
      break;
    default:
      break;
  }
}

function startCapture(socket) {
  const rate = session.audio.sampleRate;
  // ScriptProcessorNode is deprecated in favour of AudioWorklet, and is still the only capture
  // path that needs no separate module file. A worklet would mean a second asset or a blob URL,
  // for no behavioural gain on a page this small.
  session.source = session.audio.createMediaStreamSource(session.stream);
  session.node = session.audio.createScriptProcessor(4096, 1, 1);
  session.node.onaudioprocess = (event) => {
    if (socket.readyState !== WebSocket.OPEN) {
      return;
    }
    // MUTE LIVES HERE, and nowhere else.
    //
    // It withholds frames. It does not stop the microphone track, it does not close the socket,
    // and it does not tear down the audio graph. That is the entire mechanism, and it is what the
    // control is FOR:
    //
    //   * The conversation stays open, so the AGENT KEEPS ITS CONTEXT. This is the whole reason
    //     mute exists rather than a hang-up-and-redial: hanging up loses the context, and the
    //     vendor documents no way to resume a conversation once the socket has closed. Mute is
    //     therefore the only pause this page can offer that the agent survives.
    //   * Nothing is torn down, so nothing has to be rebuilt. Stopping the track and re-acquiring
    //     it would rebuild the capture graph in the middle of a conversation, which is disruptive
    //     to no purpose when the goal is simply to stop being heard for a while.
    //
    // Anyone tidying `stop()` later: `track.stop()` MUST NOT become reachable from here, not even
    // conditionally. It ends the conversation, and with it everything the agent knows.
    if (session.muted) {
      return;
    }
    const mono = event.inputBuffer.getChannelData(0);
    const pcm = floatToPcm16(downsampleTo16k(mono, rate));
    socket.send(
      JSON.stringify({
        user_audio_chunk: bytesToBase64(new Uint8Array(pcm.buffer)),
      })
    );
  };
  session.source.connect(session.node);
  // A ScriptProcessorNode only fires while it is connected to the graph. Routing it at zero gain
  // keeps it running without echoing the microphone into the speaker.
  const silence = session.audio.createGain();
  silence.gain.value = 0;
  session.node.connect(silence);
  silence.connect(session.audio.destination);
}

function teardown() {
  stopPlayback();
  if (session.node) {
    session.node.disconnect();
    session.node.onaudioprocess = null;
  }
  if (session.source) {
    session.source.disconnect();
  }
  if (session.stream) {
    // Hang up — and ONLY hang up — releases the microphone. `teardown()` runs when the
    // conversation is over, and the conversation being over is precisely what makes releasing the
    // stream correct here. Mute must never reach this line: it would end the call and lose the
    // agent's context, which is the one thing mute exists to preserve.
    for (const track of session.stream.getTracks()) {
      track.stop();
    }
  }
  if (session.audio) {
    session.audio.close();
  }
  session.node = null;
  session.source = null;
  session.stream = null;
  session.audio = null;
  session.socket = null;
  session.connected = false;
  session.muted = false;
  renderControls();
}

function stop() {
  // With no call there is no Hang up in the pane at all, so this is unreachable from the screen;
  // it stays as a guard rather than as a second way to end something that is already over.
  if (!session.socket) {
    return;
  }
  session.socket.close();
  setStatus("Call ended.");
  setState("ended");
  teardown();
  noteConversationEnded();
}

// --- the controls --------------------------------------------------------------------------------
//
// The pane has three shapes, and each one offers exactly what there is to do:
//
//   idle          one action, both large columns:  Talk
//   live          two actions:                     Hang up · Listening/Muted
//   after a call  one action, both large columns:  Start a new call, with the memory caveat on it
//
// Hang up is ABSENT rather than dimmed when there is no call. It used to sit there fully
// saturated — the loudest thing on a screen that was simultaneously saying, three times over,
// that the call had ended.

// True once a call has ended in this session, so the idle pane can say "Talk" the first time and
// "Start a new call" afterwards. They are different offers: the second one starts from nothing.
let hasEnded = false;

// ...and true when the reason it ended was a SUSPENSION rather than anything the reader did or
// anything that broke. Same offer, different word: "Resume" reads as carrying on, which is what
// the reader is trying to do. The clause under it refuses to imply the continuity they might
// otherwise assume from that word. `#54 resume-recovery`.
let hasSuspended = false;

function renderControls() {
  const talk = el("talk");
  const label = el("talk-label");
  const note = el("talk-note");
  const live = Boolean(session.socket);

  el("hang-up").hidden = !live;
  el("control-pane").className = live ? "" : "solo";

  note.hidden = true;
  note.textContent = "";

  if (!live) {
    talk.className = "control control-talk";
    if (hasSuspended) {
      label.textContent = "Resume";
    } else {
      label.textContent = hasEnded ? "Start a new call" : "Talk";
    }
    if (hasEnded) {
      // "Resume" is the honest word for what the reader wants and a dishonest word for what the
      // agent gets, so the clause under it says which of the two this is.
      note.textContent = hasSuspended ? "a new call — the agent starts fresh" : "the agent starts fresh";
      note.hidden = false;
    }
    return;
  }
  if (!session.connected) {
    talk.className = "control control-talk";
    label.textContent = "Connecting…";
    return;
  }
  if (session.muted) {
    // The slash across the microphone comes from this class; see web/voice.css.
    talk.className = "control control-talk muted";
    label.textContent = "Muted";
    return;
  }
  // `live` is what runs the pulse animation.
  talk.className = "control control-talk live";
  label.textContent = "Listening";
}

function setMuted(muted) {
  session.muted = muted;
  renderControls();
  // One line, because there is one line. The three sentences this used to be are in Settings,
  // under "What the controls do", where somebody who wants them can read them.
  setStatus(muted ? "Muted — the agent still remembers." : "Listening — say something.");
}

/**
 * Silence the agent's VOICE, not the agent.
 *
 * Audio frames are dropped in `handle`; `agent_response` keeps writing what the agent said into
 * the transcript. So this is the control for reading the agent in a room where you cannot listen
 * to it, and it deliberately does not touch the microphone, the socket, or mute.
 */
function setSpeakerOff(off) {
  session.speakerOff = off;
  if (off) {
    stopPlayback(); // whatever is already scheduled would otherwise keep talking.
  }
  el("speaker").className = off ? "control control-mini off" : "control control-mini on";
  el("speaker").setAttribute("aria-pressed", off ? "true" : "false");
  el("speaker-label").textContent = off ? "Silent" : "Sound";
  setStatus(
    off ? "Agent voice off — its replies still arrive as text." : "Agent voice on."
  );
}

/** The one large control: start a call when idle, toggle mute when one is live. */
function onTalk() {
  if (!session.socket) {
    return guard(start)();
  }
  setMuted(!session.muted);
  return Promise.resolve();
}

function guard(fn) {
  return (...args) =>
    Promise.resolve(fn(...args)).catch((error) => {
      // Both places: the panel is what the owner reads, the status line is the one-liner. Neither
      // is the console.
      showError(error.message);
      setStatus("could not start the conversation");
      teardown();
    });
}

// --- raw Discord, rendered ------------------------------------------------------------------
//
// The value of this view is being able to point at a specific real message — the agent has
// described messages that did not exist. So every line carries its author and its message id.
//
// Channel text is third-party data written by whoever is in the channel. NOTHING below turns it
// into markup: there is no innerHTML, no insertAdjacentHTML, no template string that becomes a
// document. Every fragment of a message becomes an element created HERE whose text is assigned
// with textContent, which is escaping by construction rather than escaping by remembering. The
// markdown subset is deliberately small, and the only sink that is not plain text — a link's href —
// is scheme-checked before anything is written to it.

/** Only these become clickable. Everything else is shown as the text it is. */
const SAFE_LINK = /^https?:\/\//i;

/**
 * One inline construct.
 *
 * Groups, in the order they are tried: code span, bold, strikethrough, italic (asterisk), italic
 * (underscore), link, user mention, channel mention. Code is first so that backticked text is
 * taken verbatim; a construct is not parsed across a line break.
 */
const INLINE =
  /`([^`\n]+)`|\*\*([^*\n]+)\*\*|~~([^~\n]+)~~|\*([^*\n]+)\*|_([^_\n]+)_|\[([^\]\n]*)\]\(([^)\s]*)\)|<@!?(\d+)>|<#(\d+)>/g;

function styled(tag, className, text) {
  const node = document.createElement(tag);
  if (className) {
    node.className = className;
  }
  node.textContent = text;
  return node;
}

/**
 * A run of plain message text.
 *
 * A span rather than a text node so that every fragment is built the same way, through the one
 * function that assigns textContent — there is no second path to audit.
 */
function plain(text) {
  return styled("span", "", text);
}

function mdLink(label, href) {
  // A URL is a sink. `javascript:` and `data:` execute; a scheme-less string resolves against this
  // origin and can be made to look like somewhere else entirely. None of those becomes a tappable
  // link — the message is shown with its URL as visible text instead, which is strictly more
  // informative than a link the operator cannot inspect on a phone.
  if (!SAFE_LINK.test(href)) {
    return plain(`${label} (${href})`);
  }
  const anchor = document.createElement("a");
  anchor.textContent = label;
  anchor.setAttribute("href", href);
  anchor.setAttribute("rel", "noopener noreferrer nofollow");
  anchor.setAttribute("target", "_blank");
  return anchor;
}

function renderInline(parent, text) {
  INLINE.lastIndex = 0;
  let at = 0;
  let match = INLINE.exec(text);
  while (match !== null) {
    if (match.index > at) {
      parent.append(plain(text.slice(at, match.index)));
    }
    at = match.index + match[0].length;
    if (match[1] !== undefined) {
      parent.append(styled("code", "md-code", match[1]));
    } else if (match[2] !== undefined) {
      parent.append(styled("strong", "", match[2]));
    } else if (match[3] !== undefined) {
      parent.append(styled("s", "", match[3]));
    } else if (match[4] !== undefined) {
      parent.append(styled("em", "", match[4]));
    } else if (match[5] !== undefined) {
      parent.append(styled("em", "", match[5]));
    } else if (match[6] !== undefined) {
      parent.append(mdLink(match[6], match[7]));
    } else if (match[8] !== undefined) {
      // Rendered as the id, not as a name: this page has no user directory, and inventing a
      // display name here is exactly the kind of thing this view exists to catch.
      parent.append(styled("span", "md-mention", `@${match[8]}`));
    } else if (match[9] !== undefined) {
      parent.append(styled("span", "md-mention", `#${match[9]}`));
    }
    match = INLINE.exec(text);
  }
  if (at < text.length) {
    parent.append(plain(text.slice(at)));
  }
}

const FENCE = /^\s*```/;
const QUOTE = /^\s*>\s?/;

function renderMarkdownInto(parent, raw) {
  const lines = String(raw === null || raw === undefined ? "" : raw).split("\n");
  let i = 0;
  while (i < lines.length) {
    if (FENCE.test(lines[i])) {
      const body = [];
      i += 1;
      while (i < lines.length && !FENCE.test(lines[i])) {
        body.push(lines[i]);
        i += 1;
      }
      i += 1; // the closing fence, or the end of an unclosed block.
      parent.append(styled("pre", "md-pre", body.join("\n")));
      continue;
    }
    if (QUOTE.test(lines[i])) {
      const quote = document.createElement("div");
      quote.className = "md-quote";
      const body = [];
      while (i < lines.length && QUOTE.test(lines[i])) {
        body.push(lines[i].replace(QUOTE, ""));
        i += 1;
      }
      renderInline(quote, body.join("\n"));
      parent.append(quote);
      continue;
    }
    const paragraph = document.createElement("div");
    renderInline(paragraph, lines[i]);
    parent.append(paragraph);
    i += 1;
  }
  return parent;
}

function discordNode(message) {
  const li = document.createElement("li");
  // `#56 message-hover-highlight`. A class of its own rather than styling `#discord-log li`
  // directly: the treatment must not be able to reach `#transcript` rows, and the behavioural
  // suite needs something on a REAL rendered row to assert the stylesheet's hook exists.
  li.className = "discord-message";
  const meta = document.createElement("div");
  meta.className = "meta";
  const author = document.createElement("span");
  // An author name is channel data too, and a display name can be anything at all.
  author.textContent = message.author_is_bot ? `${message.author} (bot)` : String(message.author);
  const stamp = document.createElement("span");
  // `#52 operator-timezone`. The server converts once, into the operator's configured zone, and
  // hands back a string that is already correct — so prefer it. The ISO slice is the fallback for
  // a server too old to send `spoken_time`, and it is UTC-as-Discord-reported-it. This page and
  // web/app.js must not disagree about what time a message was posted.
  stamp.textContent =
    message.spoken_time || String(message.timestamp || "").replace("T", " ").slice(0, 16);
  const id = document.createElement("span");
  id.className = "msg-id";
  id.textContent = `id ${message.id}`;
  meta.append(author, stamp, id);
  const body = document.createElement("div");
  body.className = "body";
  renderMarkdownInto(body, message.content);
  li.append(meta, body);
  // The SAME call the voice transcript makes, on the same arguments, so the two lists cannot end
  // up with two idioms for the one behaviour. `#47 scrollback-stability`.
  foldable(li, meta, body, message.content);
  // `#51 reply-view`. Every raw message can be answered, and the affordance is on the row rather
  // than in a menu — Discord's own idiom, and the thing that makes a reply a REPLY rather than a
  // loose message.
  //
  // Its accessible name carries NO channel data. An author string is written by whoever is in the
  // channel and a display name can be anything at all; the row already shows the author and the
  // id, and a button that repeats them into an attribute buys nothing worth the question of
  // whether that attribute is a sink.
  const reply = document.createElement("button");
  reply.className = "reply-button";
  reply.setAttribute("type", "button");
  reply.setAttribute("title", "Reply to this message");
  reply.textContent = "Reply";
  // A closure over the message OBJECT, not over its id: an id would be looked up again later
  // against a list that the next poll may have replaced.
  reply.addEventListener("click", () => openReply(message));
  meta.append(reply);
  return li;
}

// `#62 message-count-accuracy`, carried across from web/app.js where it was fixed first.
//
// The length of what the server returned is the FETCH WINDOW, not a channel total. Discord gives a
// bot no message count for a guild text channel, so the number is the channel's own only when the
// server reports `complete` — the fetch came back short, meaning there is nothing older. Otherwise
// no digit is shown at all: a confidently wrong count is worse than no count, and this one was
// wrong in the direction that makes the bridge look like it is losing messages.
//
// `!== true` rather than `=== false`, so a server too old to send the field is treated as unknown.
// It takes the label so every branch reads as a whole sentence. Appending " from <channel>" to a
// summary that already ends in a clause produced "older ones are not loaded from lead team".
function channelSummary(count, complete, label) {
  if (complete !== true) {
    return `${label} — the most recent messages; older ones are not loaded`;
  }
  if (count === 0) {
    return `no messages in ${label}`;
  }
  return `${count} message${count === 1 ? "" : "s"} from ${label}`;
}

/**
 * @param {{keepPosition?: boolean}} [options] `keepPosition` marks a RE-read of a channel already
 *   on screen — the background poll, or the Refresh button. It must not drag the reader to the
 *   bottom while they are reading older messages; it follows the newest line only if that is
 *   where they already were. The FIRST load of a channel is the other case and does not pass it:
 *   arriving at the top of a long history means scrolling past everything already read.
 */
/** The id of the newest message the channel list is currently showing, or null. */
let discordNewestId = null;

async function loadDiscord(options) {
  const keepPosition = Boolean(options && options.keepPosition);
  const channel = el("discord-channel").value;
  if (!channel) {
    setStatus("no channel to read — this server has none configured.");
    return;
  }
  if (discordFetchInFlight) return;
  discordFetchInFlight = true;
  const area = el("scroll-area");
  const wasAtNewest = atBottom(area);
  const previousTop = area.scrollTop;
  try {
    if (!keepPosition) {
      setStatus("fetching the channel…");
    }
    const payload = await api(`/api/v1/channels/${encodeURIComponent(channel)}/messages`);
    const list = el("discord-log");
    list.replaceChildren();
    for (const message of payload.messages) {
      list.append(discordNode(message));
    }
    setStatus(channelSummary(payload.messages.length, payload.complete, payload.channel.label));
    const newest = payload.messages.length
      ? payload.messages[payload.messages.length - 1].id
      : null;
    // Something ARRIVED only if the newest id moved. A refresh that returns the same messages must
    // not raise the chip, or a background poll every 45 seconds would offer to jump the reader to
    // a bottom that has not changed since the last time they declined.
    const arrived = newest !== null && discordNewestId !== null && newest !== discordNewestId;
    discordNewestId = newest;
    if (keepPosition && !wasAtNewest) {
      area.scrollTop = previousTop;
      if (arrived) {
        setJumpNewest(true, "discord");
      }
    } else {
      scrollToNewest();
      setJumpNewest(false, "discord");
    }
    renderScrollTools();
  } finally {
    discordFetchInFlight = false;
  }
}

// --- replying to a channel message -------------------------------------------------------------
//
// `#51 reply-view`. The server half already existed and had no caller:
// `POST /api/v1/channels/{id}/reply` takes `{text, reply_to}` and sets Discord's
// `message_reference.message_id`, which is what makes the answer a REPLY in the channel rather
// than a loose message that happens to follow. Until now the only way to reach it was the voice
// agent, so a reply typed by hand did not exist and `#50` could not tell an answered message from
// an unanswered one — Discord records a reference only for replies made through the affordance.
//
// Two decisions the issue leaves open, made here rather than left to the reader of a diff:
//
//   * THE POSTED REPLY IS APPENDED, not re-fetched. A refetch replaces every child of the log,
//     which destroys the anchor the reader's position is measured against — so "your reply
//     appeared" would cost "and you lost your place". The server hands back the Message it
//     actually posted, which is better evidence than a refetch anyway: it is what Discord
//     accepted, not what a later read happened to return.
//   * DRAFTS ARE PER MESSAGE. One global draft would silently hand what you wrote about one
//     message to a reply to a different one, which is the kind of mistake that gets posted.

const DRAFTS_KEY = "gent-talk.voice.drafts";

/** message id -> the unsent text. Mirrored to storage so a reload does not lose it. */
const drafts = new Map();

function loadDrafts() {
  let stored = null;
  try {
    stored = JSON.parse(localStorage.getItem(DRAFTS_KEY) || "null");
  } catch (_error) {
    stored = null;
  }
  if (!stored || typeof stored !== "object") {
    return;
  }
  for (const [id, text] of Object.entries(stored)) {
    if (typeof text === "string" && text) {
      drafts.set(id, text);
    }
  }
}

/** Written back with the same read-after-write check `persistMicSettings` makes, for the reason. */
function persistDrafts() {
  const encoded = JSON.stringify(Object.fromEntries(drafts));
  try {
    localStorage.setItem(DRAFTS_KEY, encoded);
  } catch (_error) {
    return false;
  }
  return localStorage.getItem(DRAFTS_KEY) === encoded;
}

let replyTarget = null;
// Where the reader was in the channel when they opened this. Captured with the SAME mechanism the
// fold control uses (`#47 scrollback-stability`), not a second one.
let replyScrollMark = null;

function rememberDraft() {
  if (!replyTarget) {
    return;
  }
  const text = el("reply-text").value;
  if (text.trim()) {
    drafts.set(replyTarget.id, text);
  } else {
    drafts.delete(replyTarget.id);
  }
  persistDrafts();
}

/** Open the reply screen on one specific message. */
function openReply(message) {
  replyTarget = message;
  // BEFORE the screen changes: once #screen-main is hidden nothing in it has a rectangle, so the
  // anchor has to be taken while the reader can still see it.
  replyScrollMark = captureScroll();
  const target = el("reply-target");
  target.replaceChildren();
  // The same renderer the channel list uses. Untrusted text, so the same guarantee: every fragment
  // is an element built here with textContent, and there is no second path.
  renderMarkdownInto(target, message.content);
  el("reply-target-meta").textContent = `${
    message.author_is_bot ? `${message.author} (bot)` : String(message.author)
  } · id ${message.id}`;
  el("reply-text").value = drafts.get(message.id) || "";
  el("reply-state").textContent = "";
  showScreen("reply");
}

function closeReply() {
  rememberDraft();
  showScreen("main");
  if (replyScrollMark) {
    restoreScroll(replyScrollMark);
    replyScrollMark = null;
  }
  replyTarget = null;
}

/**
 * Post it, as a real Discord reply.
 *
 * Failures are handled HERE rather than by `guard`, and that is the point: a reply that did not
 * post must leave the text exactly where the reader typed it, on the screen they typed it on, and
 * must not touch a live call. Losing what somebody wrote because the network blinked is the one
 * outcome this must not have.
 */
async function sendReply() {
  if (!replyTarget) {
    return;
  }
  const text = el("reply-text").value.trim();
  if (!text) {
    el("reply-state").textContent = "Nothing to send yet — write something first.";
    return;
  }
  const channel = el("discord-channel").value;
  const target = replyTarget;
  el("reply-state").textContent = "Posting…";
  el("reply-send").disabled = true;
  try {
    const payload = await api(`/api/v1/channels/${encodeURIComponent(channel)}/reply`, {
      method: "POST",
      body: { text, reply_to: target.id },
    });
    if (payload && payload.posted) {
      el("discord-log").append(discordNode(payload.posted));
    }
    drafts.delete(target.id);
    persistDrafts();
    el("reply-text").value = "";
    el("reply-state").textContent = "";
    closeReply();
  } catch (error) {
    // Stay put. The text is still in the box, and the reason is beside it.
    el("reply-state").textContent = `Not posted: ${redact(error.message)}`;
  } finally {
    el("reply-send").disabled = false;
  }
}

// --- keeping the channel view fresh -----------------------------------------------------------
//
// The channel used to be fetched exactly ONCE, on the first switch into the view, because the
// load was guarded on the log being empty. Everything after that was whatever had been true the
// first time you looked. On the owner's phone that meant a view hours out of date, presented with
// no hint that it was stale — which is worse than an empty view, because it reads as current.
//
// There is no webhook and no push path yet (#44 live-push), so until there is, the view PULLS:
// once on every entry, and then on a timer for as long as it is the view being looked at.

const DISCORD_POLL_MS = 45000;
let discordPollTimer = null;
let discordFetchInFlight = false;

function stopDiscordPolling() {
  if (discordPollTimer !== null) {
    clearTimeout(discordPollTimer);
    discordPollTimer = null;
  }
}

/**
 * Self-rescheduling rather than setInterval, for two reasons: a slow fetch can never stack a
 * second one up behind it, and the next delay is armed only once the previous poll has actually
 * finished. Polling stops the moment the channel stops being the visible view, so a voice call
 * is never sharing its network with a background refresh nobody is looking at.
 */
function scheduleDiscordPoll() {
  stopDiscordPolling();
  discordPollTimer = setTimeout(() => {
    discordPollTimer = null;
    if (currentView !== "discord") return;
    guardQuietly(async () => {
      await loadDiscord({ keepPosition: true });
      if (currentView === "discord") {
        scheduleDiscordPoll();
      }
    })();
  }, DISCORD_POLL_MS);
}

// --- sign-in ---------------------------------------------------------------------------------

function setTokenState(text) {
  el("token-state").textContent = text;
}

function applyClientConfig(config) {
  const select = el("discord-channel");
  select.replaceChildren();
  for (const channel of config.channels || []) {
    const option = document.createElement("option");
    option.value = channel.id;
    option.textContent = channel.label;
    select.append(option);
  }
  if ((config.channels || []).length > 0) {
    select.value = config.channels[0].id;
  }
}

/**
 * Prove the saved token actually works, then show the main interface.
 *
 * A refusal (401/403) is the one failure that means "sign in again", and only that one sends the
 * owner back to the sign-in screen. Anything else — server down, network gone — leaves the token
 * alone and lands on the main screen with the error visible, because bouncing to a sign-in form
 * would blame the wrong thing.
 */
async function signIn() {
  if (!token()) {
    showScreen("signin");
    return false;
  }
  let config = null;
  try {
    config = await api("/api/v1/client-config");
  } catch (error) {
    if (error.refused) {
      showError(`This server refused that token: ${error.message}`);
      setStatus("that token was refused — paste the write-scope token");
      showScreen("signin");
      return false;
    }
    showError(`Signed in, but gent-talk did not answer: ${error.message}`);
    showScreen("main");
    return true;
  }
  applyClientConfig(config);
  clearError();
  showScreen("main");
  // Short, because the invitation itself now lives in the empty transcript — the largest thing on
  // an idle screen — rather than competing for the one strip where a phone shows text worst.
  setStatus("Ready.");
  setState("idle");
  return true;
}

// A banner that says "saved" while the button looks untouched leaves two questions open at once:
// did the click register, and did the save work? So the button itself changes — immediately on
// click, and again on the result — and the line under it states what is stored right now.
const SAVE_LABEL = "Save token";
let saveRevert = null;

function markSave(label, className, disabled) {
  const button = el("save-token");
  button.textContent = label;
  button.className = className;
  button.disabled = disabled;
  if (saveRevert !== null) {
    clearTimeout(saveRevert);
    saveRevert = null;
  }
}

async function saveToken() {
  clearError();
  const value = el("api-token").value.trim();
  markSave("Saving…", "", true);
  if (!value) {
    markSave(SAVE_LABEL, "", false);
    setTokenState("no token saved in this browser");
    showError("There is nothing to save — paste your write-scope API token first.");
    setStatus("nothing to save");
    return;
  }
  localStorage.setItem(TOKEN_KEY, value);
  // Read it back rather than assuming: private browsing and a full quota both make setItem throw
  // or silently do nothing, and "saved" would then be a lie the owner only discovers later.
  if (localStorage.getItem(TOKEN_KEY) !== value) {
    markSave(SAVE_LABEL, "", false);
    setTokenState("no token saved in this browser");
    showError(
      "This browser refused to store the token. Private browsing and a full storage quota both " +
        "do this; the token was NOT saved."
    );
    return;
  }
  markSave("Saved ✓", "ok", false);
  setTokenState("token saved in this browser");
  setStatus("ready");
  saveRevert = setTimeout(() => {
    markSave(SAVE_LABEL, "", false);
  }, 2500);
  await signIn();
}

function forgetToken() {
  clearError();
  localStorage.removeItem(TOKEN_KEY);
  el("api-token").value = "";
  markSave(SAVE_LABEL, "", false);
  setTokenState("no token saved in this browser");
  setStatus("token forgotten");
  showScreen("signin");
}

// --- wiring ------------------------------------------------------------------------------------

el("api-token").value = token();
el("save-token").addEventListener("click", guardQuietly(saveToken));
el("forget-token").addEventListener("click", forgetToken);

// The checkboxes are the live truth `start()` reads, so they are what gets restored on load.
const restoredMicSettings = storedMicSettings();
for (const [id, key] of MIC_TOGGLES) {
  el(id).checked = restoredMicSettings[key];
  el(id).addEventListener("change", micSettingsChanged);
}

// The column the reader last chose, restored before anything is drawn into it. Applied
// unconditionally: on a phone the stylesheet never consults the value, so there is nothing to
// branch on here and no second definition of "is this a desktop" to drift.
applyReadingWidth(storedReadingWidth());
el("reading-width").addEventListener("input", () => readingWidthChanged(el("reading-width").value));
el("width-grip").addEventListener("pointerdown", onGripDown);
el("width-grip").addEventListener("pointermove", onGripMove);
el("width-grip").addEventListener("pointerup", onGripUp);
el("width-grip").addEventListener("pointercancel", onGripUp);
el("width-grip").addEventListener("keydown", onGripKey);

// `#54 resume-recovery`. The one signal a browser gives for "this page went away": it is what
// distinguishes a socket that died because the reader switched apps from one that died because
// something is broken.
document.addEventListener("visibilitychange", onVisibility);

el("talk").addEventListener("click", onTalk);
el("hang-up").addEventListener("click", stop);
el("clear-view").addEventListener("click", onClear);
el("speaker").addEventListener("click", () => setSpeakerOff(!session.speakerOff));
el("dismiss-banner").addEventListener("click", dismissBanner);
el("open-settings").addEventListener("click", () => showScreen("settings"));
el("close-settings").addEventListener("click", () => showScreen(screenBeforeSettings));
// `#51 reply-view`. Both ways out of the reply screen go through `closeReply`, so neither can be
// the one that forgets to put the reader back where they were reading.
loadDrafts();
el("close-reply").addEventListener("click", closeReply);
el("reply-cancel").addEventListener("click", closeReply);
el("reply-send").addEventListener("click", guardQuietly(sendReply));
// A draft survives leaving the screen without sending, so it is written as it is typed rather than
// only on the way out — a way out that is not a control (the browser's own back, a reload) would
// otherwise lose it.
el("reply-text").addEventListener("input", rememberDraft);
el("view-switch").addEventListener("click", () => {
  const next = currentView === "voice" ? "discord" : "voice";
  showView(next);
  if (next !== "discord") {
    stopDiscordPolling();
    return;
  }
  // Deliberately NOT guarded on the log being empty. That guard is what made the view stale:
  // after the first load it never fetched again, so switching back showed you the channel as it
  // had been, with nothing on screen admitting it.
  guardQuietly(async () => {
    await loadDiscord();
    if (currentView === "discord") {
      scheduleDiscordPoll();
    }
  })();
});
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
el("refresh-discord").addEventListener("click", guardQuietly(() => loadDiscord({ keepPosition: true })));
// Changing channel is not a re-read; it is a different history, and its bottom is where to start.
el("discord-channel").addEventListener("change", guardQuietly(() => loadDiscord()));
el("collapse-all").addEventListener("click", collapseAll);
el("jump-newest").addEventListener("click", scrollToNewest);
// The chip is an offer to go somewhere the reader may simply go themselves. Once they are there it
// has nothing left to say, so it takes itself away rather than waiting to be tapped.
el("scroll-area").addEventListener("scroll", () => {
  if (jumpNewestWanted[currentView] && atBottom(el("scroll-area"))) {
    setJumpNewest(false);
  }
});

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

setTokenState(token() ? "token saved in this browser" : "no token saved in this browser");
setStatus(token() ? "Checking your token…" : "Paste your write-scope API token first.");
showView("voice");
renderEmptyState();
renderControls();
// No token means no interface to show yet: the sign-in screen is the whole page until there is
// one. With a token, prove it before showing the main screen.
if (token()) {
  guardQuietly(signIn)();
} else {
  showScreen("signin");
}
