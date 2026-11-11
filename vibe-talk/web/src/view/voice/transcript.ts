import { postKey, sentWhere, sentNotes, proposalFate, restoredSpeaker, oldestFirst } from "../../present/call/index.ts";
// The call's transcript, recap and approval cards, with their existing timers and queues.
// Dependencies are live properties of the page composition (#624 voice-page-modularity).
import type { PendingPost, PendingPostResponse, SentRecord, VoiceSend, VibeTalkV1ServerFrame } from "../../contract/index.ts";
import * as VibeTalkContract from "../../contract/index.ts";
import type { ApiRequestOptions, ApiRequestError } from "../../core/api/index.ts";
import type { Boundary, Recap } from "../../core/call/index.ts";
import type { MessageLinkRef } from "../../foundation/index.ts";
import type { ElementById } from "../dom/index.ts";
import type { CallSession, Resume, VoiceRow, Timer } from "./types.ts";

export interface ResumeLast { attempted: boolean; armed: boolean; dropped: number; failed: boolean; words: number | null; acked: boolean; refused: boolean; unsupported: boolean; unsent: boolean; replaced: boolean; }
export interface CallWindow { conversationId: string | null; from: number | null; to: number | null; }
export interface StartMarker { recap: Recap; previous_end?: string; }
export interface RecordDetail extends Partial<SentRecord> { kind?: string; why?: string; recap?: Recap; previous_end?: string; }
export interface TranscriptTurn extends StoredTurn { speaker: string; conversation_id: string; }
export interface StoredTurn { text: string; at_ms?: number; speaker?: string; conversation_id?: string; detail?: RecordDetail | null; }
interface PostedMessage { id?: string; thread?: { is_root?: boolean; id?: string; } | null; }
interface TranscriptAnswer { armed?: boolean; id?: number; included?: number; dropped?: number; words?: number; state?: string; error?: string; detail?: string; refusal?: { explanation?: unknown }; posted?: number | PostedMessage; uncertain?: boolean; send_seq?: number; parts?: unknown[]; unsent?: string; turn?: StoredTurn; conversations?: {id: string}[]; turns?: TranscriptTurn[]; has_more?: unknown; next_before?: string; forgotten?: number; }
export interface Outcome { kind: string; proposal?: PendingPost | null; reason?: string; withdrawn?: boolean; }
export interface SpokenYesTurn { text: string; card: string | null; turn: number | null; at: number; cardFrom: {handle: string; words: number} | null; }
export interface SpokenYesHeard { turns: SpokenYesTurn[]; sinceUser: {text: string}[]; interposed: boolean; cut: boolean; done: boolean; answerAt: number; playEnd: number; turn: number | null; replies: Set<string>; }
export interface YesHold { key: string; li: HTMLElement | null; state: "tentative" | "accepted" | "committing" | "sent" | "released"; serial: number | null; conversationId: string | null; timer: Timer | null; kept: boolean; }
interface HeardReport { inFlight: boolean; next: string | null; sent: string; }
type RefusedReason = "ended" | "withdrawn" | "hint" | "earlier_draft" | "replaced_session" | "interrupted" | "mismatch" | "no_question" | "interposed" | "mention" | "symbols" | "early" | "echo" | "echo_cancellation_off" | "typed_call" | "typed_yes" | "expired" | "token" | "busy" | "heard_more" | "said_more" | "changed" | "typed" | "gone";
export interface VoiceTranscriptDependencies<Column extends object> {
  session: CallSession;
  el<K extends string>(id: K): K extends keyof ElementById ? ElementById[K] : HTMLElement;
  leftEl(id: string): HTMLElement;
  api(path: string, options?: ApiRequestOptions): Promise<TranscriptAnswer>;
  apiDecoded(type: "PendingPostResponse", path: string, options?: ApiRequestOptions): Promise<PendingPostResponse>;
  columns: {main: Column; side: Column & {mode: string}};
  currentScreen: string;
  currentView: string;
  cardsOpening: boolean;
  threadListShown: boolean;
  tokenScope: string;
  voiceSendSetting: {ask_before_send?: boolean} | null;
  hasEnded: boolean;
  hasSuspended: boolean;
  resumeAllowed: boolean;
  resumeConversationId: string | null;
  resumeLast: ResumeLast;
  relayAnswerAwaited: unknown;
  knownChannels: readonly {id: string; name?: string}[];
  chatProviderName: string;
  sidePane: object;
  COLLAPSE_OVER_CHARS: number;
  FOLD_LESS: string;
  FOLD_MORE: string;
  OLDER_TRIGGER_PX: number;
  addDetail(text: string): void;
  applySearch(): void;
  atBottom(element: HTMLElement, slack?: number): boolean;
  blankResumeLast(): ResumeLast;
  canSendText(): boolean;
  cardCameUnderLists(gap: number): void;
  cardMovedUnderLists(delta: number): void;
  chatServiceOf(channel: string): string;
  drawnBy(): boolean;
  focusList(pane: object | null): void;
  foldOnTap(li: HTMLElement, fold: HTMLButtonElement): void;
  foldable(li: HTMLElement, meta: HTMLElement, body: HTMLElement, text: string): HTMLButtonElement | null;
  followFoot(element: HTMLElement): void;
  followIfPinned(pinned: boolean, unseen?: number, view?: string): void;
  guardQuietly<A extends unknown[]>(fn: (...args: A) => unknown): (...args: A) => unknown;
  heightUnderLists(): number;
  holdingReader(run: () => void): void;
  inColumn<T>(column: Column, run: () => T): T;
  jumpToSentMessage(detail: MessageLinkRef): Promise<unknown>;
  keepTurnFold(was: HTMLElement, next: HTMLElement): boolean;
  liveFollowers(): {main: boolean};
  normaliseHasMore(value: unknown): boolean | null;
  noteTypedTurn(text: string): void;
  placeChipsOverComposer(): void;
  preservingScroll(run: () => void): void;
  reconnectedSessionRefused(verb?: string): void;
  recoverUnsettledTurns(): Promise<number>;
  redact(text: unknown): string;
  renderControls(): void;
  renderEmptyState(): void;
  renderJumpNewest(): void;
  renderResumeState(): void;
  renderScrollTools(): void;
  resumeArmed(): boolean;
  resumeWanted(): boolean;
  scrollToNewest(): void;
  searchable(row: HTMLElement, ...text: string[]): void;
  sendClientEvent(event: {type: string; text?: string}, kind?: string): boolean;
  sendVibeTalkFrame(frame: object): void;
  setStatus(text: string): void;
  settleNoticeForRecap(taken: boolean, why: string): void;
  showError(text: string): void;
  tapRow(li: HTMLElement, fold: HTMLButtonElement, act: (li: HTMLElement, fold: HTMLButtonElement) => void): void;
  threadListOnScreen(): boolean;
}

import { STORED_SEAM_DETAIL } from "../../core/call/index.ts";
import { agentDraft } from "../../present/call/index.ts";
import { answerWords } from "../../present/call/index.ts";
import { askedAboutSending } from "../../present/call/index.ts";
import { boundaryFor } from "../../core/call/index.ts";
import { seamDetailFor as callSeamDetailFor } from "../../core/call/index.ts";
import { childByClass } from "../../view/dom/index.ts";
import { classifySpokenAnswer } from "../../present/call/index.ts";
import { couldBeSpokenYes } from "../../present/call/index.ts";
import { isExactReadBack } from "../../present/call/index.ts";
import { isSpokenYesAcknowledgement } from "../../present/call/index.ts";
import { mayBeYes } from "../../present/call/index.ts";
import { namesWhereToSend } from "../../present/call/index.ts";
import { nodeWithin } from "../../view/dom/index.ts";
import { notSentDetail } from "../../core/call/index.ts";
import { outcomeNotice } from "../../present/call/index.ts";
import { readBackRefusal } from "../../present/call/index.ts";
import { callRecapLine as recapLine } from "../../core/call/index.ts";
import { refusalWords } from "../../present/notices/index.ts";
import { spokenOutcomeKind } from "../../core/call/index.ts";
import { spokenYesSeam } from "../../core/call/index.ts";
import { stamp } from "../../present/time/index.ts";
import { timeNode } from "../../view/channel/index.ts";
import { unsayableDraft } from "../../present/call/index.ts";

export function createVoiceTranscript<Column extends object>(deps: VoiceTranscriptDependencies<Column>) {

// `#1251 scroll-single-owner`: the transcript's follow state, noted from the reader's own
// place as the listkeepers note Main's. Each instantaneous `atBottom` measure alone loses
// the reader after one unfollowed growth (a seam's detail landing, a chip wrapping): the
// gap passes the slack once, and every later measure reads false though the reader never
// scrolled away. The note stands from the last time the reader was at the foot, and is
// cleared only when their scroll has dropped below where following left it — the owner's
// own writes take the scroll to the foot or hold it, never below the noted place.
let footing = false;
let footingTop = 0;

/** Whether this act follows the foot, and the note kept up to date by the answer. */
function pinnedHere(area: HTMLElement): boolean {
  if (deps.atBottom(area)) {
    footing = true;
    footingTop = area.scrollTop;
    return true;
  }
  if (footing && area.scrollTop >= footingTop - 1) return true;
  footing = false;
  return false;
}

/** Note where a follow left the reader, for `pinnedHere`'s next answer. */
function noteFollow(area: HTMLElement, pinned: boolean): void {
  if (pinned) {
    footing = true;
    footingTop = area.scrollTop;
  }
}


// Recording is FIRE-AND-FORGET, and a failure is retried, never latched.
//
// A store that is down must not be able to interrupt a conversation: the owner is driving, the
// agent is talking, and an error panel per turn would be worse than losing the record. So a
// failure is said ONCE, in Settings, where it can be read afterwards, the turn is kept, and
// recording tries again — at once, and again ahead of the next turn. `#1303
// voice-transcript-share`: the old rule, one failure disables recording for the rest of the
// page's life, is what left most calls with nothing stored at all: a long-lived PWA page
// failed once and then filed nothing, on any call, until it happened to be reloaded. It
// never reaches `teardown()` and never ends a call.
let recordingBroken: boolean = false;

/** How many times one record is attempted before it is kept for the next flush. */
const RECORD_ATTEMPTS = 3;

/** Records a transient failure could not file yet, in the order they were said. */
let unrecorded: Array<{ conversationId: string; record: Record<string, unknown> }> = [];


/**
 * One turn, built but not yet anywhere.
 *
 * `mine` and `theirs` differ in side, colour and corner — three signals at once — because the two
 * speakers used to be told apart by nothing but a small grey word.
 *
 * Split out of `line()` for the stored record, which arrives a page at a time ABOVE what is
 * already on screen (`#128 transcript-history`). Those rows cannot be appended one by one: every
 * append asks whether the reader is parked at the bottom and then either follows the list or
 * raises the jump-to-newest chip, and neither is right for forty turns landing over the reader's
 * head. They are assembled here and prepended in one anchored mutation.
 */
function turnNode(who: string, text: string, atMs?: number | undefined) {
  const mine = who === "you";
  const li = document.createElement("li") as VoiceRow;
  li.className = mine ? "mine" : "theirs";
  const meta = document.createElement("div");
  meta.className = "meta";
  const author = document.createElement("span");
  author.className = "who";
  author.textContent = who;
  // A RESTORED line carries the instant the server recorded, not the instant the page was
  // reloaded. Stamping a two-hour-old sentence with the current clock is the specific way a
  // durable transcript lies about itself. Restored lines can be days old, so the day is on hover,
  // as on a channel row (`#260 timestamp-hover`).
  const when = atMs === undefined ? new Date() : new Date(atMs);
  const at = timeNode("at", Number.isNaN(when.getTime()) ? null : when, stamp(when.getTime()));
  meta.append(author, at);
  const body = document.createElement("div");
  body.className = "body";
  body.textContent = text; // untrusted text: never innerHTML.
  li.append(meta, body);
  // `#431 voice-transcript-expand`. The message is its own fold control, as a channel row is
  // (`tapRow`): the owner, in a car, tapped a long reply's card and nothing opened.
  const fold = deps.foldable(li, meta, body, text);
  if (fold) deps.tapRow(li, fold, deps.foldOnTap);
  // Who said it as well as what was said: on the transcript "assistant" is a real thing to look
  // for, and it is the word printed on the row.
  deps.searchable(li, who, text);
  // `#211 link-filter`. What the Links filter reads this turn's links from: its words as said,
  // before anything was made of them for the screen.
  li.linkText = String(text === null || text === undefined ? "" : text);
  return li;
}


/** One turn, at the end of the transcript — where everything said in the LIVE call goes. */
function line(who: string, text: string, atMs?: number | undefined) {
  // Measured BEFORE the append: appending is what changes the answer.
  const area = deps.el("scroll-area");
  const pinned = pinnedHere(area);
  const li = turnNode(who, text, atMs);
  deps.el("transcript").append(li);
  deps.renderEmptyState();
  // A row arriving into a filtered list is filtered too. Without this, a live turn spoken while
  // the reader is searching lands on screen among the matches whether or not it is one.
  deps.applySearch();
  deps.followIfPinned(pinned, 1);
  noteFollow(area, pinned);
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
function seam(label: string, detail: string) {
  const li = document.createElement("li") as VoiceRow;
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
      // The least that shows it: never a move for one already on screen. `#280 scroll-stability`.
      li.scrollIntoView({ block: "nearest" });
    }
  });
  details.append(summary, body);
  li.append(details);
  return li;
}


/**
 * A seam at the end of the transcript.
 *
 * `seam()` above only BUILDS one, because `#63 status-line-placement` gave the channel list a seam
 * of its own and that one belongs at the TOP of its list rather than at the end. Placement is
 * therefore the caller's, and the transcript's callers share this.
 *
 * `into` is for the stored record, whose rows are collected off-screen and put on the list in one
 * anchored mutation (`#128 transcript-history`). A seam among THOSE must not append to the live
 * list and must not ask where the reader is: it is not arriving at the end, and the reader is not
 * looking at it. Everything else places at the end, which is what the default does.
 */
function transcriptSeam(label: string, detail: string, into: HTMLElement[] | null = null) {
  const li = seam(label, detail);
  if (into !== null) {
    into.push(li);
    return li;
  }
  // Measured before the append and not before the build: building one attaches nothing.
  const area = deps.el("scroll-area");
  const pinned = pinnedHere(area);
  deps.el("transcript").append(li);
  deps.renderEmptyState();
  // A seam explains the rows beside it, so while a filter is on it goes away with them. It
  // declares nothing to `searchable`, which is exactly how `applySearch` knows that.
  deps.applySearch();
  deps.followIfPinned(pinned);
  noteFollow(area, pinned);
  return li;
}


// True from the moment a conversation opens until the end has been announced in the transcript.
let conversationOpen: boolean = false;


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
// What the seam says inside its disclosure, per cause, and what it says when resuming is armed
// (`SEAM_DETAILS`, `RESUME_SEAM_DETAILS`, `#54 resume-recovery`), are web/src/core/call's, with every
// other word a seam and the line under a call say (`#624 voice-page-modularity`).

/** The seam's sentence for how a call ended, by whether resuming is armed (web/src/core/call). */
function seamDetailFor(cause: string) {
  return callSeamDetailFor(cause, deps.resumeArmed());
}


function noteConversationEnded(cause: string = "ended") {
  if (!conversationOpen) {
    return;
  }
  conversationOpen = false;
  // `#1303 voice-transcript-share`. A call that ends mid-outage still files what it kept.
  recordQueue = recordQueue.then(() => flushUnrecorded());
  // The call that just ended is what the NEXT one resumes from. Read here rather than in
  // `teardown`, which clears the session — and only when a conversation was really open, so a
  // failed mint cannot point the next call at a conversation that has no turns. Nor can a call
  // that opened and recorded nothing, one that dropped at once: it is not a conversation the
  // server holds. `#270 voice-transcript-markers`.
  if (deps.session.conversationId && deps.session.recordedAny) {
    deps.resumeConversationId = deps.session.conversationId;
  }
  // The next conversation's start marker says how this one ended.
  lastEndCause = cause;
  // From here the large control is a different offer — a NEW call, from nothing — and it says so.
  deps.hasEnded = true;
  // ...and if the drop was a suspension, the offer has a different WORD on it, because "Start a
  // new call" reads as an invitation to begin something and this is an invitation to carry on.
  deps.hasSuspended = cause === "suspended";
  deps.renderControls();
  // Two facts, and only two: what the line means, and which control would have avoided it. The
  // version before this one was fifty-seven words of vendor archaeology one tap inside a
  // disclosure, which is not shorter than a paragraph — it is a paragraph nobody opens. The rest
  // of it (why a hang-up loses the context, and that the lines above are still your own record)
  // lives in Settings under "What the controls do", which is where the long form belongs.
  //
  // Drawn at the moment of the DROP, not when Resume is tapped: the boundary is where the
  // conversation actually broke, and marking it later would put the reader's own next turn on the
  // wrong side of it.
  //
  // The card first: a draft retired by Hang up leaves its "Not sent" line inside the conversation
  // that ended, above the boundary, where a reload draws it. `#269 approval-card-inline`.
  settleCardAtCallEnd(cause);
  // `#270 voice-transcript-markers`. The line says which conversation it ends, so a send that
  // settles late is drawn inside it, above the line, where a reload draws it; and how it ended, for
  // the recap the next call is sent to be said beside (`settleBoundary`).
  drawBoundary(cause, deps.session.conversationId);
}


// --- what recap a new conversation was sent ----------------------------------------------------------
//
// `#270 voice-transcript-markers`. The "new conversation" seam says what the agent below it was handed
// of the conversation above: on its surface, a chip — "412-word recap sent", "no recap sent", "recap
// unsupported" — and inside the disclosure, below the sentence saying how the call before ended
// (`#268 voice-call-drops`, unchanged), a sentence of its own about the recap. Neither overwrites the
// other, so each keeps its own budget. "Sent", never "recapped" or "heard": the hosted service
// acknowledges nothing, so sent is all this page knows there. `#331 resume-on-websocket`: a
// `vibe-talk-v1` bridge does answer its `replay` frame, and the live line under the call
// (`renderCallRecap`) says "Caught the agent up" once it has; the seam, which a reload draws from the
// stored marker, keeps "sent".
//
// The same seam comes back after a reload: the first record of each conversation is a
// `conversation_start` marker holding its recap and how the call before it ended, written lazily — just
// before that first record — so a call that recorded nothing leaves no conversation at all, and never
// becomes the one a new call resumes from.

// The recap sentence, per state (`RECAP_DETAILS`, `#649 voice-agent-tools`: cumulative across recent
// calls within a size budget), is web/src/core/call's.

/** The biggest number a stored marker may hold, as the server checks it. */
const MARKER_NUMBER_MAX = 1000000;


/**
 * Why replay is off although the operator turned it on, from client-config: `bridge_declined` or
 * `bridge_unconfirmed` (`#331 resume-on-websocket`), or null when that is not why. Any word counts,
 * so a reason this page does not know yet still marks a new conversation "recap unsupported".
 * @type {string | null}
 */
let replayUnavailable: string | null = null;


/**
 * `#512 voice-recap-private-stack`. Whether a new call's recap rides in the voice-agent profile the
 * `vibe-talk-v1` bridge fetches as its session opens, rather than in a `replay` frame: client-config's
 * `replay_via_profile`.
 */
let resumeViaProfile = false;


/** How the last call on this page ended, as `noteConversationEnded` saw it; null after a load. */
let lastEndCause: string | null = null;


/** The conversation-start markers not yet filed, by conversation: each goes just before that conversation's first record. */
const startMarkers: Map<string, StartMarker> = new Map();


/**
 * What a new call was handed, as the marker records it: a recap sent — its words, lines, the older
 * lines left out, and how it went — or why none was.
 *
 * `words` is null when the server's replay did not say how many words it held — an older one —
 * which is a recap whose size nobody can state, never a recap of none.
 *
 * On `vibe-talk-v1` a payload with no transport is a session that did not list `replay`
 * (`#331 resume-on-websocket`): it was sent nothing because it could take nothing.
 *
 * @param {unknown} resume what `fetchResume` answered, null for nothing
 * @param {"contextual_update" | "client_data" | "replay_frame" | "profile" | null} transport how it went, null when it did not
 * @returns {{state: "sent", words: number | null, lines: number, dropped: number, transport: string} | {state: "none", why: string, dropped: number}}
 */
function recapFor(resume: Resume | null, transport: "contextual_update" | "client_data" | "replay_frame" | "profile" | null): {state: "sent"; words: number | null; lines: number; dropped: number; transport: string} | {state: "none"; why: string; dropped: number} {
  const capped = (value: unknown) => Math.min(MARKER_NUMBER_MAX, Math.max(0, Math.round(Number(value) || 0)));
  const dropped = capped(deps.resumeLast.dropped);
  if (resume && transport) {
    return {
      state: "sent",
      words: typeof resume.words === "number" ? capped(resume.words) : null,
      lines: capped(resume.included),
      dropped: capped(resume.dropped),
      transport,
    };
  }
  if (replayUnavailable || (resume && deps.session.protocol === "vibe-talk-v1")) {
    return { state: "none", why: "unsupported", dropped: 0 };
  }
  if (deps.resumeLast.failed) return { state: "none", why: "failed", dropped: 0 };
  if (deps.resumeLast.attempted && dropped > 0) return { state: "none", why: "too_long", dropped };
  if (deps.resumeLast.attempted) return { state: "none", why: "empty", dropped: 0 };
  return { state: "none", why: "off", dropped: 0 };
}


// How many words a recap held (`recapWords`), a seam's chip (`recapChip`) and recap sentence
// (`recapSentence`), and everything a seam says (`boundaryFor`) are web/src/core/call's.

/**
 * Write `boundary` onto the seam `li`: its label, chip, title and both sentences. Places nothing.
 *
 * @param {HTMLElement} li
 * @param {{word: string, chip: string | null, detail: string, recapDetail: string | null}} boundary
 */
function fillBoundary(li: Element, boundary: {word: string, chip: string | null, detail: string, recapDetail: string | null}) {
  const summary = childByClass(li, "seam-summary");
  const label = childByClass(li, "seam-label");
  const detail = childByClass(li, "seam-detail");
  if (!summary || !label || !detail || !detail.parentNode) return;
  label.textContent = boundary.word;
  let chip = childByClass(summary, "seam-recap");
  if (boundary.chip) {
    if (!chip) {
      chip = document.createElement("span");
      chip.className = "seam-recap";
      summary.insertBefore(chip, label.nextSibling);
    }
    chip.textContent = boundary.chip;
  } else if (chip) {
    summary.removeChild(chip);
  }
  detail.textContent = boundary.detail;
  const details = detail.parentNode;
  let recap = childByClass(details, "seam-recap-detail");
  if (boundary.recapDetail) {
    if (!recap) {
      recap = document.createElement("p");
      recap.className = "seam-recap-detail";
      details.insertBefore(recap, detail.nextSibling);
    }
    recap.textContent = boundary.recapDetail;
  } else if (recap) {
    details.removeChild(recap);
  }
  summary.setAttribute("title", [boundary.detail, boundary.recapDetail].filter(Boolean).join(" "));
}


/** {@link fillBoundary} on a seam in the live list, which may wrap at 360: followed only by a reader at the newest line. */
function setSeamRecap(li: Element, boundary: Boundary) {
  const area = deps.leftEl("scroll-area");
  const pinned = pinnedHere(area);
  fillBoundary(li, boundary);
  deps.followIfPinned(pinned);
  noteFollow(area, pinned);
}


/**
 * The call that just opened was handed `recap`: say so on the boundary above it — the one the last
 * call's end drew, or the one the restored record ends with — while nothing said is between the two.
 * There is always one when anything is above: a call that opened ended with one, and a restored
 * record ends with one. A screen with nothing on it has nothing above to have been recapped.
 * Answers the line it said so on, or null: none when something said, drawn as it comes and filed
 * only when its turn ends, already stands below the line, or there is no line.
 *
 * @param {unknown} recap
 * @returns {Element | null}
 */
function settleBoundary(recap: Recap) {
  const rows = [...deps.leftEl("transcript").children];
  for (let at = rows.length - 1; at >= 0; at -= 1) {
    const row = rows[at]!;
    if (row.getAttribute("data-ends") !== null) {
      setSeamRecap(row, boundaryFor(recap, row.getAttribute("data-cause")));
      return row;
    }
    // Something said below every boundary: the line above it is not this call's.
    if (row.getAttribute("data-search") !== null) return null;
  }
  return null;
}


/**
 * The "new conversation" line at the end of the transcript: the one place the live page draws it.
 *
 * @param {string | null} cause how the call above it ended, when the page saw that
 * @param {string | null} endsId which conversation it ends
 */
function drawBoundary(cause: string | null, endsId: string | null) {
  const boundary = transcriptSeam("new conversation", cause ? seamDetailFor(cause) : STORED_SEAM_DETAIL);
  if (endsId) boundary.setAttribute("data-ends", endsId);
  if (cause) boundary.setAttribute("data-cause", cause);
  return boundary;
}


/**
 * The call opened and was handed `recap`: the boundary above it says so, and the conversation it is
 * filed under — now, or when the voice service names it — starts with a marker saying so.
 */
function noteCallRecap(recap: Recap): void  {
  deps.session.recap = recap;
  deps.session.recapSeam = settleBoundary(recap);
  renderCallRecap();
}


// --- what the call in progress was given --------------------------------------------------------
//
// `#331 resume-on-websocket`. The owner: a visible indicator when a call starts, of how much earlier
// conversation the agent was given or why it started fresh. The seam above the call says it too, but
// a seam exists only where something is above it and says it in a chip; this line is under the call,
// in words, for as long as the call lasts, and the seam keeps the fact afterwards.

// Why a call started fresh, per recap `why` (`CALL_FRESH`), is web/src/core/call's.

/**
 * The line under a call for `recap`, given whether the voice service acknowledged it (`ack`): web/src/core/call
 * `callRecapLine`, with this page's reason for a fresh start no table names (`freshBecause`).
 *
 * @param {unknown} recap
 * @param {string | null} ack `accepted` once a `vibe-talk-v1` bridge has said it took the record
 */
function callRecapLine(recap: Recap, ack: string | null) {
  return recapLine(recap, ack, freshBecause);
}


/** Why a call started fresh when it was resuming that was off or unsupported: the server's reason. */
function freshBecause(why: unknown) {
  if (replayUnavailable === "bridge_declined") return "the voice service does not take a recap";
  if (replayUnavailable === "bridge_unconfirmed") return "the voice service has not said it takes a recap";
  if (replayUnavailable || why === "unsupported") return "this voice session takes no recap";
  if (!deps.resumeAllowed) return "resuming is off on this server";
  if (!deps.resumeWanted()) return "resuming is off";
  return "there was no earlier conversation to resume";
}


/** Draw the line under the call: shown while a call has a settled recap, hidden otherwise. */
function renderCallRecap() {
  const line = deps.el("call-recap");
  const said = deps.session.socket && deps.session.recap ? callRecapLine(deps.session.recap, deps.session.recapAck) : "";
  if (line.textContent === said && line.hidden === !said) return;
  const area = deps.leftEl("scroll-area");
  const pinned = pinnedHere(area);
  line.textContent = said;
  line.hidden = !said;
  if (said) {
    line.setAttribute("data-recap", deps.session.recap!.state === "sent" ? (deps.session.recapAck || "sent") : "fresh");
  } else {
    line.removeAttribute("data-recap");
  }
  // `#352 voice-reconnect-on-bridge-error`. Whether the session now running is one a reconnect opened.
  if (said && deps.session.recap!.reconnected) line.setAttribute("data-reconnected", "");
  else line.removeAttribute("data-reconnected");
  deps.followIfPinned(pinned);
  noteFollow(area, pinned);
}


/**
 * `#331 resume-on-websocket`. The record of the conversation above, handed to a `vibe-talk-v1` call
 * as the first frame after its first `session_started` — and only to a session that lists `replay`
 * in `capabilities`. A bridge that predates the frame answers it with `error`, which ends the
 * session, so a session that does not list it is sent nothing, and the line under the call says so.
 *
 * @param {VibeTalkV1ServerFrame} message the `session_started`
 */
function offerReplayAtSessionStart(message: VibeTalkV1ServerFrame) {
  const resume = deps.session.pendingResume;
  if (!resume || message.type !== "session_started") return;
  deps.session.pendingResume = null;
  const listed = Array.isArray(message.capabilities) && message.capabilities.includes("replay");
  const armed = typeof resume.profileRecap === "number" ? resume.profileRecap : null;
  if (listed) {
    // `#512 voice-recap-private-stack`. A session that lists `replay` after all is sent the frame,
    // which it answers; the recap armed in the profile is taken back.
    if (armed !== null) withdrawProfileRecap(armed);
    sendReplayFrame(resume);
    deps.session.recapAck = "waiting";
    awaitReplayAnswer();
    noteCallRecap(recapFor(resume, "replay_frame"));
  } else if (armed !== null) {
    // `#512`. Armed in the profile the bridge fetches as its session opens: asked about now, and
    // drawn as taken only once the server says a fetch served it. The conversation's first record
    // waits for that answer, as it waits for a `replay_ack`.
    deps.session.recapAck = "waiting";
    awaitReplayAnswer(PROFILE_RECAP_ANSWER_WAIT_MS);
    noteCallRecap(recapFor(resume, "profile"));
    watchProfileRecap(armed, settleCallProfileRecap, firstRecapAbandoned(deps.session.conversationId));
  } else {
    deps.resumeLast = { ...deps.resumeLast, armed: false, unsupported: true };
    deps.addDetail("this voice session does not list replay, so it was sent no recap");
    noteCallRecap(recapFor(resume, null));
    deps.renderResumeState();
  }
  expectStartMarker(deps.session.conversationId, deps.session.recap!, lastEndCause);
}


/**
 * The `replay` frame for `resume`, the server's record: the first frame a session that lists `replay`
 * is sent, at the call's start or after a reconnect (`#352 voice-reconnect-on-bridge-error`).
 *
 * @param {unknown} resume what the server's replay route answered
 */
function sendReplayFrame(resume: Resume): void  {
  const count = (value: unknown) => Math.min(MARKER_NUMBER_MAX, Math.max(0, Math.round(Number(value) || 0)));
  deps.sendVibeTalkFrame({
    type: "replay",
    text: String(resume.text),
    lines: count(resume.included),
    dropped: count(resume.dropped),
    words: count(resume.words),
  });
}


/**
 * The bridge's answer to the `replay` frame: taken, or refused with its reason. Exactly one per
 * frame; one nobody is waiting for says nothing.
 *
 * `#352 voice-reconnect-on-bridge-error`. The answer of a session a reconnect opened changes only
 * what the call says now: the line under the call, the connection details, the transcript's
 * "reconnected" line, and whether the page's word on a card goes again. Not the call's start
 * marker, nor what Settings says about how the call began.
 *
 * @param {VibeTalkV1ServerFrame} message the `replay_ack`
 */
function noteReplayAck(message: VibeTalkV1ServerFrame): void  {
  if (message.type !== "replay_ack" || deps.session.recapAck !== "waiting") return;
  if (deps.session.recap && deps.session.recap!.reconnected) {
    if (message.accepted === true) {
      deps.session.recapAck = "accepted";
      deps.addDetail("the reconnected session took this call's record");
      deps.settleNoticeForRecap(true, "");
    } else {
      const reason = typeof message.reason === "string" && message.reason.trim() ? message.reason.trim() : "no reason given";
      deps.session.recapAck = "refused";
      deps.session.recap = { state: "none", why: "refused", dropped: 0, reconnected: true };
      deps.addDetail(`the reconnected session refused this call's record: ${reason}`);
      deps.reconnectedSessionRefused();
      deps.settleNoticeForRecap(false, "the new session refused the record that says what the card was");
    }
    renderCallRecap();
    return;
  }
  if (message.accepted === true) {
    deps.session.recapAck = "accepted";
    deps.resumeLast = { ...deps.resumeLast, acked: true };
    deps.addDetail("the voice service took the recap");
    renderCallRecap();
  } else {
    deps.session.recapAck = "refused";
    const reason = typeof message.reason === "string" && message.reason.trim() ? message.reason.trim() : "no reason given";
    deps.addDetail(`the voice service refused the recap: ${reason}`);
    deps.resumeLast = { ...deps.resumeLast, armed: false, refused: true };
    markRecapNotHanded({ state: "none", why: "refused", dropped: 0 });
  }
  deps.renderResumeState();
  // After the marker has what the answer said: it is written once the records ahead of it are.
  endReplayAnswerWait();
}


/**
 * The call's session was handed no recap after all — refused, or not taken from the profile — and
 * `recap` says which.
 *
 * The session is the one every turn of the call went to, so the line above the call says so however
 * much has been drawn or filed below it, and so does the conversation's start marker, which waits for
 * this answer (`replayAnswerWait`). A marker already written, because that wait ran out before an
 * answer this late, is what a reload draws: the line keeps it as well, so the two never disagree.
 *
 * @param {{state: "none", why: string, dropped: number}} recap
 */
function markRecapNotHanded(recap: {state: "none", why: string, dropped: number}) {
  deps.session.recap = recap;
  const start = unwrittenStartMarker(deps.session.conversationId);
  const seam = deps.session.recapSeam;
  if (start) {
    start.recap = recap;
    if (seam && seam.parentNode) setSeamRecap(seam, boundaryFor(recap, seam.getAttribute("data-cause")));
  }
  renderCallRecap();
}


// --- a recap in the profile the bridge fetches ------------------------------------------------------
//
// `#512 voice-recap-private-stack`. A bridge that lists no `replay` may still fetch the voice-agent
// profile as each session opens and use its prompt verbatim; the server says so with
// `replay_via_profile`. Then the page arms the recap (`POST /api/v1/voice-recap`) just before it opens
// the socket, the bridge's fetch takes it into the prompt, and once the session has started the page
// asks what became of it — a few times, then a last `DELETE`, which takes it back unless a fetch
// already served it and answers which. Only a served recap is drawn as one; one never served says the
// call started fresh because the voice service did not take it. And one the call no longer needs —
// hung up before its session started, or a session that lost a turn — is taken back, so no later
// session is handed it unasked.

/** How often the page asks whether the bridge's fetch took the armed recap. */
const PROFILE_RECAP_POLL_MS = 500;


/**
 * When, after the session started, the page's last word on an armed recap goes, whatever the answers
 * before it took: an absolute deadline, so slow answers cannot push it later.
 */
const PROFILE_RECAP_WAIT_MS = 3500;


/** How long one question about an armed recap may take before it counts as unanswered. */
const PROFILE_RECAP_ASK_TIMEOUT_MS = 1500;


/**
 * How long the conversation's first record waits for the answer about a recap armed in the profile:
 * past the last word, its one retry and their timeouts, so the start marker is written with what the
 * answer said.
 */
const PROFILE_RECAP_ANSWER_WAIT_MS = 8000;


/**
 * Arm a recap of `conversationId` for the bridge's next profile fetch. Never throws: answers what was
 * armed — its number and the counts the server built — or why nothing was: `unavailable` when the
 * server no longer hands recaps over that way, so a session that lists `replay` can still be sent the
 * frame.
 *
 * @param {string} conversationId
 * @returns {Promise<{armed: {profileRecap: number, included: number, dropped: number, words: number | null} | null, why: "empty" | "too_long" | "failed" | "unavailable" | null, dropped: number}>}
 */
async function armProfileRecap(conversationId: string) {
  const count = (value: unknown) => Math.min(MARKER_NUMBER_MAX, Math.max(0, Math.round(Number(value) || 0)));
  try {
    const answer = await deps.api("/api/v1/voice-recap", { method: "POST", body: { conversation_id: conversationId } });
    if (answer && answer.armed === true && typeof answer.id === "number") {
      deps.session.armedRecap = answer.id;
      return {
        armed: {
          profileRecap: answer.id,
          included: count(answer.included),
          dropped: count(answer.dropped),
          words: typeof answer.words === "number" ? count(answer.words) : null,
        },
        why: null,
        dropped: 0,
      };
    }
    const dropped = count(answer && answer.dropped);
    return { armed: null, why: dropped > 0 ? "too_long" : "empty", dropped };
  } catch (caught) {
  const error = caught as ApiRequestError;
    if (error && error.status === 409) {
      deps.addDetail("this server no longer hands a recap over in the voice service's instructions");
      return { armed: null, why: "unavailable", dropped: 0 };
    }
    deps.addDetail(`the recap could not be armed for the voice service: ${error.message}`);
    return { armed: null, why: "failed", dropped: 0 };
  }
}


/**
 * A new call's recap, armed in the profile just before its socket opens: `resume` with the armed
 * recap's number and the counts the server built; `resume` as it was when the server no longer arms
 * one (a session that lists `replay` is then sent the frame); or null when nothing could be armed,
 * which Settings then says as a fetch of the record would have.
 *
 * @param {unknown} resume what `fetchResume` answered
 */
async function armRecapForCall(resume: Resume) {
  const conversationId = deps.resumeConversationId;
  if (!conversationId) return null;
  const armed = await armProfileRecap(conversationId);
  if (armed.armed) {
    deps.resumeLast = {
      ...deps.resumeLast,
      dropped: armed.armed.dropped,
      words: armed.armed.words,
    };
    deps.renderResumeState();
    return { ...resume, ...armed.armed };
  }
  if (armed.why === "unavailable") return resume;
  deps.resumeLast = armed.why === "failed"
    ? { ...deps.blankResumeLast(), attempted: true, failed: true }
    : { ...deps.blankResumeLast(), attempted: true, dropped: armed.dropped };
  deps.renderResumeState();
  return null;
}


/**
 * One question about armed recap `id`: its state, or null when it went unanswered within
 * PROFILE_RECAP_ASK_TIMEOUT_MS or failed.
 *
 * @param {number} id
 * @param {boolean} last whether this is the page's last word, which takes it back unless served
 * @returns {Promise<string | null>}
 */
async function askAboutRecap(id: number, last: boolean) {
  const controller = typeof AbortController === "function" ? new AbortController() : null;
  const timer = controller ? setTimeout(() => controller.abort(), PROFILE_RECAP_ASK_TIMEOUT_MS) : null;
  try {
    /** @type {{method?: string, signal?: AbortSignal}} */
    const options: {method?: string, signal?: AbortSignal} = last ? { method: "DELETE" } : {};
    if (controller) options.signal = controller.signal;
    const answer = await deps.api(`/api/v1/voice-recap/${id}`, options);
    return answer && typeof answer.state === "string" ? answer.state : null;
  } catch (caught) {
  const error = caught as ApiRequestError;
    deps.addDetail(`the page could not ask whether the voice service took the recap: ${error.message}`);
    return null;
  } finally {
    if (timer !== null) clearTimeout(timer);
  }
}


/**
 * Ask what became of armed recap `id` until a fetch served it or the deadline passes, then settle it
 * with a last word that takes it back unless served, asked once more if unanswered, and hand `settle`
 * the answer: served, not, or null when nobody could say. Stops, saying nothing to `settle`, once the
 * call has moved to another socket or let the watch go; `abandoned` then gets the answer to the page's
 * withdrawal instead (`abandonWatchedRecap`).
 *
 * @param {number} id
 * @param {(served: boolean | null) => void} settle
 * @param {(served: boolean | null) => void} [abandoned]
 */
function watchProfileRecap(id: number, settle: (served: boolean | null) => void, abandoned: (served: boolean | null) => void = () => {}) {
  const socket = deps.session.socket;
  const deadline = Date.now() + PROFILE_RECAP_WAIT_MS;
  deps.session.profileRecap = { id, timer: null, abandoned };
  const ours = () => deps.session.socket === socket && deps.session.profileRecap !== null && deps.session.profileRecap.id === id;
  const finish = (/** @type {boolean | null} */ served: boolean | null) => {
    deps.session.profileRecap = null;
    if (deps.session.armedRecap === id) deps.session.armedRecap = null;
    settle(served);
  };
  let retried = false;
  const ask = async () => {
    if (!ours()) return;
    const watch = /** @type {{id: number, timer: ReturnType<typeof setTimeout> | null}} */ (deps.session.profileRecap!);
    watch.timer = null;
    const last = Date.now() >= deadline;
    const state = await askAboutRecap(id, last);
    if (!ours()) return;
    if (state === "served") return finish(true);
    if (last && state === null && !retried) {
      retried = true;
      return void ask();
    }
    if (last) return finish(state === null || state === "unknown" ? null : false);
    if (state === "unknown") return finish(null);
    if (state !== null && state !== "armed") return finish(false);
    watch.timer = setTimeout(() => void ask(), Math.max(0, Math.min(PROFILE_RECAP_POLL_MS, deadline - Date.now())));
  };
  void ask();
}


/** Take back armed recap `id`, unasked for an answer: the call no longer needs it. */
function withdrawProfileRecap(id: number) {
  if (deps.session.armedRecap === id) deps.session.armedRecap = null;
  deps.api(`/api/v1/voice-recap/${id}`, { method: "DELETE" }).catch(() => {});
}


/**
 * Stop asking about the recap being watched — the call moved to another session, or ended — and take
 * it back, handing what the withdrawal answered to the watch's `abandoned`. Answers whether one was
 * being watched.
 */
function abandonWatchedRecap() {
  const watch = deps.session.profileRecap;
  if (!watch) return false;
  deps.session.profileRecap = null;
  if (deps.session.armedRecap === watch.id) deps.session.armedRecap = null;
  if (watch.timer !== null) clearTimeout(watch.timer);
  void askAboutRecap(watch.id, true).then((state) =>
    watch.abandoned(state === "served" ? true : state === null || state === "unknown" ? null : false));
  return true;
}


/** Take back a recap this call armed and has not settled, however far the call got. */
function withdrawArmedRecap() {
  if (abandonWatchedRecap()) return;
  if (deps.session.armedRecap !== null) withdrawProfileRecap(deps.session.armedRecap);
}


/**
 * What became of the recap armed in the profile for the session this call opened with: taken, as a
 * `replay_ack` that accepted it would say; not, as a refusal would, saying why; or unknown, when the
 * page could not learn it, which leaves it said as sent and no more.
 *
 * @param {boolean | null} served
 */
function settleCallProfileRecap(served: boolean | null) {
  if (served === true) {
    deps.session.recapAck = "accepted";
    deps.resumeLast = { ...deps.resumeLast, acked: true };
    deps.addDetail("the voice service took the recap: the instructions it fetched for this session carried it");
    renderCallRecap();
  } else if (served === false) {
    deps.session.recapAck = "not_taken";
    deps.addDetail("the voice service did not take the recap: no fetch of its instructions carried it before the page stopped asking");
    deps.resumeLast = { ...deps.resumeLast, armed: false, unsupported: true };
    markRecapNotHanded({ state: "none", why: "not_taken", dropped: 0 });
  } else {
    deps.session.recapAck = null;
    deps.addDetail("the page could not learn whether the voice service took the recap, so it is said as sent, no more");
    renderCallRecap();
  }
  deps.renderResumeState();
  endReplayAnswerWait();
}


/**
 * The answer about the call's first recap, after the call moved to a new session or ended before the
 * page stopped asking: only the conversation's start marker, while it is unwritten, and Settings take
 * it — the line under the call is about the session now running — and the first record goes.
 *
 * @param {string | null} conversationId the conversation the call was filed under
 * @returns {(served: boolean | null) => void}
 */
function firstRecapAbandoned(conversationId: string | null) {
  return (served: boolean | null) => {
    if (served === true) {
      deps.resumeLast = { ...deps.resumeLast, acked: true };
    } else if (served === false) {
      deps.resumeLast = { ...deps.resumeLast, armed: false, unsupported: true };
      const start = unwrittenStartMarker(conversationId);
      if (start) start.recap = { state: "none", why: "not_taken", dropped: 0 };
    }
    deps.renderResumeState();
    endReplayAnswerWait();
  };
}


/**
 * `#331 resume-on-websocket`. How long after the `replay` frame the conversation's first record may
 * wait for the bridge's answer to it (`replayAnswerWait`).
 */
const REPLAY_ANSWER_WAIT_MS = 5000;


/**
 * `#331 resume-on-websocket`. The wait for the bridge's answer to this call's `replay` frame, while
 * there is one: what the record queue waits on, and what ends it.
 *
 * The conversation's start marker says what that answer did, and the line above the call follows the
 * marker. Written before the answer, the marker would say "recap sent" for good, and a refusal could
 * then change neither — and a typed call files its first message in the very handler that sends the
 * frame, so every one would. So a first record filed while the answer is awaited goes into the queue
 * behind this wait, with every record after it, and its marker is written as its request goes, from
 * what is known by then. The wait ends with the answer, with a replacement session (the answer is
 * then nobody's), with the call, or REPLAY_ANSWER_WAIT_MS after the frame went, whichever comes first:
 * a bridge that never answers holds the record back no longer than that.
 *
 * @type {{done: Promise<void>, end: () => void, timer: ReturnType<typeof setTimeout>} | null}
 */
let replayAnswerWait: {timer: Timer; end: () => void; done: Promise<void>} | null = null;


/**
 * The start marker filed behind {@link replayAnswerWait} and not yet written, with its conversation.
 *
 * @type {{conversationId: string, start: {recap: unknown, previous_end?: string}} | null}
 */
let startAwaitingAnswer: {conversationId: string; start: StartMarker} | null = null;


/**
 * The `replay` frame went, or a recap was armed in the profile: a first record filed from now waits
 * for its answer, `ms` at most.
 *
 * @param {number} [ms]
 */
function awaitReplayAnswer(ms: number = REPLAY_ANSWER_WAIT_MS) {
  endReplayAnswerWait();
  /** @type {() => void} */
  let end: () => void = () => {};
  /** @type {Promise<void>} */
  const done: Promise<void> = new Promise((resolve) => {
    end = () => resolve();
  });
  replayAnswerWait = { done, end, timer: setTimeout(endReplayAnswerWait, ms) };
}


/** Stop waiting for the answer: a record held for it goes as soon as those ahead of it have. */
function endReplayAnswerWait() {
  const wait = replayAnswerWait;
  if (!wait) return;
  replayAnswerWait = null;
  clearTimeout(wait.timer!);
  wait.end();
}


/**
 * The start marker of `conversationId` while it can still change: not yet filed, or filed behind the
 * wait for the answer and not yet written. Null once it is written.
 *
 * @param {string | null} conversationId
 */
function unwrittenStartMarker(conversationId: string | null) {
  if (!conversationId) return null;
  const start = startMarkers.get(conversationId);
  if (start) return start;
  const held = startAwaitingAnswer;
  return held && held.conversationId === conversationId ? held.start : null;
}


/** The conversation `conversationId` has begun: its first record is preceded by its start marker. */
function expectStartMarker(conversationId: string | null, recap: Recap, previousEnd: string | null) {
  if (!conversationId) return;
  startMarkers.set(conversationId, { recap, previous_end: previousEnd || undefined });
  if (startMarkers.size > 16) startMarkers.delete(startMarkers.keys().next().value!);
}


/**
 * The hosted voice service named the conversation this call is filed under. The first time, that
 * conversation starts with a marker of what the call was handed. Again on the same socket, a
 * replacement session that was handed nothing has begun a conversation of its own: the line between
 * the two says so, and so does its marker.
 *
 * @param {string | null} previous the conversation filed under until now
 */
function noteConversationNamed(previous: string | null) {
  if (deps.session.sessionStarts <= 1) {
    expectStartMarker(deps.session.conversationId, deps.session.recap || recapFor(null, null), lastEndCause);
    return;
  }
  if (!previous || previous === deps.session.conversationId) return;
  const recap = noteSessionReplaced();
  setSeamRecap(drawBoundary("replaced", previous), boundaryFor(recap, "replaced"));
  // `#331 resume-on-websocket`. The line under the call is about the session now running.
  renderCallRecap();
  expectStartMarker(deps.session.conversationId, recap, null);
}


/**
 * `#331 resume-on-websocket`. The voice service replaced this call's session, and the session now
 * running was sent no recap: whatever went, went to the one before it. The call holds that as its
 * recap, an answer the old session still owed is nobody's, and Settings stops saying the last call
 * was resumed, though the next call is offered the record as before. Answers the recap.
 */
function noteSessionReplaced() {
  const recap = { state: "none", why: "replaced_session", dropped: 0 };
  deps.session.recap = recap;
  deps.session.recapAck = null;
  // A record held for that answer goes, with what its caller makes of the replacement first.
  endReplayAnswerWait();
  if (deps.resumeLast.armed) {
    deps.resumeLast = { ...deps.resumeLast, replaced: true };
    deps.renderResumeState();
  }
  return recap;
}


/**
 * `#331 resume-on-websocket`. A second `session_started` on a `vibe-talk-v1` socket: the bridge
 * replaced the model session. The record goes once per call, as the first frame to the session it
 * opened with, so the replacement is sent none, as on the hosted backend (`noteConversationNamed`).
 * The conversation is still this call's, so no line is drawn between the two sessions: the line
 * under the call says the session now running started fresh, and, while nothing has been said
 * since a recap went, so do the line above the call and the conversation's start marker, as a
 * refusal does (`noteReplayAck`). Once something has been said, those keep what its session was sent.
 * "Said" is the line's own test (`settleBoundary`), and the marker follows the line, so a reload says
 * what it did: a turn is drawn as it comes but filed, with the marker before it, only when it ends,
 * so a marker still unfiled is no sign that nothing was said.
 */
function noteReplacedV1Session() {
  const handed = deps.session.recap;
  const recap = noteSessionReplaced();
  deps.addDetail("the voice service replaced the session, and the new one was sent no recap");
  if (handed && handed.state === "sent" && settleBoundary(recap)) {
    const start = unwrittenStartMarker(deps.session.conversationId);
    if (start) start.recap = recap;
  }
  renderCallRecap();
}


/**
 * Something is about to be filed in `conversationId`. A conversation's first record is preceded by
 * its start marker, and from then on this call has recorded something.
 */
function fileStartMarker(conversationId: string) {
  if (conversationId === deps.session.conversationId) deps.session.recordedAny = true;
  const start = startMarkers.get(conversationId);
  if (!start) return;
  startMarkers.delete(conversationId);
  if (!transcriptRecordsSupported) return;
  // `#331 resume-on-websocket`. While the bridge's answer to this call's recap is awaited, the
  // marker, and every record filed after it, waits for that answer in the queue, and is written
  // from what the answer made of it (`replayAnswerWait`).
  const wait = conversationId === deps.session.conversationId ? replayAnswerWait : null;
  if (wait) {
    startAwaitingAnswer = { conversationId, start };
    recordQueue = recordQueue.then(() => wait.done);
  }
  void postRecord(conversationId, () => {
    if (startAwaitingAnswer && startAwaitingAnswer.start === start) startAwaitingAnswer = null;
    // A server that keeps records says how many words its replay held. Were one not to, the marker
    // still needs a number: 0 over lines that hold words, which `recapWords` reads back as unknown.
    const recap = start.recap && start.recap.state === "sent" && start.recap.words === null
      ? { ...start.recap, words: 0 }
      : start.recap;
    /** @type {Record<string, unknown>} */
    const marker: Record<string, unknown> = { kind: "conversation_start", recap };
    if (start.previous_end) marker.previous_end = start.previous_end;
    return { speaker: "marker", text: "", marker };
  });
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

let clearArmedTimer: Timer | null = null;


function disarmClear() {
  if (clearArmedTimer !== null) {
    clearTimeout(clearArmedTimer);
    clearArmedTimer = null;
  }
  deps.el("clear-view").className = "control control-mini";
  deps.el("clear-view-label").textContent = "Clear";
}


function armClear() {
  deps.el("clear-view").className = "control control-mini armed";
  deps.el("clear-view-label").textContent = "Sure?";
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
    deps.setStatus("Tap Clear again to empty the transcript.");
    return;
  }
  disarmClear();
  deps.el("transcript").replaceChildren();
  deps.renderEmptyState();
  deps.renderScrollTools(); // the folds went with the lines they were attached to.
  // ...and so did the walk back through the record. Clear empties the SCREEN — offering "Earlier
  // turns" on the empty screen it just made would quietly refill it, which is the one thing the
  // sentence beside this control promises it does not do. `#128 transcript-history`.
  forgetTranscriptWalk();
  if (deps.session.socket) {
    transcriptSeam(
      "view cleared",
      "The screen was emptied; nothing else was. The agent still has everything said before " +
        "this point. Hang up is what ends the call."
    );
    deps.setStatus("Transcript cleared. The agent has not forgotten anything.");
  } else {
    deps.setStatus("Transcript cleared.");
  }
}


// --- confirming a post the assistant proposed ----------------------------------------------------
//
// `#34 voice-chat-write-confirm`. While the owner has "Ask me before the voice agent sends" on (the
// shipped default; `#264 voice-send-approval`), the voice agent's `post_reply` tool never sends. It
// leaves ONE pending proposal on the server — channel, reply target, exact text — and the owner
// confirms it outside the model's tools: by tapping Send here, or by a spoken yes that THIS PAGE
// judges (see "a spoken yes to the card" below). Everything a model can reach is the proposing
// half, so a model talked into posting by a message it read can at worst put a card on this screen.
// With the setting off the tool posts at once and no card appears; nothing here changes for that.
//
// The card shows the SERVER's copy, fetched from `/api/v1/post-proposals`, never anything the
// agent said. Send restates that copy with the handle, and the server refuses unless all of it
// still matches, so what the owner read is what gets posted. The handle travels only on those two
// requests: it is not in the tool's answer, the transcript, or unknown log.
//
// Watched only during a write-scope call. A read-scope token cannot see proposals at all, and
// outside a call there is no agent to propose one.
const POST_WATCH_WAIT_SECONDS = 25;

const POST_WATCH_RETRY_MS = 3000;

const POST_EXPIRY_TICK_MS = 1000;

// How long Send and Don't send stay disabled after a card appears or its text is replaced by a newer
// proposal, so that a tap already on its way lands on nothing rather than on words the owner has not
// read. Both: the card appears as the next message in the list, where a tap aimed at the last row may
// land on either button. `#269 approval-card-inline`.
const POST_CHANGE_HOLD_MS = 2000;


const postWatch = {
  // Bumped to stop a loop: a loop whose generation is no longer current returns at its next await.
  generation: 0,
  running: false,
  /** @type {AbortController | null} */
  controller: null as AbortController | null,
  /** @type {PendingPost | null} */
  shown: null as PendingPost | null,
  // The serial of the last proposal the server reported, shown or not: the long poll's `seen`.
  /** @type {number | null} */
  received: null as number | null,
  // The handle on the card when the owner hung up. The next call does not offer it: a draft from a
  // conversation he ended is not something to confirm in a new one. A call that DROPPED keeps its
  // card instead (`dropped`). `#269 approval-card-inline`.
  /** @type {string | null} */
  ignored: null as string | null,
  // The card outlived a call that ended without a Hang up — the service closed it, the connection
  // failed, the page was put away — and is kept, tap only, until it lapses or the next call opens.
  dropped: false,
  // The drafts whose "Not sent" line is drawn, under `postKey`, so none is marked twice.
  /** @type {Set<string>} */
  notSent: new Set() as Set<string>,
  // When the shown proposal lapses, fixed once when it first appears so a re-render cannot extend it.
  deadline: 0,
  tick: null as Timer | null,
  // Set while both buttons are held after a card appears; `changed` marks a replacement until it is
  // settled.
  hold: null as Timer | null,
  changed: false,
  busy: false,
  // Whether the owner opened a long draft with More. A new draft starts folded again.
  unfolded: false,
  // Set while the main screen keeps the card's room for a moment after the card went.
  /** @type {ReturnType<typeof setTimeout> | null} */
  roomTimer: null as ReturnType<typeof setTimeout> | null,
  // `#270 voice-transcript-markers`. The send memory's cursor, for the page's whole life: the
  // newest `send_seq` this page has read, null until its first read ever says where the server is,
  // and the run of the server (`instance`) whose numbering it is in.
  /** @type {number | null} */
  sentThrough: null as number | null,
  /** @type {string | null} */
  sentInstance: null as string | null,
  // Each proposal this page showed, under `postKey` — its serial AND the run of the server that
  // numbered it, because a restarted server numbers its proposals from one again and the page
  // stays open across that — with its handle and where it was shown: a send of one of them is this
  // page's, and belongs to the conversation it was shown in, wherever the server says it began.
  /** @type {Map<string, {handle: string, conversationId: string | null, window: {conversationId: string | null, from: number | null, to: number | null} | null}>} */
  shownSerials: new Map() as Map<string, {handle: string, conversationId: string | null, window: {conversationId: string | null, from: number | null, to: number | null} | null}>,
  // The frozen card drawn for each send, under the same key, so no send is drawn twice: the card,
  // the `send_seq` it was drawn under when known, and the handle of the proposal it froze from when
  // the commit's answer drew it (null when the send memory did).
  /** @type {Map<string, {li: HTMLElement, sendSeq: number | null, handle: string | null}>} */
  sentDrawn: new Map() as Map<string, {li: HTMLElement, sendSeq: number | null, handle: string | null}>,
  // This page's calls in the server's clock, each `{conversationId, from, to}`: a send that began
  // inside one is this page's. `window` is the open one, whose end is still "now".
  /** @type {Array<{conversationId: string | null, from: number | null, to: number | null}>} */
  windows: [] as Array<{conversationId: string | null, from: number | null, to: number | null}>,
  /** @type {{conversationId: string | null, from: number | null, to: number | null} | null} */
  window: null as {conversationId: string | null, from: number | null, to: number | null} | null,
  // The server's clock at the last read, and this page's when it arrived, to close a window whose
  // last read failed.
  lastNowMs: 0,
  lastNowAt: 0,
  // The frozen card the focus goes to when the card it froze from goes with the focus inside it.
  /** @type {HTMLElement | null} */
  parkOn: null as HTMLElement | null,
};


/** Whether this server takes the voice page's transcript records. `#270 voice-transcript-markers`. */
let transcriptRecordsSupported = false;


// What a refused confirmation means, per code. Every one of them says that THIS tap posted nothing,
// because that is the question the owner has at that moment.
const POST_REFUSALS: Record<string, string> = {
  proposal_expired: "That proposal lapsed before Send reached the server. Nothing was posted.",
  proposal_superseded:
    "The assistant replaced that proposal with a newer one before Send arrived. Nothing was posted from that card.",
  proposal_used:
    "That proposal had already been settled — confirmed, withdrawn, or refused. This tap posted nothing.",
  proposal_mismatch: "The server's proposal did not match this card, so nothing was posted.",
  proposal_unknown: "The server holds no such proposal (it may have restarted). Nothing was posted.",
};


function startPostWatch() {
  if (deps.tokenScope !== "write" || postWatch.running) return;
  resetSpokenYes();
  // `#264 voice-send-approval`. Read once, as the call starts, because that is when the agent was
  // handed its instructions: one told to post at once asks nothing, so no yes is judged in this call.
  spokenYes.askFirst = !(deps.voiceSendSetting && deps.voiceSendSetting.ask_before_send === false);
  postWatch.running = true;
  postWatch.generation += 1;
  // A card kept from a call that dropped is this call's to show now, still tap only.
  postWatch.dropped = false;
  // `#270 voice-transcript-markers`. This call's window on the send memory opens at its first read.
  postWatch.window = { conversationId: null, from: null, to: null };
  postWatch.windows.push(postWatch.window);
  void watchPostProposals(postWatch.generation);
}


function stopPostWatch() {
  // First, and silently: the call is over, so there is nobody to tell about a yes it interrupted.
  resetSpokenYes();
  postWatch.running = false;
  postWatch.generation += 1;
  if (postWatch.controller) postWatch.controller.abort();
  postWatch.controller = null;
  // The card is NOT taken down here. What becomes of it depends on how the call ended, which only
  // `noteConversationEnded` knows: a Hang up retires it, and a call the owner did not end keeps it,
  // tap only — `resetSpokenYes` above has made it ineligible for a yes — until it lapses. `#269
  // approval-card-inline`.
  // `#270 voice-transcript-markers`. The call's window closes at one last read of the send memory,
  // which also brings a send that settled between the last waiting read and now.
  const window = postWatch.window;
  postWatch.window = null;
  if (window) {
    window.conversationId = deps.session.conversationId;
    void finalSendsRead(window);
  }
}


/** The path of one read of the proposal and the send memory. */
function postWatchPath(waitSeconds: number) {
  const query = [];
  if (waitSeconds > 0) {
    query.push(`wait=${waitSeconds}`);
    if (postWatch.received !== null) query.push(`seen=${postWatch.received}`);
  }
  // On every read once the cursor is known, a retry after an error included.
  if (transcriptRecordsSupported && postWatch.sentThrough !== null) {
    query.push(`sent_after=${postWatch.sentThrough}`);
  }
  return query.length ? `/api/v1/post-proposals?${query.join("&")}` : "/api/v1/post-proposals";
}


/**
 * What one read said about the send memory. `#270 voice-transcript-markers`.
 *
 * The page's first read ever only learns where the server's numbering is. Every later one carries
 * the sends past the page's cursor, and each that is this page's — it began inside one of this
 * page's calls, or it is a proposal this page showed — is drawn as a frozen card, once, and filed
 * in the transcript from the server's own copy.
 *
 * The cursor only goes FORWARD within one run of the server: two reads whose answers crossed on the
 * way — the last read of a call that ended and the first of the next — must not take it back and
 * have the next read bring sends already drawn. A different run numbers its sends from its own
 * clock, above anything the last one handed out, so its number is taken as it is; nothing drawn
 * here is forgotten, because every card is known by its run as well as its serial.
 *
 * @param {PendingPostResponse} answer
 */
function noteSendMemory(answer: PendingPostResponse) {
  if (!transcriptRecordsSupported) return;
  if (typeof answer.now_ms === "number") {
    postWatch.lastNowMs = answer.now_ms;
    postWatch.lastNowAt = Date.now();
    if (postWatch.window && postWatch.window.from === null) postWatch.window.from = answer.now_ms;
  }
  // An older server says nothing about a send memory: there is nothing to read.
  if (typeof answer.sends_through !== "number") return;
  const instance = answer.instance || "";
  if (postWatch.sentThrough === null) {
    postWatch.sentThrough = answer.sends_through;
    postWatch.sentInstance = instance;
    return;
  }
  for (const send of answer.sends || []) drawSentFromMemory(send, instance);
  if (instance === postWatch.sentInstance) {
    postWatch.sentThrough = Math.max(postWatch.sentThrough, answer.sends_through);
  } else {
    postWatch.sentThrough = answer.sends_through;
    postWatch.sentInstance = instance;
  }
}


/**
 * Remember that the page showed `proposal`, in the conversation on screen. The same proposal shown
 * again — after a drop, in the next call — keeps the conversation it was first shown in; another
 * proposal under a key seen before (a server that restarted and numbered from one again, but named
 * no run) replaces it. Bounded, oldest out.
 *
 * @param {PendingPost} proposal
 */
function noteProposalShown(proposal: PendingPost) {
  const key = postKey(proposal.instance, proposal.serial);
  const held = postWatch.shownSerials.get(key);
  if (held && held.handle === proposal.handle) return;
  postWatch.shownSerials.delete(key);
  postWatch.shownSerials.set(key, {
    handle: proposal.handle,
    conversationId: deps.session.conversationId,
    window: postWatch.window,
  });
  if (postWatch.shownSerials.size > 64) {
    postWatch.shownSerials.delete(postWatch.shownSerials.keys().next().value!);
  }
}


/**
 * The conversation the proposal under `key` was shown in: undefined when this page never showed
 * it, null when nobody can say. A proposal shown before the voice service named the conversation —
 * the hosted protocol names it a moment after the call opens — belongs to the conversation its call
 * was given since, which that call's window holds.
 *
 * @param {string} key
 * @returns {string | null | undefined}
 */
function shownConversation(key: string) {
  const held = postWatch.shownSerials.get(key);
  if (!held) return undefined;
  if (held.conversationId) return held.conversationId;
  const window = held.window;
  if (!window) return null;
  return (window === postWatch.window ? deps.session.conversationId : window.conversationId) || null;
}


/**
 * The conversation a send that began at `beganAt` (server clock) belongs to on this page, or
 * undefined when it began in none of this page's calls.
 */
function windowConversation(beganAt: number) {
  for (let at = postWatch.windows.length - 1; at >= 0; at -= 1) {
    const window = postWatch.windows[at]!;
    if (window.from === null || beganAt < window.from) continue;
    if (window.to !== null && beganAt > window.to) continue;
    return window.to === null && window === postWatch.window ? deps.session.conversationId : window.conversationId;
  }
  return undefined;
}


/**
 * @param {VoiceSend} send
 * @param {string} instance the run of the server that answered, for a send that does not say
 */
function drawSentFromMemory(send: VoiceSend, instance: string) {
  const detail = send.detail;
  const key = postKey(send.instance || instance, detail.serial);
  const shown = shownConversation(key);
  const drawn = postWatch.sentDrawn.get(key);
  if (drawn && (drawn.sendSeq === null || drawn.sendSeq === detail.send_seq)) {
    // Drawn from the commit's answer, which had no number to file it under: this one does.
    if (drawn.sendSeq === null) drawn.sendSeq = detail.send_seq;
    recordSent(detail.send_seq, shown || deps.session.conversationId, key);
    return;
  }
  const conversationId = shown !== undefined ? shown : windowConversation(detail.begun_at_ms);
  if (conversationId === undefined) return;
  const turn = { speaker: "sent", text: send.text, at_ms: detail.sent_at_ms, detail: { kind: "sent", ...detail } };
  const li = placeSentCard(turn, conversationId || null);
  postWatch.sentDrawn.set(key, { li, sendSeq: detail.send_seq, handle: null });
  // `#269 approval-card-inline`. A yes whose commit's answer was lost was shown again; it sent.
  if (detail.how === "voice") absorbAnswerRow(conversationId || null, detail.serial);
  // A card still up for this send — its commit's answer is on its way, or was lost: the memory says
  // it went, so the card freezes into this one, the focus with it.
  if (postWatch.shown && postKey(postWatch.shown.instance, postWatch.shown.serial) === key) {
    postWatch.parkOn = li;
    showPostProposal(null);
  }
  recordSent(detail.send_seq, conversationId || null, key);
}


/** The window's last read: what settled before it closed, and when it did. */
async function finalSendsRead(window: CallWindow) {
  if (!transcriptRecordsSupported || postWatch.sentThrough === null) {
    window.to = postWatch.lastNowMs + (Date.now() - postWatch.lastNowAt);
    return;
  }
  try {
    const answer = await deps.apiDecoded("PendingPostResponse", postWatchPath(0));
    window.to = typeof answer.now_ms === "number"
      ? answer.now_ms
      : postWatch.lastNowMs + (Date.now() - postWatch.lastNowAt);
    noteSendMemory(answer);
  } catch (_error) {
    // The server's clock now, as near as this page can say: a window left open would claim
    // another tab's sends for a call that is over.
    window.to = postWatch.lastNowMs + (Date.now() - postWatch.lastNowAt);
  }
}


/** One read of the send memory outside a call, for a commit whose answer was lost. */
async function readSendsOnce() {
  if (!transcriptRecordsSupported || postWatch.sentThrough === null || postWatch.running) return;
  try {
    noteSendMemory(await deps.apiDecoded("PendingPostResponse", postWatchPath(0)));
  } catch (_error) {
    // Nothing more to do: the card the commit left is still there for the owner to read.
  }
}


async function watchPostProposals(generation: number) {
  // The first read does not wait, so a proposal already pending when the call opened is shown now.
  let waitSeconds = 0;
  let first = true;
  while (postWatch.generation === generation) {
    const controller = typeof AbortController === "function" ? new AbortController() : null;
    postWatch.controller = controller;
    const path = postWatchPath(waitSeconds);
    try {
      const answer = await deps.apiDecoded(
        "PendingPostResponse", path, controller ? { signal: controller.signal } : undefined
      );
      if (postWatch.generation !== generation) return;
      noteSendMemory(answer);
      const proposal = answer.proposal;
      postWatch.received = proposal ? proposal.serial : null;
      if (first) {
        // Shown, for a tap, but not this call's agent's draft: no spoken yes may send it.
        first = false;
        noteProposalsAtCallOpen(proposal);
      }
      const next = proposal && proposal.handle === postWatch.ignored ? null : proposal;
      // A commit or a withdrawal on its way settles the card itself, from its answer. The server
      // wakes this watch the moment it spends the handle, before that answer; taken down here, the
      // card would vanish before the frozen card that replaces it is drawn, and take the focus with
      // it. `#270 voice-transcript-markers`.
      if (!(next === null && postWatch.busy && postWatch.shown)) {
        noteCardTakenDown(answer, next);
        showPostProposal(next);
      }
      waitSeconds = POST_WATCH_WAIT_SECONDS;
    } catch (caught) {
  const error = caught as ApiRequestError;
      if (postWatch.generation !== generation) return;
      // A token that cannot see proposals will not start seeing them by asking again.
      if (error.refused) {
        postWatch.running = false;
        return;
      }
      waitSeconds = 0;
      await new Promise((resolve) => setTimeout(resolve, POST_WATCH_RETRY_MS));
    }
  }
}


/** @param {PendingPost | null} proposal */
function showPostProposal(proposal: PendingPost | null): void  {
  if (postWatch.tick !== null) {
    clearTimeout(postWatch.tick);
    postWatch.tick = null;
  }
  const previous = postWatch.shown;
  // Measured BEFORE the card is put up or its text changes, because putting it up is what changes
  // the answer: a reader at the newest line follows the card down, one scrolled up is not moved.
  // `#269 approval-card-inline`.
  const pinned = followsCard();
  const cardsFoot = cardsAtFootUnderCard();
  const card = deps.leftEl("post-confirm");
  const hadFocus = nodeWithin(card, document.activeElement);
  postWatch.shown = proposal;
  noteCardForSpokenYes(proposal, previous);
  if (!proposal) {
    if (postWatch.hold !== null) clearTimeout(postWatch.hold);
    postWatch.hold = null;
    postWatch.changed = false;
    postWatch.unfolded = false;
    // Asked BEFORE the card goes, which is what changes the answer. `#280 scroll-stability`.
    const alone = cardAloneOnScreen();
    placePostCard();
    if (alone) deps.inColumn(deps.columns.main, deps.scrollToNewest);
    if (hadFocus) parkFocusOffCard();
    deps.renderJumpNewest();
    return;
  }
  const fresh = !previous || previous.handle !== proposal.handle;
  if (fresh) {
    // `#270 voice-transcript-markers`. A send of this proposal is this page's, and belongs to the
    // conversation it was shown in, wherever the send memory says it began.
    noteProposalShown(proposal);
    postWatch.deadline = Date.now() + proposal.expires_in_ms;
    // Words the owner may be about to confirm appeared under a tap aimed at whatever was there
    // before — a row, the old text, or, now that the card is the next message in the list, the
    // spot the last row was in. BOTH buttons are held long enough for that tap to miss: a tap that
    // landed on Don't send would withdraw a draft the owner had not read either. It says when the
    // text changed.
    if (postWatch.hold !== null) clearTimeout(postWatch.hold);
    postWatch.changed = Boolean(previous);
    postWatch.unfolded = false;
    postWatch.hold = setTimeout(() => {
      postWatch.hold = null;
      if (postWatch.shown) showPostProposal(postWatch.shown);
    }, POST_CHANGE_HOLD_MS);
    // Said once, politely: the countdown is not a live region, so this is how a reader who is not
    // looking at the card hears that a draft is waiting, or that its words changed.
    deps.setStatus(previous
      ? `The assistant changed the draft to ${proposal.channel_name} — read it again.`
      : `Draft to ${proposal.channel_name} is waiting: Send or Don't send.`);
  }
  deps.el("post-confirm-where").textContent = postCardWhere(proposal);
  // Verbatim, as text. It is the one thing on the card the owner is confirming, so nothing is
  // shortened or reformatted — except a sign-in token, which is never put on screen and never sent.
  const shown = deps.redact(proposal.text);
  deps.el("post-confirm-text").textContent = shown;
  // A long draft is folded, with More to read the rest in place: never a scroll box inside the
  // scrolling list. Folded by the length that folds a message, so the card agrees with the rows.
  const long = shown.length > deps.COLLAPSE_OVER_CHARS;
  const more = deps.el("post-confirm-more");
  more.hidden = !long;
  const folded = long && !postWatch.unfolded;
  card.toggleAttribute("data-folded", folded);
  more.textContent = folded ? deps.FOLD_MORE : deps.FOLD_LESS;
  more.setAttribute("aria-expanded", folded ? "false" : "true");
  // "Send to lead team": still beginning with Send, as the agent's instructions and the help say.
  // A long name is cut on screen; the accessible name stays whole.
  const sendLabel = `Send to ${proposal.channel_name}`;
  deps.el("post-confirm-send-label").textContent = sendLabel;
  deps.el("post-confirm-send").setAttribute("aria-label", sendLabel);
  deps.el("post-confirm-send").disabled =
    postWatch.busy || postWatch.hold !== null || shown !== proposal.text;
  renderPostCancel();
  renderPostExpiry();
  placePostCard();
  // Only over the list on screen: a draft that arrives while the owner is in Settings waits docked
  // there, and must not move the list he left. Under the thread list that is the left column's, laid
  // out behind the cards, and a reader left on its newest line is kept on it as the docked card takes
  // its height, so that Ctrl+1 finds them there with the card (`leaveThreadList`).
  if (fresh && deps.currentScreen === "main") deps.followIfPinned(pinned, 0, deps.currentView);
  // ...and the cards over it, whose foot the card takes as it comes, and as its words change or open.
  if (cardsFoot) keepCardsAtFoot();
  deps.renderJumpNewest();
}


/**
 * Whether the thread list is up with its reader at the foot of its cards, asked BEFORE the card docked
 * under them comes or changes size: as the dock growing under them does (`#281 view-keys` x `#280
 * scroll-stability`), it keeps a reader at the foot there and leaves one up the cards on their card.
 * Kept at once, in the same task, so that nothing waits for the browser to report the sizes; the
 * dock-growth keeper (`heightUnderLists`) covers the card growing with nothing redrawn.
 */
function cardsAtFootUnderCard() {
  return deps.threadListOnScreen() && deps.columns.side.mode === "list" && !deps.cardsOpening && deps.atBottom(deps.leftEl("side-scroll"));
}


/**
 * Whether the reader sees nothing of the list on screen but the send card at its foot, and perhaps the
 * box under it: the card stands across the list's top edge. Asked BEFORE the card goes. `#269
 * approval-card-inline` x `#280 scroll-stability`.
 *
 * A long draft opened by More stands taller than a phone's list — in Chromium 907px in a list 739px tall
 * at 393x852 — so a reader near its foot sees the card, Send on screen and tappable, and no row. The card
 * is the list's next message, and that reader is where it is: once it goes, its place is the list's
 * foot, so they are put on the newest line, and follow the list as a reader at its foot does. Nothing
 * else puts them there. Chromium's own scroll anchoring holds the box under the card where it stood as
 * the card goes, and so leaves the reader exactly as far above the foot as they were. Readers 30 to 80px
 * up Main or All, and 30 to 100px up the call's view, past the slack and with the box on screen, were
 * left there: the box took back its room a second later with them held on their row (`holdDraftRoom`),
 * the jump came with its room, and they ended 132 to 210px above the foot, offered the jump, no longer
 * following, at 393x852, 360x780 and 375x667. With anchoring turned off (`overflow-anchor: none`), as in
 * a browser that has none, the list's clamp took every one of them to the foot. On the call's view the
 * frozen card is drawn just above the card first (`placeInConversation`), and Chromium's anchoring keeps
 * the card where it stood through that too; a browser without it moves the card down under the reader,
 * and the clamp takes them to the foot once it goes.
 *
 * Only there: a reader who can see a row is on that row, which the card going below it does not move,
 * unless the list grown shorter is clamped to its foot under them. Never under the thread list, where the
 * card is docked: Main's list behind the cards has its own keeper (`cardMovedUnderLists`), and the place
 * the way back puts back is read as the list comes (`keepReaderPlace`).
 */
function cardAloneOnScreen() {
  const card = deps.leftEl("post-confirm");
  const place = card.getAttribute("data-place");
  if (deps.currentScreen !== "main" || card.hidden || (place !== "channel" && place !== "voice")) return false;
  const top = deps.leftEl("scroll-area").getBoundingClientRect().top;
  const box = card.getBoundingClientRect();
  return box.top <= top + 0.5 && box.bottom > top;
}


/** Take the cards to their foot again, above the card docked under them. */
function keepCardsAtFoot() {
  if (!deps.threadListOnScreen()) return;
  deps.followFoot(deps.leftEl("side-scroll"));
}


/**
 * Whether the reader follows the send card down as it is put up or opened: the reader on the newest
 * line of the list on screen, asked BEFORE, because the card is what changes the answer. `#269
 * approval-card-inline`.
 *
 * On the channel, by the rule a message arriving live is followed by (`liveFollowers`, `#280
 * scroll-stability`): the card is the list's tentative next message, and it moves the reader no more
 * than a message would. Not a reader a filter change holds in place (`#243`), whose message would be
 * let go and the list moved to the foot; nor one a thread's landing has just put on its first unread
 * (`#221`), which a short thread leaves within the slack of its foot on purpose, and which the card
 * would scroll up under the floating line — "something updating underneath". They are told a draft
 * waits below instead. On the call's view, the transcript's own rule (`followIfPinned`). Under the
 * thread list (`#281 view-keys`), the channel's, laid out behind the cards: the docked card takes its
 * height from the foot of that list too, and Ctrl+1 brings it back.
 */
function followsCard() {
  if (deps.currentScreen !== "main") return false;
  return deps.currentView === "discord" ? deps.liveFollowers().main : deps.atBottom(deps.leftEl("scroll-area"));
}


/**
 * Whether a spoken yes to `proposal` is settling. `#269 approval-card-inline`.
 *
 * @param {PendingPost | null} proposal
 */
function yesSettlingOn(proposal: PendingPost | null) {
  const settling = spokenYes.settling;
  return Boolean(settling && proposal && settling.handle === proposal.handle);
}


/**
 * Hold Don't send, or let it go, for what the card shows now. `#269 approval-card-inline`.
 *
 * Held while a request is on its way, and during the hold after the card appears or changes — but
 * NEVER while a spoken yes to this card settles. Then the card's own line says "tap Don't send to
 * stop", and Don't send is the one thing that stops the message going. A yes can settle inside the
 * hold: the card reached the page after the read-back, on a slow or reconnecting long poll, and the
 * owner answered at once. A Don't send held then turned a tap meant to stop the send into nothing,
 * and the message posted.
 */
function renderPostCancel() {
  const proposal = postWatch.shown;
  if (!proposal) return;
  deps.el("post-confirm-cancel").disabled = postWatch.busy || (postWatch.hold !== null && !yesSettlingOn(proposal));
}


/**
 * Where the draft goes, in words: "To lead team · replying to Alex: “is the build green yet…” · in
 * thread “release”". `#269 approval-card-inline`. Never an id: who wrote the message it answers and
 * its first words come from the server, which says nothing when it does not hold that message, and
 * then this says "as a reply to an earlier message".
 *
 * @param {PendingPost} proposal
 */
function postCardWhere(proposal: PendingPost) {
  const parts = [`To ${proposal.channel_name}`];
  if (proposal.reply_to) {
    const author = proposal.reply_to_author;
    const excerpt = proposal.reply_to_excerpt;
    if (author && excerpt) parts.push(`replying to ${author}: “${excerpt}”`);
    else if (author) parts.push(`replying to ${author}`);
    else parts.push("as a reply to an earlier message");
  }
  if (proposal.thread_name) parts.push(`in thread “${proposal.thread_name}”`);
  return parts.join(" · ");
}


/**
 * Put the card where the owner is looking. `#269 approval-card-inline`.
 *
 * The card is the tentative next message: on the main screen it is the last thing in the list on
 * screen, just above that list's message box — under the call's transcript, or under the channel's
 * messages — and on Reply, Settings and Help it is docked at the foot of the screen, so a draft never
 * waits somewhere the owner is not. ONE element moved between those places, so every id, every
 * listener and the hold stay with it. Nothing on the sign-in screen: no call can be open.
 * `data-draft` on the main screen is what lets the stylesheet make room for it there: the message
 * box shrinks to one line under it.
 *
 * `#281 view-keys`. The thread list docks it too, as the Threads screen the list replaced did. The
 * list is the right column's cards across the window, and it has no message box: the channel's, and
 * the channel's messages the card would end, are the left column, laid out behind the list and out of
 * sight, where a card would wait unseen. So it stands under the cards, just above the control bar, and
 * goes back to the foot of the left column's list as Ctrl+1, Ctrl+2 or Ctrl+4 brings that column back
 * (`leaveThreadList`). `data-screen` is "main" there, which tells the stylesheet that the control pane
 * under it carries the bottom inset.
 *
 * `#280 scroll-stability`. The card MOVED into that dock or out of it, on the main screen, is not the
 * dock growing or shrinking under Main's list, and that list's keeper is told so (`cardMovedUnderLists`):
 * the card takes its height from under the list and gives up its place at the list's foot in the same
 * move, which leaves a reader up the list where they were. Counted as growth, in Chromium Cmd+3 pulled
 * a reader as far as 380px up Main to its foot behind the cards, and Cmd+1 found them there. A card
 * arriving docked is the dock growing, as before.
 *
 * ...but not the dock growing alone. The card COMING UP docked there gives the main screen `data-draft`
 * as well, which shrinks the message box at the foot of Main's list to one line in the same layout: in
 * Chromium the channel's box went from 192px to 90px as the card took 246px from under the list. The
 * keeper reckons a reader within the slack plus the growth of the foot to have been at it, which holds
 * only when nothing else moved the foot, and so took readers up to 110px up Main for readers at its foot
 * and pulled them down behind the cards. So where Main's reader was BEFORE either change is noted for
 * the keeper, which judges by that (`cardCameUnderLists`).
 */
function placePostCard() {
  const card = deps.leftEl("post-confirm");
  const shown = postWatch.shown !== null;
  const { parent, before, place } = postCardSpot(deps.currentScreen);
  const from = card.getAttribute("data-place");
  const moving = shown && !card.hidden && deps.currentScreen === "main" && from !== place &&
    (from === "dock" ? card.getAttribute("data-screen") === "main" : place === "dock");
  const under = moving ? deps.heightUnderLists() : 0;
  const coming = shown && card.hidden && deps.currentScreen === "main" && place === "dock";
  const main = deps.leftEl("scroll-area");
  const gap = coming ? main.scrollHeight - main.scrollTop - (main.clientHeight || 0) : 0;
  if (parent && (card.parentNode !== parent || card.nextSibling !== before)) {
    parent.insertBefore(card, before);
  }
  if (card.getAttribute("data-place") !== place) card.setAttribute("data-place", place);
  // Which screen it is docked under, so the stylesheet can tell Reply's own Send from the card's.
  if (place === "dock") card.setAttribute("data-screen", deps.currentScreen);
  else card.removeAttribute("data-screen");
  markCardBesideReply();
  card.hidden = !shown || place === "none";
  if (moving) deps.cardMovedUnderLists(deps.heightUnderLists() - under);
  holdDraftRoom(shown);
  if (coming) deps.cardCameUnderLists(gap);
  // What the chips and the status pill stand on has just changed.
  deps.placeChipsOverComposer();
}


/**
 * Where the card stands on `screen` (`placePostCard`): the element it goes in, the one it goes before, and
 * the name of the place. `#269 approval-card-inline`.
 *
 * @param {string} screen
 * @returns {{parent: HTMLElement | null, before: HTMLElement | null, place: string}}
 */
function postCardSpot(screen: string) {
  if (screen === "main" && !(deps.threadListShown && deps.currentView === "discord")) {
    const channel = deps.currentView === "discord";
    return {
      parent: deps.leftEl(channel ? "pane-discord" : "pane-voice"),
      before: deps.leftEl(channel ? "channel-composer" : "voice-composer"),
      place: channel ? "channel" : "voice",
    };
  }
  if (screen !== "signin") return { parent: deps.leftEl("frame-body"), before: null, place: "dock" };
  return { parent: null, before: null, place: "none" };
}


/**
 * Back to the main screen from another, the card docked under that screen goes back to the foot of the
 * list it ends WHILE THE LIST IS STILL HIDDEN (`showScreen`). `#269 approval-card-inline` x `#280
 * scroll-stability`.
 *
 * A browser keeps a hidden list's scroll position and puts it back the first time the list is laid out
 * again, clamped to the list as it then stands. Shown with the card still docked under it, the list is
 * short of the card, and its window short of the dock's height too: in Chromium on a 393x852 phone, a long
 * draft opened by More stands 907px tall in the channel's list and 398px docked, and a reader at the list's
 * foot, 1344px down it, was put back at 831. The card came back to the foot a moment later and the reader
 * stayed 513px above it, Send about 900px below the screen and the jump offered, whether they had left by
 * the gear or by Settings and Help and came back by Back or Esc, and every reader who saw only the card
 * with them. With the card back in the list first, the list is laid out whole and the reader is where they
 * were. Under the thread list the card stays docked, and nothing is done here.
 */
function postCardBackToMain() {
  const card = deps.leftEl("post-confirm");
  const { parent, before, place } = postCardSpot("main");
  if (!parent || place === "dock" || card.getAttribute("data-place") !== "dock") return;
  parent.insertBefore(card, before);
  card.setAttribute("data-place", place);
  card.removeAttribute("data-screen");
}


/**
 * `#277 reply-in-column`, `#410 inline-reply-editor`. Reply's box, with its own filled "Send reply",
 * opens in a list — the very list the card ends, in the left column, or beside it in the right. Marked
 * while one is open, so the stylesheet outlines the card's Send, and the filled Send nearest the thumb
 * is never taken for the other.
 * Read from the boxes themselves, so it holds however a reply was opened or put away.
 */
function markCardBesideReply() {
  const card = deps.leftEl("post-confirm");
  const beside = deps.currentScreen === "main" && (!deps.leftEl("column-reply").hidden || !deps.leftEl("side-reply").hidden);
  if (card.hasAttribute("data-beside-reply") !== beside) card.toggleAttribute("data-beside-reply", beside);
}


/**
 * How long the main screen keeps the room it made for a card after the card goes. `#269
 * approval-card-inline`. Taken back at once, the message box sprang from one line to three rows
 * and its own Send, and covered the spot where the card's Send had just been: a second tap a
 * moment late landed in the box and opened the phone's keyboard. A second later it is a tap of its
 * own.
 */
const POST_ROOM_KEEP_MS = 1000;


/**
 * Make room for the card on the main screen while one is up, and keep it a moment after.
 *
 * `#280 scroll-stability`. The room given back is the box at the foot of the list growing — in Chromium
 * the channel's from 90px to 192px — and the reader is held through it as through unknown other change the
 * page makes to the list (`holdingReader`): one on the newest line stays on it, and anyone else on the
 * row they are reading. Left to the browser, a reader at the foot of Main or All was left 142px above it
 * with the jump up, and no longer followed the channel, whenever the list was redrawn in the second
 * before: the message the card sent arriving by the live feed, a re-read, the poll. Chromium's own scroll
 * anchoring is what held them otherwise, and with it turned off (`overflow-anchor: none`), as in a browser
 * that has none, they were left there every time the card went.
 */
function holdDraftRoom(shown: boolean) {
  const main = deps.leftEl("screen-main");
  if (postWatch.roomTimer !== null) {
    clearTimeout(postWatch.roomTimer);
    postWatch.roomTimer = null;
  }
  if (shown) {
    main.toggleAttribute("data-draft", true);
  } else if (main.hasAttribute("data-draft")) {
    postWatch.roomTimer = setTimeout(() => {
      postWatch.roomTimer = null;
      if (postWatch.shown === null) deps.holdingReader(() => main.toggleAttribute("data-draft", false));
    }, POST_ROOM_KEEP_MS);
  }
}


/**
 * The card went with the focus inside it. A reader on a touch screen gets it on the list, never on
 * the message box: focusing a text field from a tap opens the phone's keyboard over half the screen.
 * One using the keys or a mouse gets the box of the list on screen, where typing goes on.
 *
 * `#280 scroll-stability`. The box opens to its typing height as it takes the focus, while the room
 * made for the card is still kept (`holdDraftRoom`): in Chromium 42px more at the foot of the list.
 * The reader is held through that as through the room given back. Left to the browser, a reader at the
 * foot with the focus on More was left 42px above it, and then 142px once the room came back, wherever
 * its scroll anchoring could not hold them: turned off, or with the screen showing only the card.
 */
function parkFocusOffCard() {
  // The card froze into a row of the transcript: the focus goes with it. `#270`.
  const frozen = postWatch.parkOn;
  postWatch.parkOn = null;
  if (frozen && frozen.parentNode && deps.currentScreen === "main" && deps.currentView === "voice") {
    frozen.focus({ preventScroll: true });
    return;
  }
  if (deps.currentScreen !== "main") {
    deps.focusList(null);
    return;
  }
  // The thread list has no box, and the left column's is behind it, out of sight: the cards take the
  // keys, as from every other place the list is left by (`#281 view-keys`).
  if (deps.threadListOnScreen()) {
    deps.focusList(deps.sidePane);
    return;
  }
  const target = deps.drawnBy()
    ? deps.leftEl(deps.currentView === "discord" ? "channel-compose-text" : "compose-text")
    : deps.leftEl("scroll-area");
  deps.holdingReader(() => target.focus({ preventScroll: true }));
}


/** Whether the card is up in the list on screen and below what it shows. */
function draftBelowView() {
  if (deps.currentScreen !== "main" || postWatch.shown === null) return false;
  const card = deps.leftEl("post-confirm");
  // Docked under the thread list, it is on screen whatever the list behind shows (`#281 view-keys`).
  if (card.hidden || card.getAttribute("data-place") === "dock") return false;
  const area = deps.leftEl("scroll-area");
  return card.getBoundingClientRect().top >= area.getBoundingClientRect().top + (area.clientHeight || 0);
}


/**
 * What the card's status line says, in each of its states. `#269 approval-card-inline`.
 *
 * ONE LINE, on the narrowest card the page draws — a 360px phone's call pane, where the card stands
 * at the owner's own 15% inset — because the line sits right above the buttons: a sentence that
 * wrapped when a spoken yes began to settle moved Don't send down under the thumb at the one moment
 * it is the only way to stop the send. The stylesheet holds the line to one line's height and cuts
 * anything longer with an ellipsis, so even a larger type size never moves the buttons; these are
 * short enough that nothing is cut on a phone at the page's own size. The whole sentence is read
 * out either way: the card's accessible description includes this line.
 */
const POST_CARD_STATUS = {
  waiting: (seconds: number) => `Waiting for you · lapses in ${seconds} s`,
  changed: (seconds: number) => `Changed: read again · lapses in ${seconds} s`,
  dropped: (seconds: number) => `Call dropped · Send still posts · ${seconds} s`,
  settling: "Heard yes — tap Don't send to stop",
  lapsed: "Lapsed — nothing was posted.",
  token: "Has your sign-in token · tap Don't send",
};


function renderPostExpiry() {
  if (postWatch.tick !== null) {
    clearTimeout(postWatch.tick);
    postWatch.tick = null;
  }
  const proposal = postWatch.shown;
  if (!proposal) return;
  const status = deps.el("post-confirm-expiry");
  // A sign-in token in the text: this page will not send it, so Don't send is the only way on.
  if (deps.redact(proposal.text) !== proposal.text) {
    status.textContent = POST_CARD_STATUS.token;
    return;
  }
  const seconds = Math.max(0, Math.ceil((postWatch.deadline - Date.now()) / 1000));
  if (seconds === 0) {
    deps.el("post-confirm-send").disabled = true;
    status.textContent = POST_CARD_STATUS.lapsed;
    // A card kept through a call that dropped has no watch to take it down: it goes at its lapse,
    // once whatever is drawing it now has finished. `#269 approval-card-inline`.
    if (!postWatch.running && !postWatch.busy) void Promise.resolve().then(() => lapseKeptCard(proposal));
    return;
  }
  // A spoken yes is settling: what the next second does is send, and the one control that stops
  // it is on this card. The countdown does not write over that.
  const settling = spokenYes.settling;
  if (settling && settling.handle === proposal.handle) {
    status.textContent = POST_CARD_STATUS.settling;
  } else if (postWatch.dropped && !postWatch.running) {
    status.textContent = POST_CARD_STATUS.dropped(seconds);
  } else {
    status.textContent = postWatch.changed ? POST_CARD_STATUS.changed(seconds) : POST_CARD_STATUS.waiting(seconds);
  }
  postWatch.tick = setTimeout(() => {
    postWatch.tick = null;
    renderPostExpiry();
  }, POST_EXPIRY_TICK_MS);
}


/** The owner tapped Send. */
async function sendPostProposal() {
  const proposal = postWatch.shown;
  if (!proposal || postWatch.busy || postWatch.hold !== null) return;
  if (deps.redact(proposal.text) !== proposal.text) return;
  // A tap during a spoken yes's settle window wins, and its result is what the agent is told: it
  // heard the yes and is waiting for the page's word on it. `#264 voice-send-approval`.
  const announce = spokenYesYieldsToTap();
  const outcome = await commitShownProposal(proposal, "ui");
  if (announce) reportSpokenOutcome(outcome);
  // Any other tap during a call is told to the agent too: one that asked "Send it?" is waiting for
  // the page's word, and a later "did it go?" must have a true answer.
  else tellAgentAboutTap(outcome);
}


/**
 * Commit `proposal` — the card's server copy, restated exactly — once, and say what happened.
 *
 * The ONE commit, for a tap and for a spoken yes alike (`#264 voice-send-approval`). They share
 * `postWatch.busy`, which is what makes a tap racing a yes commit once; the server's single-use
 * handle is the backstop. Returns the outcome, so a spoken yes can tell the agent the truth.
 *
 * @param {PendingPost} proposal
 * @param {"ui" | "speaker_turn"} confirmedBy
 */
async function commitShownProposal(proposal: PendingPost, confirmedBy: "ui" | "speaker_turn") {
  postWatch.busy = true;
  deps.el("post-confirm-send").disabled = true;
  deps.el("post-confirm-cancel").disabled = true;
  /** @type {{kind: string, proposal: PendingPost, reason?: string}} */
  let outcome: {kind: string, proposal: PendingPost, reason?: string};
  try {
    const answer = await deps.api("/api/v1/post-proposals/commit", {
      method: "POST",
      body: {
        handle: proposal.handle,
        channel_id: proposal.channel_id,
        text: proposal.text,
        reply_to: proposal.reply_to,
        confirmed_by: confirmedBy,
      },
    });
    if (answer && answer.error === "partially_posted") {
      // A failed first part is reported the same way, as a partial send of nothing — unless its
      // answer was lost, when it may be in the channel: that is not confirmed, never "refused".
      // `#270 voice-transcript-markers`.
      // `#444 send-refusal-reason`. A refusal in the reader's words, where the chat service refused it.
      const refused = refusalWords(answer.refusal, deps.chatServiceOf(String(proposal.channel_id)));
      const detail = deps.redact(refused ? refused.replace(/\.$/, "") : answer.detail || "the rest was not sent");
      const nothing = Number(answer.posted) === 0;
      const unconfirmed = nothing && answer.uncertain === true;
      if (!nothing || unconfirmed) drawSentCard(proposal, confirmedBy, unconfirmed ? "unconfirmed" : "partial", answer);
      settlePostCard(proposal);
      deps.showError(unconfirmed
        ? `The post to ${proposal.channel_name} could not be confirmed: ${detail}. Check the channel before sending it again.`
        : nothing
          ? `Nothing was posted to ${proposal.channel_name}: ${detail}`
          : `Only part of the message reached ${proposal.channel_name}: ${detail}`);
      outcome = unconfirmed
        ? { kind: "unconfirmed", proposal }
        : nothing
          ? { kind: "refused", proposal, reason: "the chat service did not accept it" }
          : { kind: "partial", proposal };
    } else if (VibeTalkContract.is("CommittedPostResponse", answer)) {
      // The frozen card first, so a focus the card had goes to it rather than to the list.
      drawSentCard(proposal, confirmedBy, "sent", answer);
      settlePostCard(proposal);
      deps.setStatus(`Posted to ${proposal.channel_name}.`);
      outcome = { kind: "sent", proposal };
    } else {
      settlePostCard(proposal);
      deps.showError("vibe-talk accepted the post but did not say what it posted. Check the channel.");
      outcome = { kind: "unknown", proposal };
      // The send memory knows what it posted: asked once, a moment from now, when no call's watch
      // is reading it already. `#270 voice-transcript-markers`.
      setTimeout(() => void readSendsOnce(), SEND_MEMORY_RECHECK_MS);
    }
  } catch (caught) {
  const error = caught as ApiRequestError;
    // A request that never arrived spent nothing, so the card stays for another try. Anything the
    // server answered spent the proposal, whatever the answer was.
    if (!error.network) settlePostCard(proposal);
    // A commit whose answer was lost may have posted all the same, and the send memory would say
    // so: asked once, a moment from now, when no call's watch is reading it already.
    if (error.network) setTimeout(() => void readSendsOnce(), SEND_MEMORY_RECHECK_MS);
    if (confirmedBy === "ui") {
      deps.showError(POST_REFUSALS[error.code!] || error.message);
    } else {
      deps.showError(`Your spoken yes posted nothing: ${SPOKEN_COMMIT_REFUSALS[error.code!] || error.message}.`);
    }
    outcome = error.network
      ? { kind: "unknown", proposal }
      : {
          kind: "refused",
          proposal,
          reason: SPOKEN_COMMIT_REFUSALS[error.code!] || "the server refused it",
        };
  } finally {
    postWatch.busy = false;
    if (postWatch.shown) showPostProposal(postWatch.shown);
  }
  return outcome;
}


/**
 * The owner tapped Don't send, or (`spoken: true`) said no to the read-back.
 *
 * A tap during a call is told to the agent, which may be waiting on the page's word; a spoken no is
 * not, because the agent heard it. The click passes its event here, which has no `spoken`.
 */
async function cancelPostProposal(options?: {spoken?: boolean} | Event) {
  const spoken = Boolean(options && (options as {spoken?: boolean}).spoken === true);
  const proposal = postWatch.shown;
  if (!proposal || postWatch.busy) return;
  // A tap during the hold is one aimed at what was under that spot before the card was. A spoken
  // no is not a tap and is never held, and nor is a tap that stops a spoken yes settling on this
  // card: the card asked for that one (`renderPostCancel`). `#269 approval-card-inline`.
  if (!spoken && postWatch.hold !== null && !yesSettlingOn(proposal)) return;
  const stoppedYes = spokenYesYieldsToTap();
  postWatch.busy = true;
  try {
    await deps.api("/api/v1/post-proposals/cancel", { method: "POST", body: { handle: proposal.handle } });
    // `#270 voice-transcript-markers`. Withdrawn, on the server's word: the draft's "Not sent" line,
    // kept, where the card was — and where the focus goes if it was on the card. A spoken no draws
    // its own (`reportSpokenOutcome`), after the "no" it answers.
    if (!spoken) postWatch.parkOn = noteNotSent("withdrawn", proposal);
    settlePostCard(proposal);
    deps.setStatus("Not sent.");
    if (!spoken) tellAgentAboutTap({ kind: "withdrawn", proposal });
  } catch (caught) {
  const error = caught as ApiRequestError;
    // Already settled is still "not sent from this card" — the watch shows whatever came next.
    // What that was is not this tap's to say, so it leaves no mark: a send arrives from the send
    // memory, a lapse from the proposal's fate. `#270 voice-transcript-markers`.
    if (!error.network) settlePostCard(proposal);
    if (error.network) deps.showError(error.message);
    // The yes this tap stopped still gets its one answer: it was not sent.
    if (stoppedYes) {
      reportSpokenOutcome({ kind: "not_armed", proposal, reason: SPOKEN_YES_REASONS.withdrawn });
    }
    return false;
  } finally {
    postWatch.busy = false;
    if (postWatch.shown) showPostProposal(postWatch.shown);
  }
  return true;
}


/** Take a settled proposal off the card, unless the watch has already put a newer one there. */
function settlePostCard(proposal: PendingPost) {
  if (postWatch.shown && postWatch.shown.serial === proposal.serial) showPostProposal(null);
  postWatch.parkOn = null;
}


// --- frozen send cards ------------------------------------------------------------------------------
//
// `#270 voice-transcript-markers`. Every message the voice agent posted — tapped, said yes to, or
// sent without asking — stays in the transcript as a frozen card: the live card's own tile at the
// same inset, now saying what went where and how it was approved. Drawn at once from the commit's
// answer when this page made the commit, and from the server's send memory otherwise — a lost
// answer, a send that settled after hang-up, a direct send — once per serial. Filed in the stored
// transcript from the server's own copy (`{speaker: "sent", send}`), so a reload draws the same card.

/** How long after a commit whose answer was lost the send memory is asked about it. */
const SEND_MEMORY_RECHECK_MS = 3000;


/** The sends, by `send_seq`, this page has asked the server to file. */
const sentClaimed: Set<number> = new Set();


/**
 * {@link freezeSentCard}, from inside a commit the server has already answered as posted.
 *
 * A bug in drawing the card must never reach the commit's `catch`, which would tell the owner — and,
 * for a spoken yes, the agent — that nothing was sent when something was. So it is thrown again on
 * its own, where it still shows as an error of the page, and the commit's outcome stands. The send
 * memory still holds the send, so a later read that reports it can still draw its card.
 *
 * @param {PendingPost} proposal
 * @param {"ui" | "speaker_turn"} confirmedBy
 * @param {"sent" | "partial" | "unconfirmed"} outcome
 * @param {unknown} answer
 */
function drawSentCard(proposal: PendingPost, confirmedBy: "ui" | "speaker_turn", outcome: "sent" | "partial" | "unconfirmed", answer: TranscriptAnswer): void  {
  try {
    freezeSentCard(proposal, confirmedBy, outcome, answer);
  } catch (caught) {
  const error = caught as ApiRequestError;
    setTimeout(() => {
      throw error;
    }, 0);
  }
}


/**
 * A frozen card for a send this page committed, from the commit's answer, before the server's own
 * copy is filed: that copy corrects it when the claim answers.
 *
 * @param {PendingPost} proposal
 * @param {"ui" | "speaker_turn"} confirmedBy
 * @param {"sent" | "partial" | "unconfirmed"} outcome
 * @param {unknown} answer the commit's answer, a 200 or a 207
 */
function freezeSentCard(proposal: PendingPost, confirmedBy: "ui" | "speaker_turn", outcome: "sent" | "partial" | "unconfirmed", answer: TranscriptAnswer): void  {
  const key = postKey(proposal.instance, proposal.serial);
  const sendSeq = typeof answer.send_seq === "number" ? answer.send_seq : null;
  // Drawn already when it is this very send: the send memory's read can beat this answer, because
  // the server wakes the long poll the moment the send settles, before this answer is on its way.
  // Under one key that is the same send whoever drew it — unless this page froze another proposal
  // under it, from a server that restarted without naming its run.
  const drawn = postWatch.sentDrawn.get(key);
  if (drawn && (drawn.handle === null || drawn.handle === proposal.handle ||
    (sendSeq !== null && drawn.sendSeq === sendSeq))) return;
  const shown = shownConversation(key);
  const conversationId = shown !== undefined ? shown : deps.session.conversationId;
  const first = outcome === "sent" && answer.posted && typeof answer.posted === "object" ? answer.posted : null;
  const parts = Array.isArray(answer.parts) ? answer.parts.length : 0;
  const posted = outcome === "sent" ? parts : Number(answer.posted) || 0;
  const thread = first && first.thread && !first.thread.is_root ? String(first.thread.id) : undefined;
  const now = Date.now();
  // A partial send shows what went, not the whole draft: the 207 carries what did not, which the
  // draft ends with. Counted in characters, as the server counts them, not UTF-16 units. The
  // server's own copy replaces this when the claim is answered.
  const unsent = outcome === "sent" ? "" : String(answer.unsent || "");
  const went = outcome === "partial" && unsent && proposal.text.endsWith(unsent)
    ? proposal.text.slice(0, proposal.text.length - unsent.length)
    : proposal.text;
  const turn: StoredTurn = {
    speaker: "sent",
    text: went,
    at_ms: now,
    detail: {
      kind: "sent",
      serial: proposal.serial,
      send_seq: sendSeq === null ? 0 : sendSeq,
      channel_id: String(proposal.channel_id),
      channel_name: proposal.channel_name,
      reply_to: proposal.reply_to || undefined,
      reply_to_author: proposal.reply_to_author || undefined,
      reply_to_excerpt: proposal.reply_to_excerpt || undefined,
      thread_id: thread,
      thread_name: proposal.thread_name || undefined,
      message_id: first ? String(first.id) : undefined,
      parts_posted: posted,
      parts_total: outcome === "sent" ? parts : 0,
      uncertain_part: answer.uncertain === true ? posted + 1 : undefined,
      unsent_chars: [...unsent].length,
      outcome,
      how: confirmedBy === "ui" ? "tap" : "voice",
      origin: "commit",
      text_truncated: false,
      text_chars: [...went].length,
      begun_at_ms: now,
      sent_at_ms: now,
    },
  };
  const li = placeSentCard(turn, conversationId || null);
  postWatch.sentDrawn.set(key, { li, sendSeq, handle: proposal.handle });
  postWatch.parkOn = li;
  if (sendSeq !== null) recordSent(sendSeq, conversationId || null, key);
  else if (transcriptRecordsSupported) markNotKept(li);
}


/**
 * The frozen card for one `sent` row — the stored one, the send memory's, or the page's own until
 * the server's corrects it. ONE builder, so a card drawn live and one restored look the same.
 *
 * @param {{text: string, at_ms?: number, detail?: unknown}} turn
 */
function sentCardNode(turn: StoredTurn) {
  const detail = turn.detail && typeof turn.detail === "object" ? turn.detail : {};
  const text = String(turn.text === null || turn.text === undefined ? "" : turn.text);
  const channel = String(detail.channel_name || "the channel");
  const li = document.createElement("li") as VoiceRow;
  li.className = "mine sent-card";
  // Where the focus goes when the live card it froze from goes with the focus in it.
  li.setAttribute("tabindex", "-1");
  if (detail.serial !== undefined) li.setAttribute("data-serial", String(detail.serial));
  li.setAttribute("data-outcome", String(detail.outcome || "sent"));
  const meta = document.createElement("div");
  meta.className = "meta sent-meta";
  const mark = document.createElement("span");
  mark.className = "sent-mark";
  mark.setAttribute("aria-hidden", "true");
  // A tick only for what certainly went whole: a part sent, or a send nobody can confirm, is not
  // ticked beside words that say so, and differs by more than its colour.
  mark.textContent = SENT_MARKS[String(detail.outcome)] || SENT_MARKS.sent!;
  const status = document.createElement("span");
  status.className = "who sent-status";
  status.textContent = detail.outcome === "partial"
    ? `Partly sent to ${channel}`
    : detail.outcome === "unconfirmed"
      ? `Not confirmed: ${channel}`
      : `Sent to ${channel}`;
  const sentAt = typeof detail.sent_at_ms === "number" ? detail.sent_at_ms : turn.at_ms;
  const when = sentAt === undefined ? new Date() : new Date(sentAt);
  const at = timeNode("at", Number.isNaN(when.getTime()) ? null : when, stamp(when.getTime()));
  const how = document.createElement("span");
  how.className = "sent-how";
  how.textContent = detail.how === "voice"
    ? "approved by voice"
    : detail.how === "direct"
      ? "sent without asking"
      : "approved by tap";
  meta.append(mark, status, at, how);
  const body = document.createElement("div");
  body.className = "body";
  // What went, as text, a sign-in token never put on screen.
  body.textContent = deps.redact(text);
  li.append(meta, body);
  const where = sentWhere(detail);
  if (where) {
    const line = document.createElement("p");
    line.className = "sent-where";
    line.textContent = where;
    li.append(line);
  }
  for (const said of sentNotes(detail)) {
    const note = document.createElement("p");
    note.className = "sent-note";
    note.textContent = said;
    li.append(note);
  }
  if (detail.message_id) {
    const open = document.createElement("button");
    open.className = "chip sent-open";
    open.setAttribute("type", "button");
    open.textContent = "Open in channel";
    open.addEventListener("click", () => void openSentMessage(detail));
    li.append(open);
  }
  // A row of the transcript like unknown turn, opened by a tap on it as a turn is (`turnNode`).
  const fold = deps.foldable(li, meta, body, text);
  if (fold) deps.tapRow(li, fold, deps.foldOnTap);
  deps.searchable(li, "sent", channel, text);
  li.linkText = text;
  return li;
}


/** The glyph at the head of a frozen card, by outcome. */
const SENT_MARKS: Record<string, string> = { sent: "✓", partial: "!", unconfirmed: "?" };


/** The card's own line saying the server did not file it. */
function markNotKept(li: HTMLElement) {
  if ([...li.children].some((kid) => kid.className === "sent-note sent-not-kept")) return;
  const note = document.createElement("p");
  note.className = "sent-note sent-not-kept";
  note.textContent = "Not kept in the stored transcript.";
  deps.preservingScroll(() => li.append(note));
}


/**
 * Put a frozen card in the transcript, in the conversation it belongs to (`placeInConversation`).
 * A late commit answer, a send that settled after the next call began, or a tap on a card kept
 * through a call that dropped lands where a reload draws it.
 *
 * @param {{text: string, at_ms?: number, detail?: unknown}} turn
 * @param {string | null} conversationId
 */
function placeSentCard(turn: StoredTurn, conversationId: string | null) {
  return placeInConversation(sentCardNode(turn), conversationId, 1);
}


/**
 * Put a row the page drew — a frozen card, a draft's "Not sent" line — at the end of the transcript
 * for the conversation on screen, or, for one that has ended, in it: just above the boundary that
 * ended it, where a reload draws it too. A reload can, because the server stamps a record filed in a
 * conversation another has been written to since with that conversation's last time, not the time
 * it arrived (`store::filed_at`): the walk reads it back at the end of its own conversation, and the
 * listing — whose head the next call resumes from — keeps its order. `turns` is 1 for a message and
 * 0 for a line about one. `#270 voice-transcript-markers`.
 *
 * @param {HTMLElement} li
 * @param {string | null} conversationId
 * @param {number} [turns]
 */
function placeInConversation(li: HTMLElement, conversationId: string | null, turns: number = 0) {
  const list = deps.leftEl("transcript");
  const area = deps.leftEl("scroll-area");
  const pinned = pinnedHere(area);
  const boundary = conversationId
    ? [...list.children].reverse().find((row) => row.getAttribute("data-ends") === conversationId)
    : undefined;
  if (boundary) deps.preservingScroll(() => list.insertBefore(li, boundary));
  else list.append(li);
  deps.renderEmptyState();
  deps.applySearch();
  deps.followIfPinned(pinned, turns);
  noteFollow(area, pinned);
  return li;
}


/** The server filed the card's send: draw its own copy where the page's was. */
function correctSentCard(key: string, turn: StoredTurn) {
  const drawn = postWatch.sentDrawn.get(key);
  const li = drawn && drawn.li;
  if (!drawn || !li || !li.parentNode) return;
  const fresh = sentCardNode(turn);
  const focused = document.activeElement === li;
  // Opened by a tap, it stays open (`#431 voice-transcript-expand`), its fold button's focus with it.
  const onFold = deps.keepTurnFold(li, fresh);
  deps.preservingScroll(() => li.replaceWith(fresh));
  drawn.li = fresh;
  if (focused) fresh.focus({ preventScroll: true });
  if (onFold) /** @type {unknown} */ (fresh).foldEntry!.fold.focus({ preventScroll: true });
  deps.applySearch();
}


/**
 * Open the message a frozen card sent (`#580 open-in-chat-jump`). The owner: "Of course pressing the
 * button should jump to the right view and to the message." It used to work only when the channel
 * picker already showed the card's channel, and otherwise said to choose it there. Now it is a
 * link's jump (`jumpToSentMessage`): the channel's view, the card's channel, All unless Settings says
 * Main, and the message centred and lit; one Back returns to where it was pressed.
 */
async function openSentMessage(detail: RecordDetail) {
  const channel = String(detail.channel_id);
  if (!deps.knownChannels.some((known) => String(known.id) === channel)) {
    deps.setStatus(`That message is in ${detail.channel_name}, which this app no longer reads.`);
    return;
  }
  /** @type {MessageLinkRef} */
  const ref: MessageLinkRef = {
    service: String(deps.chatProviderName || ""),
    channel,
    message: String(detail.message_id),
    thread: detail.thread_id ? String(detail.thread_id) : null,
  };
  await deps.guardQuietly(deps.jumpToSentMessage)(ref);
}


/**
 * File the send `sendSeq` in `conversationId`'s stored transcript, from the server's copy, in the
 * order the screen shows it: after every turn drawn above the card. Once per send; `key` is the
 * card's (`postKey`).
 */
function recordSent(sendSeq: number, conversationId: string | null, key: string) {
  if (!transcriptRecordsSupported || sentClaimed.has(sendSeq)) return;
  sentClaimed.add(sendSeq);
  recordInScreenOrder(() => {
    void recordRecord(conversationId, { speaker: "sent", text: "", send: sendSeq }).then((result) => {
      const drawn = postWatch.sentDrawn.get(key);
      if (!drawn) return;
      if (result && result.turn) correctSentCard(key, result.turn);
      else if (!result || result.refused !== "send_recorded") markNotKept(drawn.li);
    });
  });
}


/** Steps waiting for the live turns drawn above them to be stored first. */
let recordsAfterLive: Array<() => void> = [];


/** Whether a live turn of the vibe-talk-v1 protocol is on screen and not yet stored. */
function liveTurnsUnstored() {
  for (const entry of deps.session.liveTurns.values()) {
    if (!entry.stored && !entry.echo) return true;
  }
  return false;
}


/**
 * Run a record step now, or once every live turn drawn above it is stored, so the record holds
 * [read-back][yes][One moment.][card] in the order the screen does.
 */
function recordInScreenOrder(step: () => void) {
  if (liveTurnsUnstored()) recordsAfterLive.push(step);
  else step();
}


/** The live turns are stored: the record steps that waited for them go now, in order. */
function flushRecordsAfterLive() {
  if (recordsAfterLive.length === 0 || liveTurnsUnstored()) return;
  const steps = recordsAfterLive;
  recordsAfterLive = [];
  for (const step of steps) step();
}


/** One POST of one record. */
function postOnce(conversationId: string, record: Record<string, unknown>) {
  return deps.api(`/api/v1/conversations/${conversationId}/turns`, { method: "POST", body: record });
}

/** Whether a failed POST may succeed later: no answer at all, or a server-side failure. */
function transientFailure(error: ApiRequestError): boolean {
  const status = Number(error.status) || 0;
  return status === 0 || status >= 500;
}

/**
 * Post one record, retrying a transient failure at once. A final refusal (any other 4xx)
 * and a transient failure that outlasts the attempts both throw, for the caller to tell
 * apart by status.
 */
async function postWithRetry(conversationId: string, record: Record<string, unknown>) {
  let lastError: unknown = null;
  for (let attempt = 0; attempt < RECORD_ATTEMPTS; attempt += 1) {
    try {
      const answer = await postOnce(conversationId, record);
      if (recordingBroken) {
        recordingBroken = false;
        setStorageState("Recording again: the store answered, and what it missed is being filed.");
      }
      if (answer && answer.turn) noteStored(conversationId, answer.turn);
      return answer;
    } catch (caught) {
      lastError = caught;
      if (!transientFailure(caught as ApiRequestError)) throw caught;
    }
  }
  throw lastError;
}

/** Say, once per outage, that recording is behind — and that it has not given up. */
function noteRecordingBehind(error: ApiRequestError) {
  if (!recordingBroken) {
    setStorageState(
      `Not recording yet: ${error.message}. The lines are kept and recording keeps trying; ` +
        `what is already stored is untouched.`
    );
  }
  recordingBroken = true;
}

/** File what an outage left behind, oldest first; what still fails stays for the next flush. */
async function flushUnrecorded(): Promise<void> {
  if (unrecorded.length === 0) return;
  const held = unrecorded;
  unrecorded = [];
  for (const entry of held) {
    try {
      const answer = await postOnce(entry.conversationId, entry.record);
      if (answer && answer.turn) noteStored(entry.conversationId, answer.turn);
    } catch (caught) {
      if (transientFailure(caught as ApiRequestError)) unrecorded.push(entry);
    }
  }
  if (unrecorded.length === 0 && recordingBroken) recordingBroken = false;
}

/** Keep one record a transient failure could not file, bounded so a dead store cannot grow it for ever. */
function keepUnrecorded(conversationId: string, record: Record<string, unknown>) {
  unrecorded.push({ conversationId, record });
  if (unrecorded.length > 500) unrecorded.splice(0, unrecorded.length - 500);
}


/**
 * Post one transcript record — a send, a marker — in the queue the turns go through. Resolves to
 * `{turn}` as stored, `{refused: code}` for a refusal that is final, or null when nothing was kept.
 *
 * A refusal is NEVER the end of recording, as a turn's failure is: 404 and 409 say the record is
 * not the server's to file or is filed already, and a 400 or 413 is said in Settings. Only a
 * request that never arrived, or a 5xx, says the store is gone.
 *
 * @param {string | null} conversationId
 * @param {Record<string, unknown>} body
 */
function recordRecord(conversationId: string | null, body: Record<string, unknown>) {
  if (!transcriptRecordsSupported || !conversationId) return Promise.resolve(null);
  // A conversation's first record is its start marker, a send or a draft not sent included.
  fileStartMarker(conversationId);
  return postRecord(conversationId, body);
}


/**
 * {@link recordRecord}'s request, in the queue the turns go through.
 *
 * @param {string} conversationId
 * @param {Record<string, unknown> | (() => Record<string, unknown>)} body the record, or what writes
 *   it as the request goes: a start marker held for an answer says what the answer was
 */
function postRecord(conversationId: string, body: Record<string, unknown> | (() => Record<string, unknown>)) {
  const done = recordQueue.then(async () => {
    await flushUnrecorded();
    const record = typeof body === "function" ? body() : body;
    try {
      const answer = await postWithRetry(conversationId, record);
      return answer && answer.turn ? { turn: answer.turn } : null;
    } catch (caught) {
  const error = caught as ApiRequestError;
      const status = Number(error.status) || 0;
      if (status === 404 || status === 409) return { refused: String(error.code || status) };
      if (status >= 400 && status < 500) {
        setStorageState(`A record of this call was not kept: ${error.message}.`);
        return { refused: String(error.code || status) };
      }
      // A transient failure keeps the record and says so once; the next record flushes it.
      // `#1303 voice-transcript-share`: never a permanent not-recording state.
      keepUnrecorded(conversationId, record);
      noteRecordingBehind(error);
      return null;
    }
  });
  recordQueue = done.then(() => undefined);
  return done;
}


// --- drafts that were not sent -----------------------------------------------------------------------
//
// `#270 voice-transcript-markers`. A draft that went nowhere leaves a line where it was waiting, so a
// read-back followed by nothing does not read, months later, as a question nobody answered. One
// label, and one fixed sentence per reason, the same live and restored. On a server that takes
// records the line is kept, as a `not_sent` marker in the draft's conversation.
//
// Drawn only on the server's word: Don't send once the server withdrew the draft, a spoken no once it
// did, a lapse once the server says the draft EXPIRED, and a Hang up with the draft still waiting.
// Nothing for a draft a newer one replaced (the newer one is on the card), one that was committed (the
// send memory draws what went), or one somebody else withdrew.

// What a "Not sent" line says, per reason (`NOT_SENT_SEAMS`; a spoken no's is the spoken yes's
// `declined`), is web/src/core/call's (`notSentDetail`). `ended` stays true whatever comes after it:
// Hang up only puts the draft away on this page, and the server keeps it until it lapses.

/**
 * How long after a kept card's lapse the server is asked what became of it, so its "Not sent" line
 * says lapsed only on the server's word. `#269 approval-card-inline`.
 */
const POST_FATE_RECHECK_MS = 1000;


/**
 * A draft's "Not sent" line, for `why`: into `into` (a restored run, or a throwaway for one placed by
 * the caller). Focusable, as a place for the focus to go when the card goes with it inside.
 *
 * @param {string} why
 * @param {HTMLElement[]} into
 */
function notSentSeam(why: string, into: HTMLElement[]) {
  const detail = notSentDetail(why);
  const li = transcriptSeam("Not sent", detail, into);
  li.setAttribute("data-not-sent", String(why));
  li.setAttribute("tabindex", "-1");
  return li;
}


/**
 * `proposal` was not sent, for `why`: draw its line in the conversation it was shown in (or
 * `conversationId`), keep it as a marker, and return the line — or null when this draft already has
 * one.
 *
 * @param {"withdrawn" | "declined" | "lapsed" | "ended"} why
 * @param {PendingPost} proposal
 * @param {string | null} [conversationId]
 */
function noteNotSent(why: "withdrawn" | "declined" | "lapsed" | "ended", proposal: PendingPost, conversationId?: string | null): HTMLElement | null  {
  const key = postKey(proposal.instance, proposal.serial);
  if (postWatch.notSent.has(key)) return null;
  postWatch.notSent.add(key);
  if (postWatch.notSent.size > 64) postWatch.notSent.delete(postWatch.notSent.values().next().value!);
  let where = conversationId;
  if (where === undefined) {
    const shown = shownConversation(key);
    where = shown !== undefined && shown !== null ? shown : deps.session.conversationId;
  }
  const li = placeInConversation(notSentSeam(why, []), where || null);
  if (why === "lapsed") {
    deps.setStatus(`The draft to ${proposal.channel_name} lapsed; nothing was posted.`);
    tellAgentOfLapse(proposal);
  }
  if (transcriptRecordsSupported && where) {
    const marker = { kind: "not_sent", why, serial: proposal.serial };
    recordInScreenOrder(() => void recordRecord(where, { speaker: "marker", text: "", marker }));
  }
  return li;
}


/** A read is taking the card down, or replacing it: a draft the server let lapse says so. */
function noteCardTakenDown(answer: PendingPostResponse, next: PendingPost | null): void  {
  const shown = postWatch.shown;
  if (!shown || (next && next.handle === shown.handle) || postWatch.busy) return;
  if (proposalFate(answer, shown) === "expired") noteNotSent("lapsed", shown);
}


/**
 * A card kept through a call that dropped reached its lapse with no watch to take it down: it goes
 * now, and a moment later the server is asked what became of it.
 *
 * @param {PendingPost} proposal
 */
function lapseKeptCard(proposal: PendingPost) {
  if (postWatch.shown !== proposal || postWatch.running || postWatch.busy) return;
  showPostProposal(null);
  setTimeout(() => void readLapsedFate(proposal), POST_FATE_RECHECK_MS);
}


/** @param {PendingPost} proposal */
async function readLapsedFate(proposal: PendingPost) {
  let answer;
  try {
    answer = await deps.apiDecoded("PendingPostResponse", postWatchPath(0));
  } catch (_error) {
    return; // Nothing more to say: the card has gone, and a lapse is not drawn on a guess.
  }
  noteSendMemory(answer);
  if (proposalFate(answer, proposal) === "expired") noteNotSent("lapsed", proposal);
}


/**
 * The call has ended, for `cause`, with `proposal` on the card. `#269 approval-card-inline`.
 *
 * A Hang up is a decision about the call, and retires the draft with it: the card goes, the next
 * call does not offer it, and its "Not sent" line says so — unless a send or a withdrawal is on its
 * way, whose answer settles the card. Every other ending — the service closed the call, the
 * connection failed, the page was put away — is one the owner did not choose, and keeps the card,
 * tap only, until it lapses: on 2026-10-13 a drop came nineteen seconds after a tap on Send, and
 * discarding the draft at a drop loses messages he meant to send.
 *
 * @param {string} cause
 */
function settleCardAtCallEnd(cause: string) {
  const proposal = postWatch.shown;
  if (!proposal) return;
  if (cause !== "ended") {
    postWatch.dropped = true;
    renderPostExpiry();
    return;
  }
  if (postWatch.busy) return;
  postWatch.ignored = proposal.handle;
  postWatch.parkOn = noteNotSent("ended", proposal, deps.session.conversationId);
  showPostProposal(null);
  postWatch.parkOn = null;
}


// --- a spoken yes to the card ----------------------------------------------------------------------
//
// `#264 voice-send-approval`. The owner drives with this page. With "Ask me before the voice agent
// sends" on, the agent reads its draft back and asks "Send it?", and a hands-free yes has to be
// possible, the way a phone assistant takes one. But the yes is judged HERE, by fixed rules over what
// this page observed — never by the model. A model that could decide the owner had agreed is a model
// that can be talked into deciding it, which is the whole thing the card exists to prevent. It works
// the same for every voice provider because it uses only what both wire protocols already carry.
//
// What counts, all of it checked when the answer's final transcript arrives (`spokenYesRefusal`):
//
//   * THE CARD. The draft is the one this call's agent proposed — not one already waiting when the
//     call opened, nor one from before a replacement session — it has not lapsed, it holds no
//     sign-in token, and it is made of nothing a voice can drop or misread (`unsayableDraft`): only
//     letters, digits, spaces, apostrophes, sentence punctuation, a hyphen inside a word and a
//     spaced dash. The read-back is held to the card by its WORDS, and a voice says only words, so
//     a mention (the model says a name there), a link, a symbol such as + or #, an emoji, or
//     markup — ~~struck~~, ||hidden||, > quoted, *stressed*, `code` — could change what is posted
//     while every word still matched: "~~don't~~ merge the PR" is heard as "don't merge the PR".
//     The owner's tag the server put in front of the agent's words (`#303 voice-post-prefix`) is
//     read back with them, but is not judged so: it is his, not the model's (`agentDraft`).
//   * A REAL SPOKEN CALL with echo cancellation on. A typed call has no spoken answer, and without
//     echo cancellation the agent's own "Send it?" can come back as the answer.
//   * AN UNINTERRUPTED READ-BACK. Nothing cut the assistant off since the user last spoke, and on
//     the neutral protocol its turn completed, not interrupted, before the answer.
//   * THE READ-BACK IS EXACTLY THE TEMPLATE. Everything the assistant said since the last turn of
//     anyone's — the user's, spoken or typed, or a text turn of this page's: a relayed message, a
//     notice — or since this draft reached the card, is word for word "To {channel}: {text}. Send
//     it?" (`isExactReadBack`): at most one lead-in from a closed list, nothing inserted, nothing
//     between the text and the question, nothing after it. So the owner heard exactly what would be
//     posted and where, and nothing else since: a read-back that adds a "not", a chat message read
//     aloud, or an answer to some other question cannot stand in for it. Any turn ends it; only a
//     fresh read-back, or a new draft read back, arms a yes again.
//   * HEARD TO THE END. The answer arrived after the read-back's audio finished playing. On the
//     neutral protocol that is when the answer began (it sends partials); on the hosted protocol it
//     is when its transcript arrived, which is later.
//   * NOT AN ECHO. An answer made only of the read-back's last words, within SPOKEN_YES_ECHO_GUARD_MS
//     of its end — a bare "send it" straight after "...Send it?" — may be the agent's own voice.
//   * THE VERY NEXT ANSWER, AND A YES. The read-back is what the assistant said since the user last
//     spoke, so only the answer right after it can match it; and the answer is a yes from a closed
//     list with at most one word of filler (`classifySpokenAnswer`).
//
// A yes that passes waits SPOKEN_YES_SETTLE_MS, so a "wait" transcribed within it is not a send, and
// then commits EXACTLY the card's server copy with `confirmed_by: "speaker_turn"`, through
// `commitShownProposal` — the tap's function and the tap's busy latch, so a tap and a yes racing
// commit once. A no withdraws the draft. Anything else leaves the card for a tap.
//
// EVERY YES TO SENDING IS ANSWERED, and THE OUTCOME IS SAID THREE WAYS (`reportSpokenOutcome`). The
// agent is told to say only "One moment." on a yes and wait for this page, and it will take far more
// than the closed list for a yes. So while sending is the subject — unknown question the assistant asked
// since the owner last spoke was about sending, a turn of it ended on a read-back's "send it", or the
// answer says send itself (`askedAboutSending`) — every answer an agent could take for a yes gets a
// word back, sent or not: a plain yes, anything with a word like one ("sounds good", "uh-huh", "of
// course"), and anything short that does not start like a no (`mayBeYes`). That holds while this
// call's draft is up, and after it has gone: a yes to a draft that lapsed or was settled is told no
// draft is waiting. A yes to some other question is not waited on, so it is not answered: answering
// every "okay" would make the agent repeat itself to each acknowledgement of what it just said; nor is
// a longer answer with nothing like a yes in it, which is a request of its own. With the agent told to
// post at once (the setting off when the call began) nothing is judged at all. The agent is told
// through the call's own text channel — the `user_message` the composer and the live relay already
// use, which the neutral protocol carries as a queued `prompt` — so its next words are true and it is
// never the one that claims a send. And the page says it itself, on the status line, as a transcript
// seam and as a short tone, because a model's sentence needs a cooperative model and the owner must
// hear the truth anyway. A tap on the card during a call is told to the agent as well.
//
// WHAT IT CANNOT DO: tell the owner's voice from anyone else's, or know how sure the recogniser was.
// A passenger or the radio saying "yeah" right after a read-back counts. The README says so.

/** How long a heard yes waits for more speech before it commits. */
const SPOKEN_YES_SETTLE_MS = 1200;

/** How soon after the read-back's audio ends an answer made of its last words is taken for its echo. */
const SPOKEN_YES_ECHO_GUARD_MS = 1500;

/**
 * How long the page's word on an answer that was not a plain yes waits for a new draft, which it
 * then never says. `#579 voice-send-confirm-timing`: the owner's own "send X on channel Y" says send,
 * so it was judged as an answer to the card, and the agent, drafting X at that moment, was told the
 * last draft had lapsed or was NOT sent — and told him so as the new card came up. A request makes a
 * draft within a few seconds: the agent's reply, its tool call, and this page's watch waking.
 */
const REDRAFT_WAIT_MS = 6500;


// What a spoken answer says to "Send it?" (classifySpokenAnswer, isExactReadBack, unsayableDraft, ...)
// is web/src/present/call/'s (`#624 voice-page-modularity`).

/** Why a spoken yes was not taken, in words the agent can repeat to the owner. */
const SPOKEN_YES_REASONS: Record<RefusedReason, string> = {
  interrupted: "the read-back was interrupted",
  mismatch: "the read-back was not the card's text word for word, with nothing added",
  no_question: "the read-back did not end with the question Send it",
  interposed: "a message came between the read-back and the yes",
  mention: "the draft mentions someone, which a read-back cannot say exactly",
  symbols: "the draft has a link, a symbol or formatting in it, which a read-back cannot say exactly",
  hint: "only a plain answer such as yes or send it counts, with nothing added",
  early: "the yes came before the read-back finished playing",
  echo: "that may have been the assistant's own voice, so answer the next read-back with a plain yes",
  echo_cancellation_off: "a spoken yes needs echo cancellation on in Settings",
  typed_call: "this is a typed call, so the draft needs a tap on Send",
  typed_yes: "a typed yes does not send it, so it needs a tap on Send",
  earlier_draft: "that draft was already waiting when this call started",
  replaced_session: "the voice session restarted after that draft was made",
  expired: "the draft lapsed",
  token: "the draft contains the sign-in token",
  busy: "another send or cancel was already under way",
  heard_more: "more than a yes was heard",
  said_more: "the assistant said more than One moment, and the yes may have answered that instead",
  changed: "the draft changed before it was sent",
  withdrawn: "the draft was withdrawn on the screen",
  typed: "a typed message arrived before it was sent",
  ended: "the call ended",
  gone: "no draft is waiting; it lapsed or was already settled",
};


/** A refused spoken commit, by the server's code, in words the agent can repeat. */
const SPOKEN_COMMIT_REFUSALS: Record<string, string> = {
  proposal_expired: "the draft lapsed first",
  proposal_superseded: "a newer draft replaced it",
  proposal_used: "it had already been settled",
  proposal_mismatch: "the server's copy did not match the card",
  proposal_unknown: "the server no longer holds it",
};


/**
 * One assistant turn, as a spoken yes holds it up against the card.
 *
 * @typedef {{text: string, card: string | null, turn: number | null, at: number,
 *   cardFrom: {handle: string, words: number} | null}} SpokenYesTurn
 */
/**
 * What an answer is judged against: the assistant's turns before it, what it said since the user
 * last spoke, whether a page text turn came between, whether the read-back was cut off or completed,
 * when the answer began, and when the read-back's audio finished playing — and, for a yes that then
 * settles, the answer's turn number and the keys of the assistant turns left out as its reply.
 *
 * @typedef {{turns: SpokenYesTurn[], sinceUser: {text: string}[], interposed: boolean, cut: boolean,
 *   done: boolean, answerAt: number, playEnd: number, turn: number | null,
 *   replies: Set<string>}} SpokenYesHeard
 */

const spokenYes = {
  // Bumped by every reset, so a timer or a commit from before it drops its result.
  epoch: 0,
  // What the assistant has said since the last turn of anyone's — the user's, spoken or typed, or a
  // text turn this page sent (a relayed message, a notice) — per turn, in order, each with the handle
  // that was on the card when the turn began, its number on the neutral protocol, when its first
  // frame arrived, and how many of its words were said before a newer draft reached the card: the
  // read-back, held up against the card.
  /** @type {Map<string, SpokenYesTurn>} */
  said: new Map() as Map<string, SpokenYesTurn>,
  // What the assistant has said since the USER's last turn, page text turns included: what decides
  // whether an answer is one about sending (`askedAboutSending`), since a relayed message read out
  // after "Send it?" does not stop the owner's yes from being an answer to it.
  /** @type {Map<string, {text: string, turn: number | null, at: number}>} */
  sinceUser: new Map() as Map<string, {text: string, turn: number | null, at: number}>,
  // A text turn of this page's went since the user's last turn.
  interposed: false,
  // Something cut the assistant off since the user last spoke: the hosted protocol's interruption,
  // or an interrupted `turn_complete` that carried no turn number.
  cut: false,
  // The neutral protocol said the assistant's turn completed, not interrupted. Used only where the
  // turns carry no numbers; numbered turns are judged by `completed`.
  done: false,
  // `#268 voice-call-drops`. The neutral protocol's numbered turns completed since the user last
  // spoke: whether each was cut short, and when the audio queued by its end finishes playing.
  /** @type {Map<number, {interrupted: boolean, playEnd: number}>} */
  completed: new Map() as Map<number, {interrupted: boolean, playEnd: number}>,
  // `#268 voice-call-drops`. The user's turn the page has begun to hear and not yet judged: its key,
  // its number, its best words so far, and when the answer began — put back to 0 while the
  // assistant is still saying what the answer answers, and set again by the answer's next frame.
  /** @type {{key: string, turn: number | null, text: string, startedAt: number} | null} */
  answer: null as {key: string, turn: number | null, text: string, startedAt: number} | null,
  // The user turn already judged, so more frames of it are not judged again.
  /** @type {string | null} */
  answered: null as string | null,
  // The handle a spoken yes may commit: the card's, when this session's agent proposed it.
  /** @type {string | null} */
  eligible: null as string | null,
  // The handle on the card when this call or session opened, which no spoken yes may send, and why.
  /** @type {string | null} */
  ineligible: null as string | null,
  /** @type {"earlier_draft" | "replaced_session"} */
  ineligibleWhy: "earlier_draft" as "earlier_draft" | "replaced_session",
  // The yes waiting out SPOKEN_YES_SETTLE_MS: the card it would send, its words, the answer's turn
  // number, the assistant turns it was judged without because they were its reply (`isReplyTo`),
  // and the hold keeping its row out of sight (`#269 approval-card-inline`).
  /** @type {{handle: string, words: string, timer: unknown, proposal: PendingPost, turn: number | null, replies: Set<string>, hold?: YesHold | null} | null} */
  settling: null as {handle: string, words: string, timer: Timer | null, proposal: PendingPost, turn: number | null, replies: Set<string>, hold?: YesHold | null} | null,
  // The last draft the card showed during this call, kept after the card goes, so a yes to its
  // read-back that arrives once it has lapsed or been settled is still answered.
  /** @type {PendingPost | null} */
  lastShown: null as PendingPost | null,
  // The page's word on an answer that may have been a new request, held for REDRAFT_WAIT_MS and
  // dropped if the card changes first (`reportUnlessRedrafted`). `#579 voice-send-confirm-timing`.
  /** @type {{outcome: unknown, from: string | null, timer: unknown} | null} */
  redraft: null as {outcome: Outcome, from: string | null, timer: Timer | null} | null,
  // Whether this call's agent was told to ask before it sends, as the server said when the call
  // began: an agent that posts at once asks nothing, so there is no yes to judge.
  askFirst: true,
  turns: 0,
  // `#269 approval-card-inline`. The user turns drawn hidden while they may be a yes to the card,
  // by turn key — kept across resets, because a yes that sent stays drawn into its card.
  /** @type {Map<string, YesHold>} */
  holds: new Map() as Map<string, YesHold>,
};


/**
 * Say what came of an answer of kind `kind`: at once for a plain yes, whose agent said "One moment."
 * and is waiting; for anything else, only if the card is unchanged REDRAFT_WAIT_MS later. `#579
 * voice-send-confirm-timing`.
 *
 * @param {"affirmative" | "negative" | "hint" | "other"} kind
 * @param {{kind: string, proposal?: PendingPost | null, reason?: string}} outcome
 */
function reportAnswerOutcome(kind: "affirmative" | "negative" | "hint" | "other", outcome: {kind: string, proposal?: PendingPost | null, reason?: string}) {
  if (kind === "affirmative") reportSpokenOutcome(outcome);
  else reportUnlessRedrafted(outcome);
}


/**
 * Report `outcome` REDRAFT_WAIT_MS from now, unless the card changes first.
 *
 * An answer that is not a plain yes may be the owner asking for something new ("send hello on lead
 * team" says send), and the agent drafts it while the page judges it. A new draft on the card says
 * so, and the word about the old card is dropped: it would tell the agent, and the owner by status
 * line, seam and tone, that a draft was not sent or had lapsed just as the new one came up. The card
 * changing unknown other way — Send, Don't send, a lapse — has its own word, so this one is dropped too.
 * A plain yes is never held: its agent says only "One moment." and waits for the page.
 *
 * @param {{kind: string, proposal?: PendingPost | null, reason?: string}} outcome
 */
function reportUnlessRedrafted(outcome: {kind: string, proposal?: PendingPost | null, reason?: string}) {
  dropRedraftWait();
  const epoch = spokenYes.epoch;
  const wait = { outcome, from: postWatch.shown ? postWatch.shown.handle : null, timer: /** @type {Timer | null} */ (null as Timer | null) };
  wait.timer = setTimeout(() => {
    if (spokenYes.redraft !== wait) return;
    spokenYes.redraft = null;
    if (spokenYes.epoch !== epoch || !deps.session.connected) return;
    reportSpokenOutcome(outcome);
  }, REDRAFT_WAIT_MS);
  spokenYes.redraft = wait;
}


/** Say the word held by `reportUnlessRedrafted` at once, if one is held. */
function sayRedraftWait() {
  const wait = spokenYes.redraft;
  if (!wait) return;
  dropRedraftWait();
  reportSpokenOutcome(wait.outcome);
}


/**
 * The agent is calling `post_reply` while a word is held: a draft is on its way, so the hold starts
 * again from now rather than running out before the draft reaches the card. Hosted protocol only,
 * where the voice service reports its tool calls (`mcp_tool_call`).
 */
function extendRedraftWait() {
  const wait = spokenYes.redraft;
  if (wait) reportUnlessRedrafted(wait.outcome);
}


/** Forget the word held by `reportUnlessRedrafted`, unsaid. */
function dropRedraftWait() {
  const wait = spokenYes.redraft;
  if (!wait) return;
  clearTimeout(wait.timer!);
  spokenYes.redraft = null;
}


/**
 * Forget the conversation so far: a new call, a hang-up, or a replacement voice session.
 *
 * @param {"earlier_draft" | "replaced_session"} [why] why a draft on the card now may not be sent by voice
 */
function resetSpokenYes(why: "earlier_draft" | "replaced_session" = "earlier_draft") {
  // A row held for a yes that is not on its way is shown again: nothing will send it now.
  releaseYesHolds();
  const settling = spokenYes.settling;
  if (settling) clearTimeout(settling.timer!);
  spokenYes.settling = null;
  // A Don't send let through the card's hold for that yes is held again until the hold is over.
  if (settling) renderPostCancel();
  spokenYes.epoch += 1;
  spokenYes.said = new Map();
  spokenYes.sinceUser = new Map();
  spokenYes.interposed = false;
  spokenYes.cut = false;
  spokenYes.done = false;
  spokenYes.completed = new Map();
  spokenYes.answer = null;
  spokenYes.answered = null;
  spokenYes.eligible = null;
  spokenYes.lastShown = null;
  dropRedraftWait();
  // Whatever is on the card now was proposed to a conversation that is over, or to a session that
  // did not see it: a tap may still send it, a yes may not.
  spokenYes.ineligible = postWatch.shown ? postWatch.shown.handle : null;
  spokenYes.ineligibleWhy = why;
}


/**
 * Did the browser say it opened `stream` WITHOUT the echo cancellation it was asked for? A device or
 * an audio route can ignore the constraint, and then the agent's own "Send it?" can come back as the
 * answer. Only a track that reports `echoCancellation: false` says so; one that reports nothing is
 * taken at the request's word, since some browsers report no settings at all.
 *
 * @param {{getAudioTracks?: () => unknown[], getTracks?: () => unknown[]}} stream
 */
function echoCancellationRefused(stream: {getAudioTracks?: () => MediaStreamTrack[]; getTracks?: () => MediaStreamTrack[]}) {
  const tracks = typeof stream.getAudioTracks === "function"
    ? stream.getAudioTracks()
    : typeof stream.getTracks === "function" ? stream.getTracks() : [];
  const track = tracks[0];
  if (!track || typeof track.getSettings !== "function") return false;
  return track.getSettings().echoCancellation === false;
}


/**
 * The provider said this socket's session started. A SECOND time means a replacement session that
 * never saw the draft on the card — a yes it hears is not agreement to text the owner heard.
 */
function noteSessionStartedForSpokenYes() {
  deps.session.sessionStarts += 1;
  if (deps.session.sessionStarts > 1) resetSpokenYes("replaced_session");
}


/** The call's first look at the proposals: a draft already pending is not this call's to send. */
function noteProposalsAtCallOpen(proposal: PendingPost | null) {
  spokenYes.ineligible = proposal ? proposal.handle : null;
  spokenYes.ineligibleWhy = "earlier_draft";
}


/**
 * The card shows `proposal` where it showed `previous`.
 *
 * Nothing said is forgotten here: each assistant turn carries the handle that was on the card when
 * it began, and `readBackRefusal` holds the newest draft to what was said while it was up as well
 * as to everything since the user last spoke. A draft replaced by a shorter one — "don't merge the
 * PR" by "merge the PR" — is not confirmed by the first one's read-back, since neither is word for
 * word the second's.
 *
 * A turn already under way when the FIRST draft of it appears — one that began with no draft on the
 * card — remembers how many of its words came before it, so the words it goes on to say count as
 * said once the draft was up (`readBackRefusal`). A draft that REPLACES another in the middle of a
 * turn gets no such allowance: that turn read the old draft or began to, and the new one is held,
 * as it always was, to the turns that began once it was up. `#268 voice-call-drops`.
 */
function noteCardForSpokenYes(proposal: PendingPost | null, previous: PendingPost | null) {
  const handle = proposal ? proposal.handle : null;
  if (handle === (previous ? previous.handle : null)) return;
  // A held word about the card that was there is about a card that is not there now.
  if (spokenYes.redraft && spokenYes.redraft.from !== handle) dropRedraftWait();
  if (spokenYes.settling && spokenYes.settling.handle !== handle) abortSpokenYes("changed");
  spokenYes.eligible = handle !== null && handle !== spokenYes.ineligible ? handle : null;
  if (proposal) spokenYes.lastShown = proposal;
  if (handle === null) return;
  for (const turn of spokenYes.said.values()) {
    if (turn.card === null && turn.cardFrom === null) {
      turn.cardFrom = { handle, words: answerWords(turn.text).length };
    }
  }
}


/**
 * Is `turn` the assistant's reply to `answer`, rather than something said before it? `#268
 * voice-call-drops`.
 *
 * A bridge relays the user's transcript and the assistant's answer as two streams, and the answer's
 * FINAL transcript can arrive after the reply has begun — "One moment." on the page before the
 * "Yes." it answers — so the order of arrival cannot say which came first. An assistant turn
 * numbered at or after the answer's, whose words are the acknowledgement the agent is told to give a
 * yes (`isSpokenYesAcknowledgement`), is taken for the reply and left out of the read-back.
 *
 * That leans on an ASSUMPTION the protocol does not state: that it numbers the user's words and the
 * assistant's answer to them alike. Safety does not rest on it, because only the acknowledgement is
 * excused. Any other words in such a turn — "Or should I cancel it instead?" — stay part of what the
 * answer is judged against, so the yes is refused, whichever side of it they were said on; and an
 * interrupted turn of that kind counts as the read-back cut off (`heardBefore`). If the assumption
 * were wrong, the most it lets through is a yes whose exact, completed read-back was followed by
 * "One moment." before the owner answered — or by a streamed "One" whose turn was still open, with
 * no completion of unknown kind, when the settle window ran out: such a turn that says more, or
 * completes on "One" (or is left there by a completion that names no turn), stops the yes only
 * while it is settling (`hearAssistantForSpokenYes`, `noteTurnCompleteForSpokenYes`).
 *
 * Without numbers nothing tells a reply from more of the read-back, so an unnumbered turn is never
 * taken for a reply, and is judged as it always was.
 *
 * @param {{turn: number | null}} answer
 * @param {{turn: number | null, text: string}} turn
 */
function isReplyTo(answer: {turn: number | null}, turn: {turn: number | null, text: string}) {
  if (answer.turn === null || turn.turn === null || turn.turn < answer.turn) return false;
  const open = deps.session.lastCompletedTurn === null || turn.turn > deps.session.lastCompletedTurn;
  return isSpokenYesAcknowledgement(turn.text, open);
}


/**
 * The assistant said `text` in the turn named `key` (its whole text so far), numbered `turn` on the
 * neutral protocol.
 *
 * @param {string} key
 * @param {string} text
 * @param {number | null} [turn]
 */
function hearAssistantForSpokenYes(key: string, text: string, turn: number | null = null) {
  if (!String(text).trim()) return;
  const before = spokenYes.said.get(key);
  const card = before ? before.card : postWatch.shown ? postWatch.shown.handle : null;
  const at = before ? before.at : Date.now();
  /** @type {SpokenYesTurn} */
  const entry: SpokenYesTurn = { text: String(text), card, turn, at, cardFrom: before ? before.cardFrom : null };
  spokenYes.said.set(key, entry);
  const since = spokenYes.sinceUser.get(key);
  spokenYes.sinceUser.set(key, { text: String(text), turn, at: since ? since.at : at });
  // A turn the yes now settling was judged without, as its reply, has said more than the
  // acknowledgement: whatever it went on to say may have come before the yes, so the yes stops.
  // So does a NEW turn numbered at or after the answer that is not the acknowledgement: by the
  // same assumption it may have been said before the yes, and the yes was judged without it. One
  // that is the acknowledgement is taken for the reply, and held to that from here as the others
  // are. `#268 voice-call-drops`.
  const settling = spokenYes.settling;
  if (settling) {
    const answering = settling.turn !== null && turn !== null && turn >= settling.turn;
    if (isReplyTo({ turn: settling.turn }, entry)) {
      settling.replies.add(key);
    } else if (settling.replies.has(key) || answering) {
      abortSpokenYes("said_more");
    }
  }
  // The reply to an answer the page has begun to hear says nothing about what that answer answers:
  // it neither reopens the read-back nor moves the moment the answer began.
  const answer = spokenYes.answer;
  if (answer && isReplyTo(answer, entry)) return;
  spokenYes.done = false;
  if (answer) answer.startedAt = 0;
}


/**
 * Something cut the assistant off: an interruption, or an interrupted turn.
 *
 * @param {boolean} [numbered] whether it was a numbered turn, which `completed` holds instead: the
 *   reply to an answer being cut off must not count against the read-back before it
 */
function noteCutForSpokenYes(numbered: boolean = false) {
  if (!numbered) spokenYes.cut = true;
  // During the settle window that is the user speaking again: more than a yes.
  abortSpokenYes("heard_more");
}


/**
 * The neutral protocol's `turn_complete` for `turn`.
 *
 * Each numbered completion is kept until the user's next turn is judged, with the moment the audio
 * queued so far finishes playing: the server sends a turn's `turn_complete` after the last of its
 * audio, so that is when the owner heard the end of that turn. `#268 voice-call-drops`.
 *
 * A turn the yes now settling was judged without, as its reply, may have been excused on a
 * streamed "One" while it was open. If it completes there, it said "One", not the acknowledgement,
 * and what it was may have come before the yes: the yes stops, as for a reply that says more. Run
 * after `closeSpokenTurn`, which marks the turn completed.
 *
 * A completion that names no turn marks none completed, yet may have closed unknown of those replies;
 * so each numbered one is judged as complete, and the yes stops unless it said the whole
 * acknowledgement.
 *
 * @param {boolean} interrupted
 * @param {number | null} [turn]
 */
function noteTurnCompleteForSpokenYes(interrupted: boolean, turn: number | null = null): void  {
  if (turn !== null) {
    spokenYes.completed.set(turn, { interrupted, playEnd: deps.session.playEndMs || 0 });
  }
  if (interrupted) {
    noteCutForSpokenYes(turn !== null);
  } else if (spokenYes.said.size > 0) {
    spokenYes.done = true;
  }
  const settling = spokenYes.settling;
  if (settling && [...settling.replies].some((key) => {
    const entry = spokenYes.said.get(key);
    if (entry === undefined) return false;
    if (turn === null) return entry.turn !== null && !isSpokenYesAcknowledgement(entry.text, false);
    return !isReplyTo({ turn: settling.turn }, entry);
  })) {
    abortSpokenYes("said_more");
  }
}


/**
 * An answer that never got a final transcript, judged once its turn has completed. `#268
 * voice-call-drops`.
 *
 * A bridge may stream the owner's words only as hypotheses and complete the turn without ever
 * marking one final. The page stores the row at that `turn_complete`, because that is when its
 * words are as good as they are going to get, and the same is true of the answer. Called after the
 * page has finished with the `turn_complete` itself, so a notice this judgement sends to the agent
 * queues behind a turn that is over rather than inside one that is closing. The settle window
 * follows, as for unknown yes.
 *
 * @param {number | null} turn the turn that completed
 */
function judgeAnswerAtTurnEnd(turn: number | null) {
  const answer = spokenYes.answer;
  if (!answer || answer.turn === null || turn === null || turn < answer.turn) return;
  hearUserForSpokenYes(answer.key, answer.text, true, answer.turn);
}


/**
 * A text turn went to the agent — typed, canned, relayed, or this page's own notice. As far as the
 * read-back is concerned it is a turn of its own: whatever the assistant said before it is no longer
 * the last thing the owner heard, so it is forgotten, and only a fresh read-back arms a yes. It is
 * the USER's turn only when the user wrote or pressed it (`noteUserTextForSpokenYes`).
 */
function noteTypedForSpokenYes() {
  abortSpokenYes("typed");
  spokenYes.said = new Map();
  spokenYes.interposed = true;
  spokenYes.cut = false;
  spokenYes.done = false;
  spokenYes.completed = new Map();
  if (spokenYes.answer) spokenYes.answer.startedAt = 0;
}


/** The user typed or pressed a message to the agent: their turn, so the question before it is answered. */
function noteUserTextForSpokenYes() {
  spokenYes.sinceUser = new Map();
  spokenYes.interposed = false;
}


/**
 * The user said `text` in the turn named `key` — its whole text so far, numbered `turn` on the
 * neutral protocol — and whether that is final.
 *
 * Any frame times the answer and can stop a yes that is settling; the FIRST final of a turn is
 * judged, once, against what the assistant said BEFORE it since the user last spoke
 * (`heardBefore`). What the assistant says in reply, which can reach this page before the answer's
 * own final transcript does, is no part of that; it is kept, as the first thing said since.
 *
 * A late frame of an OLDER turn than the answer the page has begun to hear is not that answer
 * (`refuseLateAnswer`). `#268 voice-call-drops`.
 *
 * @param {string} key
 * @param {string} text
 * @param {boolean} final
 * @param {number | null} [turn]
 */
function hearUserForSpokenYes(key: string, text: string, final: boolean, turn: number | null = null) {
  const now = Date.now();
  const current = spokenYes.answer;
  if (current && current.key !== key && turn !== null && current.turn !== null && turn < current.turn) {
    if (final && key !== spokenYes.answered) refuseLateAnswer(key, text);
    return;
  }
  if (key !== spokenYes.answered) {
    if (!current || current.key !== key) {
      spokenYes.answer = { key, turn, text: String(text), startedAt: now };
    } else {
      current.text = String(text);
      if (!current.startedAt) current.startedAt = now;
    }
  }
  const settling = spokenYes.settling;
  if (settling && answerWords(text).join(" ") !== settling.words) abortSpokenYes("heard_more");
  if (!final || key === spokenYes.answered) return;
  spokenYes.answered = key;
  const answer = spokenYes.answer || { key, turn, text: String(text), startedAt: now };
  spokenYes.answer = null;
  // What this answer is judged against, taken before it is forgotten: the read-back belongs to the
  // answer right after it and to no later one. Its reply, if unknown has arrived, is kept as the first
  // thing said since.
  const heard = heardBefore(answer);
  spokenYes.said = new Map([...spokenYes.said].filter(([entryKey]) => heard.replies.has(entryKey)));
  spokenYes.sinceUser = new Map([...spokenYes.sinceUser].filter(([, entry]) => isReplyTo(answer, entry)));
  spokenYes.completed = new Map(
    [...spokenYes.completed].filter(([number]) => answer.turn !== null && number >= answer.turn)
  );
  spokenYes.interposed = false;
  spokenYes.cut = false;
  spokenYes.done = false;
  // The same yes again while one is already settling or committing is not a second answer.
  if (spokenYes.settling || postWatch.busy) return;
  judgeSpokenAnswer(text, heard);
}


/**
 * The final transcript of an older user turn arrived after a newer one had begun: the owner's
 * "Yes" finished, then "wait, no" started, and only then did the bridge mark "Yes." final. The
 * newer answer is the one the owner is giving now and stays the one the page is hearing; this one
 * is judged as more than a yes was heard — a no still withdraws the draft, nothing sends — and a yes
 * already settling stops. `#268 voice-call-drops`.
 *
 * @param {string} key
 * @param {string} text
 */
function refuseLateAnswer(key: string, text: string) {
  spokenYes.answered = key;
  abortSpokenYes("heard_more");
  if (postWatch.busy) return;
  const answer = spokenYes.answer;
  if (!answer) return;
  judgeSpokenAnswer(text, heardBefore(answer), "heard_more");
}


/**
 * Everything `answer` is judged against: what the assistant said before it since the user last
 * spoke, and the facts about that read-back. `#268 voice-call-drops`.
 *
 * On the neutral protocol with numbered turns, "completed, not interrupted, before the answer" and
 * "heard to the end" are asked of the read-back's OWN turn — its `turn_complete`, and the moment the
 * audio queued by then finished playing — not of whatever the assistant is saying now, which may
 * already be its reply. The answer began at its first frame, or at its reply's first frame if that
 * came sooner: a reply cannot start before the words it answers, so the earlier of the two is still
 * no earlier than the answer began, and judging from it is never more lenient.
 *
 * Only the acknowledgement is left out as the reply (`isReplyTo`). A numbered turn at or after the
 * answer's that completed interrupted counts as the read-back cut off unless all it said was the
 * acknowledgement: it may have been said before the answer, and the owner may not have heard it out.
 *
 * @param {{key: string, turn: number | null, startedAt: number}} answer
 * @returns {SpokenYesHeard}
 */
function heardBefore(answer: {key: string, turn: number | null, startedAt: number}) {
  const entries = [...spokenYes.said];
  const replies = new Set(entries.filter(([, entry]) => isReplyTo(answer, entry)).map(([key]) => key));
  const turns = entries.filter(([key]) => !replies.has(key)).map(([, entry]) => entry);
  const sinceUser = [...spokenYes.sinceUser.values()].filter((entry) => !isReplyTo(answer, entry));
  let answerAt = answer.startedAt || Date.now();
  for (const [key, entry] of entries) {
    if (replies.has(key)) answerAt = Math.min(answerAt, entry.at);
  }
  // The numbered turns whose words were only the acknowledgement: the one kind of turn at or after
  // the answer's whose interruption says nothing about the read-back.
  const acknowledged = new Set(
    entries.filter(([key]) => replies.has(key)).map(([, entry]) => entry.turn)
  );
  const before = (number: number) => answer.turn === null || number < answer.turn;
  const cutBefore = [...spokenYes.completed].some(([number, end]) =>
    end.interrupted && (before(number) || !acknowledged.has(number))
  );
  let done = spokenYes.done;
  let playEnd = deps.session.playEndMs || 0;
  const numbered = turns.length > 0 && turns.every((entry) => entry.turn !== null);
  if (deps.session.protocol === "vibe-talk-v1" && numbered) {
    const end = spokenYes.completed.get(Math.max(...turns.map((entry) => Number(entry.turn))));
    done = Boolean(end && !end.interrupted);
    if (end) playEnd = end.playEnd;
  }
  return {
    turns,
    sinceUser,
    interposed: spokenYes.interposed,
    cut: spokenYes.cut || cutBefore,
    done,
    answerAt,
    playEnd,
    turn: answer.turn,
    replies,
  };
}


/**
 * Act on one final answer: settle a yes that passes, withdraw on a no, and say plainly why anything
 * an agent could take for a yes that does not pass sent nothing.
 *
 * @param {string} text
 * @param {SpokenYesHeard} heard
 * @param {keyof typeof SPOKEN_YES_REASONS | null} [refused] why a yes is refused whatever the
 *   read-back was: an answer that is no longer the one the owner is giving (`refuseLateAnswer`)
 */
function judgeSpokenAnswer(text: string, heard: SpokenYesHeard, refused: RefusedReason | null = null): void {
  if (!deps.session.connected || !spokenYes.askFirst) return;
  // The owner spoke again with no new draft on the card: the word held for the last answer is said
  // now, as it would have been before the hold, so a "did it go?" never meets an agent the page has
  // told nothing. This answer is then judged on its own.
  sayRedraftWait();
  const shown = postWatch.shown;
  const kind = classifySpokenAnswer(text);
  const asked = askedAboutSending(heard.sinceUser, text);
  if (!shown) {
    // This call's draft has gone — it lapsed and the watch took it down, or it was settled — and an
    // agent that asked about it, told to say only "One moment." on a yes, would wait for nothing. A
    // send that names where it goes is a new request, not an answer to a draft that is not there.
    if (spokenYes.lastShown && asked && mayBeYes(kind, text) && !namesWhereToSend(text)) {
      reportAnswerOutcome(kind, { kind: "gone", proposal: spokenYes.lastShown });
    }
    return;
  }
  const eligible = shown.handle === spokenYes.eligible;
  if (kind === "negative") {
    if (eligible && asked) void cancelBySpokenNo();
    return;
  }
  // A yes to some other question while the card is up ("want to hear the rest?") is not one the
  // agent waits on the page for: it says so on the status line, and only for a plain yes.
  if (!asked) {
    if (eligible && kind === "affirmative") {
      deps.setStatus("Not sent — a yes counts only as the answer to the read-back. Tap Send to post it.");
    }
    return;
  }
  // Anything an agent could take for a yes, to a question about sending, is ALWAYS answered: the
  // agent says only "One moment." and waits for this page, so silence would leave it waiting and the
  // owner not knowing. A longer answer with nothing like a yes in it is a request of its own.
  if (!mayBeYes(kind, text)) return;
  if (!eligible) {
    reportAnswerOutcome(kind, {
      kind: "not_armed", proposal: shown, reason: SPOKEN_YES_REASONS[spokenYes.ineligibleWhy],
    });
    return;
  }
  if (kind !== "affirmative") {
    reportAnswerOutcome(kind, { kind: "not_armed", proposal: shown, reason: SPOKEN_YES_REASONS.hint });
    return;
  }
  const refusal = refused || spokenYesRefusal(shown, text, heard);
  if (refusal) {
    reportSpokenOutcome({ kind: "not_armed", proposal: shown, reason: SPOKEN_YES_REASONS[refusal] });
    return;
  }
  const epoch = spokenYes.epoch;
  const settling = {
    handle: shown.handle,
    words: answerWords(text).join(" "),
    timer: /** @type {Timer | null} */ (null as Timer | null),
    proposal: shown,
    turn: heard.turn,
    replies: heard.replies,
    hold: /** @type {YesHold | null} */ (null as YesHold | null),
  };
  settling.timer = setTimeout(() => {
    if (spokenYes.settling !== settling || spokenYes.epoch !== epoch) return;
    spokenYes.settling = null;
    renderPostCancel();
    void commitBySpokenYes(settling.proposal, epoch, settling.hold);
  }, SPOKEN_YES_SETTLE_MS);
  spokenYes.settling = settling;
  // `#269 approval-card-inline`. The answer's row, drawn hidden while it might be this yes, stays
  // hidden while it settles and sends.
  settling.hold = acceptYesHold(spokenYes.answered, shown.serial);
  deps.setStatus("Heard yes — sending…");
  // `#269 approval-card-inline`. The card's own line says so too, with how to stop it — and the way
  // to stop it is live, even inside the hold of a card that has only just appeared.
  renderPostCancel();
  renderPostExpiry();
}


/**
 * A message typed to the agent while the card is up, judged BEFORE it goes (sending it forgets what
 * the assistant said before it). A typed yes never sends — the Send button is on the same screen —
 * but the agent was told to say only "One moment." on a yes, so one like it to a question about
 * sending is answered: not sent, and why — or, once this call's draft has gone, that none is
 * waiting. Returns that outcome, or null when there is nothing to say.
 */
function typedAnswerToCard(text: string) {
  if (!deps.canSendText() || !spokenYes.askFirst || spokenYes.settling || postWatch.busy) return null;
  const kind = classifySpokenAnswer(text);
  if (kind !== "affirmative" && kind !== "hint") return null;
  if (!askedAboutSending([...spokenYes.sinceUser.values()], text)) return null;
  const shown = postWatch.shown;
  if (!shown) {
    return spokenYes.lastShown && !namesWhereToSend(text) ? { kind: "gone", proposal: spokenYes.lastShown } : null;
  }
  const why = shown.handle !== spokenYes.eligible
    ? spokenYes.ineligibleWhy
    : deps.session.chat ? "typed_call" : "typed_yes";
  return { kind: "not_armed", proposal: shown, reason: SPOKEN_YES_REASONS[why] };
}


/**
 * Why a yes to `shown` must not send it, as a key of SPOKEN_YES_REASONS, or null when it may.
 *
 * @param {PendingPost} shown
 * @param {string} answer
 * @param {SpokenYesHeard} heard
 */
function spokenYesRefusal(shown: PendingPost, answer: string, heard: SpokenYesHeard) {
  if (shown.handle !== spokenYes.eligible) return spokenYes.ineligibleWhy;
  if (postWatch.deadline <= Date.now()) return "expired";
  if (deps.redact(shown.text) !== shown.text) return "token";
  if (deps.session.chat) return "typed_call";
  if (deps.session.echoCancellation !== true) return "echo_cancellation_off";
  if (heard.cut || (deps.session.protocol === "vibe-talk-v1" && !heard.done)) return "interrupted";
  // Before the read-back, which cannot see what makes a draft unsayable: its words may all match.
  // The agent's words only: the owner's tag in front of them is the server's (`agentDraft`).
  const unsayable = unsayableDraft(agentDraft(shown));
  if (unsayable) return unsayable;
  const readBack = readBackRefusal(shown, heard.turns);
  if (readBack) {
    // An exact read-back with a relayed message or a notice after it: it was not the last thing said.
    const readBackBefore = heard.interposed &&
      heard.sinceUser.some((turn) => isExactReadBack(answerWords(turn.text), shown));
    return readBackBefore ? "interposed" : readBack;
  }
  const answeredAt = heard.answerAt || Date.now();
  // When the READ-BACK finished playing, not whatever is queued now: the reply to this answer may
  // already be playing. `#268 voice-call-drops`.
  const playEnd = heard.playEnd || 0;
  if (answeredAt < playEnd) return "early";
  // The read-back ends "...Send it?", so its last words are the card's last two and "send it": an
  // answer made only of those, straight after, may be the agent's own voice coming back.
  const tail = new Set(answerWords(heard.turns.map((turn) => turn.text).join(" ")).slice(-4));
  const words = answerWords(answer);
  if (words.every((word) => tail.has(word)) && answeredAt - playEnd < SPOKEN_YES_ECHO_GUARD_MS) {
    return "echo";
  }
  return null;
}


/** Stop a yes that is settling, and say why when there is a reason to say. */
function abortSpokenYes(reason: RefusedReason) {
  const settling = spokenYes.settling;
  if (!settling) return;
  clearTimeout(settling.timer!);
  spokenYes.settling = null;
  releaseYesHold(settling.hold || null);
  renderPostCancel();
  renderPostExpiry();
  reportSpokenOutcome({ kind: "not_armed", proposal: settling.proposal, reason: SPOKEN_YES_REASONS[reason] });
}


/**
 * The owner tapped the card while a yes to it was settling. The tap wins, and the yes stops
 * without a word of its own: the tap's result is what the agent is told, once — Send's in full, as
 * the yes's would have been, and Don't send's as a tap.
 *
 * @returns {boolean} whether a yes was stopped, so the tap's result must answer it
 */
function spokenYesYieldsToTap() {
  const settling = spokenYes.settling;
  if (!settling || !postWatch.shown || settling.handle !== postWatch.shown.handle) return false;
  clearTimeout(settling.timer!);
  spokenYes.settling = null;
  // The tap is what approves it, and its card says so: the yes is a turn of its own again.
  releaseYesHold(settling.hold || null);
  renderPostCancel();
  renderPostExpiry();
  return true;
}


/**
 * The settle window passed: check once more, then commit what the card shows.
 *
 * @param {PendingPost} proposal
 * @param {number} epoch
 * @param {YesHold | null} [hold] the yes's row, kept out of sight until this says whether it sent
 */
async function commitBySpokenYes(proposal: PendingPost, epoch: number, hold: YesHold | null = null) {
  const shown = postWatch.shown;
  let refusal: RefusedReason | null = null;
  if (!shown || shown.handle !== proposal.handle) refusal = "changed";
  else if (postWatch.busy) refusal = "busy";
  else if (postWatch.deadline <= Date.now()) refusal = "expired";
  else if (deps.redact(shown.text) !== shown.text) refusal = "token";
  else if (!deps.session.connected) refusal = "ended";
  if (refusal || !shown) {
    releaseYesHold(hold);
    reportSpokenOutcome({ kind: "not_armed", proposal, reason: SPOKEN_YES_REASONS[refusal || "changed"] });
    return;
  }
  commitYesHold(hold);
  const outcome = await commitShownProposal(shown, "speaker_turn");
  // `#269 approval-card-inline`. A yes that sent is drawn into the frozen card for good; unknown other
  // outcome shows it again. One whose answer was lost may yet be in the send memory.
  if (outcome.kind === "sent" || outcome.kind === "partial" || outcome.kind === "unconfirmed") sentYesHold(hold);
  else releaseYesHold(hold, outcome.kind === "unknown");
  // A call that ended, or a session that was replaced, while the commit was on the wire hears
  // nothing about it; the screen still says what happened.
  if (spokenYes.epoch === epoch) reportSpokenOutcome(outcome);
}


/** A no to the card's "Send it?": withdraw the draft. The agent heard the no itself. */
async function cancelBySpokenNo() {
  if (postWatch.busy) return;
  const proposal = postWatch.shown;
  const withdrawn = await cancelPostProposal({ spoken: true });
  reportSpokenOutcome({ kind: "declined", proposal, withdrawn: withdrawn === true });
}


// --- a spoken yes the page acts on shows no turn ----------------------------------------------------
//
// `#269 approval-card-inline`. A yes that sends the message is drawn INTO the frozen card it sent —
// "approved by voice" — rather than standing as a turn of its own above it: the owner said "yes" to
// the card, and the card is what answers. So the row of a user turn that could still become a yes to
// the card is drawn hidden from its first frame (`data-absorbed`), and stays hidden only if that yes
// sends. Anything else shows it again, in its own place. The asymmetry is deliberate: a turn that
// starts like a yes and becomes something else appears a frame late; a yes that is acted on does not
// flash up first. The one exception is a yes whose commit's answer was lost: it is shown again,
// because nobody can yet say it sent, and hidden again if the send memory then reports the send
// approved by voice (`absorbAnswerRow`).
//
// The RECORD always keeps the yes, as an ordinary `you` turn, tagged `answer: <serial>` when the page
// had acted on it by the time it was filed. A reload hides it only when a `sent` row approved by voice
// for that serial follows it, so the record never loses words, and a tag on a yes that sent nothing
// hides nothing.

/** How long a hold waits, after its turn's last frame, for that turn to be judged. Longer than `#268`'s quiet-turn bound. */
const YES_HOLD_JUDGE_MS = 10000;

/** How long a yes the page took stays hidden while it settles and commits, at most. */
const YES_HOLD_MAX_MS = 20000;


/**
 * One user turn drawn hidden while it may be a yes to the card.
 *
 * @typedef {{key: string, li: HTMLElement | null, state: "tentative" | "accepted" | "committing" | "sent" | "released",
 *   serial: number | null, conversationId: string | null, timer: unknown, kept: boolean}} YesHold
 */

/**
 * Might the user turn saying `text` be a yes to the card, one the page would act on? Only while a
 * draft of this call's agent is on the card and nothing is settling or sending it, only as an answer
 * while sending is the subject, and only for words that could still become a yes.
 *
 * @param {string} text
 */
function mightBeCardYes(text: string) {
  if (!spokenYes.askFirst || deps.session.chat) return false;
  const shown = postWatch.shown;
  if (!shown || shown.handle !== spokenYes.eligible || postWatch.busy || spokenYes.settling) return false;
  if (/[?¿]/u.test(String(text))) return false;
  if (!askedAboutSending([...spokenYes.sinceUser.values()], text)) return false;
  return couldBeSpokenYes(answerWords(text));
}


/**
 * The user turn `key` was drawn as `li` (or redrawn: a hypothesis grew) saying `text`. Hide it while
 * it may be a yes to the card; show it, for good, once it cannot be.
 *
 * @param {string} key
 * @param {string} text
 * @param {HTMLElement | null} li
 */
function noteYesCandidate(key: string, text: string, li: HTMLElement | null): void {
  const held = spokenYes.holds.get(key);
  if (held) {
    if (li) held.li = li;
    if (held.state !== "tentative") return;
    if (!mightBeCardYes(text)) {
      releaseYesHold(held);
      return;
    }
    armYesHold(held, YES_HOLD_JUDGE_MS);
    return;
  }
  if (!li || !mightBeCardYes(text)) return;
  /** @type {YesHold} */
  const hold: YesHold = { key, li, state: "tentative", serial: null, conversationId: null, timer: null, kept: false };
  li.setAttribute("data-absorbed", "");
  spokenYes.holds.set(key, hold);
  if (spokenYes.holds.size > 32) spokenYes.holds.delete(spokenYes.holds.keys().next().value!);
  armYesHold(hold, YES_HOLD_JUDGE_MS);
}


/** Bound how long `hold` keeps its row hidden. */
function armYesHold(hold: YesHold, ms: number) {
  if (hold.timer !== null) clearTimeout(hold.timer);
  hold.timer = setTimeout(() => {
    hold.timer = null;
    releaseYesHold(hold);
  }, ms);
}


/**
 * The yes the page has just begun to settle is the turn `key`: its row stays hidden while it settles
 * and sends, and is marked as the answer to proposal `serial`.
 *
 * @param {string | null} key
 * @param {number} serial
 * @returns {YesHold | null}
 */
function acceptYesHold(key: string | null, serial: number) {
  const hold = key === null ? undefined : spokenYes.holds.get(key);
  if (!hold || hold.state !== "tentative") return null;
  hold.state = "accepted";
  hold.serial = serial;
  hold.conversationId = deps.session.conversationId;
  if (hold.li) hold.li.setAttribute("data-answer", String(serial));
  armYesHold(hold, YES_HOLD_MAX_MS);
  return hold;
}


/** The yes `hold` is being committed. */
function commitYesHold(hold: YesHold | null): void  {
  if (hold && hold.state === "accepted") hold.state = "committing";
}


/** The yes `hold` sent the message: its row stays hidden for good, drawn into the frozen card. */
function sentYesHold(hold: YesHold | null): void  {
  if (!hold || (hold.state !== "accepted" && hold.state !== "committing")) return;
  hold.state = "sent";
  if (hold.timer !== null) clearTimeout(hold.timer);
  hold.timer = null;
}


/**
 * Show `hold`'s row again, in its own place, for good. `kept`: its commit's answer was lost, so the
 * send memory may yet say it went (`absorbAnswerRow`).
 *
 * @param {YesHold | null} hold
 * @param {boolean} [kept]
 */
function releaseYesHold(hold: YesHold | null, kept: boolean = false) {
  if (!hold || hold.state === "released" || hold.state === "sent") return;
  hold.state = "released";
  hold.kept = kept;
  if (hold.timer !== null) clearTimeout(hold.timer);
  hold.timer = null;
  const li = hold.li;
  if (!li || !li.hasAttribute("data-absorbed")) return;
  const area = deps.el("scroll-area");
  const pinned = pinnedHere(area);
  li.removeAttribute("data-absorbed");
  deps.applySearch();
  deps.followIfPinned(pinned);
  noteFollow(area, pinned);
}


/** The turn `key` has been judged: unless the page took it as a yes, its row is shown. */
function settleYesCandidate(key: string): void  {
  const hold = spokenYes.holds.get(key);
  if (hold && hold.state === "tentative" && spokenYes.answered === key) releaseYesHold(hold);
}


/** A turn completed and whatever answer it held was judged: every judged turn not taken is shown. */
function settleYesCandidates() {
  for (const key of spokenYes.holds.keys()) settleYesCandidate(key);
}


/** Show every row still held for a yes that is not on its way: the conversation so far is forgotten. */
function releaseYesHolds() {
  for (const hold of spokenYes.holds.values()) {
    if (hold.state === "tentative" || hold.state === "accepted") releaseYesHold(hold);
  }
}


/**
 * The send memory says proposal `serial` went, approved by voice, in `conversationId`: a yes whose
 * commit's answer was lost was shown again, and is drawn into the card now after all.
 *
 * @param {string | null} conversationId
 * @param {number} serial
 */
function absorbAnswerRow(conversationId: string | null, serial: number) {
  const holds = [...spokenYes.holds.values()].filter((hold) =>
    hold.serial === serial && hold.conversationId === conversationId);
  if (holds.some((hold) => hold.state === "sent")) return;
  const hold = holds.reverse().find((one) => one.state === "released" && one.kept);
  if (!hold || !hold.li || !hold.li.parentNode) return;
  hold.state = "sent";
  const li = hold.li;
  deps.preservingScroll(() => li.setAttribute("data-absorbed", ""));
  deps.applySearch();
}


// The transcript seam's explanation per outcome of a spoken yes (`SPOKEN_YES_SEAMS`) is web/src/core/call's.

// The seam's label per outcome (`SPOKEN_YES_SEAM_LABELS`) is web/src/core/call's (`spokenYesSeam`).

/** Tell the agent `notice` on the call's text channel, if a call is open to hear it. */
function tellAgent(notice: string | null): void  {
  if (notice === null || !deps.canSendText()) return;
  // Remembered as typed, so a protocol that echoes it back as a user transcript does not show it
  // as the owner's words or judge it as an answer.
  deps.noteTypedTurn(notice);
  // The page's own word: it waits for an answer to finish rather than cutting it. `#330`.
  deps.sendClientEvent({ type: "user_message", text: notice }, "notice");
}


/**
 * A tap on the card during a call: told to the agent, and only to it — the screen already said it.
 * An agent that asked "Send it?" is waiting for the page's word, and a later "did it go?" must have
 * a true answer.
 */
function tellAgentAboutTap(outcome: Outcome) {
  tellAgent(outcomeNotice(outcome, "tap"));
}


/**
 * This call's draft lapsed on the server with nobody's answer: told to the agent, so it neither waits
 * on a card that has gone nor, asked later, says it is still there. `#579 voice-send-confirm-timing`.
 * Only a draft this call's agent made and the card showed; one already waiting when the call began,
 * or left from an agent told to post at once, is not its to report.
 *
 * @param {PendingPost} proposal
 */
function tellAgentOfLapse(proposal: PendingPost) {
  if (!spokenYes.askFirst || proposal.handle === spokenYes.ineligible) return;
  // A yes settling on it is stopped by the card going, and says so itself; a notice of the page's own
  // would cut that yes short first, as a typed turn does, and be a second word on one draft.
  if (spokenYes.settling) return;
  if (!spokenYes.lastShown || spokenYes.lastShown.handle !== proposal.handle) return;
  tellAgent(outcomeNotice({ kind: "lapsed", proposal }, "tap"));
}


/**
 * Say what came of a yes: to the agent, on the status line, in the transcript, and as a tone.
 *
 * @param {{kind: string, proposal?: PendingPost | null, reason?: string, withdrawn?: boolean}} outcome
 *   `withdrawn`, for a spoken no: whether the server withdrew the draft
 */
function reportSpokenOutcome(outcome: {kind: string, proposal?: PendingPost | null, reason?: string, withdrawn?: boolean}): void  {
  const channel = (outcome.proposal && outcome.proposal.channel_name) || "the channel";
  const kind = spokenOutcomeKind(outcome.kind);
  let status;
  if (kind === "sent") {
    status = `Sent to ${channel}.`;
  } else if (kind === "partial") {
    status = `Only part reached ${channel} — the rest was not sent.`;
  } else if (kind === "unknown" || kind === "unconfirmed") {
    status = "Could not confirm the send — check the channel.";
  } else if (kind === "refused") {
    status = `Not sent — ${outcome.reason || "the server refused it"}.`;
  } else if (kind === "declined") {
    status = "Not sent.";
  } else if (kind === "gone") {
    status = `Nothing sent — ${SPOKEN_YES_REASONS.gone}.`;
  } else {
    status = `Not sent — ${outcome.reason || "it could not be confirmed by voice"}.`;
  }
  tellAgent(outcomeNotice({ ...outcome, kind }, "voice"));
  deps.setStatus(status);
  // `#270 voice-transcript-markers`. A send — whole, partial or unconfirmed — is the frozen card,
  // which says "approved by voice" and is kept; a seam beside it would say it twice. A no that
  // withdrew the draft is that draft's "Not sent" line, kept as well; one the server did not take
  // withdrew nothing, and says nothing here.
  if (kind === "declined") {
    if (outcome.withdrawn && outcome.proposal) noteNotSent("declined", outcome.proposal);
  } else if (kind !== "sent" && kind !== "partial" && kind !== "unconfirmed") {
    const { label: word, detail } = spokenYesSeam(kind);
    transcriptSeam(word!, detail!);
  }
  playSpokenYesTone(kind === "sent");
}


/**
 * A short tone for the outcome: a rising pair for sent, one low note for not sent.
 *
 * Generated as an AudioBuffer on the call's own audio context and queued like speech — after
 * whatever is already queued, and ahead of whatever comes next, so the agent's next words wait for
 * it rather than talking over it — so it is heard even when the model says nothing. Skipped when the
 * agent's voice is silenced, in a typed call, and after hang-up — there is nothing to play it with
 * then, and the screen says it.
 */
function playSpokenYesTone(sent: boolean) {
  const audio = deps.session.audio;
  if (!audio || deps.session.chat || deps.session.speakerOff) return;
  const rate = audio.sampleRate || 48000;
  const notes: [number, number][] = sent ? [[660, 0.11], [880, 0.14]] : [[330, 0.25]];
  const length = Math.round(notes.reduce((sum, [, seconds]) => sum + seconds, 0) * rate);
  const buffer = audio.createBuffer(1, length, rate);
  const samples = buffer.getChannelData(0);
  const ramp = 0.01 * rate;
  let offset = 0;
  for (const [frequency, seconds] of notes) {
    const count = Math.round(seconds * rate);
    for (let i = 0; i < count && offset + i < length; i += 1) {
      const envelope = Math.min(1, i / ramp, (count - i) / ramp);
      samples[offset + i] = 0.2 * envelope * Math.sin((2 * Math.PI * frequency * i) / rate);
    }
    offset += count;
  }
  const node = audio.createBufferSource();
  node.buffer = buffer;
  node.connect(audio.destination);
  const now = audio.currentTime;
  const at = Math.max(now, deps.session.playAt || 0);
  node.start(at);
  deps.session.playAt = at + buffer.duration;
  deps.session.playEndMs = Math.max(deps.session.playEndMs || 0, Date.now() + (deps.session.playAt - now) * 1000);
  deps.session.playing.push(node);
  node.onended = () => {
    deps.session.playing = deps.session.playing.filter((n) => n !== node);
  };
}


// --- the durable transcript --------------------------------------------------------------------
//
// `#48 transcript-storage`. Everything on this screen used to live only in the DOM: a reload, a
// crash, or a phone deciding to reclaim the tab took the whole conversation with it. The server
// now keeps it, and the two records must not be able to disagree about what erases what:
//
//   * Clear empties the SCREEN and leaves the stored record alone.
//   * Forget stored conversations erases the RECORD and leaves the screen alone.
//
// Both sentences are on the Settings screen next to the button, because a control that quietly
// does more than the screen says is the failure this whole page is written against.

/**
 * Ids the server will accept: letters, digits, '-' and '_', at most 64 characters.
 *
 * A vendor id that already fits is used as it is, so the stored conversation can be matched up
 * with the vendor's own record of the same call. Anything else gets a LOCAL id rather than a
 * cleaned-up version of itself: stripping the illegal bytes out is what makes `a/b` and `ab` the
 * same conversation, and two different calls writing into one transcript is a worse failure than
 * an id that does not match the vendor's.
 */
function conversationIdFrom(vendorId: unknown) {
  const raw = String(vendorId || "");
  if (raw.length > 0 && raw.length <= 64 && /^[A-Za-z0-9_-]+$/.test(raw)) {
    return raw;
  }
  // A call the vendor never named — or named in a way this server will not accept — still
  // happened. Local, unguessable enough for a filename, and deliberately not a credential: it is
  // only a key in this owner's own store.
  return `local-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}


function setStorageState(text: string) {
  const state = deps.el("storage-state");
  if (state) {
    state.textContent = text;
  }
}


/**
 * Record one turn, without letting the store interfere with the call.
 *
 * Deliberately not awaited by its callers and deliberately silent on the call screen: the owner
 * is in a car. A failure is reported once, in Settings, retried, and never disables recording
 * (`#1303 voice-transcript-share`).
 *
 * ONE AT A TIME, IN THE ORDER THEY WERE SAID. Every turn joins `recordQueue` and the next POST is
 * not issued until the previous one has answered. Firing them off in parallel looks harmless and
 * is not: the SERVER stamps `seq` and `at_ms` at arrival, so two requests milliseconds apart that
 * complete out of order — different connections, one retransmit, unknown ordinary jitter — are stored
 * in the order they LANDED. That inversion is then permanent and invisible: the restored
 * transcript shows the answer above the question, with server timestamps that agree with it, and
 * nothing anywhere records that they were swapped.
 *
 * The cost is that recording lags a fast exchange by one round trip. Nothing on the screen waits
 * for it, so the owner cannot tell.
 */
let recordQueue: Promise<unknown> = Promise.resolve();


/**
 * `#511 voice-explicit-send-only`. What the page has heard the owner say, told to the server as it
 * arrives.
 *
 * The voice agent sends to the chat only when the owner explicitly asks it to, and the server checks
 * each send against the owner's own words. It never sees a call's frames, and a spoken turn is filed
 * in the transcript only when it settles — after the agent's tool call — so the page says what it
 * heard while the turn is still open: each user turn's whole text so far, never an echo of the
 * page's own prompt and never the assistant's words. Nothing at all while a turn the page itself
 * sent is in flight — a relayed message, or the page's word on a card — whose echo could otherwise
 * pass for the owner's words: the owner speaking over one is a send the server may refuse, never a
 * chat message taken for his ask. One request per turn at a time, the newest text replacing what
 * was waiting; a failed one is tried again with the next text, and otherwise dropped, because the
 * worst it costs is a send the server refuses and the agent asks about.
 *
 * Keys name the page (`HEARD_PAGE`) and the voice session (`heardEpoch`, moved on by every session
 * start), because a reconnected session numbers its turns from one again.
 */
let HEARD_PAGE: string;
function initializeHeardPage(value: string) { HEARD_PAGE = value; }

/** The server keeps no more of one turn than this; neither does the page send more. */
const HEARD_TEXT_MAX = 2000;

/** How much of the start of a longer turn is sent; the rest is its end, where a closing ask is. */
const HEARD_HEAD = 500;

let heardEpoch = 0;

let heardTyped = 0;

/** @type {Map<string, {inFlight: boolean, next: string | null, sent: string}>} */
const heardReports: Map<string, HeardReport> = new Map();


/**
 * @param {string} key the page's name for the turn, stable while it grows
 * @param {string} text everything the owner has said in it so far
 */
function reportHeard(key: string, text: string): void  {
  const whole = String(text || "").trim();
  if (!whole || pageTurnInFlight()) return;
  const said = whole.length <= HEARD_TEXT_MAX ? whole
    : `${whole.slice(0, HEARD_HEAD)} … ${whole.slice(whole.length - (HEARD_TEXT_MAX - HEARD_HEAD - 3))}`;
  const full = `${HEARD_PAGE}:${heardEpoch}:${key}`;
  let report = heardReports.get(full);
  if (!report) {
    report = { inFlight: false, next: null, sent: "" };
    heardReports.set(full, report);
    // The server keeps the last few minutes; the page need not remember more turns than that.
    while (heardReports.size > 64) heardReports.delete(heardReports.keys().next().value!);
  }
  if (said === report.sent || said === report.next) return;
  report.next = said;
  if (!report.inFlight) flushHeard(full, report);
}


/** Send the turn's newest text, and again when it grew while that request was out. */
function flushHeard(key: string, report: HeardReport) {
  const text = report.next;
  if (text === null) return;
  report.next = null;
  report.inFlight = true;
  deps.api("/api/v1/voice-heard", { method: "POST", body: { key, text } })
    .then(() => { report.sent = text; }, () => {})
    .finally(() => {
      report.inFlight = false;
      if (report.next !== null) flushHeard(key, report);
    });
}


/**
 * Whether a turn the page itself sent is in flight: a relayed message or the page's word on a card
 * on the websocket protocol, or a relayed message on the hosted one, whose echo the vendor may send
 * back as the owner's words.
 */
function pageTurnInFlight() {
  return (deps.session.turnInFlight !== null && deps.session.turnInFlight.kind !== "typed") || deps.relayAnswerAwaited !== null;
}


/**
 * @param {string} who
 * @param {string} text
 * @param {{answerOf?: YesHold | null}} [options] `answerOf`: the hold on a user turn that may be a
 *   spoken yes to the card (`#269 approval-card-inline`)
 */
function recordTurn(who: string, text: string, options: {answerOf?: YesHold | null} = {}): void  {
  if (!deps.session.conversationId || !text) {
    return;
  }
  // A `note` is the page's own line — that a turn played with no text, `#330` — never the agent's.
  const speaker = who === "you" ? "you" : who === "note" ? "note" : "agent";
  // Captured now: by the time this turn reaches the front of the queue the session may have moved
  // on, and a turn belongs to the conversation it was spoken in.
  const conversationId = deps.session.conversationId;
  const hold = options.answerOf || null;
  // `#270 voice-transcript-markers`. A conversation's first record is its start marker.
  fileStartMarker(conversationId);
  recordQueue = recordQueue.then(async () => {
    // What an outage left behind goes first, in the order it was said. `#1303
    // voice-transcript-share`: a failure no longer stops the queue at all.
    await flushUnrecorded();
    /** @type {{speaker: string, text: string, answer?: number}} */
    const body: {speaker: string, text: string, answer?: number} = { speaker, text };
    // `#270 voice-transcript-markers`. A spoken yes the page acted on is filed as the answer to the
    // proposal it sent: read HERE, when the request goes, which is never before the handler that
    // stored the turn has judged it — a `vibe-talk-v1` turn with no final is stored at its
    // `turn_complete` a moment before the judgement in the same handler. The tag only lets a reload
    // draw the yes into the card that follows it; on one that sent nothing it hides nothing.
    if (hold && transcriptRecordsSupported && hold.serial !== null &&
      (hold.state === "accepted" || hold.state === "committing" || hold.state === "sent")) {
      body.answer = hold.serial;
    }
    try {
      await postWithRetry(conversationId, body);
    } catch (caught) {
      const error = caught as ApiRequestError;
      if (transientFailure(error)) {
        // Kept, and flushed ahead of the next turn; this call is still the one the next
        // call resumes from (`#270 voice-transcript-markers`).
        keepUnrecorded(conversationId, body);
        deps.session.recordedAny = true;
        noteRecordingBehind(error);
      } else {
        setStorageState(`A record of this call was not kept: ${error.message}.`);
      }
    }
  });
}


// --- browsing the record -------------------------------------------------------------------------
//
// `#128 transcript-history`. `#48` above made the transcript durable; what came BACK was one
// conversation — the newest, whole, and nothing else. Two things were wrong with that, and they
// pull in opposite directions: the call before the newest one was unreachable from the interface
// entirely, and a single long call was restored in its entirety whether or not the reader ever
// scrolled to the top of it.
//
// So the record is walked the way the channel is walked, through `GET /api/v1/transcript`: the
// newest page of turns wherever they were said, a server cursor to step back from, and a chip
// that says more exists. One mechanism for both lists, `preservingScroll` included, because
// prepending above the viewport is the one mutation a browser's own scroll anchoring does not
// cover — see `#65 scrollback-paging` and `#47 scrollback-stability`.
//
// TWO THINGS THIS IS NOT, because both were asked about and both belong elsewhere:
//
//   * It sends NOTHING to the voice agent. Catching a new call up on what was said before is
//     `#46 conversation-replay`, which has its own switch and is off unless the operator turns
//     it on. Browsing the record costs nothing; replaying it spends tokens at a vendor, so the
//     two are deliberately not one feature.
//   * It does not decide what is KEPT. That is `storage.retain_days`, on the server.

/** What one step back asks for. The server clamps it, so this is a want and not a promise. */
const TRANSCRIPT_PAGE_LIMIT = 40;


/**
 * Is anything older than the topmost row still in the record?
 *
 * Three values, for the same reason `olderPages().moreAbove` has three: `true` there is more, `false`
 * the reader has reached the beginning of everything stored, `undefined` the server did not say.
 */
let transcriptMoreAbove: boolean | null = false;

let transcriptOlderCursor: string | null = null;

let transcriptFetchInFlight = false;


/**
 * Which conversation the TOPMOST restored row belongs to.
 *
 * A boundary is a property of the PAIR of rows on either side of it, so a step back has to know
 * what it is arriving above. Without this, two different calls are prepended flush against each
 * other and nothing on the screen says the agent below the join never heard the words above it —
 * which is the one claim this page draws rules to avoid making by accident.
 */
let oldestRestoredConversation: string | null = null;


// One wording for every boundary in the stored record (`STORED_SEAM_DETAIL`, held to the same word
// budget as the other seams) is web/src/core/call's.
const STORED_SEAM_LABEL = "earlier conversation";


/**
 * The nodes for a run of stored turns, oldest first, with a rule wherever the call changes.
 *
 * ONE builder for both paths — the first page goes at the end of an empty transcript, every step
 * back goes at the front of a full one — because two builders would be two definitions of where a
 * boundary belongs, and they would drift the first time one of them was touched.
 *
 * `trailingSeam` says whether a rule is owed BELOW the last turn here: at the first load it always
 * is, because whatever the live call says next is a different conversation from anything in the
 * record; on a step back it is owed only when the run ends in a different call from the row it is
 * landing above.
 *
 * `moreAbove` says whether the record holds anything older than this run, so a conversation that
 * starts at its head still has a boundary above it (`#270 voice-transcript-markers`).
 */
function restoredRun(turns: TranscriptTurn[], trailingSeam: boolean, moreAbove: boolean = false) {
  const rows: HTMLElement[] = [];
  const absorbed = absorbedAnswers(turns);
  // Drawn from one place on purpose. A boundary between two stored calls and a boundary between
  // the record and the live one are the same boundary, and a second call site here is how the
  // two wordings would come to differ. Each says which conversation it ends, as a live one does
  // (`#270 voice-transcript-markers`), so a late send is drawn above the right line.
  //
  // `#270 voice-transcript-markers`. A conversation that starts with its start marker says what the
  // live page said above it: "new conversation", the recap it was sent, and how the call before it
  // ended (`boundaryFor`). One without — recorded before markers — keeps "earlier conversation".
  const rule = (ends: string | null, start: RecordDetail | null = null) => {
    const li = transcriptSeam(STORED_SEAM_LABEL, STORED_SEAM_DETAIL, rows);
    if (ends !== null) li.setAttribute("data-ends", ends);
    if (start) {
      li.setAttribute("data-marker", "");
      if (start.previous_end) li.setAttribute("data-cause", String(start.previous_end));
      fillBoundary(li, boundaryFor(start.recap, start.previous_end));
    }
  };
  /** The start marker `turn` is, or null. */
  const startOf = (turn: TranscriptTurn) => (turn.speaker === "marker" && turn.detail && turn.detail.kind === "conversation_start"
    ? turn.detail
    : null);
  let previous = null;
  for (const turn of turns) {
    noteStored(turn.conversation_id, turn);
    if (previous !== null && turn.conversation_id !== previous) {
      rule(previous, startOf(turn));
    } else if (previous === null && moreAbove && startOf(turn)) {
      // At the head of the run, with older rows still to come: which conversation it ends is
      // filled in when they arrive (`loadOlderTurns`).
      rule(null, startOf(turn));
    }
    // `#270 voice-transcript-markers`. A message the voice agent posted is its frozen card, drawn
    // by the builder the live one is, and a draft that was not sent is the line the live page drew
    // for it, in the same words. A marker is not a turn: nothing else is drawn for one here.
    if (turn.speaker === "sent") {
      rows.push(sentCardNode(turn));
    } else if (turn.speaker === "marker") {
      const detail = turn.detail;
      if (detail && detail.kind === "not_sent") notSentSeam(String(detail.why), rows);
    } else {
      const li = turnNode(restoredSpeaker(turn.speaker), turn.text, turn.at_ms);
      // `#269 approval-card-inline`. A spoken yes that sent the card after it is drawn into that
      // card, as it was live: there, out of sight, and still in the record.
      if (absorbed.has(turn)) {
        li.setAttribute("data-absorbed", "");
        li.setAttribute("data-answer", String(turn.detail!.serial));
      }
      rows.push(li);
    }
    previous = turn.conversation_id;
  }
  if (previous !== null && trailingSeam) {
    rule(previous);
  }
  return rows;
}


/**
 * The sends approved by voice, as `conversation:serial`, whose yes a step back may still bring: a
 * page of the record can end between a yes and the card it sent. `#269 approval-card-inline`.
 */
let restoredWantAnswer: Set<string> = new Set();


/**
 * Which `you` rows of a run of stored turns are a spoken yes drawn into the card it sent: walking
 * from the newest, each `sent` row approved by voice claims the newest `answer`-tagged `you` row
 * before it, in its conversation, that names its serial. Only that one: an earlier yes to the same
 * draft, tagged and then stopped, sent nothing and is drawn as a turn.
 *
 * @param {unknown[]} turns oldest first
 * @returns {Set<unknown>}
 */
function absorbedAnswers(turns: TranscriptTurn[]) {
  const absorbed = new Set();
  const want = restoredWantAnswer;
  for (let at = turns.length - 1; at >= 0; at -= 1) {
    const turn = turns[at]!;
    const detail = turn.detail;
    if (!detail || typeof detail !== "object") continue;
    const key = `${turn.conversation_id}:${detail.serial}`;
    if (turn.speaker === "sent" && detail.how === "voice") {
      want.add(key);
    } else if (turn.speaker === "you" && detail.kind === "answer" && want.has(key)) {
      want.delete(key);
      absorbed.add(turn);
    }
  }
  return absorbed;
}


/** The query that asks a transcript read for record rows, when this server keeps them. `#270`. */
const recordsQuery = () => (transcriptRecordsSupported ? "&records=1" : "");


function renderOlderTurnsControl() {
  const button = deps.el("load-older-turns");
  button.hidden = transcriptMoreAbove !== true;
  button.disabled = transcriptFetchInFlight;
  button.textContent = transcriptFetchInFlight ? "Loading earlier turns…" : "Earlier turns";
}


/** Forget where the walk had got to. The rows it produced are gone, so its cursor is meaningless. */
function forgetTranscriptWalk() {
  transcriptMoreAbove = false;
  transcriptOlderCursor = null;
  oldestRestoredConversation = null;
  restoredWantAnswer = new Set();
  renderOlderTurnsControl();
}


/**
 * Put a bounded suffix of the stored record back on the screen, once, at sign-in.
 *
 * A failure here is NOT an error panel. The page works perfectly well without a store — that is
 * what it did until `#48` landed — and a server with no `storage.path` answers 503 by design, so
 * treating that as a fault would put a red banner on a correctly configured deployment.
 *
 * ONCE PER PAGE, and the flag is why. `signIn()` runs again whenever a token is saved, so pasting
 * a second token — or the same one twice — used to append the same conversation underneath
 * itself, with a second "earlier conversation" rule between the copies. Nothing about the screen
 * said the duplicate was a duplicate.
 */
let restoredStoredConversation = false;


/** Stored turns this page already has on screen or has filed itself, by server identity. `#1303 voice-transcript-share`. */
const seenStoredKeys: Set<string> = new Set();

/** A stored turn's identity across devices: its conversation, server stamp, speaker and words. */
function storedKey(conversationId: string, turn: { at_ms?: number; speaker?: string; text?: string }): string {
  return `${conversationId}:${turn.at_ms ?? ""}:${turn.speaker ?? ""}:${turn.text ?? ""}`;
}

/** Remember one stored turn, so a refresh from another device never draws it twice. */
function noteStored(conversationId: string, turn: { at_ms?: number; speaker?: string; text?: string }): void {
  seenStoredKeys.add(storedKey(conversationId, turn));
}

let transcriptRefreshInFlight = false;

/**
 * Pick up turns another device recorded while this page was open. `#1303
 * voice-transcript-share`: two devices, one transcript.
 *
 * The first read is still {@link loadStoredConversation}'s; this is every later one. It
 * asks for the newest page only, keeps the turns this page has not seen, and appends
 * them in the order they were said. It runs when the page is opened again — it becomes
 * visible, or takes focus — which is when the owner looks at the other device. A failure
 * is silent here: the record on screen is untouched, and Settings keeps the last word
 * the first read said.
 *
 * @returns how many turns arrived
 */
async function refreshStoredConversation(): Promise<number> {
  if (!restoredStoredConversation) {
    await loadStoredConversation();
    return 0;
  }
  if (deps.tokenScope === "read" || transcriptRefreshInFlight) return 0;
  transcriptRefreshInFlight = true;
  try {
    const payload = await deps.api(`/api/v1/transcript?limit=${TRANSCRIPT_PAGE_LIMIT}${recordsQuery()}`);
    const turns = oldestFirst<TranscriptTurn>(payload);
    const fresh = turns.filter((turn) => !seenStoredKeys.has(storedKey(turn.conversation_id, turn)));
    if (fresh.length === 0) return 0;
    const pinned = deps.atBottom(deps.el("scroll-area"));
    deps.el("transcript").append(...restoredRun(fresh, false));
    deps.renderEmptyState();
    deps.renderScrollTools();
    deps.followIfPinned(pinned, fresh.length);
    return fresh.length;
  } catch (_caught) {
    return 0;
  } finally {
    transcriptRefreshInFlight = false;
  }
}


/** What Settings says about the stored record when the saved token is read-scope. */
const READ_SCOPE_STORAGE_STATE =
  "Stored conversations need the write-scope token; this one may read channels only.";


let refreshListenersArmed = false;

/**
 * Arm the cross-device pickup on the first explicit read of the record, never at
 * construction (`#624 voice-page-modularity` keeps construction free of effects).
 * `#1303 voice-transcript-share`.
 */
function armRefreshOnOpen(): void {
  if (refreshListenersArmed) return;
  refreshListenersArmed = true;
  if (typeof document !== "undefined" && typeof document.addEventListener === "function") {
    document.addEventListener("visibilitychange", () => {
      if (!document.hidden && deps.currentView === "voice") void refreshStoredConversation();
    });
  }
  if (typeof window !== "undefined" && typeof window.addEventListener === "function") {
    window.addEventListener("focus", () => {
      if (deps.currentView === "voice") void refreshStoredConversation();
    });
  }
}

async function loadStoredConversation() {
  armRefreshOnOpen();
  if (restoredStoredConversation) {
    return;
  }
  // `#38 read-token-conversation-probe`. Stored conversations are write-scope by design, so a
  // read-scope page asking for them could only earn a 403 — and a console error on every load of
  // an otherwise working page. Say why there is nothing here instead of asking. The once-flag is
  // deliberately NOT set: saving the write-scope token afterwards signs in again, and then the
  // record is restored as usual.
  if (deps.tokenScope === "read") {
    setStorageState(READ_SCOPE_STORAGE_STATE);
    return;
  }
  restoredStoredConversation = true;
  // `#330 voice-long-turn-drop`. A turn a page that did not come back was in the middle of is filed
  // first, so the record read below holds it where it was said.
  try {
    const recovered = await deps.recoverUnsettledTurns();
    if (recovered > 0) {
      deps.addDetail(`filed ${recovered} turn${recovered === 1 ? "" : "s"} an earlier page was still saying when the app closed`);
    }
  } catch (_error) {
    // Left in storage for the next opening; restoring the record goes on without it.
  }
  // The LISTING is still read, and for two things the record itself cannot answer: how many
  // conversations are stored, which is the standing sentence in Settings; and which one a new
  // call would resume from, because `#46 conversation-replay` resumes A conversation and there is
  // no such thing as resuming "the last forty turns".
  let listing = null;
  try {
    listing = await deps.api("/api/v1/conversations");
  } catch (caught) {
  const error = caught as ApiRequestError;
    setStorageState(`No stored conversations available: ${error.message}`);
    return;
  }
  const conversations = (listing && listing.conversations) || [];
  if (conversations.length === 0) {
    setStorageState("Nothing stored yet. This screen is recorded from the next call onwards.");
    return;
  }
  let payload = null;
  try {
    payload = await deps.api(`/api/v1/transcript?limit=${TRANSCRIPT_PAGE_LIMIT}${recordsQuery()}`);
  } catch (caught) {
  const error = caught as ApiRequestError;
    setStorageState(`Could not read the stored transcript: ${error.message}`);
    return;
  }
  const turns = oldestFirst<TranscriptTurn>(payload);
  transcriptMoreAbove = deps.normaliseHasMore(payload.has_more);
  transcriptOlderCursor = payload.next_before || null;
  oldestRestoredConversation = turns.length > 0 ? turns[0]!.conversation_id : null;
  if (turns.length > 0) {
    deps.el("transcript").append(...restoredRun(turns, true, transcriptMoreAbove === true));
    deps.renderEmptyState();
    deps.renderScrollTools(); // the folds arrived with the rows they are attached to.
    // Entering a view lands on the NEWEST turn, which is what `line()` would have done for the
    // last of these had they been appended one at a time. A restored record opens at its end.
    deps.scrollToNewest();
  }
  renderOlderTurnsControl();
  // `#46 conversation-replay`. The conversation a new call would resume from is the most recent
  // the server holds. Set even when resuming is off, because the toggle can be turned on without
  // a reload and the answer must not then be "nothing to resume from".
  deps.resumeConversationId = conversations[0]!.id;
  deps.renderResumeState();
  setStorageState(
    `${conversations.length} conversation${conversations.length === 1 ? "" : "s"} stored on the ` +
      `server. Clear empties this screen only; Forget below is what erases them.`
  );
}


/**
 * One step further back through the record.
 *
 * Guarded against re-entry rather than debounced, exactly like the channel's walk: the automatic
 * trigger fires on every scroll event and a phone produces a great many of those.
 */
async function loadOlderTurns() {
  if (transcriptMoreAbove !== true || !transcriptOlderCursor || transcriptFetchInFlight) {
    return;
  }
  transcriptFetchInFlight = true;
  renderOlderTurnsControl();
  try {
    const payload = await deps.api(
      `/api/v1/transcript?limit=${TRANSCRIPT_PAGE_LIMIT}` +
        `&before=${encodeURIComponent(transcriptOlderCursor)}${recordsQuery()}`
    );
    const turns = oldestFirst<TranscriptTurn>(payload);
    // The step is OVER before the anchored mutation, so that every consequence of it — the rows,
    // the boundary and the chip's final state — is one change of height rather than three.
    transcriptMoreAbove = deps.normaliseHasMore(payload.has_more);
    transcriptOlderCursor = payload.next_before || null;
    transcriptFetchInFlight = false;
    if (turns.length === 0) {
      renderOlderTurnsControl();
      return;
    }
    const endsIn = turns[turns.length - 1]!.conversation_id;
    const list = deps.el("transcript");
    // `#270 voice-transcript-markers`. A list that starts with a conversation's start marker has
    // its boundary already: it ends whatever this step brings, and no second, generic one is drawn.
    const head = list.children[0];
    const markerHead = Boolean(head && head.hasAttribute("data-marker"));
    if (markerHead && head!.getAttribute("data-ends") === null) head!.setAttribute("data-ends", endsIn);
    const arriving = restoredRun(turns, endsIn !== oldestRestoredConversation && !markerHead, transcriptMoreAbove === true);
    oldestRestoredConversation = turns[0]!.conversation_id;
    deps.preservingScroll(() => {
      // Prepending, expressed the way the channel's walk expresses it: the rows already on screen
      // are handed back in place behind the arriving ones, in one mutation.
      list.replaceChildren(...arriving, ...list.children);
      // Inside the SAME anchored mutation, and this is the step nobody photographs: the LAST step
      // of the walk HIDES the chip, which is a sibling above the transcript inside the scrolling
      // element. Taking its height away afterwards jerks the reader by the height of a button on
      // the one step where they have finally arrived at the beginning.
      renderOlderTurnsControl();
    });
    deps.renderEmptyState();
    deps.renderScrollTools();
  } catch (caught) {
  const error = caught as ApiRequestError;
    // Said in Settings rather than on the call screen, for the same reason a failed recording is:
    // the owner may be in a car, and a step back that did not happen has cost them nothing.
    setStorageState(`Could not read further back: ${error.message}`);
  } finally {
    // The success path has already done both and doing them again is a no-op. This is for the
    // FAILURE path, where the step must stop reporting itself in flight.
    transcriptFetchInFlight = false;
    renderOlderTurnsControl();
  }
}


/** The reader has arrived at the top of what is loaded. Take the next step for them. */
function maybeLoadOlderTurns() {
  if (deps.currentView !== "voice" || transcriptMoreAbove !== true || transcriptFetchInFlight) {
    return;
  }
  if (deps.el("scroll-area").scrollTop > deps.OLDER_TRIGGER_PX) {
    return;
  }
  loadOlderTurns();
}


/** Erase every stored conversation. Leaves the screen exactly as it is, and says so. */
async function forgetConversations() {
  if (deps.tokenScope === "read") {
    // The server would refuse it; say so without asking and earning the 403.
    setStorageState(`Nothing was erased: ${READ_SCOPE_STORAGE_STATE}`);
    return;
  }
  try {
    const result = await deps.api("/api/v1/conversations", { method: "DELETE" });
    const count = (result && result.forgotten) || 0;
    setStorageState(
      `Erased ${count} stored conversation${count === 1 ? "" : "s"}. The lines still on the ` +
        `screen are unaffected — Clear is what empties those.`
    );
  } catch (caught) {
  const error = caught as ApiRequestError;
    setStorageState(`Nothing was erased: ${error.message}`);
  }
}
return { initializeHeardPage,
POST_CARD_STATUS,
renderPostExpiry,
get recordingBroken() { return recordingBroken; }, set recordingBroken(value: typeof recordingBroken) { recordingBroken = value; },
turnNode,
line,
seam,
transcriptSeam,
get conversationOpen() { return conversationOpen; }, set conversationOpen(value: typeof conversationOpen) { conversationOpen = value; },
noteConversationEnded,
get replayUnavailable() { return replayUnavailable; }, set replayUnavailable(value: typeof replayUnavailable) { replayUnavailable = value; },
get resumeViaProfile() { return resumeViaProfile; }, set resumeViaProfile(value: typeof resumeViaProfile) { resumeViaProfile = value; },
get lastEndCause() { return lastEndCause; }, set lastEndCause(value: typeof lastEndCause) { lastEndCause = value; },
recapFor,
noteCallRecap,
freshBecause,
renderCallRecap,
offerReplayAtSessionStart,
sendReplayFrame,
noteReplayAck,
armProfileRecap,
armRecapForCall,
watchProfileRecap,
withdrawProfileRecap,
abandonWatchedRecap,
withdrawArmedRecap,
REPLAY_ANSWER_WAIT_MS,
get startAwaitingAnswer() { return startAwaitingAnswer; }, set startAwaitingAnswer(value: typeof startAwaitingAnswer) { startAwaitingAnswer = value; },
endReplayAnswerWait,
expectStartMarker,
noteConversationNamed,
noteReplacedV1Session,
CLEAR_ARMED_MS,
disarmClear,
onClear,
get postWatch() { return postWatch; },
get transcriptRecordsSupported() { return transcriptRecordsSupported; }, set transcriptRecordsSupported(value: typeof transcriptRecordsSupported) { transcriptRecordsSupported = value; },
startPostWatch,
stopPostWatch,
showPostProposal,
placePostCard,
postCardBackToMain,
markCardBesideReply,
draftBelowView,
sendPostProposal,
cancelPostProposal,
flushRecordsAfterLive,
get spokenYes() { return spokenYes; },
reportAnswerOutcome,
extendRedraftWait,
resetSpokenYes,
echoCancellationRefused,
noteSessionStartedForSpokenYes,
hearAssistantForSpokenYes,
noteCutForSpokenYes,
noteTurnCompleteForSpokenYes,
judgeAnswerAtTurnEnd,
noteTypedForSpokenYes,
noteUserTextForSpokenYes,
hearUserForSpokenYes,
typedAnswerToCard,
noteYesCandidate,
releaseYesHold,
settleYesCandidate,
settleYesCandidates,
conversationIdFrom,
get recordQueue() { return recordQueue; }, set recordQueue(value: typeof recordQueue) { recordQueue = value; },
get heardEpoch() { return heardEpoch; }, set heardEpoch(value: typeof heardEpoch) { heardEpoch = value; },
get heardTyped() { return heardTyped; }, set heardTyped(value: typeof heardTyped) { heardTyped = value; },
reportHeard,
recordTurn,
loadStoredConversation,
refreshStoredConversation,
loadOlderTurns,
maybeLoadOlderTurns,
forgetConversations
};
}

export type VoiceTranscript = ReturnType<typeof createVoiceTranscript>;
