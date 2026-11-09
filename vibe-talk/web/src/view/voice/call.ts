import { relayTurn as formatRelayTurn, relayCount } from "../../present/call/index.ts";
import { recoveryMayRetry, outputRateFrom as speechOutputRateFrom } from "../../core/speech/index.ts";
// The voice call controller, moved from voice.js (#624 voice-page-modularity).
import type * as VibeTalk from "../../contract/index.ts";
import * as VibeTalkContract from "../../contract/index.ts";
import type { CallSession, LiveTurn, VoiceRow, Resume, ReconnectAttempt, Timer } from "./types.ts";
import { SPEECH_HOLD_CAP_MS_DEFAULT, SPEECH_HOLD_DBFS_DEFAULT, bufferDbfs, downsampleTo, floatToPcm16 } from "../../core/speech/index.ts";
import { LOST_TURN, RECONNECT_RECORDS_WAIT_MS, reconnectAsked as callReconnectAsked, reconnectFreshWords, reconnectSeamDetail, reconnectVerdict } from "../../core/call/index.ts";
import { childByClass } from "../../view/dom/index.ts";
import { CUT_OFF_MARK, answerWords, classifySpokenAnswer, closeReason, mergeHypothesis, shownSpoken, spokenText } from "../../present/call/index.ts";
import { serviceEndedSentence } from "../../present/notices/index.ts";
import { isNoise } from "../../present/rows/index.ts";

interface UnsettledCopy { conversation: string | null; saved_at: number; turns: {who: string; text: string; at: number}[]; }
interface Initiation { type: string; dynamic_variables?: {vibe_talk_resume?: string}; conversation_config_override?: {conversation: {text_only: boolean}}; }
interface HostedFrame { type: string; conversation_initiation_metadata_event?: {conversation_id?: string; agent_output_audio_format?: string}; audio_event?: {audio_base_64: string}; agent_response_event?: {agent_response?: string}; user_transcription_event?: {user_transcript?: string}; ping_event?: {event_id?: unknown}; mcp_tool_call?: {state?: string; tool_call_id?: string; tool_name?: string}; }

export interface CallPorts { socket: {open(url: string): WebSocket}; audio: {contextClass(): typeof AudioContext | undefined}; }
export interface VoiceCallDependencies {
  noteTypedForSpokenYes: () => void;
  setStatus: (text: unknown) => void;
  addDetail: (text: unknown) => void;
  transcriptSeam: (label: unknown, detail: unknown, into?: unknown) => HTMLLIElement;
  WebSocket: Pick<typeof WebSocket, "OPEN">;
  typedAnswerToCard: (text: unknown) => { kind: string; proposal: VibeTalk.PendingPost; reason?: undefined; } | { kind: string; proposal: VibeTalk.PendingPost; reason: string; };
  noteUserTextForSpokenYes: () => void;
  line: (who: unknown, text: unknown, atMs?: number) => VoiceRow;
  recordTurn: (who: string, text: string, options?: { answerOf?: import("./transcript.ts").YesHold | null; }) => void;
  reportHeard: (key: string, text: string) => void;
  heardTyped: number;
  el: <K extends string>(id: K) => K extends keyof import("../../view/dom/index.ts").ElementById ? import("../../view/dom/index.ts").ElementById[K] : HTMLElement;
  reportAnswerOutcome: (kind: "affirmative" | "negative" | "hint" | "other", outcome: { kind: string; proposal?: VibeTalk.PendingPost | null; reason?: string; }) => void;
  storedExactly: (key: unknown, encoded: unknown) => boolean;
  deviceStorage: () => import("../../ports/index.ts").StoragePort;
  fitGrowingBox: (field: HTMLElement) => void;
  leftEl: (id: string) => HTMLElement;
  keepBoxInView: (box: HTMLElement, area: HTMLElement) => void;
  currentScreen: string;
  currentView: string;
  /** `#1039 documents-read-parity`: the Documents reader (not its list) is the view showing. */
  docsReaderOpen: boolean;
  CANNED_PROMPTS: { key: string; button: string; field: string; title: string; text: string; said: string; }[];
  token: () => string;
  apiDecoded: <K extends keyof VibeTalk.Contract>(type: K, path: string, options?: { method?: string; body?: unknown; signal?: AbortSignal; }) => Promise<VibeTalk.Contract[K]>;
  readingMode: boolean;
  nowPlaying: unknown;
  pendingRead: unknown;
  navigator: Pick<Navigator, "wakeLock">;
  showError: (text: string, options?: { stay?: boolean; }) => void;
  callAudioRoute: {inputKind: string; outputKind: string} | null;
  redact: (text: unknown) => string;
  api: (path: string, options?: import("../../core/api/index.ts").ApiRequestOptions) => Promise<unknown>;
  clearError: () => void;
  guardQuietly: (fn: unknown) => (...args: unknown[]) => Promise<unknown>;
  conversationalVoice: { name: string; settings_url: unknown; instance_id: unknown; };
  setState: (name: unknown) => void;
  startAwaitingAnswer: { conversationId: string; start: { recap: unknown; previous_end?: string; }; } | null;
  fetchResume: () => Promise<Resume | null>;
  showDetail: (text: unknown) => void;
  micSettings: () => MediaTrackConstraints;
  openCallMicrophone: (mic: unknown) => Promise<MediaStream>;
  echoCancellationRefused: (stream: { getAudioTracks?: () => unknown[]; getTracks?: () => unknown[]; }) => boolean;
  browserPorts: () => CallPorts;
  applyCallOutput: (context: unknown) => Promise<void>;
  refreshAudioDeviceSelects: () => Promise<void>;
  renderAudioRouteReadout: () => Promise<void>;
  resumeViaProfile: boolean;
  armRecapForCall: (resume: Resume | null) => Promise<Resume | null>;
  dismissBanner: () => void;
  conversationOpen: boolean;
  startPostWatch: () => void;
  conversationIdFrom: (vendorId: unknown) => string;
  noteCallRecap: (recap: unknown) => void;
  recapFor: (resume: unknown, transport: "contextual_update" | "client_data" | "replay_frame" | "profile" | null) => { state: string; words: number | null; lines: number; dropped: number; transport: string; } | { state: string; why: string; dropped: number; };
  expectStartMarker: (conversationId: unknown, recap: unknown, previousEnd: unknown) => void;
  lastEndCause: unknown;
  handOverAgentPrompt: (initiation: unknown, handed: unknown) => { note: unknown; replaced: boolean; };
  renderAfterHandOver: () => void;
  noteConversationEnded: (cause?: string) => void;
  noteSessionStartedForSpokenYes: () => void;
  heardEpoch: number;
  noteConversationNamed: (previous: string | null) => void;
  spokenYes: { epoch: number; said: Map<string, import("./transcript.ts").SpokenYesTurn>; sinceUser: Map<string, { text: string; turn: number | null; at: number; }>; interposed: boolean; cut: boolean; done: boolean; completed: Map<number, { interrupted: boolean; playEnd: number; }>; answer: { key: string; turn: number | null; text: string; startedAt: number; } | null; answered: string | null; eligible: string | null; ineligible: string | null; ineligibleWhy: "earlier_draft" | "replaced_session"; settling: { handle: string; words: string; timer: unknown; proposal: VibeTalk.PendingPost; turn: number | null; replies: Set<string>; hold?: import("./transcript.ts").YesHold | null; } | null; lastShown: VibeTalk.PendingPost | null; redraft: { outcome: unknown; from: string | null; timer: unknown; } | null; askFirst: boolean; turns: number; holds: Map<string, import("./transcript.ts").YesHold>; };
  hearAssistantForSpokenYes: (key: string, text: string, turn?: number | null) => void;
  noteYesCandidate: (key: string, text: string, li: HTMLElement | null) => void;
  hearUserForSpokenYes: (key: string, text: string, final: boolean, turn?: number | null) => void;
  settleYesCandidate: (key: unknown) => void;
  noteCutForSpokenYes: (numbered?: boolean) => void;
  extendRedraftWait: () => void;
  atBottom: (area: unknown, slack?: number) => boolean;
  followIfPinned: (pinned: unknown, turns?: number, view?: string) => void;
  releaseYesHold: (hold: import("./transcript.ts").YesHold | null, kept?: boolean) => void;
  turnNode: (who: unknown, text: unknown, atMs?: number) => VoiceRow;
  applySearch: () => void;
  isFolded: (entry: unknown) => boolean;
  setFolded: (entry: unknown, folded: unknown) => void;
  flushRecordsAfterLive: () => void;
  recordingBroken: boolean;
  recordQueue: Promise<unknown>;
  abandonWatchedRecap: () => boolean;
  withdrawProfileRecap: (id: unknown) => void;
  endReplayAnswerWait: () => void;
  resetSpokenYes: (why?: "earlier_draft" | "replaced_session") => void;
  armProfileRecap: (conversationId: string) => Promise<{ armed: { profileRecap: number; included: number; dropped: number; words: number | null; } | null; why: string | null; dropped: number; }>;
  resumeAllowed: boolean;
  resumeWanted: () => boolean;
  sendReplayFrame: (resume: unknown) => void;
  watchProfileRecap: (id: number, settle: (served: boolean | null) => void, abandoned?: (served: boolean | null) => void) => void;
  freshBecause: (why: unknown) => "the voice service does not take a recap" | "the voice service has not said it takes a recap" | "this voice session takes no recap" | "resuming is off on this server" | "resuming is off" | "there was no earlier conversation to resume";
  renderCallRecap: () => void;
  REPLAY_ANSWER_WAIT_MS: number;
  noteReplacedV1Session: () => void;
  offerReplayAtSessionStart: (message: VibeTalk.VibeTalkV1ServerFrame) => void;
  openDocumentNotice?: () => string | null;
  noteReplayAck: (message: VibeTalk.VibeTalkV1ServerFrame) => void;
  noteTurnCompleteForSpokenYes: (interrupted: boolean, turn?: number | null) => void;
  judgeAnswerAtTurnEnd: (turn: number | null) => void;
  settleYesCandidates: () => void;
  stopPostWatch: () => void;
  withdrawArmedRecap: () => void;
  resumeLast: { attempted: boolean; armed: boolean; dropped: number; failed: boolean; words: unknown; acked: boolean; refused: boolean; unsupported: boolean; unsent: boolean; replaced: boolean; };
  renderResumeState: () => void;
  renderReadModeControl: () => void;
  renderReadTransport: () => void;
  readInProgress: () => boolean;
  readState: string;
  closeSpeedPopover: () => void;
  resumeNote: () => "the agent starts fresh" | "the agent starts fresh — the earlier conversation could not be read" | "the agent starts fresh — the voice service took no recap" | "the earlier conversation is replayed in part" | "the earlier conversation is replayed" | "the agent starts fresh — the earlier conversation was too long to replay" | "the agent starts fresh — there was nothing to replay";
  RELAY_MAX_CHARS: number;
  channelLabel: (channelId: unknown) => unknown;
  RELAY_PREAMBLE: string;
  RELAY_FENCE_OPEN: string;
  RELAY_FENCE_CLOSE: string;
  relayMode: () => string;
  RELAY_COALESCE_MS: number;
  hiddenEmpty: (message: unknown) => boolean;
  startWatchAll: () => void;
  stopWatchAll: () => void;
}

export function createVoiceCall(deps: VoiceCallDependencies) {


const session: CallSession = {
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
  playLead: 0.25, // seconds banked before an answer starts; grows after a mid-answer stall
  playing: [], // scheduled AudioBufferSourceNodes, so an interruption can cancel them
  protocol: "unknown",
  providerName: "voice provider",
  inputRate: 16000,
  outputRate: 16000,
  lastTranscriptKey: null,
  // `#48 transcript-storage`. The id the durable record is filed under. Taken from the vendor's
  // `conversation_id` when it arrives, so a transcript can be lined up against the vendor's own
  // record of the same call; invented locally otherwise, because a call that the vendor never
  // named still happened and still deserves to survive a reload.
  conversationId: null,
  // THIS CONVERSATION IS TYPED, and the microphone was never opened for it.
  //
  // Not a mode the reader can flip: it is decided when the socket opens and it is true for the
  // life of that conversation, because that is the only honest shape for it. Everything the flag
  // gates — no getUserMedia, no AudioContext, no capture graph, no playback — is a decision taken
  // before the socket exists. See `start()`.
  chat: false,
  // Whether the vendor sent audio anyway on a conversation we asked to be text-only. Recorded
  // rather than assumed: the text-only request is an OVERRIDE, and an agent whose dashboard
  // forbids overrides ignores it silently. The page must not claim a text-only conversation it did
  // not get, so this is what turns that claim into a reported fact. See `handle`.
  vendorSentAudioInChat: false,
  // State for the provider-neutral protocol. Its backend accepts one input/response turn at a
  // time, so microphone audio, an opening greeting, and typed prompts must be sequenced instead
  // of being written concurrently to the socket.
  v1Ready: false,
  capturePaused: false,
  captureResumeTimer: null,
  audioSegmentActive: false,
  waitingForGreeting: false,
  waitingForAudioEnd: false,
  typedTurnInFlight: false,
  // Assistant output has arrived since the last `turn_complete`: a turn is being answered, whether
  // or not the page asked for it. A typed call holds its next prompt behind it.
  replyArriving: false,
  // `#268 voice-call-drops`. Anything of a spoken turn — the owner's words or the assistant's —
  // has arrived since the last `turn_complete`, so the turn is still open: a spoken call holds its
  // next text turn behind it rather than closing the microphone's segment in the middle of it.
  spokenTurnOpen: false,
  // Bounds that hold: a turn that stops sending anything without completing is let go.
  spokenTurnTimer: null,
  // ...and one that keeps sending without completing is let go SPOKEN_TURN_HOLD_CAP_MS after the
  // hold began, however often its frames restarted the bound above.
  spokenTurnCapTimer: null,
  // When the open spoken turn opened, and when anything of it last arrived, by the wall clock. A
  // relayed message, which waits in the relay's own queue rather than behind these timers, is held
  // to the same bounds from them (`relayWait`).
  spokenTurnOpenedAt: 0,
  spokenTurnActiveAt: 0,
  // `#330 voice-long-turn-drop`. The owner's last words of the open turn arrived as a FINAL
  // transcript: he has finished, and the agent is working on its answer — perhaps a tool call before
  // it says anything — so a relayed message waits for that answer, not for him (`relayWait`).
  spokenTurnHeard: false,
  // The highest turn number a `turn_complete` has named, so a late frame of it opens no new turn.
  /** @type {number | null} */
  lastCompletedTurn: null,
  // Text turns waiting for the backend, oldest first. `kind` is who wants it said, which decides
  // whether it may cut an answer short (`#330 voice-long-turn-drop`): the reader's own typed words
  // may, through an announced `interrupt`; a relayed channel message or the page's own notice may
  // not, and waits for the answer to finish playing. `relays` are the channel messages a relayed
  // turn carries, for the line that says they were not read out if the call ends first (`#337
  // voice-relay-silent`).
  // `resent` marks the turn a lost session never started, sent again to the session that replaced it
  // (`#352 voice-reconnect-on-bridge-error`).
  /** @type {{text: string, kind: "typed" | "notice" | "relay", relays: {channel: string}[], resent?: boolean}[]} */
  pendingPrompts: [],
  // `#352 voice-reconnect-on-bridge-error`. The text turn on the wire, from its `prompt` until its
  // `turn_complete`: what it said and carried, the turn the protocol had completed before it went
  // (`after`), whether the bridge has echoed it back as a user transcript and under which turn
  // number (`echoed`, `turn`), and whether any of its answer has arrived (`answered`). A relayed turn
  // the call ends before answering is said not to have been read out (`endCallRelay`). `resent`: it is
  // a lost turn already sent again once, so a session that loses it too does not get it again
  // (`queueLostTurn`).
  /** @type {{text: string, kind: "typed" | "notice" | "relay", relays: {channel: string}[], resent: boolean, after: number | null, echoed: boolean, turn: number | null, answered: boolean} | null} */
  turnInFlight: null,
  // `#330 voice-long-turn-drop`. This page sent `interrupt` to make way for a typed message and the
  // turn it cut has not completed yet: that turn's late audio is not played.
  interruptPending: false,
  // The wait for an answer's audio to finish playing before a page message goes, or null.
  playbackWaitTimer: null,
  // Whether the connection details already say a page message is waiting out a long answer.
  pageMessageHeldNoted: false,
  // The reader's send just cut the answer short, and the status line says so rather than "Sent.".
  cutForTyped: false,
  // `#512 voice-recap-private-stack`. When the microphone last heard the owner at or above the
  // speech level, and when his transcript last grew, by the wall clock, or 0; the page message held
  // for his speech, from when, with the timer that looks again; and, for the call-end record, how
  // many were held, the longest wait, and how many went at the cap.
  micSpeechAt: 0,
  ownerWordsAt: 0,
  /** @type {{since: number, timer: ReturnType<typeof setTimeout> | null} | null} */
  speechHold: null,
  speechHolds: 0,
  speechHoldMaxMs: 0,
  speechHoldCapped: 0,
  // `#512`. The recap armed in the profile for the session this call opened, while its fate is
  // being asked: its number, the timer of the next question, and what to do with an answer that
  // comes after the call let the watch go.
  /** @type {{id: number, timer: ReturnType<typeof setTimeout> | null, abandoned: (served: boolean | null) => void} | null} */
  profileRecap: null,
  // `#512`. The number of a recap this call armed and has not settled or taken back, from the
  // moment it was armed, so a call that ends before its socket opens takes it back too.
  /** @type {number | null} */
  armedRecap: null,
  // When this call last sent a text turn, an `audio_end` and an `interrupt`, by the wall clock, or 0:
  // the call-end record says how long before the end each went (`postCallEnd`).
  lastTextAt: 0,
  lastAudioEndAt: 0,
  lastInterruptAt: 0,
  // `#15 transcript-dedup`. One row per (turn, role) for the life of the call; see `upsertSpoken`.
  liveTurns: new Map(),
  // `#649 voice-agent-tools`. Each turn's "no text received" row, by turn number, for its late text.
  noTextRows: new Map(),
  anonTurn: 0,
  // `#11 voice-connect-latency`. Content-free startup marks; see `markStartup`.
  timing: null,
  // `voice-unresponsive-signal`. What this call's turns have produced; see `beginHealth`.
  health: null,
  // `#264 voice-send-approval`. When the queued agent audio finishes playing, on the wall clock:
  // a spoken yes must come after the read-back was heard to the end. 0 when nothing was played.
  playEndMs: 0,
  // `#330 voice-long-turn-drop`. The same, for the assistant's own speech alone: the page's tone for
  // a yes's outcome is not an answer a page message has to wait out.
  answerPlayEndMs: 0,
  // Whether this call's microphone was opened with echo cancellation, read once when it opened.
  echoCancellation: false,
  // `#523 voice-self-hearing`. Whether the call's opening answer has played out of the speaker, so
  // that the microphone need no longer wait for it (`micHeldForAgent`).
  greetingPlayedOut: true,
  // `#523`. Whether the session said its agent speaks first (`greeting: true`).
  greetingPromised: false,
  // `#523`. What the assistant said in its latest turns: `{words, atMs}`, for `isAgentEcho`.
  agentSaid: new Map(),
  // `#523`. When the microphone last sent a buffer while the agent was audible; 0 for never.
  echoExposedAt: 0,
  // How many times the provider said this socket's session started; a second is a replacement.
  sessionStarts: 0,
  // `#1040 documents-voice-agent`. Whether this call already told its agent about the open
  // document: once per call, at its first session start, never again on a reconnect.
  documentNoticeSent: false,
  // `#331 resume-on-websocket`. On `vibe-talk-v1`, the record fetched for this call and not yet
  // offered: it goes at the first `session_started`, and only to a session that lists `replay`.
  pendingResume: null,
  // ...and what the bridge said to it: null, `waiting`, `accepted` or `refused`.
  /** @type {string | null} */
  recapAck: null,
  // ...and the line above the call its recap was said on when the call opened, null for none.
  /** @type {Element | null} */
  recapSeam: null,
  // `#352 voice-reconnect-on-bridge-error`. The reconnect in progress, from the voice service's
  // error until the new session starts, or null: the error's words, the turn it named, what is to go
  // again, and what the new session is to be handed. Hang up or teardown clears it, and the attempt
  // stops at its next step.
  /** @type {ReconnectAttempt | null} */
  reconnect: null,
  // ...how many times this call has been carried on a new session, and when the last one was asked
  // for, by the wall clock, or 0.
  reconnects: 0,
  lastReconnectAt: 0,
  // ...and the page's word on a send card that the lost session never answered: it goes to the new
  // session only once that session has taken this call's record, which says what the card was.
  /** @type {{item: {text: string, kind: "notice", relays: {channel: string}[]}, timer: ReturnType<typeof setTimeout>} | null} */
  noticeAwaitingRecap: null,
};


// --- typing to the agent ------------------------------------------------------------------------
//
// `#43 typed-input`. Sometimes speaking is not available — a quiet room, a name the transcriber
// keeps mangling, a commit hash — and the vendor's own client offers typing. So does this one, and
// it costs no second connection and no mode switch: `user_message` is a CLIENT EVENT on the
// conversation socket that is already open, documented as processed exactly like speech. A typed
// turn and a spoken turn are therefore the same thing to the conversation, and they land in the
// same transcript.
//
// Everything below is a named module-scope function rather than logic inside a click handler, and
// that is load-bearing: `#59 text-entry-button` moved this control into the control bar, `#241
// voice-composer` moved it again into the transcript, and `#60 canned-prompt-buttons` adds buttons
// that send a fixed sentence. Each is then one call, and none is a second implementation of the send
// path that can drift from this one.

/**
 * The ONE place a JSON client event is written to the conversation socket.
 *
 * Returns whether it went. A socket that exists is not a socket that is open — `readyState` is the
 * only thing that knows — and a send on a closing socket throws, which from a click handler would
 * reach the console and nowhere else.
 *
 * "ONE place" is now literally true, and until this was written it was not. This file carried a
 * SECOND `function sendClientEvent`, several thousand lines down, next to the Discord relay — and
 * because function declarations hoist, that later one silently won every call in the file,
 * including the ones directly under this comment. The two were not equivalent: this one tested
 * only `readyState`, the later one also required `session.connected`. Reading the wrong one gives
 * the wrong answer to "when does a frame actually go", and it did: `#73 mute-is-invisible`'s
 * connect-window announcement was first placed where this definition would have sent it and the
 * live one would not. The single definition below is the behaviour that was already running; only
 * the confusion is gone. A guard in `tests/js/voice_page.test.mjs` keeps the duplicate from
 * growing back.
 *
 * The per-frame `user_audio_chunk` send in `startCapture()` deliberately does NOT come through
 * here. It is a hot path called every 4096 samples, it holds the socket in a closure and does its
 * own `readyState` check, and routing it through a shared function would put a lookup and a branch
 * in the middle of the audio thread for no gain.
 */
/**
 * @param {{type: string, text?: string}} event
 * @param {"typed" | "notice" | "relay"} [kind] who wants a `user_message` said: the reader (typed or
 *   a canned prompt), the page (its word on a yes), or a channel (a relayed message). Only the
 *   reader's own words may cut an answer short. `#330 voice-long-turn-drop`.
 * @param {{channel: string}[]} [relays] the channel messages a relayed turn carries. `#337`.
 */
function sendClientEvent(event: { type: string; text?: string; }, kind: "typed" | "notice" | "relay" = "typed", relays: { channel: string; }[] = []) {
  if (!canSendText()) {
    return false;
  }
  // A text turn is the user's next turn as far as a spoken yes is concerned: the read-back it would
  // have answered is no longer the last thing said. `#264 voice-send-approval`.
  if (event.type === "user_message") {
    deps.noteTypedForSpokenYes();
  }
  if (session.protocol === "vibe-talk-v1") {
    if (event.type === "user_message") {
      session.pendingPrompts.push({ text: String(event.text), kind, relays });
      advanceVibeTalkInput();
    }
    // This protocol has no presence or contextual-update frame. Those events are advisory, so a
    // provider that cannot represent them still accepts the local action.
    return true;
  }
  if (event.type === "user_message") session.lastTextAt = Date.now();
  session.socket!.send(JSON.stringify(event));
  return true;
}


/**
 * `#1040 documents-voice-agent`. Tell this call's agent, once, that a document is open in the
 * reader: the page's notice (title, document_key, outline, fenced opening words), read lazily
 * through `openDocumentNotice` at the first session start — never at construction, when no
 * document is open yet, and never again on a reconnect's replacement session. On
 * `vibe-talk-v1` it queues as a `notice` prompt (it waits behind a greeting like any page word,
 * and ahead of a held first typed message, so the context lands before the question it answers);
 * on the hosted protocol the same text goes silently as a `contextual_update`, the frame the
 * recap already uses there. No document open, no notice: the call is exactly as it was.
 */
function offerDocumentAtSessionStart() {
  if (session.documentNoticeSent) return;
  const notice = deps.openDocumentNotice?.();
  if (!notice) return;
  session.documentNoticeSent = true;
  if (session.protocol === "vibe-talk-v1") {
    sendClientEvent({ type: "user_message", text: notice }, "notice");
  } else {
    sendClientEvent({ type: "contextual_update", text: notice });
  }
}


/**
 * Advance one queued provider-neutral text turn when the backend is ready for it.
 *
 * Voice calls keep one audio segment open while the microphone is live. Before sending typed text
 * we close that segment and wait for its `turn_complete`; after the typed response completes we
 * open a fresh segment and resume capture. This avoids asking the backend to run two response
 * turns at once. Text-only calls have no audio segment, but still serialize multiple quick sends.
 *
 * `#330 voice-long-turn-drop`. WHO WANTS IT SAID decides whether it may cut an answer short. A long
 * spoken summary was lost when a relayed channel message, held for thirty seconds behind it, went
 * out anyway as an `audio_end` — which a bridge takes for an interrupt, dropping the rest of the
 * answer's audio and its text. Now:
 *
 *   * the reader's own typed words (`typed`) cut in AT ONCE, and only through the protocol's own
 *     `interrupt`, which the page says on the screen (`interruptForTyped`); never by a silent
 *     `audio_end` into a turn that is still talking;
 *   * a relayed channel message (`relay`) or the page's word on a yes (`notice`) never cuts an
 *     answer: it waits for the turn to complete AND for its audio to finish playing, with no cap
 *     while the answer is still talking (`releaseSpokenTurnHold`, `waitForAnswerToPlay`). A relayed
 *     message has already waited out the open spoken turn in the relay's own queue (`relayWait`), so
 *     it reaches this queue with no turn open, or with one opened since.
 */
function advanceVibeTalkInput() {
  if (
    session.protocol !== "vibe-talk-v1" ||
    !session.v1Ready ||
    session.waitingForGreeting ||
    session.waitingForAudioEnd ||
    session.typedTurnInFlight ||
    session.pendingPrompts.length === 0
  ) {
    return;
  }
  if (session.chat && session.replyArriving) {
    // A reply the page did not ask for is still arriving: a greeting this typed call did not wait
    // for. Sent now, the prompt could not be told apart from it. Its `turn_complete` sends the
    // prompt; see `noteReplyArriving` for the bound on a reply that stops before then.
    armNoReply();
    return;
  }
  if (!session.chat && session.audioSegmentActive) {
    const next = session.pendingPrompts[0]!;
    if (next.kind === "typed" && !session.interruptPending && answerSpeaking()) {
      // `#330 voice-long-turn-drop`. The reader typed while the assistant was talking: the answer is
      // cut now, explicitly and said, and the message goes once the cut turn has completed.
      interruptForTyped();
    }
    if (session.spokenTurnOpen) {
      // `#268 voice-call-drops`. A spoken turn is still open — the owner is speaking, or the
      // assistant is answering — so its own `turn_complete` is still to come. Closing the segment
      // now would leave two of them in flight, and the page would take the turn's for the
      // segment's: it sent the prompt into a turn that was still running, then took the segment's
      // late `turn_complete` for the prompt's and opened the microphone over the answer. The
      // backend is built for one turn at a time. So the text waits for this turn to end; that
      // `turn_complete` sends it (`vibeTalkTurnComplete`), and a turn that goes quiet without one
      // is let go after SPOKEN_TURN_HOLD_MS, or one that never ends after SPOKEN_TURN_HOLD_CAP_MS
      // (`holdBehindSpokenTurn`) — but never, for a page message, while an answer is still talking.
      // `#512 voice-recap-private-stack`. A hold for his speech is over: the turn holds it now.
      endSpeechHold(false);
      holdBehindSpokenTurn();
      return;
    }
    if (next.kind !== "typed" && !answerHeardOut()) {
      // `#330 voice-long-turn-drop`. The answer's turn has completed but its audio is still playing,
      // a minute of it for a long summary: a page message waits until it has been heard to the end.
      endSpeechHold(false);
      waitForAnswerToPlay();
      return;
    }
    if (session.captureResumeTimer !== null) {
      // `#268 voice-call-drops`. A `turn_complete` has just arrived, and the bridge's next handler
      // is not ready across that hand-off (VOICE_CAPTURE_RESUME_MS): the microphone's frames wait it
      // out, and so does the segment's end. The hand-off's own timer sends it
      // (`deferVoiceCaptureAcrossTurn`). Without this, a message held behind a spoken turn went out
      // as `audio_end` in the same event-loop turn as the `turn_complete` that let it go.
      endSpeechHold(false);
      return;
    }
    // Never ahead of the reader's own typed words, which are not held: one waiting behind this
    // message lets it go too.
    const typedWaiting = session.pendingPrompts.some((prompt: { text: string; kind: "typed" | "notice" | "relay"; relays: { channel: string; }[]; resent?: boolean; }) => prompt.kind === "typed");
    if (next.kind !== "typed" && !typedWaiting && holdForOwnerSpeech()) {
      // `#512 voice-recap-private-stack`. No turn is open as far as the transcripts say, but the
      // microphone hears him, or his words grew a moment ago: the speech service may have dropped
      // his transcript. Closing the segment under him is how a call was lost; the hold's own timer
      // tries again (`holdForOwnerSpeech`).
      return;
    }
    endSpeechHold(false);
    session.capturePaused = true;
    session.waitingForAudioEnd = true;
    session.audioSegmentActive = false;
    sendVibeTalkFrame({ type: "audio_end" });
    deps.setStatus("Finishing the spoken turn…");
    return;
  }
  const next = session.pendingPrompts.shift()!;
  session.typedTurnInFlight = true;
  // `#352 voice-reconnect-on-bridge-error`. A typed message sent again after a reconnect: its words
  // are already on screen and in the record, so the bridge's echo of them is not drawn as the owner's
  // a second time. A relayed turn's echo, or the page's own notice's, is the turn in flight's
  // (`noteTurnInFlightEcho`, `#353 relay-echo-as-owner`).
  if (next.resent && next.kind === "typed") noteTypedTurn(next.text);
  session.turnInFlight = {
    text: next.text, kind: next.kind, relays: next.relays || [], resent: next.resent === true,
    after: session.lastCompletedTurn, echoed: false, turn: null, answered: false,
  };
  sendVibeTalkFrame({ type: "prompt", text: next.text });
  deps.setStatus("Waiting for the assistant…");
  armNoReply();
}


/**
 * How long a spoken turn that has stopped sending anything, without completing, may hold the page's
 * next text turn. `#268 voice-call-drops`. Restarted by every piece of the turn that arrives, so an
 * answer that is still talking is not cut off by it; this bounds one that went quiet.
 */
const SPOKEN_TURN_HOLD_MS = 8000;


/**
 * The longest a spoken turn may hold the page's next text turn at all, measured from when the hold
 * began and never restarted. `#268 voice-call-drops`. Without it a turn that keeps sending and never
 * completes — a steady trickle of hypotheses of background speech, a bridge that stops sending
 * `turn_complete` — held a typed message, or the page's word on a yes, for as long as it went on.
 *
 * `#330 voice-long-turn-drop`. Not over an ANSWER. An answer longer than thirty seconds is ordinary
 * — a summary of a night's messages — and this cap is what cut one off: a relayed message went out
 * as `audio_end` thirty seconds into it. A page message now holds on while the assistant is talking,
 * and a typed one has cut the answer explicitly long before this runs out; the cap still ends a hold
 * on the owner's own open turn, where an `audio_end` closes nothing but the microphone's segment.
 * That holds for a relayed channel message too, which waits in the relay's own queue and is held to
 * both bounds from the turn's own times (`relayWait`).
 */
const SPOKEN_TURN_HOLD_CAP_MS = 30000;


/**
 * After an answer's audio has finished playing, how long a page message — a relayed channel message,
 * the page's word on a yes — still waits before it goes. `#330 voice-long-turn-drop`. A breath after
 * the last word, so the owner can answer the answer first: his first word opens a spoken turn, which
 * the message then waits behind.
 */
const ANSWER_PLAYED_GRACE_MS = 600;


// --- the owner's own speech, heard by the microphone ------------------------------------------
//
// `#512 voice-recap-private-stack`. The page judged "the owner is speaking" only from his transcripts.
// A speech service that drops them — one dropped every transcript of speech begun after a turn
// completed, until the next turn started — leaves the page believing nobody is talking, and it then
// closed the microphone's segment mid-sentence to deliver a relayed message: the service took the
// message and never answered it, and the call was lost. So the page also listens to the microphone
// itself: the RMS of each capture buffer, against the level the server names (`speech_hold`), and
// whether his transcript grew in the last moments. While either says he is speaking, a page message
// — a relayed chat message, the page's word on a send card — waits, never longer than the server's
// cap, so road noise above the level cannot keep a relay from ever going. The reader's own typed
// words are not held: he chose to send them.
//
// The level and cap when the server names none (SPEECH_HOLD_DBFS_DEFAULT, SPEECH_HOLD_CAP_MS_DEFAULT),
// reading the server's (speechHoldFrom) and one capture buffer's level (bufferDbfs) are in
// web/src/core/speech/ (`#624 voice-page-modularity`).

/**
 * How long one capture buffer at or above the level keeps the owner "speaking": the pause between two
 * words, so a held message does not slip out between them.
 */
const MIC_SPEECH_HANGOVER_MS = 600;


/** How long after a partial transcript of his last grew the owner is still taken to be speaking. */
const OWNER_WORDS_HOLD_MS = 1500;


/** How often a held message looks again whether he has stopped. */
const SPEECH_HOLD_RECHECK_MS = 250;


/**
 * The level at or above which the microphone is taken to hear the owner, in dBFS, and the longest one
 * page message waits for him, in milliseconds (0: never). From client-config.
 *
 * @type {{levelDbfs: number, capMs: number}}
 */
let speechHold: { levelDbfs: number; capMs: number; } = { levelDbfs: SPEECH_HOLD_DBFS_DEFAULT, capMs: SPEECH_HOLD_CAP_MS_DEFAULT };


/**
 * A capture buffer arrived: note when the microphone last heard the owner at or above the level.
 *
 * @param {Float32Array} samples
 */
function noteMicLevel(samples: Float32Array<ArrayBufferLike>) {
  if (bufferDbfs(samples) >= speechHold.levelDbfs) session.micSpeechAt = Date.now();
}


/** Does the microphone, or his transcript, say the owner is speaking now? Never while muted. */
function ownerSpeakingLocally() {
  if (session.muted) return false;
  const now = Date.now();
  return (session.micSpeechAt > 0 && now - session.micSpeechAt < MIC_SPEECH_HANGOVER_MS) ||
    (session.ownerWordsAt > 0 && now - session.ownerWordsAt < OWNER_WORDS_HOLD_MS);
}


/**
 * Hold the page message at the head of the queue while the owner is speaking, up to the cap from when
 * the hold began. Answers whether it is held; a hold that ends is said by `endSpeechHold`.
 */
function holdForOwnerSpeech() {
  if (speechHold.capMs <= 0 || !ownerSpeakingLocally()) return false;
  const now = Date.now();
  const held = session.speechHold;
  if (held && now - held.since >= speechHold.capMs) {
    endSpeechHold(true);
    return false;
  }
  const since = held ? held.since : now;
  if (!held) deps.setStatus("Waiting for you to finish speaking…");
  if (held && held.timer !== null) clearTimeout(held.timer);
  const socket = session.socket;
  const timer = setTimeout(() => {
    const hold = session.speechHold;
    if (hold) hold.timer = null;
    if (session.socket !== socket) return;
    advanceVibeTalkInput();
  }, Math.max(1, Math.min(SPEECH_HOLD_RECHECK_MS, since + speechHold.capMs - now)));
  session.speechHold = { since, timer };
  return true;
}


/**
 * The page message held for the owner's speech goes: count it for the call-end record, and say in the
 * connection details how long it waited, and whether the cap let it go. Nothing when none was held.
 *
 * @param {boolean} capped whether the cap ended the hold with the microphone still hearing him
 */
function endSpeechHold(capped: boolean) {
  const held = session.speechHold;
  if (!held) return;
  session.speechHold = null;
  if (held.timer !== null) clearTimeout(held.timer);
  const waited = Math.max(0, Date.now() - held.since);
  session.speechHolds += 1;
  session.speechHoldMaxMs = Math.max(session.speechHoldMaxMs, waited);
  if (capped) session.speechHoldCapped += 1;
  deps.addDetail(
    capped
      ? `a page message waited ${Math.round(speechHold.capMs / 1000)} s, the most it may, for you to ` +
        "finish speaking; the microphone still heard speech, so it went anyway"
      : `a page message waited ${(waited / 1000).toFixed(1)} s for you to finish speaking`
  );
}


/** Let a hold go without the message going: the session it was for has ended. */
function dropSpeechHold() {
  const held = session.speechHold;
  if (held && held.timer !== null) clearTimeout(held.timer);
  session.speechHold = null;
}


/** Is the assistant's audio still being played? Nothing played on a typed call or with Sound off. */
function answerAudible() {
  return Date.now() < (session.answerPlayEndMs || 0);
}


/** Has the last answer been heard to its end, and the breath after it passed? */
function answerHeardOut() {
  return Date.now() >= (session.answerPlayEndMs || 0) + ANSWER_PLAYED_GRACE_MS;
}


/**
 * Is an answer being spoken: output of the open turn has arrived and it has not completed, or its
 * audio is still playing? `#330 voice-long-turn-drop`.
 */
function answerSpeaking() {
  return (session.spokenTurnOpen && session.replyArriving) || answerAudible();
}


/** Wait for the answer's audio to finish playing, then try the queued text turn again. */
function waitForAnswerToPlay() {
  if (session.playbackWaitTimer !== null) return;
  const socket = session.socket;
  const wait = Math.max(0, (session.answerPlayEndMs || 0) + ANSWER_PLAYED_GRACE_MS - Date.now());
  session.playbackWaitTimer = setTimeout(() => {
    session.playbackWaitTimer = null;
    if (session.socket !== socket) return;
    advanceVibeTalkInput();
  }, wait);
}


/**
 * Cut the answer that is being spoken, to make way for the reader's typed message: stop playing it,
 * keep its words so far marked as cut off, send the protocol's `interrupt` when its turn is still
 * running at the far side, and say so — on the transcript, the status line and in the connection
 * details. `#330 voice-long-turn-drop`.
 *
 * The protocol answers an `interrupt` with exactly one interrupted `turn_complete`, which is what
 * the message then waits for (`holdBehindSpokenTurn`); until it comes, late audio of the cut turn is
 * not played (`interruptPending`). An answer whose turn had already completed and was only still
 * playing needs no frame at all: nothing is running at the far side.
 */
function interruptForTyped() {
  const running = session.spokenTurnOpen && session.replyArriving;
  stopPlayback();
  markAnswerCut();
  // Filed now, ahead of the message that cut it, as it was said ahead of it. A late frame of the cut
  // turn still corrects its row, not its record.
  settleSpoken();
  if (running) {
    session.interruptPending = true;
    sendVibeTalkFrame({ type: "interrupt" });
  }
  deps.transcriptSeam(
    "answer cut off",
    "You sent a message while the assistant was answering, so the page stopped the answer to send " +
      "it. What it said up to there is kept above."
  );
  deps.setStatus("You cut the answer short to send your message.");
  session.cutForTyped = true;
  deps.addDetail("the page interrupted the assistant's answer to send a typed message");
}


/** Wait for the open spoken turn to end before closing the segment; see `advanceVibeTalkInput`. */
function holdBehindSpokenTurn() {
  if (session.spokenTurnTimer === null) armSpokenTurnHold();
  if (session.spokenTurnCapTimer !== null) return;
  const socket = session.socket;
  session.spokenTurnCapTimer = setTimeout(() => {
    session.spokenTurnCapTimer = null;
    releaseSpokenTurnHold(
      socket,
      "cap",
      `a spoken turn held the page's next message for ${Math.round(SPOKEN_TURN_HOLD_CAP_MS / 1000)} s ` +
        "without completing; the message went anyway"
    );
  }, SPOKEN_TURN_HOLD_CAP_MS);
}


function armSpokenTurnHold() {
  if (session.spokenTurnTimer !== null) clearTimeout(session.spokenTurnTimer);
  const socket = session.socket;
  session.spokenTurnTimer = setTimeout(() => {
    session.spokenTurnTimer = null;
    releaseSpokenTurnHold(
      socket,
      "quiet",
      `a spoken turn went quiet for ${Math.round(SPOKEN_TURN_HOLD_MS / 1000)} s without completing; ` +
        "the page's next message went anyway"
    );
  }, SPOKEN_TURN_HOLD_MS);
}


/**
 * Let the held text go without the turn's end — it went quiet, or held it too long — as it went
 * before the page waited, and say so, with `why`, where the call's other mechanics are recorded.
 *
 * `#330 voice-long-turn-drop`. Except where letting it go would cut an answer that is still talking.
 * A page message holds on while the assistant's audio is playing, and while its output of the open
 * turn is still arriving: the cap does not end that hold, since an answer longer than the cap is
 * ordinary. Only the turn's `turn_complete`, and then the end of its audio, lets it go — or the turn
 * going quiet, nothing of it arriving for SPOKEN_TURN_HOLD_MS with nothing left to play, which is a
 * stalled turn and no longer an answer being spoken. A typed message cuts the answer through
 * `interrupt`, said, and waits for that.
 *
 * @param {WebSocket | null} socket the call the hold was for
 * @param {"quiet" | "cap"} bound which of the two bounds ran out
 * @param {string} why
 */
function releaseSpokenTurnHold(socket: WebSocket | null, bound: "quiet" | "cap", why: string) {
  if (session.socket !== socket || !session.spokenTurnOpen) return;
  clearSpokenTurnHold();
  const next = session.pendingPrompts[0]!;
  if (next && next.kind !== "typed" && (answerAudible() || (bound === "cap" && session.replyArriving))) {
    if (bound === "cap" && !session.pageMessageHeldNoted) {
      session.pageMessageHeldNoted = true;
      deps.addDetail(
        "a page message has waited " +
          `${Math.round(SPOKEN_TURN_HOLD_CAP_MS / 1000)} s for the assistant's answer to finish, and waits on`
      );
    }
    holdBehindSpokenTurn();
    return;
  }
  if (next && next.kind === "typed" && !session.interruptPending && answerSpeaking()) {
    deps.addDetail(why);
    // Cuts the answer, and holds again for the interrupted turn's `turn_complete`.
    advanceVibeTalkInput();
    return;
  }
  letSpokenTurnGo(why);
  advanceVibeTalkInput();
}


/**
 * Stop waiting for the open spoken turn's `turn_complete`, which a bound says is not coming, and say
 * why where the call's other mechanics are recorded. The next text turn then closes the segment.
 *
 * @param {string} why
 */
function letSpokenTurnGo(why: string) {
  clearSpokenTurnHold();
  session.spokenTurnOpen = false;
  session.spokenTurnHeard = false;
  deps.addDetail(why);
}


/** Nothing is waiting on a spoken turn any more: both of the hold's bounds go. */
function clearSpokenTurnHold() {
  if (session.spokenTurnTimer !== null) {
    clearTimeout(session.spokenTurnTimer);
    session.spokenTurnTimer = null;
  }
  if (session.spokenTurnCapTimer !== null) {
    clearTimeout(session.spokenTurnCapTimer);
    session.spokenTurnCapTimer = null;
  }
}


/**
 * Something of a spoken turn arrived — the owner's words, the assistant's, or its audio — so the turn
 * is open until its `turn_complete`. A frame of a turn that has ALREADY completed (a late final
 * transcript) opens nothing.
 *
 * @param {number | null} [turn] the frame's turn number, when it has one
 * @returns {boolean} whether the frame counted as part of the open turn
 */
function noteSpokenTurnActivity(turn: number | null = null) {
  if (session.chat || session.protocol !== "vibe-talk-v1") return false;
  if (turn !== null && session.lastCompletedTurn !== null && turn <= session.lastCompletedTurn) return false;
  const now = Date.now();
  if (!session.spokenTurnOpen) {
    session.spokenTurnOpenedAt = now;
    session.spokenTurnHeard = false;
  }
  session.spokenTurnActiveAt = now;
  session.spokenTurnOpen = true;
  // A text turn is waiting on this one, which is still talking: give it the whole bound again.
  if (session.spokenTurnTimer !== null) armSpokenTurnHold();
  return true;
}


/**
 * The owner's words of a spoken turn arrived: the turn is open, and whether he has finished is what
 * this transcript says — a final one says he has, a hypothesis that he is speaking again.
 * `#330 voice-long-turn-drop`; see `spokenTurnHeard`.
 *
 * @param {number | null} turn the frame's turn number, when it has one
 * @param {boolean} final
 */
function noteOwnerWords(turn: number | null, final: boolean) {
  if (noteSpokenTurnActivity(turn)) session.spokenTurnHeard = final;
}


/** A `turn_complete`: no spoken turn is open, and nothing is waiting on one. */
function closeSpokenTurn(turn: number | null = null) {
  session.spokenTurnOpen = false;
  session.spokenTurnHeard = false;
  session.interruptPending = false;
  session.pageMessageHeldNoted = false;
  if (turn !== null) {
    session.lastCompletedTurn = Math.max(session.lastCompletedTurn ?? turn, turn);
  }
  clearSpokenTurnHold();
}


/**
 * Output of a turn in progress arrived: PCM, or assistant text.
 *
 * While a typed call holds a prompt behind that turn, each piece restarts the no-reply bound, so
 * the bound measures the turn going quiet without completing. A typed call plays none of the PCM
 * and so never judges it as heard; without the restart, a greeting still speaking after the bound
 * would be reported as no reply, and one that stalled after its next transcript never would.
 *
 * @param {number | null} [turn] the turn the output belongs to, when the frame says
 * @param {boolean} [text] whether it is assistant text rather than PCM
 */
function noteReplyArriving(turn: number | null = null, text: boolean = false) {
  session.replyArriving = true;
  noteSpokenTurnActivity(turn);
  noteTurnInFlightAnswer(turn, text);
  if (session.chat && !session.typedTurnInFlight && session.pendingPrompts.length > 0) {
    armNoReply();
  }
}


/**
 * `#352 voice-reconnect-on-bridge-error`. Output that may answer the text turn in flight arrived. An
 * assistant transcript numbered for a turn the protocol completed before the prompt went, or before
 * the turn the bridge echoed it under, is the late tail of that earlier turn, not this one's answer.
 * PCM carries no turn number, so it counts only once the bridge has echoed the prompt: a bridge
 * accepts a prompt and echoes it before it answers, and audio before that may be the last of an
 * interrupted turn.
 *
 * @param {number | null} turn the turn the output belongs to, when the frame says
 * @param {boolean} text whether it is assistant text rather than PCM
 */
function noteTurnInFlightAnswer(turn: number | null, text: boolean) {
  const inFlight = session.turnInFlight;
  if (!inFlight || inFlight.answered) return;
  if (text) {
    if (turn !== null && inFlight.after !== null && turn <= inFlight.after) return;
    if (turn !== null && inFlight.turn !== null && turn < inFlight.turn) return;
  } else if (!inFlight.echoed) {
    return;
  }
  inFlight.answered = true;
}


/**
 * `#352 voice-reconnect-on-bridge-error`. A user transcript arrived: the bridge's echo of the text
 * turn in flight, the first time its words come back whole and final. It names the turn the bridge
 * gave the prompt, which is the turn a bridge that loses it names in its error.
 *
 * @param {string} said
 * @param {boolean} final
 * @param {number | null} turn
 * @returns {boolean} whether it was that echo
 */
function noteTurnInFlightEcho(said: string, final: boolean, turn: number | null) {
  const inFlight = session.turnInFlight;
  if (!inFlight || inFlight.echoed || !final || said.trim() !== inFlight.text.trim()) return false;
  if (turn !== null && inFlight.after !== null && turn <= inFlight.after) return false;
  inFlight.echoed = true;
  inFlight.turn = turn;
  return true;
}


/**
 * One `vibe-talk-v1` control frame onto the conversation socket.
 *
 * Every JSON frame this page sends on that protocol comes through here, typed against the
 * generated contract, so the checker refuses a frame type or field the protocol does not define.
 * Readiness stays with the callers, which already know which state the session is in. When each
 * text turn, `audio_end` and `interrupt` went is noted for the call-end record (`postCallEnd`).
 *
 * @param {VibeTalk.VibeTalkV1ClientFrame} frame
 * @param {boolean} [counted] false for a goodbye's frames, which are not what ended anything
 */
function sendVibeTalkFrame(frame: VibeTalk.VibeTalkV1ClientFrame, counted: boolean = true) {
  const now = Date.now();
  // A goodbye's `audio_end` is not what a record after it is asking about, so it is not noted.
  if (counted) {
    if (frame.type === "prompt") session.lastTextAt = now;
    else if (frame.type === "audio_end") session.lastAudioEndAt = now;
    else if (frame.type === "interrupt") session.lastInterruptAt = now;
  }
  session.socket!.send(JSON.stringify(frame));
}


/** Is there a live conversation for a client event or typed text to reach? */
function canSendText() {
  return Boolean(
    session.socket && session.connected && session.socket.readyState === deps.WebSocket.OPEN
  );
}


// The vendor does not document whether a typed `user_message` is echoed back as a
// `user_transcript` the way speech is. If it is, the same sentence would appear twice: once because
// this page rendered it at the moment it was sent, and once when the echo arrives. So a typed turn
// is remembered briefly and a transcript matching it inside that window is dropped.
//
// UNVERIFIED, and stated as such rather than presented as knowledge: settling it costs one billed
// run of `scripts/run.sh --smoke-agent`, whose `converse()` already sends `user_message`, and that
// run has not been made. The window is a guess. It is short enough that a reader who genuinely says
// the same sentence twice a minute later still sees both.
const TYPED_ECHO_WINDOW_MS = 10000;


// A LIST, not one slot. Two typed turns inside the window used to leave the first duplicable: the
// second overwrote it, so the vendor's echo of the first matched nothing and was rendered a second
// time AND recorded to the server twice. Tapping Sumry and then Blockers is two sends in a second,
// so this is the ordinary case rather than a corner.
let recentTyped: {text: string; at: number}[] = [];


function noteTypedTurn(said: string) {
  const now = Date.now();
  recentTyped = recentTyped.filter((t) => now - t.at < TYPED_ECHO_WINDOW_MS);
  recentTyped.push({ text: said, at: now });
}


/** Would this arriving transcript be the echo of something just typed? */
function isEchoOfTyped(said: string) {
  const now = Date.now();
  const want = said.trim();
  // Consume the match: an echo answers for exactly one send, so saying the same sentence twice on
  // purpose still shows twice.
  const i = recentTyped.findIndex(
    (t) => t.text === want && now - t.at < TYPED_ECHO_WINDOW_MS
  );
  if (i === -1) {
    return false;
  }
  recentTyped.splice(i, 1);
  return true;
}


/**
 * Say something to the agent in writing, on the conversation that is open. Returns whether it went.
 *
 * REFUSING IS VISIBLE. A tap that does nothing at all is the failure this whole page is written
 * against, and with no conversation open there is nothing for the text to reach — so it says which
 * control would fix that, and it does not throw away what was typed. The message box never reaches
 * this refusal: with nothing open it starts a conversation instead (`sendTyped`). The canned prompts
 * still can, and are greyed out for exactly that reason (`renderCannedPrompts`).
 */
/**
 * @param {{fromComposer?: boolean}} [options] Clearing the field is only correct when the field is
 *   where the text came from. A canned prompt that wiped a half-written message would destroy work
 *   the page explicitly promises to keep, and it is one tap away from the composer at all times.
 */
function sendUserMessage(text: unknown, options?: { fromComposer?: boolean; }) {
  const fromComposer = Boolean(options && options.fromComposer);
  const said = String(text === null || text === undefined ? "" : text).trim();
  if (said === "") {
    return false;
  }
  // A canned prompt pressed while the box's first message waits for its conversation. The socket
  // may already be open, but the conversation is not accepted yet, and this would go ahead of the
  // message the reader sent first. `deliverHeldTurn` empties the slot before it calls this.
  if (heldTurn !== null) {
    deps.setStatus("Your message is still waiting for the conversation to open — send this after it.");
    return false;
  }
  // A typed yes to the card never sends, but it is answered. Judged before it goes, because
  // sending it forgets what the assistant said before it. `#264 voice-send-approval`.
  const answerToCard = deps.typedAnswerToCard(said);
  if (!canSendText() || !sendClientEvent({ type: "user_message", text: said })) {
    deps.setStatus("Start a call first — typed messages reach the agent in a live conversation.");
    return false;
  }
  deps.noteUserTextForSpokenYes();
  // Rendered HERE rather than waited for: the vendor may or may not echo it (see above), and a
  // turn that only appears if the vendor chooses to reflect it is a turn that can silently vanish.
  deps.line("you", said);
  deps.recordTurn("you", said);
  deps.reportHeard(`typed:${++deps.heardTyped}`, said);
  noteTypedTurn(said);
  if (fromComposer) {
    deps.el("compose-text").value = "";
    rememberComposeDraft();
  }
  // A send that cut the answer short keeps saying so (`interruptForTyped`), rather than "Sent.".
  if (!session.cutForTyped) deps.setStatus("Sent.");
  session.cutForTyped = false;
  if (answerToCard) deps.reportAnswerOutcome(classifySpokenAnswer(said), answerToCard);
  return true;
}


// How often composing pings the agent. The vendor documents `user_activity` as resetting the turn
// timeout without touching conversation content, which is exactly the complaint it answers: someone
// typing is PRESENT, and without this the agent reads the silence as absence and starts asking
// whether anyone is still there.
const ACTIVITY_INTERVAL_MS = 30000;


let lastActivityAt = 0;


/** A keystroke. Tell the agent someone is there, at most once every interval. */
function noteComposing() {
  if (!canSendText()) {
    return false;
  }
  const now = Date.now();
  if (now - lastActivityAt < ACTIVITY_INTERVAL_MS) {
    return false;
  }
  // Sent BEFORE the clock is advanced only if it actually went: a frame that never reached a
  // half-closed socket must not silence the next thirty seconds of pings.
  if (!sendClientEvent({ type: "user_activity" })) {
    return false;
  }
  lastActivityAt = now;
  return true;
}


// `#193 compose-draft-loss`. What is typed into the call composer survives a reload or a killed
// PWA, as the channel and reply composers already do. Cleared only by a send that went.
const COMPOSE_DRAFT_KEY = "vibe-talk.voice.compose-draft";


function rememberComposeDraft() {
  const text = deps.el("compose-text").value;
  try {
    if (text.trim()) {
      if (!deps.storedExactly(COMPOSE_DRAFT_KEY, text)) {
        deps.setStatus("This browser could not save what you typed. Keep this page open until you send it.");
      }
    } else {
      deps.deviceStorage().removeItem(COMPOSE_DRAFT_KEY);
    }
  } catch (_error) {
    // Storage refused outright: the text is still in the field, which is all that can be kept.
  }
  // `#340 composer-grow-box`. As typed, and as a send clears it.
  deps.fitGrowingBox(deps.leftEl("compose-text"));
}


function storedComposeDraft() {
  try {
    return deps.deviceStorage().getItem(COMPOSE_DRAFT_KEY) || "";
  } catch (_error) {
    return "";
  }
}


// --- the message box ----------------------------------------------------------------------------
//
// `#241 voice-composer`. The owner: "Type mode on voice agent takes away my Voice/Channel toggle ...
// Can we just have a message box at the BOTTOM of the scroll history? Is there any problem with that
// rather than an explicit Type toggle? I.e. do we need to explicitly engage some alternate mode of
// the remote voice agent? ... we could always do that belatedly either when you start typing or when
// you send a text message."
//
// No mode has to be engaged to TYPE. During a call a typed turn is one more client event on the
// socket that is already open (`sendClientEvent`): `user_message` on the hosted vendor's protocol,
// and on `vibe-talk-v1` a `prompt`, sent once the spoken turn in progress has been closed. What
// IS chosen when a conversation opens is whether it is spoken or typed — a typed one never asks for
// the microphone, and on the hosted vendor's protocol it also asks for text-only replies — so with
// nothing open the box starts a typed conversation, and it does so BELATEDLY, on the first send.
// Not on focus and not on typing: clicking into the box, or writing a draft in it, opens no
// connection and bills nothing.
//
// It replaces `#59 text-entry-button`, a Type toggle that turned the control bar into a text field
// and so took the Voice/Channel switch, the gear and the rest of the bar away while you typed. The
// box is in the call's transcript now, the last thing in the scrolling list as the channel's box is,
// and the bar does not change when you write.

/**
 * The first message, waiting for the conversation it started to open. Null when nothing waits.
 *
 * ONE, not a queue. While it waits Send is disabled, Enter refuses with a sentence saying why
 * (`sendTyped`), and the canned prompts are greyed and refuse (`sendUserMessage`) — so nothing else
 * can join it or overtake it, and a second press cannot start a second conversation underneath the
 * first.
 *
 * The text STAYS IN THE BOX while it waits, and leaves the box only when it goes: a conversation
 * that fails to open, or that the server closes before it has accepted it, leaves the reader exactly
 * what they wrote, with the status saying it did not go, and a send that went is what clears the
 * box, as it always was. "Goes" means the server has accepted the conversation, which is later than
 * the socket opening — see `deliverHeldTurn`.
 *
 * @type {string | null}
 */
let heldTurn: string | null = null;


/**
 * Whether `start()` is between its first line and the socket it opens: asking vibe-talk for a
 * session, fetching what to resume, and, for a spoken call, asking for the microphone.
 *
 * `session.socket` cannot answer "is a conversation starting" on its own, because it is only set
 * once all of those awaits are over. Without this, Talk pressed during them, or Send pressed after
 * Talk, ran a SECOND `start()` alongside the first: two minted sessions, two sockets — two billed
 * conversations — with the first socket overwritten and never closed, its `onclose` later tearing
 * down whichever call was live, and a held message written down the wrong socket ahead of that
 * socket's initiation frame. So `start()` refuses while this is set, `sendTyped` treats it as a
 * conversation already on its way and holds the message for it, and the big control reads
 * "Connecting…" through it.
 */
let startInFlight = false;


/**
 * The Send button and the Enter key, which must not be two different opinions about sending.
 *
 *   * A message is already waiting for its conversation: refused, out loud. Checked FIRST, because
 *     the socket is open before the server has accepted the conversation, and a send in that gap
 *     would overtake the message that is waiting.
 *   * A conversation is open: the text goes down its socket now, as it always did.
 *   * One is on its way — a start still asking for its session or the microphone, a call that has
 *     not connected yet, or the typed conversation an earlier send started: the text is held and
 *     goes once that conversation is accepted (`deliverHeldTurn`).
 *   * Nothing is open: the text is held, and a TYPED conversation is started for it. No microphone
 *     is asked for. See `start()` and `CHAT MODE` there.
 *
 * Returns whether it went, or with nothing open the promise of the conversation starting, which the
 * page suite awaits.
 */
function sendTyped() {
  const said = deps.el("compose-text").value.trim();
  if (said === "") {
    return false;
  }
  if (heldTurn !== null) {
    deps.setStatus("Still opening the conversation — your message goes as soon as it is open.");
    return false;
  }
  if (canSendText()) {
    const sent = sendUserMessage(said, { fromComposer: true });
    // Back in the box for the next one: a tap on Send took the focus to the button.
    if (sent) deps.el("compose-text").focus();
    return sent;
  }
  heldTurn = said;
  renderControls();
  if (session.socket || startInFlight) {
    deps.setStatus(
      session.chat
        ? "Opening a typed conversation — your message goes as soon as it is open."
        : "Your message goes as soon as the call is connected."
    );
    return false;
  }
  return guard(start)({ chat: true }).then(() => {
    if (heldTurn !== null && session.socket) {
      deps.setStatus("Opening a typed conversation — your message goes as soon as it is open.");
    }
  });
}


/**
 * The server has accepted the conversation: send what was held for it.
 *
 * Called when the SERVER says the conversation has started, not when the socket opens, on both
 * protocols:
 *
 *   * the hosted vendor's `conversation_initiation_metadata`. vibe-talk's own client of that vendor
 *     waits for the same frame before it writes a message (`initiate` in src/elevenlabs/socket.rs),
 *     because a conversation written to before it is answered with a protocol close. It is also the
 *     frame that names the conversation, and `recordTurn` files the message under that name; sent
 *     at `onopen` it was filed under no conversation, or the previous one.
 *   * `vibe-talk-v1`'s `session_started`. A `prompt` sent before it waits in `pendingPrompts`, so at
 *     `onopen` the message left the box, was drawn as sent and said "Sent." while it was still on
 *     the page — and a bridge that accepted the socket and then failed to start its session took it
 *     with it. In a typed conversation the prompt goes on the wire here, in this call.
 *
 * The box is cleared only if it still holds what was sent. A reader who went on writing while the
 * conversation opened keeps what they wrote; what went is what they pressed Send on.
 */
function deliverHeldTurn() {
  if (heldTurn === null) return;
  const said = heldTurn;
  heldTurn = null;
  sendUserMessage(said, { fromComposer: deps.el("compose-text").value.trim() === said });
  renderControls();
}


/**
 * Bring the whole of the call's message box into the list's view: the box and its Send, or, when
 * the list is shorter than the box, the box's top, where the reader types. Nothing moves when it is
 * all in view already.
 *
 * The box is IN the scrolling list, so it can be partly out of view in two ways a box fixed to the
 * screen never is. A tap on the part showing focuses it with Send still under the dock, and a
 * browser scrolls a focused field only as far as the field itself. And the on-screen keyboard takes
 * its height from the list (`interactive-widget=resizes-content`): a list that loses a third of its
 * height keeps its scroll position, so the foot of it — the box being typed in — goes under the
 * dock. So this runs when the box takes the focus, and when the list shrinks while it has it.
 */
function keepComposerInView() {
  deps.keepBoxInView(deps.leftEl("voice-composer"), deps.leftEl("scroll-area"));
}


/** Whether the reader is writing in the call's box on the call's view, which is when it is kept in view. */
function writingToAgent() {
  return deps.currentScreen === "main" && deps.currentView === "voice" &&
    document.activeElement === deps.leftEl("compose-text");
}


/** What a canned button says when there is no conversation for its sentence to reach. */
const CANNED_DISABLED_TITLE =
  "Start a call first — this sends a message into a live conversation, and there is not one.";


/**
 * Grey the canned buttons when there is no call, and say why on hover.
 *
 * The owner's report: Sumry and Blockers looked live at idle and did nothing when pressed. That was
 * true only in the narrowest sense — `sendUserMessage` has always refused and put a sentence on the
 * status line — but a refusal you discover by pressing is not the same as a control that tells you
 * beforehand, and the status line is transient and lives at the other end of the screen.
 *
 * The choice recorded, because the issue offered two: grey them out, or have them START a call the
 * way the message box's first send does. Greyed, for now, and deliberately — a tap on a five-letter
 * button silently opening a billed vendor conversation is a surprise with a price on it, and
 * Blockers additionally spends CODING-AGENT work. The box may start a typed conversation because
 * the reader has just written a message and pressed Send (`#241 voice-composer`); these two are not
 * that.
 *
 * `aria-disabled` rather than `disabled`: see the note in web/voice.css. The button stays hoverable
 * so the tooltip the owner asked for actually appears.
 */
function renderCannedPrompts() {
  // Not while the box's first message waits for the conversation it started: see `sendUserMessage`.
  const live = canSendText() && heldTurn === null;
  for (const entry of deps.CANNED_PROMPTS) {
    const button = deps.el(entry.button);
    button.setAttribute("aria-disabled", live ? "false" : "true");
    button.setAttribute("title", live ? entry.title : CANNED_DISABLED_TITLE);
  }
}


async function openVoiceSession() {
  if (!deps.token()) {
    throw new Error("no API token saved on this phone yet");
  }
  const payload = await deps.apiDecoded("VoiceSession", "/api/v1/voice-session");
  if (!payload.websocket_url) {
    throw new Error("vibe-talk answered without a voice WebSocket URL");
  }
  return payload;
}


// --- audio helpers ---------------------------------------------------------------------------

function base64ToBytes(b64: string) {
  const binary = atob(b64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}


function bytesToBase64(bytes: Uint8Array) {
  let binary = "";
  const chunk = 0x8000; // apply() has an argument-count limit; chunk to stay under it.
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode.apply(null, bytes.subarray(i, i + chunk) as unknown as number[]);
  }
  return btoa(binary);
}

/** Decode the negotiated format, naming this call's provider only if it is unsupported. */
function outputRateFrom(format: string | undefined) {
  return speechOutputRateFrom(format, () => session.providerName);
}


/**
 * How far ahead of the speaker a voice answer is scheduled, at most.
 *
 * Answers arrive at speaking pace, so the audio banked before the first sample plays is the only
 * cushion against a network stall: with 0.05 s, every hiccup on a highway was heard as a pause.
 * It starts small, because a conversation is latency-sensitive, and doubles after each stall.
 */
const CALL_MAX_LEAD_SECONDS = 2;

/** A queue that ran dry less than this long ago ran dry mid-answer. */
const CALL_STALL_WINDOW_SECONDS = 1;


/**
 * The voice socket's address. A deployment may configure a PATH (`/ws?mode=…`), which means this
 * page's own host: one configuration then serves every host it is deployed on, each page talking to
 * the voice bridge behind its own front door. An absolute URL is used exactly as given.
 *
 * Why it matters: a configuration naming one host's address, shared with a second host, sent the
 * second host's page to the first host's bridge, where the sign-in its front door had granted did
 * not apply and the socket never opened.
 */
function voiceSocketUrl(configured: unknown) {
  const url = String(configured || "");
  if (!url.startsWith("/") || url.startsWith("//")) return url;
  const origin = String(window.location.origin || "");
  return origin.replace(/^http/, "ws") + url;
}


function playPcm(b64: string) {
  playPcmBytes(base64ToBytes(b64));
}


function playPcmBytes(bytes: Uint8Array) {
  const pcm = new Int16Array(bytes.buffer, bytes.byteOffset, Math.floor(bytes.byteLength / 2));
  const buffer = session.audio!.createBuffer(1, pcm.length, session.outputRate);
  const channel = buffer.getChannelData(0);
  for (let i = 0; i < pcm.length; i += 1) {
    channel[i] = pcm[i]! / 0x8000;
  }
  const node = session.audio!.createBufferSource();
  node.buffer = buffer;
  node.connect(session.audio!.destination);
  const now = session.audio!.currentTime;
  if (session.playAt < now + 0.005) {
    // Nothing queued. A gap of under a second inside an answer is a network stall, not a new
    // answer: bank more before resuming, so the next stall of the same size is not heard.
    const stalled = session.playAt > 0 && now - session.playAt < CALL_STALL_WINDOW_SECONDS;
    if (stalled) session.playLead = Math.min(CALL_MAX_LEAD_SECONDS, session.playLead * 2);
    session.playAt = now + session.playLead;
  }
  node.start(session.playAt);
  // Scheduled, not heard yet: the first sound starts when the audio clock reaches `playAt`.
  markStartup("greeting_audible", Date.now() + (session.playAt - now) * 1000);
  session.playAt += buffer.duration;
  session.playEndMs = Date.now() + (session.playAt - now) * 1000;
  session.answerPlayEndMs = session.playEndMs;
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
  // Whatever was queued will not be heard now; the read-back, if it was one, ended here.
  session.playEndMs = Math.min(session.playEndMs, Date.now());
  session.answerPlayEndMs = Math.min(session.answerPlayEndMs, Date.now());
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
// the issue rules it out. Resume is a tap, and it runs the ordinary `start()` — which asks the
// provider for a FRESH voice session every time, so an expired session endpoint is never reused.

// How long after the page comes back a close still counts as part of the suspension. iOS commonly
// delivers the close on the way BACK rather than while hidden, so a window with no grace at all
// would miss the exact case this exists for. Too wide and a genuine drop moments after a tab
// switch gets excused, which is why the suite carries a negative control for a VISIBLE failure.
const SUSPENSION_GRACE_MS = 2500;


// `onerror` arms this; `onclose` cancels it. Short, because it is only bridging the gap between
// two events the browser fires back to back.
const FAILURE_REPORT_MS = 250;

let failureTimer: Timer | null = null;


/**
 * How long the voice socket may take to open before the page stops waiting. `#209
 * voice-socket-open`. Healthy starts opened in 0.4 to 3.1 seconds. Without a deadline, a front door
 * that takes the connection and never answers it left the call on "Connecting…" for as long as the
 * browser's own network timeout, which can be minutes.
 */
const SOCKET_OPEN_TIMEOUT_MS = 9000;

let openTimer: Timer | null = null;


/** What the page says when a start cannot reach the voice service. */
const VOICE_UNREACHABLE = "The voice service is not reachable.";


/**
 * Whether the last start never reached the voice service, and how it was started, so Try again
 * can start the same kind of conversation. `#209 voice-socket-open`.
 */
let hasUnreachable = false;

let lastStartChat = false;


function clearOpenTimer() {
  if (openTimer !== null) {
    clearTimeout(openTimer);
    openTimer = null;
  }
}


let hiddenDuringCall = false;

let visibleAt = 0;


// Short on purpose: `#status` is one line with `white-space: nowrap` and an ellipsis, so anything
// longer than about forty characters is simply not readable on a phone.
const SUSPENDED_STATUS = "Paused — the app was in the background.";


/**
 * KEEP THE SCREEN ON WHILE SOMETHING IS BEING SAID.
 *
 * An Android phone locks its screen after its idle timeout, and locking it stops a page's audio:
 * a read cut off mid-message, a call that goes silent on the highway. A web page cannot reliably
 * keep playing with the screen off, but Chrome lets it ask for the screen to stay on, so it asks
 * while a call is connected or read-aloud is on, and lets go as soon as neither is.
 *
 * The browser releases the lock whenever the page is hidden, so it is asked for again when the
 * page comes back. A browser without the API, or one that refuses (battery saver), simply keeps
 * its usual timeout; nothing else depends on the lock.
 */
let screenWakeLock: WakeLockSentinel | null = null;

let screenWakeLockPending = false;


function wantsScreenAwake() {
  try {
    return session.socket !== null || deps.readingMode || deps.nowPlaying !== null || deps.pendingRead !== null;
  } catch (_error) {
    // Asked before the page's state finished initialising: nothing is playing yet.
    return false;
  }
}


async function syncScreenWakeLock() {
  const api = typeof deps.navigator === "undefined" ? undefined : deps.navigator.wakeLock;
  if (!api || typeof api.request !== "function") return;
  const want = wantsScreenAwake() && document.visibilityState === "visible";
  if (want && screenWakeLock === null && !screenWakeLockPending) {
    screenWakeLockPending = true;
    try {
      const lock = await api.request("screen");
      screenWakeLock = lock;
      lock.addEventListener("release", () => {
        if (screenWakeLock === lock) screenWakeLock = null;
      });
      // Wanted when asked, perhaps not by the time the browser answered.
      if (!wantsScreenAwake()) syncScreenWakeLock();
    } catch (_error) {
      // Refused (battery saver, no user activation yet): the phone keeps its usual timeout.
    } finally {
      screenWakeLockPending = false;
    }
  } else if (!want && screenWakeLock !== null) {
    const lock = screenWakeLock;
    screenWakeLock = null;
    try {
      await lock.release();
    } catch (_error) {
      // Already released by the browser.
    }
  }
}


function wasSuspended() {
  return (
    hiddenDuringCall &&
    (document.visibilityState === "hidden" || Date.now() - visibleAt < SUSPENSION_GRACE_MS)
  );
}


/**
 * The connection of a call that HAD opened failed: lost without a closing handshake, or an error
 * the browser reported on it. A start that never opened is `reportUnreachable`'s.
 *
 * `#268 voice-call-drops`. This used to say "this browser could not open its WebSocket" — for a
 * call that had been open and talking for twenty seconds when its connection was reset. It says
 * what happened, how far into the call, and that the reader did not end it.
 */
function reportFailure() {
  failureTimer = null;
  const span = callSpan();
  deps.showError(
    `The connection to ${session.providerName} failed${span ? ` ${span} into the call` : ""}: ` +
      "it was cut without a goodbye — by the network, the phone or the voice service, not by " +
      "Hang up. Start a new call to carry on.",
    { stay: true }
  );
  deps.setStatus("The connection to the voice agent failed.");
}


// --- how a call ended ----------------------------------------------------------------------------
//
// `#268 voice-call-drops`. Calls ended on their own and nothing on this side said why: the only
// record was the voice bridge's log, which saw the page say goodbye and could not see what made it.
// So every call that opened is recorded once, when it ends — by whom or what, the close code, how
// long it had been open — as one content-free line in this app's log, and the screen says why
// unless the reader ended it. A hang-up, a drop, the service closing the call and the page ending it
// after the service's error are four different lines.

/** How long this call has been open, in words — "22 s", "3 min 5 s" — or "" when it never opened. */
function callSpan() {
  const health = session.health;
  if (!health || health.openedAt === null) return "";
  const seconds = Math.max(0, Math.round((Date.now() - health.openedAt) / 1000));
  if (seconds < 60) return `${seconds} s`;
  const minutes = Math.floor(seconds / 60);
  return seconds % 60 ? `${minutes} min ${seconds % 60} s` : `${minutes} min`;
}


/**
 * The longest close reason the call-end record carries: the length of a WebSocket close frame's
 * reason, which is the longest a voice service's own close can give. `#330 voice-long-turn-drop`.
 */
const CLOSE_REASON_MAX_CHARS = 120;


/**
 * Record, once per call that opened, how it ended: `POST /api/v1/voice-end`. Names and numbers,
 * like the startup and health records, and one line of the voice service's own words — its close
 * reason or its error — capped, with the token taken out; the server's allowlist refuses anything
 * else. Sent before the call is torn down, which is what clears the record it reads.
 *
 * `#330 voice-long-turn-drop`. For EVERY protocol, not only the deployment's own: a call on the
 * hosted service that ended mid-answer left no line anywhere. And with what decides who ended it:
 * how long before the end this page last sent a text turn, an `audio_end` or an `interrupt` (a
 * bridge may take either of the last two for a cut), whether the page was in sight, and how much of
 * the answer's audio was still waiting to be played.
 *
 * `#352 voice-reconnect-on-bridge-error`. A call the page carried on a new session records each
 * socket it replaced, as `reconnected`, and every record after the first reconnect says how many came
 * before it (`reconnects`); a call that never reconnected sends no count, as before.
 *
 * @param {"hang_up" | "error_frame" | "server_closed" | "connection_lost" | "suspended" | "reconnected" | "reconnect_limit" | "reconnect_failed"} endedBy
 * @param {number | null} code the close code the browser reported, when it reported one
 * @param {string} [reason] the voice service's own words: its close reason, or its error
 */
function postCallEnd(endedBy: "hang_up" | "error_frame" | "server_closed" | "connection_lost" | "suspended" | "reconnected" | "reconnect_limit" | "reconnect_failed", code: number | null, reason: string = "") {
  const health = session.health;
  if (!health || health.openedAt === null || health.endPosted) return;
  health.endPosted = true;
  // Every protocol the server named for the call; its allowlist refuses one it does not know.
  if (session.protocol === "unknown") return;
  const now = Date.now();
  /** @type {{protocol: string, chat: boolean, ended_by: string, since_open_ms: number, turns: number, close_code?: number, visibility: string, queued_audio_ms: number, last_text_ms?: number, last_audio_end_ms?: number, last_interrupt_ms?: number, reconnects?: number, speech_holds?: number, speech_hold_max_ms?: number, speech_hold_capped?: number, audio_input?: string, audio_output?: string, close_reason?: string}} */
  const body: { protocol: string; chat: boolean; ended_by: string; since_open_ms: number; turns: number; close_code?: number; visibility: string; queued_audio_ms: number; last_text_ms?: number; last_audio_end_ms?: number; last_interrupt_ms?: number; reconnects?: number; speech_holds?: number; speech_hold_max_ms?: number; speech_hold_capped?: number; audio_input?: string; audio_output?: string; close_reason?: string; } = {
    protocol: session.protocol,
    chat: health.chat,
    ended_by: endedBy,
    since_open_ms: Math.max(0, Math.round(now - health.openedAt)),
    turns: health.turns,
    visibility: document.visibilityState === "hidden" ? "hidden" : "visible",
    queued_audio_ms: Math.min(CALL_AUDIO_MAX_MS, Math.max(0, Math.round((session.playEndMs || 0) - now))),
  };
  if (typeof code === "number" && code >= 1000 && code <= 4999) body.close_code = code;
  const ago = (at: number) => Math.min(CALL_SPAN_MAX_MS, Math.max(0, Math.round(now - at)));
  if (session.lastTextAt) body.last_text_ms = ago(session.lastTextAt);
  if (session.lastAudioEndAt) body.last_audio_end_ms = ago(session.lastAudioEndAt);
  if (session.lastInterruptAt) body.last_interrupt_ms = ago(session.lastInterruptAt);
  if (session.reconnects > 0) body.reconnects = session.reconnects;
  // `#512 voice-recap-private-stack`. The page messages this socket held for the owner's speech, and
  // then counted afresh: each record is its own socket's.
  if (session.speechHolds > 0) {
    body.speech_holds = Math.min(SPEECH_HOLDS_MAX, session.speechHolds);
    body.speech_hold_max_ms = Math.min(CALL_SPAN_MAX_MS, Math.round(session.speechHoldMaxMs));
    body.speech_hold_capped = Math.min(SPEECH_HOLDS_MAX, session.speechHoldCapped);
  }
  // `#619 voice-bluetooth-output`. Kinds only, never a device label: the record is content-free.
  // An unclassifiable route says nothing, exactly as a call that captured nothing does.
  if (deps.callAudioRoute && !health.chat
    && (deps.callAudioRoute.inputKind !== "other" || deps.callAudioRoute.outputKind !== "other")) {
    body.audio_input = deps.callAudioRoute.inputKind;
    body.audio_output = deps.callAudioRoute.outputKind;
  }
  session.speechHolds = 0;
  session.speechHoldMaxMs = 0;
  session.speechHoldCapped = 0;
  const said = deps.redact(String(reason || "")).replace(/[\u0000-\u001f\u007f]+/gu, " ").replace(/\s+/gu, " ").trim();
  if (said) body.close_reason = Array.from(said).slice(0, CLOSE_REASON_MAX_CHARS).join("");
  // Diagnostics, not the call: a failure here is not worth a word on the screen.
  deps.api("/api/v1/voice-end", { method: "POST", body }).catch(() => {});
}


/** The longest call offset the server's record accepts: a day. */
const CALL_SPAN_MAX_MS = 86400000;

/** The most queued audio the server's record accepts: an hour. */
const CALL_AUDIO_MAX_MS = 3600000;

/** The most held page messages the server's record accepts. `#512 voice-recap-private-stack`. */
const SPEECH_HOLDS_MAX = 10000;


/**
 * A start that never reached the voice service: refused, closed, or silent past the deadline.
 * `#209 voice-socket-open`.
 *
 * Said as what it is — the voice service, not this browser — with a Try again beside it. The old
 * sentence ("this browser could not open its WebSocket") read like a fault in the phone, offered
 * nothing to press, and was answered with a hand-made retry, two to four times in a row.
 *
 * @param {string} how what happened, as a sentence
 * @param {string} hint what may explain it, as a sentence, or ""
 */
function reportUnreachable(how: string, hint: string) {
  failureTimer = null;
  hasUnreachable = true;
  deps.showError(`${VOICE_UNREACHABLE} ${how}${hint ? ` ${hint}` : ""} Try again in a moment.`);
  deps.el("retry-start").hidden = false;
  deps.setStatus(VOICE_UNREACHABLE);
  renderControls();
}


/**
 * Record, for the startup line, that this start's socket never opened, and how it ended. Names
 * and numbers only, like every other startup mark.
 *
 * @param {number | null} code the close code the browser reported, if it reported one
 * @param {boolean} timedOut whether the page's own deadline ended it
 */
function noteSocketFailure(code: number | null, timedOut: boolean) {
  const timing = session.timing;
  if (!timing || timing.sent) return;
  markStartup("socket_failed");
  timing.socket = { socket_timeout: timedOut };
  if (typeof code === "number" && code >= 1000 && code <= 4999) {
    timing.socket.socket_close_code = code;
  }
}


/**
 * Why a deployment's own voice socket may be refused, when its address says so: it names a host
 * other than this page's. That shared configuration once refused every call on one host for two
 * days, and nothing on the page said where the socket was going.
 *
 * @param {string} socketUrl the address the socket was opened on
 * @returns {string}
 */
function foreignSocketHint(socketUrl: string) {
  if (session.protocol !== "vibe-talk-v1") return "";
  const host = (url: string) => ((/^[a-z]+:\/\/(?:[^/@?#]*@)?([^/:?#]+)/iu.exec(url) || [])[1] || "").toLowerCase();
  const target = host(socketUrl);
  const own = host(String(window.location.origin || ""));
  if (!target || !own || target === own) return "";
  return `Its address is on ${target}, not on ${own}, and that host may not accept the sign-in of this page.`;
}


/** Try again: start the same kind of conversation the failed start was. */
function retryStart() {
  deps.clearError();
  if (lastStartChat && deps.el("compose-text").value.trim() !== "") {
    return deps.guardQuietly(sendTyped)();
  }
  return guard(start)(lastStartChat ? { chat: true } : undefined);
}


// --- the call ----------------------------------------------------------------------------------

/**
 * Open a conversation.
 *
 * @param {{chat?: boolean}} [options] `chat: true` opens a TEXT-ONLY conversation: the microphone
 *   is never requested, no AudioContext is created, no capture graph is built, and nothing is
 *   played back. See `CHAT MODE` below.
 */
async function start(options?: { chat?: boolean; }) {
  if (session.socket) {
    deps.setStatus("already connected");
    return;
  }
  // ONE start at a time (`startInFlight`). Refused without a word of its own: the big control
  // already reads "Connecting…", and the status line is saying what the start is waiting for —
  // the session or the microphone prompt — which is the more useful of the two sentences.
  if (startInFlight) {
    return;
  }
  startInFlight = true;
  try {
    await openConversation(options);
  } finally {
    // Before `guard` sees a failure and tears down, and after a success has set `session.socket`,
    // which from then on is what says a conversation exists.
    startInFlight = false;
  }
}


/**
 * What the voice session asked this page to hand the agent of the call being opened, or null (`#272
 * voice-agent-instructions`). Moved here from the legacy script (`#624 voice-page-modularity`): only the
 * call reads and writes it.
 */
let voicePromptForCall: unknown = null;

/** `start()`'s body, run only while it holds `startInFlight`. */
async function openConversation(options: {chat?: boolean} | undefined) {
  const chat = Boolean(options && options.chat);
  beginStartupTiming(chat);
  beginHealth(chat);
  deps.clearError();
  // A new call is a clean slate for all three of these. `hasSuspended` in particular: leaving it
  // set would keep the large control reading "Resume" during and after the call it started.
  session.failed = false;
  session.chat = chat;
  hasUnreachable = false;
  lastStartChat = chat;
  session.providerName = deps.conversationalVoice.name;
  session.vendorSentAudioInChat = false;
  hiddenDuringCall = false;
  hasSuspended = false;
  deps.setState("working");
  deps.setStatus("Asking vibe-talk for a voice session…");
  // "Connecting…" on the big control from here, not from the socket: see `startInFlight`.
  renderControls();
  const minted = await openVoiceSession();
  markStartup("session_acquired");
  session.protocol = minted.protocol || "unknown";
  session.providerName = String(minted.provider || deps.conversationalVoice.name);
  session.inputRate = minted.input_sample_rate || 16000;
  session.outputRate = minted.output_sample_rate || 16000;
  voicePromptForCall = minted.agent_prompt || null; // `#272 voice-agent-instructions`
  session.lastTranscriptKey = null;
  session.liveTurns = new Map();
  session.noTextRows = new Map();
  session.anonTurn = 0;
  resetVibeTalkTurnState();
  session.pendingPrompts = [];
  session.playEndMs = 0;
  session.answerPlayEndMs = 0;
  session.echoCancellation = false;
  // `#523 voice-self-hearing`. Whatever the agent says first, nobody has heard yet. A
  // `vibe-talk-v1` session says at `session_started` whether it greets at all.
  session.greetingPlayedOut = false;
  session.greetingPromised = false;
  session.agentSaid = new Map();
  session.echoExposedAt = 0;
  session.sessionStarts = 0;
  session.documentNoticeSent = false;
  session.lastTextAt = 0;
  session.lastAudioEndAt = 0;
  session.lastInterruptAt = 0;
  // `#512 voice-recap-private-stack`. Nothing held for the owner's speech, nothing armed, yet.
  session.speechHolds = 0;
  session.speechHoldMaxMs = 0;
  session.speechHoldCapped = 0;
  session.profileRecap = null;
  session.armedRecap = null;
  // `#270 voice-transcript-markers`. Nothing recorded and nothing handed over yet in this call —
  // and no conversation named for it. The last call's name outlives its teardown, for the line
  // that ends it; kept into this call, a draft shown before the hosted service names this one
  // was filed in the last one, and a turn recorded before then went there too.
  session.recordedAny = false;
  session.recap = null;
  session.recapAck = null;
  session.recapSeam = null;
  session.pendingResume = null;
  session.conversationId = null;
  deps.startAwaitingAnswer = null;
  // `#352 voice-reconnect-on-bridge-error`. A new call has reconnected nothing yet.
  session.reconnect = null;
  session.reconnects = 0;
  session.lastReconnectAt = 0;
  session.noticeAwaitingRecap = null;
  // Fetched HERE, before the socket exists, so a slow or failing store delays the call rather than
  // racing `onopen` — a payload that arrived after the agent had already spoken would be a
  // "here is what we said" delivered into the middle of a sentence.
  let resume = await deps.fetchResume();
  const validity = minted.valid_for_seconds
    ? `; session valid for about ${Math.round(minted.valid_for_seconds / 60)} minutes`
    : "";
  const voiceTarget = String(minted.websocket_url).split(/[?#]/u, 1)[0]!
    .replace(/^(wss?:\/\/)[^/@]+@/u, "$1");
  deps.showDetail(
    `${minted.provider || "voice provider"} · ${session.protocol}${validity} · ${voiceTarget}`
  );

  // CHAT MODE: THE MICROPHONE IS NOT OPENED, AND THAT IS THE WHOLE FEATURE.
  //
  // The owner's complaint was precise: reaching a text interface required starting a voice call and
  // then muting it and silencing it — three acts to arrive at "typing", and the phone showing the
  // microphone as in use throughout, because it really was. Mute withholds frames from a live
  // capture graph; it does not close the microphone, and it is documented here at length as
  // deliberately not doing so. So mute could never be the answer to this: the answer is not
  // acquiring the microphone in the first place.
  //
  // Everything below is skipped, not disabled: no `getUserMedia`, so no permission prompt and no
  // in-use indicator; no AudioContext; no capture graph in `socket.onopen`; and no playback in
  // `handle`. There is nothing to mute because there is nothing running.
  if (!chat) {
    deps.setStatus("Asking for the microphone…");
    // Read HERE, at the moment the stream opens — which is why a toggle flipped mid-call cannot
    // reach this one, and why `micSettingsChanged` says so out loud.
    const mic = deps.micSettings();
    session.stream = await deps.openCallMicrophone(mic);
    // `#264 voice-send-approval`: a spoken yes is taken only with echo cancellation on, as opened.
    session.echoCancellation = mic.echoCancellation === true && !deps.echoCancellationRefused(session.stream);
    if (!session.echoCancellation) {
      // `#523 voice-self-hearing`. Said once, because it changes how the call behaves.
      deps.addDetail("echo cancellation is off for this call, so the microphone pauses while the assistant " +
        "speaks: talking over it is not heard");
    }
    session.audio = new (deps.browserPorts().audio.contextClass()!)();
    await session.audio.resume(); // iOS starts it suspended until a gesture.
    await deps.applyCallOutput(session.audio);
    deps.refreshAudioDeviceSelects();
    deps.renderAudioRouteReadout();
    markStartup("microphone_ready");
  }

  // `#512 voice-recap-private-stack`. Armed last, just before the socket: the bridge fetches the
  // profile as its session opens, and an armed recap waits for that a minute at most.
  if (resume && deps.resumeViaProfile && session.protocol === "vibe-talk-v1") {
    resume = await deps.armRecapForCall(resume);
  } else if (deps.resumeViaProfile && session.protocol === "vibe-talk-v1") {
    // A call handed no recap says so, so one an earlier page armed and never took back — a reload
    // cut it off — is not handed to this call unasked.
    deps.api("/api/v1/voice-recap", { method: "DELETE" }).catch(() => {});
  }
  deps.setState("working");
  const socketUrl = voiceSocketUrl(minted.websocket_url);
  const socket = deps.browserPorts().socket.open(socketUrl);
  socket.binaryType = "arraybuffer";
  session.socket = socket;
  session.muted = false;
  renderControls();
  wireCallSocket(socket, socketUrl, resume, chat);
}


/**
 * The call's socket, opened on `socketUrl`, given its handlers: the deadline for it to open, what
 * opening it starts, every frame it delivers, an error on it, and its close.
 *
 * @param {WebSocket} socket the call's socket, already `session.socket`
 * @param {string} socketUrl the address it was opened on, for the reason a start failed
 * @param {Resume | null} resume what `fetchResume` answered for this call, null for nothing
 * @param {boolean} chat whether the conversation is typed
 * @param {ReconnectAttempt | null} [reconnect] `#352 voice-reconnect-on-bridge-error`: the reconnect
 *   this socket carries the call on. It opens nothing the call already has, and until its session
 *   starts, every way it can fail ends the call as `reconnect_failed` rather than as a start that
 *   never reached the voice service, except a close while the page was in the background, which is
 *   the suspension it is
 */
function wireCallSocket(socket: WebSocket, socketUrl: string, resume: Resume | null, chat: boolean, reconnect: ReconnectAttempt | null = null) {
  // `#209 voice-socket-open`. Whether THIS socket ever opened: a close before it did is a start
  // that never reached the voice service, whatever the close code says.
  let opened = false;
  clearOpenTimer();
  const openDeadline = () => {
    openTimer = null;
    if (session.socket !== socket || opened) return;
    // A page sent to the background while connecting: a phone holds its timers and its network,
    // so a deadline that ran out there says nothing about the voice service. Wait again, and
    // judge once the reader has been back for longer than the suspension grace.
    if (wasSuspended() || document.visibilityState === "hidden") {
      openTimer = setTimeout(openDeadline, SOCKET_OPEN_TIMEOUT_MS);
      return;
    }
    // Stop listening before closing: the close this causes is the page's own, and has nothing
    // more to report than what is reported here.
    socket.onopen = null;
    socket.onmessage = null;
    socket.onerror = null;
    socket.onclose = null;
    if (failureTimer !== null) {
      clearTimeout(failureTimer);
      failureTimer = null;
    }
    if (reconnect) {
      failReconnect(`its new voice socket did not open within ${Math.round(SOCKET_OPEN_TIMEOUT_MS / 1000)} s`);
      return;
    }
    noteSocketFailure(null, true);
    try {
      socket.close();
    } catch (_error) {
      // A socket that is already closing has nothing more to close.
    }
    reportUnreachable(
      `Nothing answered at its address within ${Math.round(SOCKET_OPEN_TIMEOUT_MS / 1000)} seconds.`,
      foreignSocketHint(socketUrl)
    );
    deps.dismissBanner();
    deps.addDetail(`the voice socket did not open within ${Math.round(SOCKET_OPEN_TIMEOUT_MS / 1000)} s`);
    teardown();
  };
  openTimer = setTimeout(openDeadline, SOCKET_OPEN_TIMEOUT_MS);

  socket.onopen = () => {
    // Hung up while it was connecting: it opened for nobody. `#268 voice-call-drops`.
    if (session.socket !== socket) return;
    opened = true;
    clearOpenTimer();
    if (reconnect) {
      // `#352`. The call is open already: the conversation, its relay and its post watch go on, and
      // its record keeps counting from when the call opened. Only the new session is awaited.
      session.connected = true;
      renderControls();
      return;
    }
    markStartup("socket_open");
    if (session.health) {
      session.health.openedAt = Date.now();
    }
    if (session.protocol === "vibe-talk-v1") {
      session.connected = true;
      deps.conversationOpen = true;
      deps.startPostWatch();
      beginCallRelay();
      session.conversationId = deps.conversationIdFrom(null);
      if (resume) {
        // `#331 resume-on-websocket`. The record goes at `session_started`, and only to a session
        // that lists `replay`, so what this call was handed is settled there
        // (`offerReplayAtSessionStart`).
        session.pendingResume = resume;
      } else {
        // `#270 voice-transcript-markers`. What this call was handed of the conversation above —
        // nothing, and why — on the line above it, and in its first record.
        deps.noteCallRecap(deps.recapFor(null, null));
        deps.expectStartMarker(session.conversationId, session.recap, deps.lastEndCause);
      }
      deps.setState("live");
      deps.setStatus("Opening the voice session…");
      renderControls();
      // A message held for this conversation is NOT sent here: it goes at `session_started`
      // (`deliverHeldTurn`), so a session that never starts leaves it in the box.
      return;
    }
    // The initiation frame is UNCHANGED on the default path. `contextual_update` is the default
    // transport because the alternative — carrying the text on this frame under
    // `dynamic_variables` — depends on the agent's dashboard security settings permitting
    // overrides and fails SILENTLY when they do not, which is the worst possible failure shape for
    // a feature whose entire risk is claiming a continuity it does not have. The server chooses;
    // the page does not guess.
    const initiation: Initiation = { type: "conversation_initiation_client_data" };
    const carriedOnInitiation = resume && resume.transport === "client_data";
    if (carriedOnInitiation) {
      initiation.dynamic_variables = { vibe_talk_resume: resume.text };
    }
    // ASKED FOR, NOT RELIED ON. A text-only response mode is settled once, here, at initiation —
    // which is exactly why Sound does not renegotiate one mid-call, and why this is the only place
    // it can be requested at all. It is an OVERRIDE, so an agent whose dashboard forbids overrides
    // ignores it and keeps sending audio.
    //
    // That failure is survivable HERE in a way it is not elsewhere in this file, and the reason is
    // worth stating: chat mode's guarantee is "this page never opened your microphone", and that
    // guarantee is made entirely on this side of the socket. The override only decides whether the
    // vendor also stops sending audio down. If it is ignored, the conversation is still typed,
    // still silent, and still mic-free — it merely wastes downstream bandwidth. `handle` notices
    // and records it rather than letting the page imply a negotiation that did not happen.
    if (chat) {
      initiation.conversation_config_override = { conversation: { text_only: true } };
    }
    // `#272 voice-agent-instructions`. The owner's words, in the form the server chose.
    const handed = deps.handOverAgentPrompt(initiation, voicePromptForCall);
    socket.send(JSON.stringify(initiation));
    if (handed.note) {
      socket.send(JSON.stringify(handed.note));
    }
    // A replaced prompt took any `{{vibe_talk_resume}}` placeholder in the console's with it, so a
    // record carried on `dynamic_variables` would reach nothing: it goes as a note as well.
    if (resume && (!carriedOnInitiation || handed.replaced)) {
      // Immediately after, and before any audio: the vendor documents this as non-interrupting
      // background information, and the first agent turn is the one that has to know.
      socket.send(JSON.stringify({ type: "contextual_update", text: resume.text }));
    }
    // Only once the frames are on their way, and unable to throw into the rest of the start.
    deps.renderAfterHandOver();
    // `#270 voice-transcript-markers`. What went, on the line above the call. Its conversation's
    // start marker waits for the voice service to name it (`noteConversationNamed`).
    deps.noteCallRecap(deps.recapFor(resume, resume
      ? carriedOnInitiation && !handed.replaced ? "client_data" : "contextual_update"
      : null));
    session.connected = true;
    deps.conversationOpen = true;
    deps.startPostWatch();
    beginCallRelay();
    // A mute engaged during the CONNECT WINDOW could not be announced when it happened. The socket
    // is assigned before it is open, deliberately — that is what makes the talk control mute
    // rather than dial a second call — but `sendClientEvent` refuses a socket the page is not yet
    // connected on, so the announcement was dropped. Without this the call then runs muted with
    // the agent never told, which is precisely the "are you there?" that `#73 mute-is-invisible`
    // exists to prevent, and it is the worst case of it: the whole call rather than one pause.
    //
    // It goes here, AFTER `session.connected`, because that is what `sendClientEvent` tests, and
    // before `startCapture` so that nothing about this call happens before the agent is told.
    if (session.muted) {
      announceMute(true);
    }
    deps.setState("live");
    if (heldTurn !== null) {
      // It goes when the vendor answers the initiation (`deliverHeldTurn`), which is a moment away.
      deps.setStatus("Connected — sending your message…");
    } else {
      deps.setStatus(chat ? "Connected — type a message." : "Connected — say something.");
    }
    renderControls();
    if (!chat) {
      startCapture(socket);
    }
  };

  socket.onmessage = (event: MessageEvent<unknown>) => {
    // `#268 voice-call-drops`. A socket this page has finished with speaks for no call. Whatever
    // it still delivers is not the current call's: an `error` frame on it would reach `endCall`,
    // which acts on `session.socket` — the NEXT call's, once there is one — and hang that call up.
    if (session.socket !== socket) return;
    if (event.data instanceof ArrayBuffer) {
      if (session.protocol === "vibe-talk-v1") {
        noteReplyArriving();
      }
      if (session.protocol === "vibe-talk-v1" && !session.chat) {
        // Before the speaker-off drop: silencing the agent must not look like a dead service.
        noteAgentAudio(new Uint8Array(event.data));
      }
      // `#330 voice-long-turn-drop`. Not the late audio of an answer the reader's message cut: the
      // page stopped playing it, and this would start it again.
      if (session.protocol === "vibe-talk-v1" && !session.chat && !session.speakerOff && !session.interruptPending) {
        markStartup("greeting_audio_received");
        playPcmBytes(new Uint8Array(event.data));
      }
      return;
    }
    let message = null;
    try {
      message = JSON.parse(event.data as string);
    } catch (_error) {
      return; // a frame we do not understand is not a reason to tear the call down.
    }
    // `handle` throws for a format this page cannot decode, and `onmessage` is not inside
    // `guard()` — it is called by the browser, not by us — so an unhandled throw here would land
    // in the console, which is exactly where this page's failures used to go to die. Against the
    // in-page wire fake that could not happen; against a real socket it is one negotiated setting
    // away.
    //
    // A `vibe-talk-v1` frame is checked against the public contract first. A `type` this page does
    // not know is ignored, as the protocol promises a newer server; a known type in the wrong shape
    // is a peer this page cannot follow, and says so here rather than as a missing field later.
    // That refusal is the peer's error, so it is reported as one: a refused `turn_complete` would
    // otherwise leave the no-reply timer to call a talking service silent.
    let frame = null;
    try {
      if (session.protocol === "vibe-talk-v1") {
        frame = VibeTalkContract.decodeTagged("VibeTalkV1ServerFrame", message);
        if (frame !== null) handleVibeTalk(frame);
      } else {
        handle(socket, message);
      }
    } catch (error) {
      deps.showError((error as Error).message);
      deps.setStatus("the agent sent something this page cannot handle");
      if (session.protocol === "vibe-talk-v1" && frame === null) {
        noteAgentError();
        reportHealth("error_frame");
      }
    }
  };

  socket.onerror = () => {
    // NOT `showError` any more. `onerror` fires BEFORE the close, and only the close knows whether
    // this browser had backgrounded the page — which is the difference between "something is
    // broken" and "you switched apps". So this records the fact and arms a short timer; the close
    // cancels it and classifies. An error with no close following still reaches the screen, which
    // is what the timer is for: silence would be the old bug in a new coat.
    //
    // Only for the call's own socket: one this page has finished with must not mark the next call
    // failed, nor arm a report that lands on it. `#268 voice-call-drops`.
    if (session.socket !== socket) return;
    session.failed = true;
    if (failureTimer !== null) {
      clearTimeout(failureTimer);
    }
    failureTimer = setTimeout(
      reconnect
        ? () => {
          failureTimer = null;
          if (session.reconnect === reconnect) failReconnect("its new voice socket failed before its session started");
          else reportFailure();
        }
        : opened
          ? reportFailure
          : () => reportUnreachable("Its address did not accept the connection.", foreignSocketHint(socketUrl)),
      FAILURE_REPORT_MS
    );
  };

  socket.onclose = (event: CloseEvent) => {
    // THE one classifier. Three outcomes, and they are three different things to say:
    //
    //   suspended  the page was in the background — recoverable, and no failure happened
    //   failed     something really went wrong between this browser and the selected provider
    //   ended      the call is over, either because it was hung up or because it ran out
    //
    // The old code had two of these and reported the first as the second.
    //
    // `#268 voice-call-drops`. "Ended" here is never the reader's Hang up: that tears the call down
    // itself (`endCall`), so its socket is no longer the call's by the time its close arrives, and
    // that close — which can come seconds later, after a `quit` the service took its time over —
    // changes nothing but the connection details. Tearing down from it used to end whatever call
    // the reader had started since.
    if (session.socket !== socket) {
      deps.addDetail(`closed with code ${event.code}${event.reason ? `: ${event.reason}` : ""}`);
      return;
    }
    if (failureTimer !== null) {
      clearTimeout(failureTimer);
      failureTimer = null;
    }
    clearOpenTimer();
    if (reconnect && session.reconnect === reconnect && !wasSuspended()) {
      // `#352`. Closed before the new session started: the reconnect failed, and the call ends.
      deps.addDetail(`closed with code ${event.code}${event.reason ? `: ${event.reason}` : ""}`);
      const reason = deps.redact(String(event.reason || "")).slice(0, 120).replace(/[\s.]+$/u, "");
      failReconnect(opened
        ? `its new voice session closed before it started${reason ? `: ${reason}` : ""}`
        : "its new voice socket was refused");
      return;
    }
    // `#209 voice-socket-open`. Closed before it ever opened, and not by the reader: the start
    // never reached the voice service, whether or not an error came first.
    const neverOpened = !opened;
    const cause = wasSuspended()
      ? "suspended"
      : session.failed || neverOpened
        ? "failed"
        : "ended";
    // Closed without a closing handshake: the browser's 1006, which it reports after an error.
    const lost = session.failed || event.code === 1006;
    if (cause === "failed" && neverOpened) {
      noteSocketFailure(event.code, false);
      reportUnreachable("Its address did not accept the connection.", foreignSocketHint(socketUrl));
    } else if (cause === "failed") {
      reportFailure();
    } else {
      // `#268 voice-call-drops`. The service's side ended a call the reader was in, and it does
      // not stop silently: the panel says so and how far in, and the transcript's seam keeps it.
      if (cause === "ended") {
        deps.showError(serviceEndedSentence(callSpan(), lost, deps.redact(event.reason || "")), { stay: true });
      }
      deps.setState(cause);
      // A session-endpoint failure also shows up here as an immediate close, so the page has to say
      // something — but "code 1005" is not something. It says what happened in words; the number
      // goes where numbers belong, in the connection details on the settings screen.
      deps.setStatus(cause === "suspended" ? SUSPENDED_STATUS : closeReason(event.code));
    }
    // A reconnect's socket carries a call that opened long ago, so its end is recorded even unopened.
    if (!neverOpened || reconnect) {
      postCallEnd(cause === "suspended" ? "suspended" : lost ? "connection_lost" : "server_closed", event.code,
        event.reason || "");
    }
    // The banner is about a conversation that no longer exists, and the code is the one thing on
    // this page that must not be read as user-facing. Put the banner away FIRST, then record the
    // number where numbers belong: the connection details on the settings screen.
    deps.dismissBanner();
    deps.addDetail(`closed with code ${event.code}${event.reason ? `: ${event.reason}` : ""}`);
    teardown();
    deps.noteConversationEnded(cause === "ended" && !lost ? "closed" : cause === "ended" ? "failed" : cause);
  };
}


function handle(socket: WebSocket, message: HostedFrame) {
  switch (message.type) {
    case "conversation_initiation_metadata": {
      const meta = message.conversation_initiation_metadata_event || {};
      const previous = session.conversationId;
      deps.noteSessionStartedForSpokenYes();
      deps.heardEpoch += 1;
      session.conversationId = deps.conversationIdFrom(meta.conversation_id);
      // `#270 voice-transcript-markers`. Before the held message is filed below: the conversation's
      // first record is its start marker.
      deps.noteConversationNamed(previous);
      // `#1040`. The open document's context, silent on this protocol, before the held message.
      offerDocumentAtSessionStart();
      // The vendor has accepted the conversation, and named it: the message the box held for it
      // goes now, filed under that name. Before the format is read, which throws for a format this
      // page cannot PLAY — a reason to say so, not to swallow a typed message.
      deliverHeldTurn();
      session.outputRate = outputRateFrom(meta.agent_output_audio_format);
      deps.addDetail(
        `conversation ${meta.conversation_id || "?"} · agent audio ${
          meta.agent_output_audio_format || "pcm_16000"
        }`
      );
      break;
    }
    case "audio":
      // Before anything below drops it: a relayed turn's answer is talking, heard or not. `#337`.
      noteRelayAnswerOutput(pcmMsOfBase64((message.audio_event || {}).audio_base_64));
      // In chat mode there is NOTHING to play it with — no AudioContext was ever created — so this
      // is a hard drop rather than a preference. It is also evidence: audio arriving on a
      // conversation this page asked to be text-only means the override was not honoured. Said
      // ONCE, into the connection details, because a per-frame report would be thousands of lines
      // and because the reader's conversation is unaffected either way.
      if (session.chat) {
        if (!session.vendorSentAudioInChat) {
          session.vendorSentAudioInChat = true;
          deps.addDetail(
            "asked for a text-only conversation and the agent sent audio anyway — the override " +
              "is refused by its settings. Nothing is played and the microphone was never opened; " +
              "this only costs downstream bandwidth."
          );
        }
        break;
      }
      // Sound off silences the agent's VOICE, not the agent: the frame is dropped here, and the
      // `agent_response` case below still writes what it said into the transcript.
      if (!session.speakerOff) {
        playPcm((message.audio_event || {}).audio_base_64!);
      }
      break;
    case "agent_response":
      // "assistant", not "agent". This page's sibling view is a channel full of CODING agents
      // posting under their own names, and a transcript that labels the voice as "agent" invites
      // the reader to think one of those is talking. Only the displayed word changes: `line()`
      // still tells the two speakers apart by `who === "you"`, so side, tint and corner are
      // untouched.
      {
        const said = (message.agent_response_event || {}).agent_response || "";
        noteRelayAnswerOutput();
        deps.line("assistant", said);
        deps.recordTurn("assistant", said);
        // Counted for the call-end record, which says how many turns a call had. `#330`.
        if (session.health) session.health.turns += 1;
        deps.spokenYes.turns += 1;
        deps.hearAssistantForSpokenYes(`assistant:${deps.spokenYes.turns}`, said);
        noteAgentSaid(`assistant:${deps.spokenYes.turns}`, said); // `#523 voice-self-hearing`
      }
      break;
    case "user_transcript":
      {
        const said = (message.user_transcription_event || {}).user_transcript || "";
        // A typed turn was already rendered when it was sent, so an echo of it is the same turn
        // arriving a second time — not a second turn. `#43 typed-input`.
        if (isEchoOfTyped(said)) {
          break;
        }
        // `#523 voice-self-hearing`. Nor is the assistant's own voice, heard through the speaker: not
        // drawn, filed or reported as his, and never a yes. Said in the connection details.
        const heardAt = Date.now();
        if (nearAgentSpeech(heardAt, ECHO_FINAL_GRACE_MS) && isAgentEcho(said, heardAt, agentAudible(heardAt))) {
          noteAgentEchoHeard(said);
          break;
        }
        // He spoke while the agent was silent: what it says next is an answer, not a greeting.
        if (!agentAudible()) session.greetingPlayedOut = true;
        const li = deps.line("you", said);
        // This protocol sends no partials: every user transcript is a final turn of its own.
        deps.spokenYes.turns += 1;
        const key = `you:${deps.spokenYes.turns}`;
        deps.reportHeard(key, said);
        // `#269 approval-card-inline`. Out of sight from the moment it is drawn while it may be a yes
        // to the card, and judged BEFORE it is recorded, so the record knows whether the page acted
        // on it.
        deps.noteYesCandidate(key, said, li);
        deps.hearUserForSpokenYes(key, said, true);
        deps.settleYesCandidate(key);
        deps.recordTurn("you", said, { answerOf: deps.spokenYes.holds.get(key) || null });
      }
      break;
    case "interruption":
      stopPlayback();
      deps.noteCutForSpokenYes();
      noteRelayAnswerCut();
      break;
    case "ping":
      socket.send(
        JSON.stringify({ type: "pong", event_id: (message.ping_event || {}).event_id })
      );
      break;
    case "mcp_tool_call":
      answerVendorApproval(socket, message.mcp_tool_call || {});
      break;
    default:
      break;
  }
}


/**
 * The voice service asks this page to approve a tool call. `#579 voice-send-confirm-timing`.
 *
 * A tool the agent's settings mark "requires approval" reaches the client as `mcp_tool_call` in state
 * `awaiting_approval`, answered by `mcp_tool_approval_result`; the call fails when nobody answers
 * within its `approval_timeout_secs`. The README had owners put `post_reply` there, and this page
 * never answered, so the agent was told its send had timed out. While the server asks first,
 * `post_reply` only proposes — the server holds the draft for the card, and the owner's Send or yes
 * is the approval — so the page approves it. "Asks first" is read from the server as the request
 * arrives, as the server reads it for every `post_reply`, and not from what this page last heard:
 * the owner may have turned it off on another device, and then the vendor's gate is the one left.
 * A read that fails, or a server that posts at once, approves nothing. Nor is any other tool
 * approved: those keep whatever gate the owner set at the vendor, and the connection details say
 * one is waiting. A call from a second MCP server that also names a `post_reply` is not told apart;
 * this page knows no service id of its own to tell it by.
 *
 * Other states of a `post_reply` call say the agent is calling it, so a word held over an answer
 * that may have asked for this draft waits for it (`extendRedraftWait`).
 *
 * @param {WebSocket} socket
 * @param {{state?: string, tool_call_id?: string, tool_name?: string}} call
 */
function answerVendorApproval(socket: WebSocket, call: { state?: string; tool_call_id?: string; tool_name?: string; }) {
  if (call.tool_name === "post_reply" && (call.state === "loading" || call.state === "awaiting_approval")) {
    deps.extendRedraftWait();
  }
  if (call.state !== "awaiting_approval" || typeof call.tool_call_id !== "string") return;
  if (call.tool_name !== "post_reply" || !deps.spokenYes.askFirst) {
    deps.addDetail(
      `the voice service is waiting for someone to approve ${call.tool_name || "a tool"}; this page ` +
        "does not, so the call will time out — set that tool to run without approval in the agent's settings"
    );
    return;
  }
  const id = call.tool_call_id;
  void (async () => {
    let asks = false;
    try {
      const setting = await deps.apiDecoded("VoiceSettings", "/api/v1/voice-settings");
      asks = setting.ask_before_send === true;
    } catch (_error) {
      asks = false;
    }
    if (session.socket !== socket) return;
    if (!asks) {
      deps.addDetail("the voice service asked to approve post_reply; not approved, since this server now posts at once");
      return;
    }
    socket.send(JSON.stringify({ type: "mcp_tool_approval_result", tool_call_id: id, is_approved: true }));
    deps.addDetail("the voice service asked to approve post_reply; approved, since it only drafts — the card asks you");
  })();
}


// --- live transcript reconciliation ------------------------------------------------------------
//
// `#15 transcript-dedup` and `#12 live-transcript-latency`. A provider that streams speech
// recognition sends the SAME utterance several times: partial hypotheses that grow and get
// corrected ("…messages Google Chat." becomes "…messages in Google Chat."), and a final one. The
// page used to append every frame as a new row and store every one of them, so the record held
// the same sentence twice with one word different, and the next reader could not tell which was
// said.
//
// So a spoken turn is ONE row, keyed by (turn, role), updated in place as better text arrives,
// and stored ONCE when the turn ends. The same words said again in a LATER turn are a new key and
// a new row: repeating yourself is a thing people really do, and the record must keep it.

/**
 * One frame of speech, into the row for its (turn, role).
 *
 * A partial frame is the provider's current hypothesis for the segment after everything already
 * settled, so it REPLACES the previous partial rather than merging with it — that is what makes
 * a correction mid-sentence harmless. A final frame settles into the turn with `mergeHypothesis`,
 * which is also what copes with a provider that sends every fragment, and then the whole turn
 * again, all marked final. A frame without a turn number cannot be matched to anything, so each
 * final one closes its own row.
 *
 * `pageEcho`: the frame is a bridge's echo of a prompt the page said on a channel's behalf or its
 * own, never the owner's words (`#353 relay-echo-as-owner`), so it is not drawn or filed as his.
 */
function upsertSpoken(who: string, turn: number | null | undefined, said: string, final: boolean, pageEcho: boolean = false) {
  const anonymous = turn === undefined || turn === null;
  const key = anonymous ? `anon:${session.anonTurn}:${who}` : `${turn}:${who}`;
  // A new turn from a speaker ends that speaker's previous one.
  for (const [other, entry] of session.liveTurns) {
    if (other !== key && entry.who === who && !entry.stored) {
      storeSpoken(entry);
    }
  }
  let entry = session.liveTurns.get(key);
  if (!entry) {
    entry = { key, who, settled: "", pending: "", li: null, atMs: Date.now(), stored: false, echo: false,
      cut: false, savedCopy: false, takenElsewhere: false };
    // A typed prompt reflected back is already on screen; see `isEchoOfTyped`. A relayed one, or the
    // page's own notice, was never the owner's to show. `#353 relay-echo-as-owner`.
    entry.echo = who === "you" && final && (pageEcho || isEchoOfTyped(said));
    // `#523 voice-self-hearing`. Whether this turn began where the agent's own voice could reach the
    // microphone: only such a turn is judged for its echo, however long it then grows.
    entry.pageEcho = entry.echo;
    entry.agentEcho = false;
    entry.nearAgent = who === "you" && nearAgentSpeech(entry.atMs, ECHO_PARTIAL_GRACE_MS);
    entry.audibleAtStart = entry.nearAgent && agentAudible(entry.atMs);
    entry.undecided = false;
    // `#649 voice-agent-tools`. The text of a turn already completed as heard without text: its words
    // go where the note said they were missing, and the record gets them when the turn is stored.
    const missing = who === "assistant" && !anonymous ? session.noTextRows.get(turn!) : undefined;
    if (missing) {
      session.noTextRows.delete(turn!);
      entry.li = missing;
      noteLateText();
    }
    session.liveTurns.set(key, entry);
  }
  if (final) {
    entry.settled = mergeHypothesis(entry.settled, said);
    entry.pending = "";
  } else {
    entry.pending = said;
  }
  if (who === "assistant") noteAgentSaid(key, spokenText(entry));
  if (entry.nearAgent) judgeAgentEcho(entry, final);
  if (!entry.echo) {
    drawSpoken(entry);
    if (who === "you") deps.noteYesCandidate(key, spokenText(entry), entry.li);
  }
  if (anonymous && final) {
    storeSpoken(entry);
    session.anonTurn += 1;
  } else if (!entry.stored) {
    // `#330 voice-long-turn-drop`. A turn still being said is written down on this device as it
    // grows, so an app killed in the middle of a long answer does not take the answer with it.
    armUnsettledSave();
  }
  return { key, entry };
}


/**
 * `#523 voice-self-hearing`. Judge a user turn begun near the agent's speech for its echo, again as
 * its words grow: an echo is taken off the screen, and a turn that grows into the owner's own words
 * is drawn as his.
 */
function judgeAgentEcho(entry: LiveTurn, final: boolean) {
  const text = spokenText(entry);
  // Begun after the agent fell silent, a turn of fewer than three words cannot be judged yet
  // (`isAgentEcho`): until it can, or it is final, nothing of it is reported as heard or taken as a
  // yes, so no echo's opening words reach the server before the turn is found to be one.
  entry.undecided = !entry.audibleAtStart && !final && answerWords(text).length < 3;
  const echoed = isAgentEcho(text, entry.atMs, entry.audibleAtStart!);
  if (echoed === entry.agentEcho) return;
  entry.agentEcho = echoed;
  entry.echo = echoed || entry.pageEcho!;
  if (!entry.echo) return;
  if (entry.li) {
    const pinned = deps.atBottom(deps.el("scroll-area"));
    if (entry.li.parentNode) entry.li.parentNode.removeChild(entry.li);
    entry.li = null;
    deps.followIfPinned(pinned);
  }
  // What the spoken yes took from the turn's words so far goes with them.
  if (deps.spokenYes.answer && deps.spokenYes.answer.key === entry.key) deps.spokenYes.answer = null;
  const hold = deps.spokenYes.holds.get(entry.key);
  if (hold) deps.releaseYesHold(hold);
}


/** Put a spoken turn's row on screen, or redraw the one it has with what it now says. */
function drawSpoken(entry: LiveTurn) {
  const text = shownSpoken(entry);
  const pinned = deps.atBottom(deps.el("scroll-area"));
  if (entry.li) {
    const li = deps.turnNode(entry.who, text, entry.atMs);
    // `#269 approval-card-inline`. A row held for a yes is redrawn held: never a frame on screen.
    for (const name of ["data-absorbed", "data-answer"]) {
      const value = entry.li.getAttribute(name);
      if (value !== null) li.setAttribute(name, value);
    }
    if (entry.cut) li.setAttribute("data-cut", "");
    const onFold = keepTurnFold(entry.li, li);
    entry.li.replaceWith(li);
    // Without moving the list: a reader scrolled up the call is not taken back to the row each frame.
    if (onFold) li.foldEntry!.fold.focus({ preventScroll: true });
    entry.li = li;
    deps.applySearch();
    deps.followIfPinned(pinned);
  } else {
    entry.li = deps.line(entry.who, text, entry.atMs);
    if (entry.cut) entry.li.setAttribute("data-cut", "");
  }
}


/**
 * `#431 voice-transcript-expand`. A turn redrawn as it grows (`drawSpoken`) keeps the fold it had.
 *
 * A fresh row arrives folded when it is long, and a voice turn has no message id for `expandedIds`
 * to carry its state by, so every frame of a streamed reply drew it folded again: a reply that grew
 * past the fold threshold while it was spoken snapped closed under the reader, and one they had
 * opened closed on the next frame. So the old row's state goes to the new one, by the one path that
 * folds a row (`setFolded`):
 *
 *   * OPEN OR FOLDED, AS THE READER LEFT IT. The reader's own tap is the only thing that changes it.
 *   * A TURN SHOWN WHOLE STAYS WHOLE. One that was short when drawn, and grew long, was never
 *     folded, and folding it now would be the jump `#47 scrollback-stability` rules out: a long
 *     message arrives folded, and is never folded after the reader has seen it whole. A streamed
 *     reply therefore reads to its end while it is spoken, and a tap folds it like any other.
 *
 * Answers whether the focus is on the old row's fold button, which goes with it: the caller puts a
 * keyboard user on the new one once it is on the page. Shared by the frozen send card the server's
 * copy replaces (`correctSentCard`), which a tap opens as it opens a turn.
 */
function keepTurnFold(was: VoiceRow, li: VoiceRow) {
  const entry = li.foldEntry;
  if (!entry) return false;
  const before = was.foldEntry;
  if (!before || !deps.isFolded(before)) deps.setFolded(entry, false);
  return Boolean(before) && document.activeElement === before!.fold;
}


/**
 * The assistant's answer in progress was cut short: every row of it not yet stored keeps what it
 * said and is marked cut off, on screen now and in the record when it is stored.
 */
function markAnswerCut() {
  for (const entry of session.liveTurns.values()) {
    if (entry.who !== "assistant" || entry.stored || entry.echo || entry.cut) continue;
    entry.cut = true;
    if (entry.li) drawSpoken(entry);
  }
}


/** Store a turn's best text, once. A late frame for it still corrects the row, not the record. */
function storeSpoken(entry: LiveTurn) {
  if (entry.stored) {
    return;
  }
  entry.stored = true;
  // `#330 voice-long-turn-drop`. Another page already filed this turn from the copy this page wrote
  // down as it was said, believing this one gone: filing it again would store it twice.
  const filedElsewhere = entry.takenElsewhere || (entry.savedCopy && unsettledTakenElsewhere());
  if (entry.agentEcho) noteAgentEchoHeard(spokenText(entry));
  if (!entry.echo && !filedElsewhere) {
    // A user turn held for a yes is filed with its hold, which says by the time the request goes —
    // after the handler storing it has judged it — whether the page acted on it. `#269`.
    const hold = entry.who === "you" ? deps.spokenYes.holds.get(entry.key) || null : null;
    deps.recordTurn(entry.who, shownSpoken(entry), { answerOf: hold });
  }
  saveUnsettledTurns();
  // `#270 voice-transcript-markers`. A record drawn below this turn waited for it to be stored.
  deps.flushRecordsAfterLive();
}


/** The turn is over: everything said in it is as good as it is going to get. */
function settleSpoken() {
  for (const entry of session.liveTurns.values()) {
    storeSpoken(entry);
  }
}


// --- a turn in progress outlives the app ---------------------------------------------------------
//
// `#330 voice-long-turn-drop`. A spoken turn is stored once, when it ends (`storeSpoken`), so a phone
// that suspends or kills the installed app in the middle of a long answer took the whole answer with
// it: the app reopened idle, and nothing anywhere held what had been said. So every turn still being
// said is written down on this device as it grows — every UNSETTLED_SAVE_MS while one is open, and at
// once when the page is hidden or unloaded, the last moments a phone promises it. A turn that ends
// normally takes its copy away. A copy left behind by a page that did not come back is filed in the
// server's record the next time the app opens (`recoverUnsettledTurns`), marked as cut off.
//
// ONE KEY, one entry per page, because two windows may each hold a call; a page's entry is rewritten
// whole each time, with when it was written, which is also how the next page tells a copy whose page
// is gone (not rewritten for UNSETTLED_ABANDONED_MS) from one whose page is merely in another window.

const UNSETTLED_KEY = "vibe-talk.voice.unsettled";

/** How often a turn still being said is written down on this device. */
const UNSETTLED_SAVE_MS = 3000;

/**
 * How long a page's written-down turn must have gone unrewritten before another page takes its page
 * for gone and files it. Several UNSETTLED_SAVE_MS, so a page that is merely slow is not robbed.
 */
const UNSETTLED_ABANDONED_MS = 15000;

/** This page load's own name among the copies, so it never files its own. */
let PAGE_INSTANCE = "";
function initializePageInstance() {
  PAGE_INSTANCE = `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}

/** The ids a turn's server record may be filed under; the same rule `conversationIdFrom` keeps. */
const RECORDABLE_CONVERSATION = /^[A-Za-z0-9_-]{1,64}$/;

/** What a recovered turn is marked with: the app closed under it. */
const APP_CLOSED_MARK = "(cut off — the app closed before this turn ended)";


let unsettledTimer: Timer | null = null;

/** Whether this page has a copy written down right now. */
let unsettledWritten = false;


/** Every page's written-down turns, by page; an empty object when there are none or storage is out. */
function readUnsettled(): Record<string, UnsettledCopy> {
  try {
    const held = JSON.parse(deps.deviceStorage().getItem(UNSETTLED_KEY) || "null");
    return held && typeof held === "object" && !Array.isArray(held) ? held : {};
  } catch (_error) {
    return {};
  }
}


/** Write every page's copies back, or take the key away when none is left. */
function writeUnsettled(all: Record<string, UnsettledCopy>) {
  try {
    if (Object.keys(all).length === 0) deps.deviceStorage().removeItem(UNSETTLED_KEY);
    else deps.deviceStorage().setItem(UNSETTLED_KEY, JSON.stringify(all));
    return true;
  } catch (_error) {
    // Storage refused: the turn is stored at its end as ever, and only an app killed first loses it.
    return false;
  }
}


/** The turns of this call still being said, which have words, and no other page has filed. */
function unsettledEntries() {
  return [...session.liveTurns.values()].filter((entry) =>
    !entry.stored && !entry.echo && !entry.takenElsewhere && spokenText(entry));
}


/** Write this page's turns in progress down now, or take its copy away when there are none. */
function saveUnsettledTurns() {
  if (unsettledWritten && !(PAGE_INSTANCE in readUnsettled())) {
    // Another page took this one for gone and filed what its copy held. Those turns are in the
    // record now: written down again they would be filed twice, and stored again at their end too.
    for (const entry of session.liveTurns.values()) {
      if (entry.savedCopy && !entry.stored) entry.takenElsewhere = true;
    }
    unsettledWritten = false;
  }
  const entries = session.conversationId && RECORDABLE_CONVERSATION.test(session.conversationId)
    ? unsettledEntries()
    : [];
  if (entries.length === 0 && !unsettledWritten) return;
  const all = readUnsettled();
  if (entries.length === 0) {
    delete all[PAGE_INSTANCE];
    writeUnsettled(all);
    unsettledWritten = false;
    return;
  }
  all[PAGE_INSTANCE] = {
    conversation: session.conversationId,
    saved_at: Date.now(),
    turns: entries.map((entry) => ({ who: entry.who, text: shownSpoken(entry), at: entry.atMs })),
  };
  if (writeUnsettled(all)) {
    unsettledWritten = true;
    for (const entry of entries) entry.savedCopy = true;
  }
}


/** Keep writing this page's turns in progress down every UNSETTLED_SAVE_MS until there are none. */
function armUnsettledSave() {
  if (unsettledTimer !== null) return;
  unsettledTimer = setTimeout(() => {
    unsettledTimer = null;
    saveUnsettledTurns();
    if (unsettledWritten) armUnsettledSave();
  }, UNSETTLED_SAVE_MS);
}


/** Did another page take this page's copy, believing it gone, and file what it held? */
function unsettledTakenElsewhere() {
  return unsettledWritten && !(PAGE_INSTANCE in readUnsettled());
}


/**
 * File the turns a page that did not come back had written down: in the conversation each was said
 * in, marked as cut off when the app closed, one at a time and in order. Before the record is read,
 * so the restored transcript shows them where they were said. Returns how many were filed.
 *
 * A copy is taken out of storage before it is filed, so two pages opening at once cannot both file
 * it; one that fails to file goes back, for the next opening to try again.
 */
async function recoverUnsettledTurns() {
  const all = readUnsettled();
  let filed = 0;
  for (const [instance, held] of Object.entries(all)) {
    if (instance === PAGE_INSTANCE) continue;
    const valid = held && typeof held === "object" && typeof held.conversation === "string" &&
      RECORDABLE_CONVERSATION.test(held.conversation) && Array.isArray(held.turns);
    if (valid && Date.now() - Number(held.saved_at || 0) < UNSETTLED_ABANDONED_MS) continue;
    const now = readUnsettled();
    delete now[instance];
    writeUnsettled(now);
    if (!valid) continue;
    const turns = held.turns.filter((turn) => turn && typeof turn.text === "string" && turn.text.trim());
    for (let i = 0; i < turns.length; i += 1) {
      const turn = turns[i]!;
      // Cut off already, by the time it was written down: the app closing is the later word on it.
      const said = turn.text.endsWith(` ${CUT_OFF_MARK}`) ? turn.text.slice(0, -CUT_OFF_MARK.length - 1) : turn.text;
      // Cut, when it must be, before the mark: the mark is what says the words stop short.
      const text = `${said.slice(0, RECOVERED_TURN_MAX_CHARS - APP_CLOSED_MARK.length - 1)} ${APP_CLOSED_MARK}`;
      try {
        await deps.api(`/api/v1/conversations/${held.conversation}/turns`, {
          method: "POST",
          body: { speaker: turn.who === "you" ? "you" : "agent", text },
        });
        filed += 1;
      } catch (error) {
        if (recoveryMayRetry(error as {status?: number; code?: string} | null)) {
          const back = readUnsettled();
          back[instance] = { ...held, turns: turns.slice(i) };
          writeUnsettled(back);
        } else {
          deps.addDetail(`a turn an earlier page was still saying when the app closed could not be filed: ${(error as Error).message}`);
        }
        break;
      }
    }
  }
  return filed;
}


/**
 * The longest recovered turn filed: under the server's own cap on a stored turn, so a recovery is not
 * refused for its length and retried at every opening.
 */
const RECOVERED_TURN_MAX_CHARS = 7900;


// --- startup timing ------------------------------------------------------------------------------
//
// `#11 voice-connect-latency`. "It takes a while to start talking" is not actionable; which phase
// takes the while is. Each mark is milliseconds since the reader pressed Start, and the record
// holds phase names, the protocol, and integers — nothing said, no identifiers — because the
// server logs it and a log is the wrong place for a conversation. The server's field allowlist
// refuses anything else.

function beginStartupTiming(chat: boolean) {
  // `socket` stays null unless the socket never opened; see `noteSocketFailure`.
  session.timing = { origin: Date.now(), chat, marks: {}, sent: false, socket: null };
}


/** Record a phase the FIRST time it happens in this call. Later occurrences are not startup. */
function markStartup(phase: string, atMs?: number) {
  const timing = session.timing;
  if (!timing || timing.sent || phase in timing.marks) {
    return;
  }
  timing.marks[phase] = Math.max(0, Math.round((atMs === undefined ? Date.now() : atMs) - timing.origin));
}


/** Once per call: after the greeting, or at hang-up if the call ended before one. */
function sendStartupTiming() {
  const timing = session.timing;
  if (!timing || timing.sent || session.protocol !== "vibe-talk-v1") {
    return;
  }
  timing.sent = true;
  const marks = timing.marks;
  const seconds = (ms: number) => `${(ms / 1000).toFixed(1)} s`;
  const shown = [];
  if (marks.provider_ready !== undefined) {
    shown.push(`ready ${seconds(marks.provider_ready)}`);
  }
  if (marks.greeting_audible !== undefined) {
    shown.push(`audible ${seconds(marks.greeting_audible)}`);
  }
  if (shown.length > 0) {
    deps.addDetail(shown.join(" · "));
  }
  // Diagnostics, not the call: a failure here is not worth a word on the screen.
  deps.api("/api/v1/voice-timing", {
    method: "POST",
    body: { protocol: session.protocol, chat: timing.chat, ...marks, ...(timing.socket || {}) },
  }).catch(() => {});
}


// --- is the voice service answering? ------------------------------------------------------------
//
// `voice-unresponsive-signal`. A live call once stayed green for its whole length while every turn
// produced nothing anyone could hear or read: turns completed, each carried PCM, and every sample
// of it was silence. So a turn is judged by what it produced, not by whether bytes arrived, and
// only from frames `vibe-talk-v1` already carries. The page cannot see WHY a service stopped
// answering and does not guess; the README lists what it cannot observe.

// The quietest sample peak that counts as sound: 256 of 32767, about -42 dBFS. Digital silence is
// 0 and dither a few units, while speech peaks in the thousands, so this sits far from both.
const AUDIBLE_PEAK = 256;


// Consecutive silent turns before the page says so. One is not enough: a service may close an
// empty turn for background noise it took for speech, or finish a tool-call-only response with
// little or no speech before the answer follows. The greeting is the exception — it is supposed
// to speak — so a silent greeting is reported after one.
const SILENT_TURNS_TO_REPORT = 2;


// How long a turn the PAGE started (the greeting, or a typed prompt) may produce nothing at all:
// the same bound read-aloud already gives this protocol. Spoken turns are detected by the service,
// so the page never knows when one began and cannot time it.
const NO_REPLY_MS = 15000;


// How much inaudible audio makes an INTERRUPTED turn count as silent. A listener hearing nothing
// says "hello?" over it, and a server may take that for barge-in; without this, talking over a
// dead service would hide it. "Inaudible" is the whole turn: one sample at AUDIBLE_PEAK anywhere
// makes it heard, so the quiet gaps between a healthy reply's words never add up to silence. Dead
// turns have been seen as short as 0.9 s, and a reply's lead-in before its first word is a small
// fraction of that, so half a second of nothing at all is not the start of an answer.
const INTERRUPTED_SILENT_MS = 500;


const UNRESPONSIVE_STATUS = "The voice service is not responding.";


function beginHealth(chat: boolean) {
  session.health = {
    chat,
    openedAt: null,
    turns: 0,
    // Consecutive completed turns that produced nothing audible or readable.
    silentRun: 0,
    // The turn in progress.
    heard: false,
    peak: 0,
    audioMs: 0,
    // ...and whether any assistant text came with it. `#330 voice-long-turn-drop`.
    text: false,
    greetingPending: false,
    // The turn in progress carried an `error` frame, which is its verdict.
    errored: false,
    // The last silent turn, which is what a report describes.
    lastPeak: 0,
    lastAudioMs: 0,
    unresponsive: false,
    // Whether this episode's unresponsive state was reported, so its recovery may be.
    episodeReported: false,
    reported: new Set(),
    noReplyTimer: null,
    // `#268 voice-call-drops`. Whether how this call ended has been recorded (`postCallEnd`).
    endPosted: false,
  };
}


/** One PCM frame: its duration, and whether any sample in it could be heard. */
function noteAgentAudio(bytes: Uint8Array) {
  const health = session.health;
  if (!health) {
    return;
  }
  const pcm = new Int16Array(bytes.buffer, bytes.byteOffset, Math.floor(bytes.byteLength / 2));
  let peak = 0;
  for (let i = 0; i < pcm.length; i += 1) {
    const magnitude = Math.abs(pcm[i]!);
    if (magnitude > peak) {
      peak = magnitude;
    }
  }
  health.peak = Math.max(health.peak, peak);
  health.audioMs += (pcm.length / session.outputRate) * 1000;
  if (peak >= AUDIBLE_PEAK) {
    heardReply();
  }
}


function noteAgentText() {
  if (session.health) {
    session.health.text = true;
    heardReply();
  }
}


/** Something audible or readable arrived: the service is answering. */
function heardReply() {
  const health = session.health!;
  health.heard = true;
  clearNoReply();
  if (!health.unresponsive) {
    return;
  }
  health.unresponsive = false;
  deps.setState("live");
  deps.setStatus("The voice service is responding again.");
  if (health.episodeReported) {
    health.episodeReported = false;
    postHealth("recovered");
  }
}


/**
 * An `error` frame is the verdict on the turn it arrives in, whether before or after that turn's
 * audio. A server may still close the turn with an ordinary `turn_complete`; counting that as a
 * silent turn, or letting the no-reply bound fire after it, would log a second cause for one
 * failure and paint over the error the page is already showing. An `error` carries no turn number,
 * so the verdict attaches to the NEXT `turn_complete`, whichever turn that closes.
 */
function noteAgentError() {
  const health = session.health;
  if (!health) {
    return;
  }
  health.errored = true;
  clearNoReply();
}


/** A `turn_complete`: was the turn it ends silent, and is that enough to say so? */
function judgeTurn(message: {interrupted?: boolean | null}, closingSegment: boolean) {
  const health = session.health;
  if (!health) {
    return;
  }
  const greeting = health.greetingPending;
  const errored = health.errored;
  health.greetingPending = false;
  health.errored = false;
  const silent = !health.heard;
  const turnPeak = health.peak;
  const turnAudioMs = health.audioMs;
  health.heard = false;
  health.peak = 0;
  health.audioMs = 0;
  health.text = false;
  // Heard is heard, however the turn ended: a reply the listener talked over, or one that carried
  // an `error`, still proves the service was answering, so it ends any run of silent turns before
  // it. Left standing, that run would join the next silent turn into a false report.
  if (!silent) {
    health.silentRun = 0;
  }
  // An interrupted turn was cut short by the listener — unless what it cut short was
  // INTERRUPTED_SILENT_MS or more of audio nobody could hear. An empty answer to the page closing
  // its own audio segment is the server acknowledging that boundary, not a reply. Neither is
  // evidence of silence. A turn that carried an `error` has had its verdict already.
  const cutShort = message.interrupted === true && !(silent && turnAudioMs >= INTERRUPTED_SILENT_MS);
  if (errored || cutShort || (closingSegment && silent)) {
    return;
  }
  health.turns += 1;
  if (!silent) {
    return;
  }
  health.silentRun += 1;
  health.lastPeak = turnPeak;
  health.lastAudioMs = turnAudioMs;
  if (greeting) {
    reportHealth("silent_greeting");
  } else if (health.silentRun >= SILENT_TURNS_TO_REPORT) {
    reportHealth("silent_turns");
  }
}


function armNoReply() {
  const health = session.health;
  if (!health) {
    return;
  }
  clearNoReply();
  health.noReplyTimer = setTimeout(() => {
    health.noReplyTimer = null;
    if (session.health === health) {
      reportHealth("no_reply");
    }
  }, NO_REPLY_MS);
}


function clearNoReply() {
  const health = session.health;
  if (health && health.noReplyTimer !== null) {
    clearTimeout(health.noReplyTimer);
    health.noReplyTimer = null;
  }
}


/**
 * Say the service is not answering, on the page and — once per cause per call — in the app log.
 *
 * An `error` frame already turned the page red through `showError`, so it only gains the log line.
 */
function reportHealth(cause: string) {
  const health = session.health;
  if (!health) {
    return;
  }
  if (cause !== "error_frame") {
    if (!health.unresponsive) {
      health.unresponsive = true;
      deps.setState("unresponsive");
    }
    // Every time, not just the first: completing the turn may have written an ordinary status.
    deps.setStatus(UNRESPONSIVE_STATUS);
  }
  if (health.reported.has(cause)) {
    return;
  }
  health.reported.add(cause);
  if (cause !== "error_frame") {
    health.episodeReported = true;
  }
  postHealth(cause);
}


/**
 * Enum names and integers only; the server refuses anything else.
 *
 * @param {string} cause
 * @param {{audioMs: number, peak: number} | null} [turn] the turn the report describes; the last
 *   silent one when not given
 */
function postHealth(cause: string, turn: { audioMs: number; peak: number; } | null = null) {
  const health = session.health!;
  if (session.protocol !== "vibe-talk-v1") {
    return;
  }
  const sinceOpen = health.openedAt === null ? 0 : Date.now() - health.openedAt;
  // Diagnostics, not the call: a failure here is not worth a word on the screen.
  deps.api("/api/v1/voice-health", {
    method: "POST",
    body: {
      protocol: session.protocol,
      chat: health.chat,
      cause,
      since_open_ms: Math.max(0, Math.round(sinceOpen)),
      turns: health.turns,
      silent_turns: health.silentRun,
      audio_ms: Math.round(turn ? turn.audioMs : health.lastAudioMs),
      peak: turn ? turn.peak : health.lastPeak,
    },
  }).catch(() => {});
}


/**
 * The words a turn's row shows when it was HEARD but none of its words arrived as text. `#330
 * voice-long-turn-drop`: a long spoken summary played and never appeared, and nothing on the screen
 * said the transcript was missing rather than that nothing was said.
 */
const SPOKEN_WITHOUT_TEXT = "spoken reply — no text received";


/**
 * A turn completed having carried audible audio and no assistant text: say so where its text would
 * have been, keep that note in the record (as the page's note, never as the assistant's words), and
 * report it — once a call, as `audio_without_text`. The service is answering, so this is not the
 * unresponsive state.
 *
 * `#649 voice-agent-tools`. Seen live 2026-10-28: a `vibe-talk-v1` bridge sent an answer's text
 * AFTER its `turn_complete`, so its words landed in a row of their own further down while this note
 * stood where they belonged. The row is kept by turn number, and the turn's late text takes its place
 * (`upsertSpoken`), reported once a call as `late_text`.
 *
 * @param {{audioMs: number, peak: number}} turn
 * @param {boolean} cut whether the turn was cut short
 * @param {number | null} [number] the turn's number, when the frame said
 */
function noteSpokenWithoutText(turn: { audioMs: number; peak: number; }, cut: boolean, number: number | null = null) {
  const said = cut ? `${SPOKEN_WITHOUT_TEXT} ${CUT_OFF_MARK}` : SPOKEN_WITHOUT_TEXT;
  const li = deps.line("assistant", said);
  li.setAttribute("data-no-text", "");
  if (number !== null) session.noTextRows.set(number, li);
  if (cut) li.setAttribute("data-cut", "");
  deps.recordTurn("note", said);
  deps.addDetail("a spoken turn played audio and no text of it arrived");
  const health = session.health;
  if (!health || health.reported.has("audio_without_text")) return;
  health.reported.add("audio_without_text");
  postHealth("audio_without_text", turn);
}


/** `#649 voice-agent-tools`. A turn's text arrived after its `turn_complete`: reported once a call. */
function noteLateText() {
  const health = session.health;
  if (!health || health.reported.has("late_text")) return;
  health.reported.add("late_text");
  postHealth("late_text");
}


/**
 * `#649 voice-agent-tools`. How long a Hang up keeps listening for the text of an answer still on its
 * way: the owner, 2026-10-28, "usually the last message I hear from it before I hang up, I don't
 * receive it". The microphone, the sound and the call end at the tap as before; only the socket stays
 * open this long after `quit`, for transcript frames, and closes at once when the service closes it.
 */
const HANGUP_TEXT_WAIT_MS = 4000;


/** What marks an answer's words that arrived after the reader hung up. */
const AFTER_HANG_UP_MARK = "(text arrived after you hung up)";


/**
 * Keep `socket`, which the call has let go, for HANGUP_TEXT_WAIT_MS: the assistant's text that still
 * arrives on it is shown — in the row of its turn when one is on screen — and filed in the
 * conversation it was said in. Nothing else on it is acted on.
 *
 * @param {WebSocket} socket
 * @param {string | null} conversationId
 * @param {Map<number, {li?: VoiceRow, entry?: LiveTurn}>} rows each turn's row on screen: a note's, or a live turn's entry
 */
function listenForTextAfterHangUp(socket: WebSocket, conversationId: string | null, rows: Map<number, {li?: VoiceRow; entry?: LiveTurn}>) {
  /** @type {Map<number | null, string>} */
  const said: Map<number, string> = new Map();
  let done = false;
  const finish = () => {
    if (done) return;
    done = true;
    clearTimeout(timer);
    try {
      socket.close();
    } catch (_error) {
      // Already closing.
    }
    for (const [turn, text] of said) {
      const row = turn === null ? undefined : rows.get(turn);
      // A live row is read now: the teardown redrew it, marked cut off, after it was noted.
      const li = row ? (row.entry ? row.entry.li : row.li) : null;
      const words = row && row.entry ? mergeHypothesis(spokenText(row.entry), text) : text;
      const shown = `${words} ${AFTER_HANG_UP_MARK}`;
      if (li && li.parentNode) {
        li.replaceWith(deps.turnNode("assistant", shown, Date.now()));
      } else {
        deps.line("assistant", shown);
      }
      fileTurnIn(conversationId, "agent", shown);
    }
  };
  socket.onmessage = (event: MessageEvent<unknown>) => {
    if (typeof event.data !== "string") return;
    let frame = null;
    try {
      frame = JSON.parse(event.data as string);
    } catch (_error) {
      return;
    }
    if (!frame || frame.type !== "transcript" || frame.role === "user" || frame.final === false) return;
    const text = typeof frame.text === "string" ? frame.text.trim() : "";
    if (!text) return;
    const turn = typeof frame.turn === "number" ? frame.turn : null;
    said.set(turn, mergeHypothesis(said.get(turn) || "", text));
  };
  socket.onclose = (event: CloseEvent) => {
    deps.addDetail(`closed with code ${event.code}${event.reason ? `: ${event.reason}` : ""}`);
    finish();
  };
  const timer = setTimeout(finish, HANGUP_TEXT_WAIT_MS);
}


/** File one turn in `conversationId`, as `recordTurn` does for the call in progress. */
function fileTurnIn(conversationId: string | null, speaker: string, text: string) {
  if (deps.recordingBroken || !conversationId || !text) return;
  deps.recordQueue = deps.recordQueue.then(() => {
    if (deps.recordingBroken) return undefined;
    return deps.api(`/api/v1/conversations/${conversationId}/turns`, {
      method: "POST",
      body: { speaker, text },
    }).catch(() => {});
  });
}


// --- a voice session lost in the middle of a call ------------------------------------------------
//
// `#352 voice-reconnect-on-bridge-error`. Seen live: after an interrupt a `vibe-talk-v1` bridge never
// started the next turn, and its watchdog sent `error` "turn 4 never started after an interrupt;
// reconnect". An `error` ends the protocol's session, and the page ended the call with it, although
// the bridge said how to recover, the microphone was still open and the owner had not hung up. So an
// error that asks the client to reconnect, once the session has started, carries the call on a new
// session instead. The page says goodbye to the old session and asks the server for a fresh one (a
// new signed address, never the old one). It keeps the microphone, the audio, the conversation and
// every channel message still to be read out. It hands the new session this call's record where that
// session takes one and Resuming is on, and otherwise says it starts fresh. And it sends the turn the
// bridge lost again: once, and only a turn the bridge named as never started with nothing of an
// answer to it arrived, since a turn that began may have acted. The transcript, the line under the
// call, the status line, the connection details and the stored record say so.
//
// Nothing else reconnects: not a hang-up, not a drop, not any other error, not an error before the
// session started, and never the hosted vendor's protocol. And not for ever: a request within
// RECONNECT_MIN_GAP_MS of the last reconnect, or past RECONNECTS_PER_CALL_MAX in one call, ends the
// call as `reconnect_limit`, and a reconnect that cannot get a working session ends it as
// `reconnect_failed`, each said in red as an `error` frame's end is.

// The bridge's words that ask for a reconnect (`RECONNECT_ASKED`), the lost turn it names (`LOST_TURN`),
// the bounds (`RECONNECT_MIN_GAP_MS`, `RECONNECTS_PER_CALL_MAX`) and the wait for this call's records
// (`RECONNECT_RECORDS_WAIT_MS`) are web/src/core/call's, with the decisions (`#624
// voice-page-modularity`).

// What the transcript's "reconnected" line says, by what the new session was handed and what went
// again (`RECONNECT_SEAM_DETAILS`), is web/src/core/call's (`reconnectSeamDetail`).

// Why a reconnected call's new session was handed nothing (`RECONNECT_FRESH`) is web/src/core/call's.

/**
 * @typedef {object} ReconnectAttempt
 * @property {string} said the voice service's words
 * @property {"relay" | "typed" | "none" | "unsent"} resend what goes to the new session again
 * @property {{text: string, kind: "notice", relays: {channel: string}[]} | null} notice the page's
 *   word on a send card the lost session never answered
 * @property {{resume: Resume | null, why: string | null} | null} recap what the new session is to be handed
 * @property {Element | null} seam the transcript's line for it, once drawn
 */

/**
 * Does the voice service's `error` ask the client to reconnect, on a `vibe-talk-v1` session that
 * started? Before `session_started` there is no session to carry on from.
 *
 * @param {string} said the error's words
 */
function reconnectAsked(said: string) {
  return callReconnectAsked(session, said);
}


/**
 * The voice service lost a turn and asked to reconnect: carry the call on a new session, or end it
 * at the bound.
 *
 * @param {string} said the error's words
 */
function reconnectOrEnd(said: string) {
  const now = Date.now();
  const verdict = reconnectVerdict(session, now);
  if (verdict === "count") {
    const times = session.reconnects;
    endCall("reconnect_limit", said, `a lost turn after ${times} reconnects in this call`);
    deps.showError(
      `The voice service lost a turn again after the page had reconnected this call ${times} times, ` +
        `so the call has ended: ${said}. Tap Talk to start a new call.`,
      { stay: true }
    );
    return;
  }
  if (verdict === "soon") {
    endCall("reconnect_limit", said, "a second lost turn within a minute of reconnecting");
    deps.showError(
      "The voice service lost a turn again less than a minute after the page reconnected, so the " +
        `call has ended: ${said}. Tap Talk to start a new call.`,
      { stay: true }
    );
    return;
  }
  reconnectCall(said, now);
}


/**
 * Carry the call on a new session. Everything up to asking the server for that session happens here,
 * in the handler of the error that asked for it: a bridge closes the lost session's socket straight
 * after its error, and that close must find the socket already let go, not the call's.
 *
 * @param {string} said the error's words
 * @param {number} now when the error arrived, by the wall clock
 */
function reconnectCall(said: string, now: number) {
  const lost = session.socket;
  // This socket's end, recorded as the session it was; the call goes on and records its own end later.
  postCallEnd("reconnected", null, said);
  const health = session.health;
  if (health) {
    health.endPosted = false;
    // The error's verdict and what the lost turn produced belong to the lost session: the new
    // session's first turn is judged from nothing.
    health.errored = false;
    health.heard = false;
    health.peak = 0;
    health.audioMs = 0;
    health.text = false;
    health.greetingPending = false;
  }
  session.reconnects += 1;
  session.lastReconnectAt = now;
  // `#512 voice-recap-private-stack`. A recap still being asked about was the lost session's: taken
  // back, and the answer to that still settles the call's start marker, which waits for it.
  const recapAnswerPending = deps.abandonWatchedRecap();
  if (session.armedRecap !== null) deps.withdrawProfileRecap(session.armedRecap);
  // What the lost session said is settled: an answer still arriving keeps its words, marked cut off.
  if (session.replyArriving) markAnswerCut();
  settleSpoken();
  saveUnsettledTurns();
  // The new session numbers its turns from the start.
  session.liveTurns = new Map();
  session.noTextRows = new Map();
  session.anonTurn = 0;
  session.lastTranscriptKey = null;
  /** @type {ReconnectAttempt} */
  const attempt: ReconnectAttempt = { said, resend: "none", notice: null, recap: null, seam: null };
  queueLostTurn(attempt, said);
  // Goodbye to the lost session, and nothing more from it.
  if (failureTimer !== null) {
    clearTimeout(failureTimer);
    failureTimer = null;
  }
  if (lost) {
    sayGoodbye(lost);
    lost.onopen = null;
    lost.onmessage = null;
    lost.onerror = null;
    lost.onclose = (event) => {
      deps.addDetail(`closed with code ${event.code}${event.reason ? `: ${event.reason}` : ""}`);
    };
    lost.close();
  }
  // The turn state goes back to a session not yet started; the call does not.
  resetVibeTalkTurnState();
  clearNoReply();
  session.sessionStarts = 0;
  if (!recapAnswerPending) deps.endReplayAnswerWait();
  session.connected = false;
  session.cutForTyped = false;
  // The capture graph sends to the socket it was built on, so it is built again on the new one. The
  // microphone stays open: no second permission prompt, and Mute keeps its place.
  if (session.node) {
    session.node.disconnect();
    session.node.onaudioprocess = null;
    session.node = null;
  }
  if (session.source) {
    session.source.disconnect();
    session.source = null;
  }
  // A draft read back to the lost session is not one the new session heard: a yes to it is not a yes.
  deps.resetSpokenYes("replaced_session");
  session.reconnect = attempt;
  deps.setState("working");
  deps.setStatus("Reconnecting after a lost turn…");
  deps.addDetail(`reconnected after the voice service lost a turn: ${said}`);
  renderControls();
  continueReconnect(attempt).catch((error) => {
    if (session.reconnect === attempt) failReconnect(String((error && (error as Error).message) || error));
  });
}


/**
 * Decide what of the lost session's turn goes to the new session, and queue it. A text turn goes
 * again only when the bridge named it as the turn that never started and none of its answer arrived:
 * a turn that began may have acted, a tool call with nothing said, and sent twice it would act twice.
 * And it goes again once: a turn already sent again, which the new session lost too, is not sent a
 * third time, however long after the last reconnect that came. Its relayed messages are said not to
 * have been read out, and a typed message is left to the reader to send again. Spoken words are
 * never sent again as text. The turn in flight is let go here, whatever is decided, so it is counted
 * once if the call then ends.
 *
 * @param {ReconnectAttempt} attempt
 * @param {string} said the error's words
 */
function queueLostTurn(attempt: ReconnectAttempt, said: string) {
  const inFlight = session.turnInFlight;
  session.turnInFlight = null;
  if (!inFlight) return;
  const named = LOST_TURN.exec(said);
  const lostTurn = named ? Number(named[1]) : null;
  const sent = inFlight.turn !== null ? inFlight.turn : (inFlight.after ?? 0) + 1;
  let why = "";
  if (inFlight.answered) why = "some of its answer had arrived";
  else if (inFlight.resent) why = "it had been sent again once already";
  else if (lostTurn === null) why = "the voice service did not say it never started";
  else if (lostTurn !== sent) why = `the voice service named turn ${lostTurn}, not the page's turn ${sent}`;
  if (!why && inFlight.kind === "notice") {
    // The page's word on a send card goes only to a session that has taken the record of the card.
    attempt.notice = { text: inFlight.text, kind: "notice", relays: [] };
    return;
  }
  if (!why) {
    session.pendingPrompts.unshift({ text: inFlight.text, kind: inFlight.kind, relays: inFlight.relays, resent: true });
    attempt.resend = inFlight.kind === "relay" ? "relay" : "typed";
    deps.addDetail("the turn the voice service lost goes to the new session again");
    return;
  }
  deps.addDetail(`the turn the voice service lost does not go again: ${why}`);
  if (inFlight.kind === "relay" && !inFlight.answered) reportUnrelayed(inFlight.relays, "lost");
  if (inFlight.kind === "typed" && !inFlight.answered) attempt.resend = "unsent";
}


/**
 * The rest of a reconnect, which waits on the server: a new session, the record to hand it, and its
 * socket. Each step first checks the attempt is still the call's: Hang up, or a failure, ends it.
 *
 * @param {ReconnectAttempt} attempt
 */
async function continueReconnect(attempt: ReconnectAttempt) {
  let minted;
  try {
    minted = await openVoiceSession();
  } catch (error) {
    if (session.reconnect === attempt) failReconnect(`the server gave no new voice session: ${(error as Error).message}`);
    return;
  }
  if (session.reconnect !== attempt) return;
  if (minted.protocol !== "vibe-talk-v1") {
    failReconnect("the server now offers a different kind of voice session");
    return;
  }
  session.providerName = String(minted.provider || deps.conversationalVoice.name);
  session.inputRate = minted.input_sample_rate || 16000;
  session.outputRate = minted.output_sample_rate || 16000;
  attempt.recap = await fetchCallRecap();
  if (session.reconnect !== attempt) return;
  // `#512 voice-recap-private-stack`. Where the bridge takes the recap in the profile it fetches, it
  // is armed now, just before the new socket opens.
  if (attempt.recap.resume && deps.resumeViaProfile && session.conversationId) {
    const armed = await deps.armProfileRecap(session.conversationId);
    if (session.reconnect !== attempt) {
      if (armed.armed) deps.withdrawProfileRecap(armed.armed.profileRecap);
      return;
    }
    if (armed.armed) attempt.recap = { resume: { ...attempt.recap.resume, ...armed.armed }, why: null };
    else if (armed.why !== "unavailable") attempt.recap = { resume: null, why: armed.why || "empty" };
  }
  const socketUrl = voiceSocketUrl(minted.websocket_url);
  const socket = deps.browserPorts().socket.open(socketUrl);
  socket.binaryType = "arraybuffer";
  session.socket = socket;
  const voiceTarget = String(minted.websocket_url).split(/[?#]/u, 1)[0]!
    .replace(/^(wss?:\/\/)[^/@]+@/u, "$1");
  deps.addDetail(`reconnecting to ${minted.provider || "voice provider"}, ${session.protocol}, at ${voiceTarget}`);
  renderControls();
  wireCallSocket(socket, socketUrl, null, session.chat, attempt);
}


/**
 * This call's record as the server would replay it, for the session a reconnect opens, with why
 * there is none: `off` (the server's replay, or this device's Resuming switch), `empty` or `failed`.
 * Never throws. Unlike a new call's (`fetchResume`), it says nothing to Settings: how the call
 * started is unchanged.
 *
 * @returns {Promise<{resume: Resume | null, why: string | null}>}
 */
async function fetchCallRecap() {
  if (!deps.resumeAllowed || !deps.resumeWanted()) return { resume: null, why: "off" };
  const conversationId = session.conversationId;
  if (!conversationId || !session.recordedAny) return { resume: null, why: "empty" };
  // The turns settled a moment ago are still on their way to the server: wait for them, a while.
  const queued = deps.recordQueue;
  /** @type {ReturnType<typeof setTimeout> | null} */
  let timer: number | null = null;
  await Promise.race([
    queued,
    new Promise((resolve: (value: unknown) => void) => {
      timer = setTimeout(resolve, RECONNECT_RECORDS_WAIT_MS);
    }),
  ]);
  if (timer !== null) clearTimeout(timer);
  try {
    const payload = await deps.api(`/api/v1/conversations/${encodeURIComponent(conversationId)}/replay`) as (Resume & {enabled: boolean; included: number}) | null;
    if (!payload || payload.enabled !== true || !(payload.included > 0) || !payload.text) {
      return { resume: null, why: "empty" };
    }
    return { resume: payload, why: null };
  } catch (error) {
    deps.addDetail(`this call's record could not be read for the new session: ${(error as Error).message}`);
    return { resume: null, why: "failed" };
  }
}


/**
 * A reconnect could not get a working session: the call ends, said in red.
 *
 * @param {string} why
 */
function failReconnect(why: string) {
  endCall("reconnect_failed", why, `a reconnect could not get a working session: ${why}`);
  deps.showError(
    `The voice service lost a turn and the page could not reconnect: ${why}. Tap Talk to start a new call.`,
    { stay: true }
  );
}


/**
 * The session a reconnect opened has started: hand it this call's record where it lists `replay` and
 * one was fetched, otherwise start it fresh and say why. Not the call's start marker, which was filed
 * when the call began, nor what Settings says about how it began.
 *
 * @param {VibeTalk.VibeTalkV1ServerFrame} message the `session_started`
 * @param {ReconnectAttempt} attempt
 */
function offerReplayAtReconnect(message: VibeTalk.VibeTalkV1ServerFrame, attempt: ReconnectAttempt) {
  const listed = message.type === "session_started" && Array.isArray(message.capabilities) &&
    message.capabilities.includes("replay");
  const fetched = attempt.recap || { resume: null, why: "off" };
  if (session.recap && session.recap.state === "sent" && !session.recap.reconnected) {
    deps.addDetail("the conversation this call resumed from was not sent again: only this call's record goes");
  }
  const armed = fetched.resume && typeof fetched.resume.profileRecap === "number" ? fetched.resume.profileRecap : null;
  if (listed && fetched.resume) {
    if (armed !== null) deps.withdrawProfileRecap(armed);
    deps.sendReplayFrame(fetched.resume);
    session.recapAck = "waiting";
    session.recap = { ...deps.recapFor(fetched.resume, "replay_frame"), reconnected: true };
    deps.addDetail("the reconnected session was sent this call's record");
  } else if (armed !== null) {
    // `#512 voice-recap-private-stack`. Armed in the profile the bridge fetched as the session opened.
    session.recapAck = "waiting";
    session.recap = { ...deps.recapFor(fetched.resume, "profile"), reconnected: true };
    deps.addDetail("the reconnected session was handed this call's record in its instructions, if it fetched them");
    deps.watchProfileRecap(armed, settleReconnectProfileRecap);
  } else {
    // Resuming off on the server or this device is said as that, not as a session that takes none.
    const why = listed ? fetched.why || "empty" : fetched.why === "off" ? "off" : "unsupported";
    session.recapAck = null;
    session.recap = { state: "none", why, dropped: 0, reconnected: true };
    deps.addDetail(`the reconnected session was sent no record: ${reconnectFreshWords(why) || deps.freshBecause(why)}`);
  }
  deps.renderCallRecap();
  const notice = attempt.notice;
  if (!notice) return;
  if (session.recapAck === "waiting") {
    session.noticeAwaitingRecap = {
      item: notice,
      timer: setTimeout(() => settleNoticeForRecap(false, "the new session did not say it took the record"),
        deps.REPLAY_ANSWER_WAIT_MS),
    };
  } else {
    deps.addDetail("the page's word on the card was not repeated to the new session: it was handed no record of the card");
  }
}


/**
 * `#512 voice-recap-private-stack`. What became of this call's record, armed in the profile for the
 * session a reconnect opened: taken, as a `replay_ack` that accepted it would say, or not, as a
 * refusal would.
 *
 * @param {boolean | null} served
 */
function settleReconnectProfileRecap(served: boolean | null) {
  if (served === null) {
    session.recapAck = null;
    deps.addDetail("the page could not learn whether the reconnected session took this call's record");
    settleNoticeForRecap(false, "nobody could say whether the new session took the record that says what the card was");
  } else if (served) {
    session.recapAck = "accepted";
    deps.addDetail("the reconnected session took this call's record");
    settleNoticeForRecap(true, "");
  } else {
    session.recapAck = "not_taken";
    session.recap = { state: "none", why: "not_taken", dropped: 0, reconnected: true };
    deps.addDetail("the reconnected session did not take this call's record: no fetch of its instructions carried it");
    reconnectedSessionRefused("did not take");
    settleNoticeForRecap(false, "the new session did not take the record that says what the card was");
  }
  deps.renderCallRecap();
}


/**
 * The page's word on a card, held for the reconnected session's answer to this call's record: sent
 * once that session took the record, which says what the card was, and otherwise let go, said.
 *
 * @param {boolean} taken
 * @param {string} why
 */
function settleNoticeForRecap(taken: boolean, why: string) {
  const held = session.noticeAwaitingRecap;
  if (!held) return;
  session.noticeAwaitingRecap = null;
  clearTimeout(held.timer);
  if (!taken) {
    deps.addDetail(`the page's word on the card was not repeated to the new session: ${why}`);
    return;
  }
  session.pendingPrompts.unshift({ ...held.item, resent: true });
  deps.addDetail("the page's word on the card goes to the new session again");
  advanceVibeTalkInput();
}


/**
 * Say in the transcript, and in the stored record, that the call moved to a new session: what
 * happened and what the new session knows.
 *
 * @param {ReconnectAttempt} attempt
 */
function noteReconnected(attempt: ReconnectAttempt) {
  const handed = session.recap && session.recap.state === "sent" ? "recap" : "fresh";
  const detail = reconnectSeamDetail(handed, attempt.resend);
  const li = deps.transcriptSeam("reconnected", detail);
  // What went again, so a refused record can change what the line says the session knows.
  li.setAttribute("data-reconnected", attempt.resend);
  attempt.seam = li;
  deps.recordTurn("note", `Reconnected: ${detail}`);
}


/**
 * The reconnected session refused this call's record: the call's last "reconnected" line now says
 * that session has seen nothing above it, and so does a second note in the stored record, whose
 * first note said otherwise.
 */
function reconnectedSessionRefused(verb: string = "refused") {
  const rows = deps.leftEl("transcript").children;
  for (let at = rows.length - 1; at >= 0; at -= 1) {
    const resend = rows[at]!.getAttribute("data-reconnected");
    if (resend === null) continue;
    const detail = reconnectSeamDetail("fresh", resend);
    const summary = childByClass(rows[at]!, "seam-summary");
    const body = childByClass(rows[at]!, "seam-detail");
    if (!detail || !summary || !body) return;
    const pinned = deps.atBottom(deps.leftEl("scroll-area"));
    body.textContent = detail;
    summary.setAttribute("title", detail);
    deps.followIfPinned(pinned);
    deps.recordTurn("note", `Reconnected, but the new session ${verb} this call's record: ${detail}`);
    return;
  }
}


/** @param {VibeTalk.VibeTalkV1ServerFrame} message */
function handleVibeTalk(message: VibeTalk.VibeTalkV1ServerFrame) {
  switch (message.type) {
    case "session_started": {
      markStartup("provider_ready");
      deps.noteSessionStartedForSpokenYes();
      deps.heardEpoch += 1;
      // A session, first or replacement, starts with no turn open, and numbers its turns afresh.
      // `#268 voice-call-drops`.
      closeSpokenTurn();
      session.lastCompletedTurn = null;
      deps.addDetail(`voice session ${message.session_id || "started"}`);
      session.v1Ready = true;
      // `#352 voice-reconnect-on-bridge-error`. The session a reconnect asked for: handed this
      // call's record where it takes one, and then sent what the lost session never started.
      const reconnected = session.reconnect;
      // `#331 resume-on-websocket`. Before anything else this call sends: the record of the
      // conversation above, when this session lists `replay`. A replacement session is sent none.
      if (reconnected) {
        session.reconnect = null;
        offerReplayAtReconnect(message, reconnected);
        noteReconnected(reconnected);
        deps.setState("live");
      } else if (session.sessionStarts > 1) {
        deps.noteReplacedV1Session();
      } else {
        deps.offerReplayAtSessionStart(message);
      }
      // `#1040`. Ahead of any held first message below: the open document's context
      // goes before the question it is context for.
      offerDocumentAtSessionStart();
      if (session.chat) {
        // A typed call does not wait for a promised greeting: the protocol does not say whether a
        // server greets a client that never sends `audio_start`, nor which turn number a greeting
        // carries, so waiting would hold every typed call's first prompt behind a greeting that
        // may never come. A prompt typed once the greeting is audibly or visibly arriving is held
        // until it completes (`replyArriving`). KNOWN LIMITATION: a prompt sent BEFORE any of the
        // greeting arrives, including one queued before `session_started`, cannot be told apart
        // from it. The greeting's first text disarms that prompt's no-reply bound, its
        // `turn_complete` is taken as the prompt's, and a second queued prompt is sent while the
        // first is still being answered. Since `#241 voice-composer` that is every typed call's
        // FIRST message, which is what starts the call and is held until here. The transcript is
        // still right, rows being kept per turn number, and the next message waits behind the
        // reply it did not ask for (`replyArriving`) once any of that reply has arrived.
        //
        // `#352`. A reconnect's new session that promises a greeting IS waited for: what goes again
        // there is a turn already sent once — the lost relay, or the reader's message — and sent
        // into the greeting, the greeting would be taken for its answer and the turn let go unread,
        // and its echo after the greeting drawn as the reader's words. The no-reply bound holds a
        // greeting that never comes. Not judged as a spoken greeting is (`greetingPending`): a typed
        // call hears no audio, so a greeting of audio alone would be taken for a silent one.
        if (reconnected && message.greeting === true) {
          session.waitingForGreeting = true;
          armNoReply();
        }
        deps.setStatus(
          session.waitingForGreeting
            ? "Reconnected — the assistant is joining…"
            : reconnected
              ? "Reconnected — type a message."
              : "Connected — type a message."
        );
        // The held first message: queued and, with nothing ahead of it, on the wire in this call.
        deliverHeldTurn();
        advanceVibeTalkInput();
        break;
      }
      session.waitingForGreeting = message.greeting === true;
      // `#523 voice-self-hearing`. Every session, first or reconnected, holds the microphone while
      // whatever its agent says first plays here, until it has played out, not only until it was
      // sent: one that does not promise a greeting may still greet (`settleGreetingPlayout`).
      session.greetingPlayedOut = false;
      session.greetingPromised = message.greeting === true;
      if (session.waitingForGreeting && session.health) {
        session.health.greetingPending = true;
        armNoReply();
      }
      if (reconnected && !session.node) {
        // `#352`. The microphone's graph goes to the new socket, paused, before anything is sent on it.
        session.capturePaused = true;
        startCapture(session.socket!);
      }
      if (reconnected && !session.waitingForGreeting && session.pendingPrompts.length > 0) {
        // `#352`. PROMPT FIRST: what the lost session never started goes at once, with no segment
        // open, as a typed call's does, and the answer's `turn_complete` opens the microphone's
        // segment after it (`vibeTalkTurnComplete`). An `audio_start` first would put an empty
        // segment, and its `audio_end`, between the hand-off and the turn.
        session.audioSegmentActive = false;
        session.capturePaused = true;
        deps.setStatus("Reconnected — sending it again…");
        advanceVibeTalkInput();
        deliverHeldTurn();
        break;
      }
      session.capturePaused = session.waitingForGreeting;
      session.audioSegmentActive = true;
      sendVibeTalkFrame({ type: "audio_start" });
      if (!session.node) {
        startCapture(session.socket!);
      }
      deps.setStatus(
        session.waitingForGreeting
          ? `${reconnected ? "Reconnected" : "Connected"} — the assistant is joining…`
          : `${reconnected ? "Reconnected" : "Connected"} — say something.`
      );
      if (!session.waitingForGreeting) {
        advanceVibeTalkInput();
        sendStartupTiming();
      }
      // A message sent while this call was connecting. From here it is any message typed into a
      // spoken call: it waits for the spoken turn in progress to close, as `advanceVibeTalkInput`
      // describes.
      deliverHeldTurn();
      break;
    }
    case "transcript": {
      const who = message.role === "user" ? "you" : "assistant";
      const said = message.text || "";
      // A provider that predates the flag sends only finished text, so absent means final.
      const final = message.final !== false;
      const key = `${message.turn ?? ""}:${who}:${final}:${said}`;
      const turnNumber = typeof message.turn === "number" ? message.turn : null;
      // `#353 relay-echo-as-owner`. The bridge echoing back a prompt the page said for a channel or
      // for itself: never the owner's words, so neither drawn nor filed as his.
      let pageEcho = false;
      if (who === "assistant" && said.trim()) {
        noteAgentText();
        noteReplyArriving(turnNumber, true);
      } else if (said.trim()) {
        // `#352 voice-reconnect-on-bridge-error`. Or the bridge echoing the page's own prompt back.
        pageEcho = noteTurnInFlightEcho(said, final, turnNumber) && session.turnInFlight !== null &&
          session.turnInFlight.kind !== "typed";
        // The owner is speaking: a spoken turn is open, whether or not anything answers it yet. A
        // final transcript says he has finished, and the turn is the agent's to answer.
        noteOwnerWords(turnNumber, final);
      }
      if (!said || key === session.lastTranscriptKey) {
        break;
      }
      session.lastTranscriptKey = key;
      if (who === "assistant") markStartup("greeting_text");
      const spoken = upsertSpoken(who, message.turn, said, final, pageEcho);
      // `#523 voice-self-hearing`. Nor the agent's own words coming back through the microphone.
      if (who === "you" && !pageEcho && !spoken.entry.echo) {
        // `#512 voice-recap-private-stack`. His words are still growing: a page message waits a
        // moment more. A final says he has finished.
        session.ownerWordsAt = final ? 0 : Date.now();
        // `#523`. He spoke while the agent was silent: what it says next is an answer, which he may
        // talk over, not a greeting.
        if (!agentAudible()) session.greetingPlayedOut = true;
      }
      // `#511 voice-explicit-send-only`. The owner's words, never the page's echo of its own prompt,
      // nor the agent's own voice (`#523 voice-self-hearing`), nor words that may yet prove to be it.
      const owners = !spoken.entry.echo && !spoken.entry.undecided;
      if (who === "you" && owners) deps.reportHeard(spoken.key, spokenText(spoken.entry));
      // The turn's whole text so far, as the row shows it, and its number: the number, not the
      // order the frames arrive in, says whether assistant words came before an answer or in reply
      // to it. `#264 voice-send-approval`, `#268 voice-call-drops`.
      if (who === "assistant") {
        deps.hearAssistantForSpokenYes(spoken.key, spokenText(spoken.entry), turnNumber);
      } else if (owners) {
        deps.hearUserForSpokenYes(spoken.key, spokenText(spoken.entry), final, turnNumber);
        deps.settleYesCandidate(spoken.key);
      }
      break;
    }
    case "error": {
      const said = message.message || message.detail || "the voice provider reported an error";
      noteAgentError();
      reportHealth("error_frame");
      // `#352 voice-reconnect-on-bridge-error`. The session a reconnect opened failed before it
      // started: the call ends, and says the reconnect failed.
      if (session.reconnect) {
        failReconnect(`the new voice session reported an error before it started: ${said}`);
        break;
      }
      // ...and an error that asks to reconnect, on a session that started, carries the call on.
      if (reconnectAsked(said)) {
        reconnectOrEnd(said);
        break;
      }
      // `vibe-talk-v1`: an error ENDS the session — no `turn_complete` follows. Hanging up here is
      // what stops the page streaming the microphone into a session the server has already
      // closed: left open, every audio frame came back as another copy of this error and the
      // phone kept reporting the microphone in use until the reader found Hang up.
      //
      // `#268 voice-call-drops`. It is the one way this page ends a call the reader did not, so it
      // says so: that the voice service reported the error, how far into the call, and its words;
      // and it is recorded as `error_frame`, not as a hang-up.
      {
        const span = callSpan();
        if (session.socket && session.protocol === "vibe-talk-v1") endCall("error_frame", said);
        deps.showError(
          `The voice service reported an error${span ? ` ${span} into the call` : ""}, and the ` +
            `call has ended: ${said}. Tap Talk to start a new call.`,
          { stay: true }
        );
      }
      break;
    }
    case "replay_ack": {
      // `#331 resume-on-websocket`. Not a turn: nothing about it opens or ends one.
      deps.noteReplayAck(message);
      break;
    }
    case "turn_complete": {
      // Read before `vibeTalkTurnComplete` clears it: closing the page's own audio segment is not
      // a request for a reply, so an empty answer to it says nothing about the service.
      const closingSegment = session.waitingForAudioEnd;
      // `#330 voice-long-turn-drop`. Cut short — by the page's `interrupt` or the server's word —
      // the answer keeps what it said, marked as cut off, before it is stored below. And a turn
      // that was heard with none of its words arriving as text says so, rather than leaving a gap.
      const cut = message.interrupted === true || session.interruptPending;
      if (cut) markAnswerCut();
      const health = session.health;
      const unwritten = !session.chat && health && health.peak >= AUDIBLE_PEAK && !health.text
        ? { audioMs: health.audioMs, peak: health.peak }
        : null;
      // Cleared FIRST: completing this turn may send the next typed prompt, which arms its own.
      clearNoReply();
      session.replyArriving = false;
      const number = typeof message.turn === "number" ? message.turn : null;
      closeSpokenTurn(number);
      if (unwritten) {
        // The owner's words of the turn are filed first, as they were said first.
        settleSpoken();
        noteSpokenWithoutText(unwritten, cut, number);
      }
      deps.noteTurnCompleteForSpokenYes(message.interrupted === true, number);
      vibeTalkTurnComplete();
      judgeTurn(message, closingSegment);
      deps.judgeAnswerAtTurnEnd(number);
      // `#269 approval-card-inline`. An answer judged at its turn's end and not taken is shown. The
      // row `vibeTalkTurnComplete` stored a moment ago is filed after this, so with its tag.
      deps.settleYesCandidates();
      break;
    }
    default:
      break;
  }
}


/** The protocol's `turn_complete`, as it was before any health judgement. */
function vibeTalkTurnComplete() {
  settleSpoken();
  sendStartupTiming();
  // `#512 voice-recap-private-stack`. The turn his partial words were part of is over.
  session.ownerWordsAt = 0;
  if (session.waitingForGreeting) {
    session.waitingForGreeting = false;
    // `#268 voice-call-drops`. The greeting is a turn the backend answered like any other, and its
    // next handler is no more ready in this event-loop turn than after any other: microphone frames
    // sent at once — the next capture callback, a few milliseconds away — are what the hand-off
    // below was measured to close the whole session on. Every other turn's end already waited; the
    // greeting's opened the microphone straight away, a few seconds into every call that greets.
    deferVoiceCaptureAcrossTurn();
    deps.setStatus(session.chat ? "Connected — type a message." : "Connected — say something.");
    advanceVibeTalkInput();
    return;
  }
  if (session.waitingForAudioEnd) {
    session.waitingForAudioEnd = false;
    advanceVibeTalkInput();
    return;
  }
  if (session.typedTurnInFlight) {
    session.typedTurnInFlight = false;
    session.turnInFlight = null;
    if (session.pendingPrompts.length > 0) {
      advanceVibeTalkInput();
    } else if (session.chat) {
      deps.setStatus("Connected — type a message.");
    } else {
      session.audioSegmentActive = true;
      sendVibeTalkFrame({ type: "audio_start" });
      deferVoiceCaptureAcrossTurn();
      deps.setStatus("Connected — say something.");
    }
    return;
  }
  // A turn the page did not start has ended, so a prompt held behind it may go.
  deferVoiceCaptureAcrossTurn();
  advanceVibeTalkInput();
}


// The internal voice bridge sends `turn_complete` as soon as the completed backend handler has
// produced its last output. Its next handler is not ready in that same event-loop turn: live
// testing found that PCM sent seven milliseconds later opened a handler which immediately ended,
// then closed the whole session. Keep the microphone graph and audio segment alive, but withhold
// its frames across that backend hand-off. 750 ms was the shortest controlled live gap that
// survived both the hand-off and continued silence afterwards.
//
// `#268 voice-call-drops`. The segment's `audio_end` waits out the same gap: a text turn that the
// `turn_complete` let go, or that arrived inside the gap, closes the segment when it ends
// (`advanceVibeTalkInput`), with the microphone still paused.
const VOICE_CAPTURE_RESUME_MS = 750;


function deferVoiceCaptureAcrossTurn() {
  if (session.chat) return;
  session.capturePaused = true;
  if (session.captureResumeTimer !== null) {
    clearTimeout(session.captureResumeTimer);
  }
  const socket = session.socket;
  session.captureResumeTimer = setTimeout(() => {
    session.captureResumeTimer = null;
    if (session.socket !== socket || !session.connected || session.chat) return;
    // A text turn waiting out the hand-off goes first: it closes the segment, and the microphone
    // stays paused for its turn rather than streaming for a moment before it.
    advanceVibeTalkInput();
    if (!session.waitingForGreeting && !session.waitingForAudioEnd && !session.typedTurnInFlight) {
      session.capturePaused = false;
    }
  }, VOICE_CAPTURE_RESUME_MS);
}


// --- the assistant's own voice at the microphone (`#523 voice-self-hearing`) ----------------------
//
// The owner, 2026-10-25: a new call greeted him, and then answered "let's connect you to your chat"
// as if he had said it. The phone's speaker plays the agent and its microphone hears it. Echo
// cancellation is asked for, but it is only as good as the reference it cancels against: this page
// plays the agent through Web Audio (`playPcmBytes`), which a phone's browser need not hand its echo
// canceller, and a track can refuse the constraint outright (`echoCancellationRefused`). And the
// protocol's greeting gate (`greeting: true`) ends at the greeting's `turn_complete`, which a bridge
// sends when it has finished SENDING the greeting, while this phone may still have seconds of it
// queued for the speaker; the hosted agent's first message has no gate at all.
//
// So three guards, none of which needs the echo canceller to work:
//   * THE GREETING. Microphone frames are withheld while the call's opening answer is still
//     audible here: nobody needs to talk over a hello, and it is the one answer that always plays
//     into an open microphone.
//   * NO ECHO CANCELLATION. With it off or refused, frames are withheld whenever the agent is
//     audible: half duplex, so the owner cannot talk over an answer, but the agent cannot answer
//     itself.
//   * THE TRANSCRIPT. A user transcript made of the words the assistant has just been saying, begun
//     while it was audible or a moment after, is taken for its echo: not drawn or filed as the
//     owner's, not reported as heard (`#511`), never a yes. The voice service has heard it already,
//     so this cannot stop an answer to it; it stops the page making the echo the owner's words.

/** How long after the last sample leaves the speaker the room may still carry it to the microphone. */
const ECHO_TAIL_MS = 250;

/**
 * How long after the microphone last sent the agent's sound a `vibe-talk-v1` user turn whose first
 * frame arrives may still be its echo. A streaming recogniser sends a partial within a moment of
 * the speech it hears, and an echo of three words or more began well before the agent fell silent.
 */
const ECHO_PARTIAL_GRACE_MS = 500;

/** The same for the hosted agent, which sends only a finished turn, after it has ended. */
const ECHO_FINAL_GRACE_MS = 1500;

/** How many of the assistant's latest turns a user transcript is compared with. */
const ECHO_TURNS = 2;

/** How long after a user turn began an assistant turn's words may arrive and still be its source. */
const ECHO_TEXT_TRAIL_MS = 1000;

/** The share of a user transcript's words, in order, that must be the assistant's for an echo. */
const ECHO_WORD_SHARE = 0.8;

/** How long the hosted agent's speaker must stay quiet before its first message counts as over. */
const ECHO_GREETING_QUIET_MS = 2000;


/**
 * When, by the wall clock, the agent's audio this page has scheduled will have died away: the last
 * scheduled sample, the output latency the browser reports (the audio clock runs ahead of the
 * speaker by that much), and the room. 0 when nothing was played in this call.
 */
function agentAudibleUntil() {
  if (!(session.playEndMs > 0)) return 0;
  const audio = session.audio;
  const latency = audio ? Number(audio.outputLatency || audio.baseLatency || 0) : 0;
  const latencyMs = Number.isFinite(latency) ? Math.min(1000, Math.max(0, latency * 1000)) : 0;
  return session.playEndMs + latencyMs + ECHO_TAIL_MS;
}


/** @param {number} [now] */
function agentAudible(now: number = Date.now()) {
  return agentAudibleUntil() > now;
}


/**
 * Has the call's opening answer played out? Once it was played and has died away: a greeting the
 * session promised (`greeting: true`) once its `turn_complete` has come too; any other, the hosted
 * agent's first message included, once the speaker has been quiet longer than a network stall in
 * the middle of one (ECHO_GREETING_QUIET_MS). The owner speaking while the agent is silent ends it
 * as well: what the agent says next is an answer, which he may talk over. Settled on every microphone buffer, muted or paused included, so a
 * greeting nobody could talk over still counts as played.
 *
 * @param {number} [now]
 */
function settleGreetingPlayout(now: number = Date.now()) {
  if (session.greetingPlayedOut || session.waitingForGreeting || !(session.playEndMs > 0)) return;
  const quietMs = session.greetingPromised ? 0 : ECHO_GREETING_QUIET_MS;
  if (now >= agentAudibleUntil() + quietMs) session.greetingPlayedOut = true;
}


/**
 * Whether microphone frames are withheld now because the agent may be heard by the microphone: its
 * opening answer still to finish, or any answer while echo cancellation is off.
 *
 * @param {number} [now]
 */
function micHeldForAgent(now: number = Date.now()) {
  if (session.chat) return false;
  const audible = agentAudible(now);
  if (!session.greetingPlayedOut) return audible;
  return audible && session.echoCancellation !== true;
}


/**
 * A microphone buffer is going to the voice service: while the agent is audible, its sound may go
 * with it, so a user turn soon after may be its echo (`nearAgentSpeech`). With nothing sent while
 * it was audible — the greeting held, half duplex, mute — the service heard no echo to transcribe.
 *
 * @param {number} [now]
 */
function noteMicSent(now: number = Date.now()) {
  if (agentAudible(now)) session.echoExposedAt = now;
}


/**
 * Remember what the assistant said in a turn, for `isAgentEcho`. The turn's newest text replaces
 * what it said before, and only the latest ECHO_TURNS are kept.
 *
 * @param {string} key the assistant turn
 * @param {string} text everything it has said in that turn so far
 */
function noteAgentSaid(key: string, text: string) {
  const known = session.agentSaid.get(key);
  session.agentSaid.delete(key);
  session.agentSaid.set(key, { words: answerWords(text), atMs: known ? known.atMs : Date.now() });
  while (session.agentSaid.size > ECHO_TURNS) session.agentSaid.delete(session.agentSaid.keys().next().value!);
}


/**
 * Whether a user turn begun at `atMs` may be the agent's echo: the microphone sent the agent's sound
 * a moment before (`noteMicSent`), within `graceMs`.
 *
 * @param {number} atMs
 * @param {number} graceMs ECHO_PARTIAL_GRACE_MS or ECHO_FINAL_GRACE_MS
 */
function nearAgentSpeech(atMs: number, graceMs: number) {
  if (session.chat || !(session.echoExposedAt > 0)) return false;
  return atMs <= session.echoExposedAt + graceMs;
}


/**
 * Is `text`, heard as the owner's in a turn that began near the agent's speech (`nearAgentSpeech`),
 * the assistant's own words coming back through the microphone? It is when nearly all of its words
 * are, in order, words of one of the assistant's latest turns whose words had arrived by the time it
 * began: a recogniser mishears an echo as it does anything else.
 *
 * A turn begun after the agent fell silent is judged only when it is three words or more and not a
 * plain yes: a short reply straight after it, a yes or a bare "send it", is the spoken yes's to judge
 * (`spokenYesRefusal`).
 *
 * @param {string} text
 * @param {number} atMs when the turn began
 * @param {boolean} audible whether the agent was audible then
 */
function isAgentEcho(text: string, atMs: number, audible: boolean) {
  const words = answerWords(text);
  if (words.length === 0) return false;
  if (!audible) {
    if (words.length < 3) return false;
    // A plain yes names nobody to send to, and the spoken yes has guards of its own; a "hint" ("send
    // it to the coding agent") does name one, and is exactly the echo #511 must never hear as an ask.
    if (classifySpokenAnswer(text) === "affirmative") return false;
  }
  const need = Math.ceil(words.length * ECHO_WORD_SHARE);
  for (const said of session.agentSaid.values()) {
    if (said.atMs > atMs + ECHO_TEXT_TRAIL_MS) continue;
    let at = 0;
    let matched = 0;
    for (const word of words) {
      const found = said.words.indexOf(word, at);
      if (found < 0) continue;
      matched += 1;
      at = found + 1;
    }
    if (matched >= need) return true;
  }
  return false;
}


/** @param {string} text */
function noteAgentEchoHeard(text: string) {
  const said = String(text).trim();
  const shown = said.length > 80 ? `${said.slice(0, 79)}…` : said;
  deps.addDetail(`the microphone picked up the assistant's own words ("${shown}"); not taken as yours`);
}


function startCapture(socket: WebSocket) {
  const rate = session.audio!.sampleRate;
  // ScriptProcessorNode is deprecated in favour of AudioWorklet, and is still the only capture
  // path that needs no separate module file. A worklet would mean a second asset or a blob URL,
  // for no behavioural gain on a page this small.
  session.source = session.audio!.createMediaStreamSource(session.stream!);
  session.node = session.audio!.createScriptProcessor(4096, 1, 1);
  session.node.onaudioprocess = (event) => {
    if (socket.readyState !== deps.WebSocket.OPEN) {
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
    // `#523 voice-self-hearing`. Before mute: a greeting played into a muted microphone is played.
    settleGreetingPlayout();
    if (session.muted) {
      return;
    }
    // `#512 voice-recap-private-stack`. Heard even across a hand-off, when no frame goes: the owner
    // speaking then is still speaking when the next message would close the segment.
    const mono = event.inputBuffer.getChannelData(0);
    noteMicLevel(mono);
    if (session.capturePaused) {
      return;
    }
    // `#523 voice-self-hearing`. Nor the agent's own voice, while nothing cancels it.
    if (micHeldForAgent()) {
      return;
    }
    noteMicSent();
    const pcm = floatToPcm16(downsampleTo(mono, rate, session.inputRate));
    if (session.protocol === "vibe-talk-v1") {
      socket.send(pcm.buffer);
      return;
    }
    socket.send(
      JSON.stringify({
        user_audio_chunk: bytesToBase64(new Uint8Array(pcm.buffer)),
      })
    );
  };
  session.source.connect(session.node);
  // A ScriptProcessorNode only fires while it is connected to the graph. Routing it at zero gain
  // keeps it running without echoing the microphone into the speaker.
  const silence = session.audio!.createGain();
  silence.gain.value = 0;
  session.node.connect(silence);
  silence.connect(session.audio!.destination);
}


function teardown() {
  // A start torn down for any reason is no longer waiting for its socket to open.
  clearOpenTimer();
  // ...nor for an error on its socket to be reported: what ended it has been said already.
  if (failureTimer !== null) {
    clearTimeout(failureTimer);
    failureTimer = null;
  }
  // Before anything is reset: the turn in progress is stored, and a call that ended before its
  // greeting still reports how far it got. An answer still arriving when the call ended keeps what
  // it said, marked as cut off. `#330 voice-long-turn-drop`.
  if (session.replyArriving) markAnswerCut();
  settleSpoken();
  saveUnsettledTurns();
  // `#337 voice-relay-silent`. What was still to be read out is said not to have been, before the
  // turns waiting for the backend are let go below.
  endCallRelay();
  deps.stopPostWatch();
  sendStartupTiming();
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
  deps.callAudioRoute = null;
  session.socket = null;
  session.connected = false;
  session.protocol = "unknown";
  session.providerName = deps.conversationalVoice.name;
  session.inputRate = 16000;
  session.outputRate = 16000;
  session.lastTranscriptKey = null;
  session.liveTurns = new Map();
  session.noTextRows = new Map();
  session.anonTurn = 0;
  session.timing = null;
  clearNoReply();
  session.health = null;
  session.muted = false;
  resetVibeTalkTurnState();
  session.pendingPrompts = [];
  session.playEndMs = 0;
  session.answerPlayEndMs = 0;
  session.echoCancellation = false;
  session.greetingPlayedOut = true;
  session.agentSaid = new Map();
  session.echoExposedAt = 0;
  session.sessionStarts = 0;
  session.documentNoticeSent = false;
  // `#352 voice-reconnect-on-bridge-error`. A reconnect in progress stops at its next step.
  // `#512 voice-recap-private-stack`. A recap armed for a session this call no longer has goes back,
  // so no later session is handed it unasked: one being asked about, one waiting for this call's
  // first session, and one a reconnect armed for a session that never started.
  const recapAnswerPending = session.profileRecap !== null;
  deps.withdrawArmedRecap();
  session.reconnect = null;
  session.reconnects = 0;
  session.lastReconnectAt = 0;
  if (session.noticeAwaitingRecap) clearTimeout(session.noticeAwaitingRecap.timer);
  session.noticeAwaitingRecap = null;
  // `#331 resume-on-websocket`. No answer to the recap can come now: a record held for one goes,
  // the turn in progress (stored above) included, saying what was known. `#512`: except the answer
  // to the page's withdrawal of a recap still being asked about, which ends the wait itself.
  if (!recapAnswerPending) deps.endReplayAnswerWait();
  // A record still waiting for a `session_started` that never came was never sent: Settings must
  // not say the call was resumed.
  if (session.pendingResume) {
    session.pendingResume = null;
    deps.resumeLast = { ...deps.resumeLast, armed: false, unsent: true };
    deps.renderResumeState();
  }
  // Chat is a property of ONE conversation, decided when its socket opened. Carrying it into the
  // next one would mean the big control silently started a typed conversation because the previous
  // one was typed, which is the sort of stickiness the placement rules of this page keep removing.
  session.chat = false;
  session.vendorSentAudioInChat = false;
  // A fresh call pings on its first keystroke rather than inheriting the last call's throttle
  // window, which would leave the agent up to thirty seconds of unexplained silence. And a typed
  // turn from the dead conversation cannot suppress a transcript in the new one.
  lastActivityAt = 0;
  recentTyped = [];
  // A message held for a conversation that never opened, or that closed before the server accepted
  // it, goes nowhere — not into the next one, which the reader has not asked for. Its text is still
  // in the box, which is where they left it, and the status says it did not go: after a send, a
  // line that only said the call had ended would leave the reader to guess.
  if (heldTurn !== null) {
    heldTurn = null;
    const said = deps.el("status").textContent.replace(/[\s.]+$/u, "");
    deps.setStatus(`${said ? `${said}. ` : ""}Your message did not go — it is still in the box.`);
  }
  renderControls();
}


/**
 * The `vibe-talk-v1` session's turn-by-turn state, back to a session that has not started: no turn
 * open, completed or on the wire, no segment, nothing waited for, and no timer of the hand-off or
 * of an answer playing out. What is queued to be said (`pendingPrompts`) is the caller's to keep or
 * let go.
 */
function resetVibeTalkTurnState() {
  session.v1Ready = false;
  if (session.captureResumeTimer !== null) {
    clearTimeout(session.captureResumeTimer);
    session.captureResumeTimer = null;
  }
  session.capturePaused = false;
  session.audioSegmentActive = false;
  session.waitingForGreeting = false;
  session.waitingForAudioEnd = false;
  session.typedTurnInFlight = false;
  session.replyArriving = false;
  closeSpokenTurn();
  session.lastCompletedTurn = null;
  session.turnInFlight = null;
  if (session.playbackWaitTimer !== null) {
    clearTimeout(session.playbackWaitTimer);
    session.playbackWaitTimer = null;
  }
  // `#512 voice-recap-private-stack`. A session not started hears nobody yet.
  dropSpeechHold();
  session.micSpeechAt = 0;
  session.ownerWordsAt = 0;
}


/** Hang up: the reader's control, and the only way this page ends a call that is working. */
function stop() {
  endCall("hang_up");
}


/**
 * Say goodbye to a `vibe-talk-v1` session (`audio_end`, `quit`) on `socket` while it is open. The only
 * place this page sends `quit`: when it ends the call (`endCall`), and when it replaces a session
 * the voice service lost (`reconnectCall`, `#352 voice-reconnect-on-bridge-error`). Neither frame is
 * noted as the last `audio_end` sent: a goodbye is not what ended anything.
 *
 * @param {WebSocket} socket the call's socket, `session.socket`
 */
function sayGoodbye(socket: WebSocket) {
  if (session.protocol === "vibe-talk-v1" && socket.readyState === deps.WebSocket.OPEN) {
    sendVibeTalkFrame({ type: "audio_end" }, false);
    sendVibeTalkFrame({ type: "quit" }, false);
  }
}


/**
 * End the call from this page: tell a `vibe-talk-v1` server goodbye (`audio_end`, `quit`), close the
 * socket, and tear the call down.
 *
 * `#268 voice-call-drops`. There are exactly two reasons, and both are recorded (`postCallEnd`) and
 * said: the reader pressed Hang up, or the server sent an `error`, which the protocol says ends its
 * session — and the page then says so in red, with the server's words. A bridge log that shows a
 * `quit`, then, shows one of these two, or a reconnect's goodbye (`sayGoodbye`), and this app's log
 * says which.
 *
 * `#352 voice-reconnect-on-bridge-error`. An `error` that asks to reconnect does not end the call, up
 * to a bound; past it the call ends as `reconnect_limit`, and a reconnect that cannot get a working
 * session ends it as `reconnect_failed`. Both are the server's error too, said in red by the caller.
 *
 * @param {"hang_up" | "error_frame" | "reconnect_limit" | "reconnect_failed"} endedBy
 * @param {string} [reason] the voice service's error, when that is what ended it
 * @param {string} [why] how the page came to end it, for the connection details, when not hang_up
 *   or error_frame
 */
function endCall(endedBy: "hang_up" | "error_frame" | "reconnect_limit" | "reconnect_failed", reason: string = "", why: string = "") {
  // With no call there is no Hang up in the pane at all, so this is unreachable from the screen;
  // it stays as a guard rather than as a second way to end something that is already over.
  if (!session.socket) {
    return;
  }
  // Recorded BEFORE the goodbye: the record says when this page last sent `audio_end`, and the
  // goodbye's own is not what ended the call. `#330 voice-long-turn-drop`.
  postCallEnd(endedBy, null, reason);
  sayGoodbye(session.socket);
  const span = callSpan();
  if (span) {
    deps.addDetail(endedBy === "hang_up"
      ? `hung up ${span} into the call`
      : why
        ? `ended by this page ${span} into the call: ${why}`
        : `ended by this page ${span} into the call, after the voice service's error`);
  }
  // From here the socket is no longer the call's: its other handlers ignore it once `teardown` has
  // let it go, and its close — whenever it comes, which can be seconds after a `quit` — touches
  // nothing but the connection details. Replaced BEFORE `close()`, which may report the close at once.
  const socket = session.socket;
  // `#649 voice-agent-tools`. A Hang up while an answer's text may still be on its way — its audio
  // arriving, or a turn heard without text — listens a moment longer for it (`listenForTextAfterHangUp`).
  const textOwed = endedBy === "hang_up" && session.protocol === "vibe-talk-v1" && !session.chat &&
    (session.replyArriving || session.noTextRows.size > 0);
  if (textOwed) {
    /** @type {Map<number, {li?: VoiceRow, entry?: LiveTurn}>} */
    const rows: Map<number, { li?: VoiceRow; entry?: LiveTurn; }> = new Map();
    for (const [turn, li] of session.noTextRows) rows.set(turn, { li });
    for (const entry of session.liveTurns.values()) {
      const turn = Number(String(entry.key).split(":")[0]);
      if (entry.who === "assistant" && !entry.echo && Number.isInteger(turn) && entry.li) {
        rows.set(turn, { entry });
      }
    }
    listenForTextAfterHangUp(socket, session.conversationId, rows);
  } else {
    socket.onclose = (event) => {
      deps.addDetail(`closed with code ${event.code}${event.reason ? `: ${event.reason}` : ""}`);
    };
    socket.close();
  }
  deps.setStatus("Call ended.");
  deps.setState("ended");
  deps.dismissBanner();
  teardown();
  deps.noteConversationEnded(endedBy === "hang_up" ? "ended" : "error");
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
  syncScreenWakeLock();
  // `#331 resume-on-websocket`. The line under the call goes with the call.
  deps.renderCallRecap();
  const talk = deps.el("talk");
  const label = deps.el("talk-label");
  const note = deps.el("talk-note");
  const live = Boolean(session.socket);

  // A TYPED conversation has no microphone to mute and no voice to silence, so the two controls
  // that act on those are ABSENT rather than sitting there inert. That is the same rule Hang up
  // already follows when there is no call, and it is the rule the owner's complaint was really
  // about: the old way to reach typing left both of them on screen, in states you had to set by
  // hand, acting on a microphone you never wanted open.
  const chat = live && session.chat;
  // READ REPLACES TALK IN THE CHANNEL VIEW. Two different things to want, and which one you want
  // is decided by the list in front of you. Talk stays available in the voice view, and a live
  // call keeps it everywhere — hiding the control that ends a conversation you are having, or
  // starting audio playback over it, would both be worse than the swap.
  // `#1039 documents-read-parity`: the Documents reader reads through the same owner, so its
  // Read and pace are the channel's too — inert there while the pane itself stays the channel's.
  const channel = deps.currentView === "discord" && !live;
  const reading = channel || (deps.docsReaderOpen && !live);
  deps.el("read-aloud").hidden = !reading;
  deps.el("read-speed").hidden = !reading;
  // The archive filter belongs to the CHANNEL, so it appears with the channel and not with a call.
  deps.el("todo-filter").hidden = !channel;
  deps.renderReadModeControl();
  deps.el("read-aloud").setAttribute("aria-pressed", deps.readingMode ? "true" : "false");
  // THE CONTROL SAYS WHICH ACT IT PERFORMS, not which state you are in. "Reading" described the
  // state and left the reader guessing what pressing it would do; "Stop" is the act, and the
  // colour change is what makes starting and ending a session legible at a glance.
  //
  // The session is VIRTUAL from the reader's perspective. The provider transport may be reused
  // between requests, but reconnects are deliberately invisible; this is still a mode the reader
  // turned on, so it needs a visible way out that looks like a way out.
  //
  // `#434 read-aloud-pause`: while a read is actually playing the act is Pause, and while it is
  // held it is Resume — the media keys say the same, from the one state (`renderReadTransport`).
  // `#537 scrub-bar-no-play-pause`: it is the only Pause on screen; the strip over the list steers.
  // With nothing playing it is "Stop" again, the way out above.
  deps.renderReadTransport();
  // `#451 read-strip-followups`: active for a read in progress as for the mode, so a read started
  // with s keeps the way-out colour and the keyboard-up slim row's Pause (`#429`), which both key
  // off this. `renderReadTransport` keeps it as a read starts, holds and ends between these draws.
  deps.el("read-aloud").setAttribute("data-active", deps.readingMode || deps.readInProgress() || deps.pendingRead !== null ? "true" : "false");
  // The state the READER cares about, on the control they pressed. Reset to a plain ready when the
  // mode is off, so a failure from the last session does not greet them on the next one.
  deps.el("read-aloud").setAttribute("data-read-state", deps.readingMode ? deps.readState : "idle");
  // The popover belongs to a control that is on screen. Leaving it open over the transcript would
  // be a dialog about a mode the reader has left.
  if (!reading) {
    deps.closeSpeedPopover();
  }
  deps.el("speaker").hidden = chat;
  deps.el("talk").hidden = chat || reading;
  // CLEAR IS FOR THE TRANSCRIPT, which is this page's own record of a conversation and can be
  // thrown away. The channel is Discord's, it is not ours to clear, and the button offered an act
  // that either means nothing or means something alarming. Absent in the channel view.
  deps.el("clear-view").hidden = reading;
  deps.el("hang-up").hidden = !live;
  deps.el("hangup-label").textContent = chat ? "End chat" : "Hang up";
  deps.el("control-pane").className = chat ? "chat" : live ? "" : "solo";
  // `#241 voice-composer`. Send works with NOTHING open — that is how a typed conversation starts —
  // so the only time it is dead is while a message it sent is waiting for its conversation to open.
  // Disabled rather than hidden: the box is a stable shape, and a control that comes and goes under
  // a thumb is worse than one that is visibly inert.
  deps.el("send-text").disabled = heldTurn !== null;
  // Same fact, same moment, two controls further along the bar. `renderControls` is called from
  // every transition that can change whether a conversation exists — open, close, mute, teardown —
  // so hanging the canned buttons off it is what keeps them from going stale.
  renderCannedPrompts();

  note.hidden = true;
  note.textContent = "";

  // AFTER the composer and the canned buttons, which are the controls a typed conversation is
  // actually driven by, and before the microphone branches, which are the ones it has none of.
  if (chat) {
    return;
  }

  if (!live) {
    talk.className = "control control-talk";
    if (startInFlight) {
      // A start is asking for its session or the microphone, and has no socket yet. Pressing this
      // again starts nothing (`start()` refuses), so it must not offer to.
      label.textContent = "Connecting…";
      return;
    }
    if (hasUnreachable) {
      // `#209 voice-socket-open`. The last start never reached the voice service, so there is no
      // call to resume and nothing new about starting one: the offer is to try the same again.
      label.textContent = "Try again";
      note.textContent = "the voice service is not reachable";
      note.hidden = false;
      return;
    }
    if (hasSuspended) {
      label.textContent = "Resume";
    } else {
      label.textContent = hasEnded ? "Start a new call" : "Talk";
    }
    if (hasEnded) {
      // "Resume" is the honest word for what the reader wants and a dishonest word for what the
      // agent gets, so the clause under it says which of the two this is.
      // `#46 conversation-replay`. This sentence used to be a constant, and the moment resuming
      // shipped it became the place the feature would lie from. It is derived now.
      const offer = deps.resumeNote();
      note.textContent = hasSuspended ? `a new call — ${offer}` : offer;
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


// --- telling the agent that the silence is deliberate --------------------------------------------
//
// `#73 mute-is-invisible`, and it is the owner's own complaint: during a long mute the agent "gets
// very annoying about asking 'Are you there?'", and turning the prompting down in the ElevenLabs
// dashboard did not help.
//
// THE CAUSE IS OURS. Mute withholds `user_audio_chunk` frames and nothing else — see the long note
// in `startCapture`, which is where mute lives and where it must stay. That is the right pause: the
// socket stays open, so the agent keeps everything it has been told. But it means that from the
// vendor's side a muted caller and a caller who simply stopped talking are the SAME BYTES: in both
// cases no audio arrives. Going quiet is exactly the condition that makes an agent check whether
// anyone is still listening, so no dashboard setting can fix this — there is nothing there to tell
// the two apart from. The only cure is to say it out loud.
//
// It is a client event on the socket that is already open, which is the whole reason this is a
// small change: the page already sends `conversation_initiation_client_data`, `pong`,
// `user_audio_chunk`, `user_message` and `user_activity` — and `contextual_update` itself is not
// even new here, since `#46 conversation-replay` sends one after the initiation frame and the
// Discord relay sends one per relayed message. This is a THIRD use of an event already on the
// wire, not a new event type and not a new mechanism. It CANNOT be an MCP tool — MCP here is
// request/response with the agent as the client, and this server issues no `Mcp-Session-Id` and
// answers `GET`/`DELETE /mcp` with 405 precisely because it has nothing to push. The conversation
// socket is the only door.
//
// UNVERIFIED AGAINST THE LIVE VENDOR, and read this before trusting it. `contextual_update` is
// believed to be an ElevenLabs client event that injects context WITHOUT consuming a turn, and this
// page already sends one for `#46 conversation-replay` — but that belief came from a recon plan
// rather than from the vendor's protocol reference, and nothing in this repository can settle it.
// Two separate questions are open: whether ElevenLabs accepts the frame at all, and whether an
// agent that reads it actually HOLDS instead of prompting. Both are answered by one billed
// `scripts/run.sh --smoke-agent` conversation — mute for a minute and listen — and THAT RUN HAS NOT
// BEEN MADE. What is checked offline is our half: `tests/js/voice_page.test.mjs` pins what this page
// puts on the wire, and `tests/elevenlabs_mock.rs` sends that sentence to the mock vendor, which
// MODELS this event in `src/elevenlabs/mock/agent.rs` — the frame is recognised, its text enters
// the agent's context, and no turn is spent on it. That model is this repository's belief about the
// contract written down where a test can state it; it is emphatically not evidence about
// ElevenLabs, and an unrecognised event would be answered with the same silence.
//
// The fallback, if the event turns out not to exist, is a short `user_message`, which is definitely
// in the protocol — and which consumes a turn, so the agent would ANSWER the announcement out loud.
// That is worse than the complaint, which is why it is the fallback and not the first choice.
//
// Worth knowing, and the owner established it: BILLING CONTINUES WHILE MUTED. A conversation is
// billed for being open; the vendor discounts silent periods but does not stop the meter. Telling
// the agent to hold makes a long mute quieter, not free.
const MUTE_NOTICE =
  "The user has muted their microphone deliberately and has stopped speaking on purpose. This is " +
  "a pause they chose, not a connection that failed. Do not ask whether they are still there and " +
  "do not prompt them to speak: hold, and skip your turn, until you are told they have unmuted.";


const UNMUTE_NOTICE =
  "The user has unmuted their microphone and can be heard again. Carry on normally.";


/**
 * Tell the live conversation which of the two silences this is. Returns whether the frame went.
 *
 * Routed through `sendClientEvent`, so a socket that is not OPEN is a no-op rather than a throw.
 * That ordering is deliberate: mute is a LOCAL fact first — it withholds audio whatever the vendor
 * does with this frame — and a mute that refused to engage because an announcement could not be
 * delivered would be strictly worse than a mute the agent cannot see.
 *
 * A dropped announcement is not always harmless, though, and there is one case where it must be
 * made good: a mute engaged in the CONNECT WINDOW, before `onopen`, would otherwise leave the
 * entire call muted with the agent never told. `socket.onopen` re-announces for exactly that case.
 * A mute on a CLOSING socket is the harmless one — there is no call left to be prompted in.
 */
function announceMute(muted: boolean) {
  return sendClientEvent({
    type: "contextual_update",
    text: muted ? MUTE_NOTICE : UNMUTE_NOTICE,
  });
}


function setMuted(muted: boolean) {
  session.muted = muted;
  // The flag above is invisible to the agent; this line is the only thing that is not.
  announceMute(muted);
  renderControls();
  // One line, because there is one line. The three sentences this used to be are in Settings,
  // under "What the controls do", where somebody who wants them can read them.
  deps.setStatus(muted ? "Muted — the agent still remembers." : "Listening — say something.");
}


/**
 * Silence the agent's VOICE, not the agent.
 *
 * Audio frames are dropped in `handle`; `agent_response` keeps writing what the agent said into
 * the transcript. So this is the control for reading the agent in a room where you cannot listen
 * to it, and it deliberately does not touch the microphone, the socket, or mute.
 *
 * THE DECISION, WRITTEN DOWN, because `#43 typed-input` asks for it explicitly and because the two
 * options are indistinguishable to the reader and very different on the wire:
 *
 *     THIS PAGE KEEPS RECEIVING THE AGENT'S AUDIO AND THROWS IT AWAY.
 *
 * It does not renegotiate the conversation into a text-only response mode. Three reasons, in the
 * order they decide it:
 *
 *   1. A text-only mode is an INITIATION-TIME negotiation, not a setting. This page sends
 *      `conversation_initiation_client_data` exactly once, in `socket.onopen`, and reads
 *      `agent_output_audio_format` exactly once, out of `conversation_initiation_metadata`.
 *      Switching mid-call therefore means closing the socket and opening a new one — and
 *      `noteConversationEnded` records what that costs: the vendor documents no way to resume a
 *      conversation, so the agent on the other side of the reconnect has never heard a word of
 *      this one. Silencing the speaker would then destroy exactly the context that Mute exists to
 *      preserve, and it would do it invisibly, because the button looks like a speaker.
 *   2. The control has to be instantly reversible. Dropping frames is reversible in the time it
 *      takes to set a boolean; a reconnect is not reversible at all.
 *   3. The only cost of the choice is downstream bandwidth on a socket that is already carrying
 *      microphone audio upstream, continuously, in the same call.
 *
 * The suite makes the decision CHECKABLE rather than merely recorded: toggling this control must
 * leave exactly one `conversation_initiation_client_data` frame on the socket.
 */
function setSpeakerOff(off: boolean) {
  session.speakerOff = off;
  if (off) {
    stopPlayback(); // whatever is already scheduled would otherwise keep talking.
  }
  deps.el("speaker").className = off ? "control control-mini off" : "control control-mini on";
  deps.el("speaker").setAttribute("aria-pressed", off ? "true" : "false");
  deps.el("speaker-label").textContent = off ? "Silent" : "Sound";
  deps.setStatus(
    off ? "Agent voice off — its replies still arrive as text." : "Agent voice on."
  );
}


/** The one large control: start a call when idle, toggle mute when one is live. */
function onTalk() {
  if (!session.socket) {
    // After a start that never reached the voice service the control reads "Try again", and does
    // that: the same kind of conversation, typed or spoken. `#209 voice-socket-open`.
    return hasUnreachable ? retryStart() : guard(start)();
  }
  setMuted(!session.muted);
  return Promise.resolve();
}


function guard<Args extends unknown[], Result>(fn: (...args: Args) => Result) {
  return (...args: Args) =>
    Promise.resolve(fn(...args)).catch((error) => {
      // Both places: the panel is what the owner reads, the status line is the one-liner. Neither
      // is the console.
      deps.showError((error as Error).message);
      deps.setStatus("could not start the conversation");
      teardown();
    });
}


/**
 * One quoted line about an arriving message, cut to a budget. NO framing: the turn carries that.
 *
 * `spoken_content` FIRST, and `content` only as the fallback. They are not the same text: the
 * server has already taken the markdown off the prepared one, said its timestamps the way a
 * listener places them, and replaced every nineteen-digit snowflake with a letter that means the
 * same account in the next message as it did in this one. Quoting the raw body instead is how
 * `read new` came to spell ids out digit by digit — this was the one path to the agent that
 * prepared nothing, because it is the one built here rather than on the server.
 *
 * The fallback is not decoration. A server older than the field sends no `spoken_content`, and
 * reading such a message badly is much better than the alternative of not reading it at all: a
 * reader who is listening cannot tell silence apart from a message that never arrived.
 *
 * The ROW on screen still shows `content`, untouched. The prepared body is for the voice; the
 * screen shows what was actually written, and that is also what keeps a letter recoverable — the
 * id it stands for is on the row the reader is looking at.
 */
function relayLine(message: VibeTalk.Message) {
  const said = String(message.spoken_content || message.content || "");
  const body = said.replace(/\s+/g, " ").trim();
  const text =
    body.length > deps.RELAY_MAX_CHARS ? `${body.slice(0, deps.RELAY_MAX_CHARS - 1)}…` : body;
  return `in ${deps.channelLabel(message.channel_id)}, ${message.author} said: ${text}`;
}

/** The complete relay turn, with the page's current framing read in its original order. */
function relayTurn(quotes: string[], mode: string) {
  return formatRelayTurn(quotes, mode, {
    get preamble() { return deps.RELAY_PREAMBLE; },
    get open() { return deps.RELAY_FENCE_OPEN; },
    get close() { return deps.RELAY_FENCE_CLOSE; },
  });
}


/**
 * Messages that have arrived and not yet been spoken, oldest first, and the timer that will speak
 * them. Each is its quoted line and its channel, for the line that says when one was not read out
 * (`#337 voice-relay-silent`).
 *
 * @type {{line: string, channel: string}[]}
 */
let relayPending: { line: string; channel: string; }[] = [];

let relayTimer: Timer | null = null;


/**
 * `#337 voice-relay-silent`. Every message this call has taken to relay, by channel and id: what
 * makes a message relayed EXACTLY ONCE when two streams carry it — the channel on screen's and the
 * stream of every channel — or a stream that dropped replays it on resuming. Emptied when a call
 * opens.
 *
 * @type {Set<string>}
 */
let relayLedger: Set<string> = new Set();


/** When the call now open connected, by the wall clock; 0 with no call. `#337 voice-relay-silent`. */
let relayCallOpenedAt = 0;


/**
 * The most channel messages one relayed turn carries. `#337 voice-relay-silent`. Since the call
 * hears every watched channel, a channel catching up after an outage can deliver a few hundred at
 * once; read out in full, that is not a turn but an hour. The rest wait for the next turn, which
 * goes when this one's answer has been heard out — each message still once, none dropped.
 */
const RELAY_TURN_MAX_MESSAGES = 8;


/**
 * How far before the call connected a message replayed on a resumed stream may have been posted and
 * still be news to the call: phone clocks drift from the chat provider's by seconds, not minutes.
 * `#337 voice-relay-silent`.
 */
const RELAY_CLOCK_SKEW_MS = 60000;


/**
 * Say the held messages as ONE turn, and forget them.
 *
 * `user_message`, NOT `contextual_update`, and that is the whole substance of the change. A
 * contextual update injects text into the agent's context WITHOUT consuming a turn: the agent
 * silently knows a message arrived and says nothing about it until asked. That was right for a
 * background note and is exactly wrong for "read new" — the reader asked to be TOLD. A
 * `user_message` consumes a turn, so the agent answers out loud.
 *
 * It deliberately does NOT go through `sendUserMessage`, which renders what it sends into the
 * transcript as the reader's own words. Nobody said this; it is the page speaking on the channel's
 * behalf, and putting it in the transcript under "you" would be a false record of the conversation.
 *
 * Both guards are re-checked HERE as well as at the door. Between the arrival and this flush the
 * call can end and the reader can turn the relay off, and either one means the turn must not go.
 * `#337 voice-relay-silent`: neither drops a held message silently any more. Turned off, the
 * connection details say how many were let go; with no call to reach, the transcript says they
 * were not read out (`reportUnrelayed`).
 */
function flushRelay() {
  relayTimer = null;
  if (!relayPending.length) return false;
  const mode = deps.relayMode();
  if (mode === "off") {
    deps.addDetail(`${relayCount(relayPending.length)} waiting to be read out ${relayPending.length === 1 ? "was" : "were"} ` +
      "let go: Arriving messages during a call was turned off");
    forgetRelayPending();
    return false;
  }
  if (session.reconnect && session.socket) {
    // `#352 voice-reconnect-on-bridge-error`. The call is moving to a new session: what waits, waits
    // for it, and says nothing over "Reconnecting".
    relayTimer = setTimeout(flushRelay, RELAY_RECHECK_MS);
    return false;
  }
  if (!canSendText()) {
    reportUnrelayed(relayPending, "ended");
    forgetRelayPending();
    return false;
  }
  // `#330 voice-long-turn-drop`. Never into an answer. A relayed message used to be held thirty
  // seconds behind a long spoken summary and then sent anyway, which cut the summary off and lost
  // its text. It waits now while the assistant is answering or its audio is still playing — and
  // whatever else arrives meanwhile joins it, so a night's worth of arrivals is still one turn. It
  // waits for the owner's own open turn too, but only within the bounds `relayWait` reads.
  const heldBy = relayWait();
  if (heldBy !== null) {
    const said = `${relayCount(relayPending.length)} will be read out ` +
      `${heldBy === "turn" ? "when you finish speaking" : "when the answer ends"}.`;
    if (said !== relayWaitSaid) {
      relayWaitSaid = said;
      deps.setStatus(said);
    }
    relayTimer = setTimeout(flushRelay, Math.max(RELAY_RECHECK_MS,
      (session.answerPlayEndMs || 0) + ANSWER_PLAYED_GRACE_MS - Date.now()));
    return false;
  }
  const held = relayPending.splice(0, RELAY_TURN_MAX_MESSAGES);
  relayWaitSaid = "";
  // What a burst leaves for the next turn starts waiting for it now.
  relayWaitingSince = relayPending.length ? Date.now() : 0;
  const sent = sendClientEvent(
    { type: "user_message", text: relayTurn(held.map((item: { line: string; channel: string; }) => item.line), mode) },
    "relay",
    held
  );
  if (!sent) reportUnrelayed(held, "ended");
  // On the hosted protocol nothing else says when this turn's answer is over. `#337`.
  if (sent && session.protocol !== "vibe-talk-v1") {
    relayAnswerAwaited = { sentAt: Date.now(), outputAt: 0, audioEndsAt: 0, held };
  }
  // The rest of a burst too large for one turn waits behind this one's answer (`relayWait`).
  if (relayPending.length && relayTimer === null) relayTimer = setTimeout(flushRelay, deps.RELAY_COALESCE_MS);
  return sent;
}


/** Nothing is waiting to be read out any more. */
function forgetRelayPending() {
  relayPending = [];
  relayWaitSaid = "";
  relayWaitingSince = 0;
}


/**
 * How often held channel messages ask again whether the answer they wait for has ended, when nothing
 * says how long it has left to play. `#330 voice-long-turn-drop`.
 */
const RELAY_RECHECK_MS = 500;


/**
 * How long an answer that has stopped arriving — no `turn_complete`, nothing of it for this long and
 * nothing of it left to play — still holds a relayed channel message. `#330 voice-long-turn-drop`.
 *
 * Longer than `SPOKEN_TURN_HOLD_MS`, which bounds the same silence for the page's word on a yes, and
 * on purpose: an agent that said "One moment." is waiting for that word, while nobody is waiting for
 * a relayed message, and the silence in the middle of an answer is most often a tool call — which a
 * voice runtime gives fifteen seconds before it drops it, then speaks again. A relay cut into that
 * silence goes as an `audio_end`, which a bridge takes for an interrupt, and the answer still to come
 * is lost. Past this the turn has stalled, and its `turn_complete` is not coming.
 *
 * The same holds BEFORE an answer's first word. Once the owner's words are final the turn is the
 * agent's, and one that answers with a tool call and no filler line sends nothing at all until the
 * call returns: that silence is the answer's, timed from his last word with this bound, not his own
 * turn gone quiet (`spokenTurnHeard`).
 */
const RELAY_STALLED_ANSWER_MS = 25000;


/**
 * On the hosted protocol, how long a relayed turn that has drawn no answer at all — none of the
 * agent's text, none of its audio — holds the next relayed turn. `#337 voice-relay-silent`.
 */
const RELAY_ANSWER_WAIT_MS = 20000;


/**
 * On the hosted protocol, how long after the last of the agent's answer to a relayed turn arrived —
 * its text or a piece of its audio — the next relayed turn still waits for more of it: an answer's
 * text can come ahead of its first audio. `#337 voice-relay-silent`.
 */
const RELAY_ANSWER_SETTLE_MS = 1500;


/**
 * `#337 voice-relay-silent`, the hosted protocol: the relayed turn last sent, until the agent's
 * answer to it has arrived and been heard out; null when none is outstanding.
 *
 * Nothing on this protocol says that a turn has ended — it has no `turn_complete` — and a
 * `user_message` sent while the agent answers cuts the answer. A burst too large for one turn went
 * out as two turns `RELAY_COALESCE_MS` apart, and the second, landing before the first one's answer
 * had begun to play, could cut it: eight messages taken as relayed, never read out, and never said
 * not to have been. So the next relayed turn waits for this one's answer: some of it has arrived
 * (`outputAt`), nothing more of it for RELAY_ANSWER_SETTLE_MS, and its audio has been heard out —
 * played, or with Sound off for as long as it would have taken to play (`audioEndsAt`) — plus
 * ANSWER_PLAYED_GRACE_MS. A relayed turn that draws no answer at all holds the next no longer than
 * RELAY_ANSWER_WAIT_MS, and the connection details say so.
 *
 * `held` are the messages it carried: one the call ends before any of its answer has arrived is said
 * not to have been read out (`endCallRelay`, `#352 voice-reconnect-on-bridge-error`).
 *
 * @type {{sentAt: number, outputAt: number, audioEndsAt: number, held: {channel: string}[]} | null}
 */
let relayAnswerAwaited: { sentAt: number; outputAt: number; audioEndsAt: number; held: { channel: string; }[]; } | null = null;


/**
 * Some of the agent's output arrived on the hosted protocol: its text, or `audioMs` of its audio,
 * counted whether or not it is played. See `relayAnswerAwaited`.
 *
 * @param {number} [audioMs]
 */
function noteRelayAnswerOutput(audioMs: number = 0) {
  const awaited = relayAnswerAwaited;
  if (awaited === null) return;
  const now = Date.now();
  awaited.outputAt = now;
  if (audioMs > 0) awaited.audioEndsAt = Math.max(awaited.audioEndsAt, now) + audioMs;
}


/** The hosted vendor cut the agent's answer off (`interruption`): no more of it will be said. */
function noteRelayAnswerCut() {
  const awaited = relayAnswerAwaited;
  if (awaited !== null) awaited.audioEndsAt = Math.min(awaited.audioEndsAt, Date.now());
}


/** How long `b64` of the agent's 16-bit PCM takes to play at the call's output rate, in ms. */
function pcmMsOfBase64(b64: unknown) {
  const text = String(b64 || "");
  const padding = text.endsWith("==") ? 2 : text.endsWith("=") ? 1 : 0;
  const bytes = Math.max(0, Math.floor((text.length * 3) / 4) - padding);
  return (bytes / 2 / (session.outputRate || 16000)) * 1000;
}


/** Must the next relayed turn on the hosted protocol wait for the last one's answer? */
function relayAnswerPending() {
  const awaited = relayAnswerAwaited;
  if (awaited === null) return false;
  const now = Date.now();
  if (!awaited.outputAt) {
    if (now - awaited.sentAt < RELAY_ANSWER_WAIT_MS) return true;
    deps.addDetail(`a relayed turn drew no answer in ${Math.round(RELAY_ANSWER_WAIT_MS / 1000)} s; ` +
      "the next was read out anyway");
  } else if (now - awaited.outputAt < RELAY_ANSWER_SETTLE_MS || now < awaited.audioEndsAt + ANSWER_PLAYED_GRACE_MS) {
    return true;
  }
  relayAnswerAwaited = null;
  return false;
}


/** What the status line last said about held messages, so it says it once a count and a reason. */
let relayWaitSaid = "";


/**
 * When the oldest message still waiting to be read out began to wait, by the wall clock; 0 when none
 * is. The cap on the owner's open turn runs from the later of this and the turn's opening.
 */
let relayWaitingSince = 0;


/**
 * What a relayed turn must wait for, or null when it may go. `#330 voice-long-turn-drop`.
 *
 *   * `"answer"`: an answer's audio still playing, on any protocol — the hosted one included, where
 *     a `user_message` sent into a playing answer cuts it too; on the hosted protocol, which says
 *     nothing when a turn ends, the agent's answer to the last relayed turn not yet arrived and
 *     heard out (`relayAnswerAwaited`, `#337 voice-relay-silent`); and on `vibe-talk-v1` an answer
 *     still arriving, or one still to come: the owner's words of the open turn are final, so he has
 *     finished and the agent is answering, perhaps with a tool call before its first word
 *     (`spokenTurnHeard`); or anything else holding the floor: a text turn being answered or waiting
 *     its turn, the greeting, a segment closing.
 *   * `"turn"`: the owner's own spoken turn, open, not completed, and his words not yet final.
 *
 * A message waits here, in the relay's own queue, where the timers `#268 voice-call-drops` put on a
 * spoken turn that holds a text turn never see it. So the open spoken turn is held to bounds read
 * from its own times. The owner's turn is let go once nothing of it has arrived for
 * SPOKEN_TURN_HOLD_MS, or SPOKEN_TURN_HOLD_CAP_MS after the message began waiting on it however often
 * it went on — a word of background speech, a trickle of hypotheses, a lost `turn_complete`, as
 * `#268` bounds them. An answer is never let go while it is arriving or playing, however long, but
 * once nothing of it has arrived and nothing has played for RELAY_STALLED_ANSWER_MS it is a stalled
 * turn — and so is a turn whose owner finished speaking that long ago with nothing answering him.
 * Final words that keep on arriving are still his turn going on, and the cap still ends them, counted
 * to the last of them rather than across the silence after it. Letting a turn go is said in the
 * connection details, as the `#268` bounds say it, and the message then closes the segment.
 *
 * @returns {"answer" | "turn" | null}
 */
function relayWait() {
  if (!answerHeardOut()) return "answer";
  if (session.protocol !== "vibe-talk-v1") return relayAnswerPending() ? "answer" : null;
  if (!session.v1Ready || session.waitingForGreeting || session.waitingForAudioEnd || session.typedTurnInFlight ||
    session.pendingPrompts.length > 0) {
    return "answer";
  }
  // A typed call has no spoken turn and plays nothing: a reply still arriving holds it until its end.
  if (session.chat) return session.replyArriving ? "answer" : null;
  if (!session.spokenTurnOpen) return null;
  const now = Date.now();
  const quiet = now - Math.max(session.spokenTurnActiveAt, session.answerPlayEndMs || 0);
  if (session.replyArriving) {
    if (quiet < RELAY_STALLED_ANSWER_MS) return "answer";
    letSpokenTurnGo(`an answer went quiet for ${Math.round(quiet / 1000)} s without completing; ` +
      "a relayed message went anyway");
    return null;
  }
  const capFrom = Math.max(session.spokenTurnOpenedAt, relayWaitingSince);
  const capped = `a spoken turn held the page's next message for ${Math.round(SPOKEN_TURN_HOLD_CAP_MS / 1000)} s ` +
    "without completing; a relayed message went anyway";
  if (session.spokenTurnHeard) {
    // He has finished and nothing of the answer has arrived yet: the agent may be in a tool call
    // before its first word, and this silence is the answer's, not his turn gone quiet.
    if (quiet >= RELAY_STALLED_ANSWER_MS) {
      letSpokenTurnGo(`a spoken turn drew no answer in ${Math.round(quiet / 1000)} s and did not complete; ` +
        "a relayed message went anyway");
      return null;
    }
    // Finished words that keep on arriving are his turn going on — background speech transcribed in
    // whole sentences — and the cap still ends it: counted to the last of them, so never across the
    // silence of a tool call after his question.
    if (session.spokenTurnActiveAt - capFrom < SPOKEN_TURN_HOLD_CAP_MS) return "answer";
    letSpokenTurnGo(capped);
    return null;
  }
  if (quiet >= SPOKEN_TURN_HOLD_MS) {
    letSpokenTurnGo(`a spoken turn went quiet for ${Math.round(quiet / 1000)} s without completing; ` +
      "a relayed message went anyway");
    return null;
  }
  if (now - capFrom >= SPOKEN_TURN_HOLD_CAP_MS) {
    letSpokenTurnGo(capped);
    return null;
  }
  return "turn";
}


/**
 * Tell the agent, if all four guards allow it.
 *
 * Each of the four is load-bearing and none of them is polish:
 *
 *   * REPLAYED. The server's stream opens with its replay tail — up to two hundred messages it
 *     already published — and every attach that carries no `Last-Event-ID` gets the whole of it:
 *     a fresh sign-in, a channel change, the reconnect after an `event: reset`. Those belong on
 *     screen and they are NOT news. Relaying them says "a message was just posted" about text
 *     that may be hours old, in a burst, into a conversation billed by the minute — the same
 *     "existing history labelled as new" failure the server's seeding tick exists to prevent,
 *     arriving through the other door. `#337 voice-relay-silent` took back the cost this rule
 *     used to carry: a stream that dropped during a call and RESUMED from its cursor replays only
 *     what it missed, and what of that was posted since the call connected is news after all
 *     (`missedDuringCall`).
 *   * SELF-POSTED. `ops::reply` posts as the bot and the server's own poller reads it back. Relay
 *     that and the agent hears its own answer as news and answers it — a loop that bills.
 *   * THE MODE. `off` is silence. The other two both speak, and both therefore cost a turn of a
 *     conversation the reader is already having.
 *   * A LIVE SOCKET. There is nowhere to send it otherwise, and queuing it for the next call
 *     would deliver stale news at the start of a conversation about something else.
 *
 * Past the guards it is HELD rather than sent: see `flushRelay`, which turns a burst into one turn
 * instead of talking over itself once per message. And ONCE: a message already taken in this call
 * (`relayLedger`) is not taken again, whichever stream brings it back. `#337 voice-relay-silent`.
 *
 * @param {{fromTail?: boolean, resumed?: boolean}} [arrival] how the stream delivered it: out of
 *   its tail, on an attach that resumed from a cursor
 */
function relayToAgent(message: VibeTalk.Message, selfPosted: boolean, replayed: boolean, arrival: { fromTail?: boolean; resumed?: boolean; } = {}) {
  // `#196 auto-read-noise`. NOISE is a fifth guard, and the one the owner asked for by name: a
  // placeholder announced into a live call is the agent saying "a new message: Working…" while he
  // drives. The server already decided it is read; the call is not told otherwise.
  // `#386 hide-empty-messages`: nor is an empty message, which has nothing to announce.
  if (selfPosted || isNoise(message) || deps.hiddenEmpty(message) || deps.relayMode() === "off" || !callReachable()) {
    return false;
  }
  if (replayed && !missedDuringCall(message, arrival)) return false;
  const key = `${message.channel_id}:${message.id}`;
  if (relayLedger.has(key)) return false;
  relayLedger.add(key);
  if (!relayPending.length) relayWaitingSince = Date.now();
  relayPending.push({ line: relayLine(message), channel: String(message.channel_id) });
  if (relayTimer === null) {
    relayTimer = setTimeout(flushRelay, deps.RELAY_COALESCE_MS);
  }
  return true;
}


/**
 * Is there a call for a channel message to be read out in: one open now, or one a reconnect is
 * carrying to a new session (`#352 voice-reconnect-on-bridge-error`), whose message waits for it?
 */
function callReachable() {
  return canSendText() || Boolean(session.reconnect && session.socket);
}


/**
 * Is this replayed message one the call missed while a stream was between attaches? Only on an
 * attach that resumed from its cursor, out of the tail, and posted since the call connected (less
 * `RELAY_CLOCK_SKEW_MS`): a fresh attach's tail, or an adapter's catch-up, is history.
 * `#337 voice-relay-silent`.
 *
 * @param {{timestamp?: string}} message
 * @param {{fromTail?: boolean, resumed?: boolean}} arrival
 */
function missedDuringCall(message: { timestamp?: string; }, arrival: { fromTail?: boolean; resumed?: boolean; }) {
  if (!arrival.resumed || !arrival.fromTail || !relayCallOpenedAt) return false;
  const posted = Date.parse(String(message.timestamp || ""));
  return Number.isFinite(posted) && posted >= relayCallOpenedAt - RELAY_CLOCK_SKEW_MS;
}


/**
 * Say that channel messages this call should have heard were not read out, and why: in the
 * transcript, where the reader looks for what the agent said, and in the connection details.
 * `#337 voice-relay-silent` — never a silent drop.
 *
 *   * `ended`: they were waiting — behind an answer, or for the burst to gather — when the call
 *     ended, and go nowhere now: not into the next call, which would be stale news.
 *   * `unanswered`: they went to the agent as a turn, and the call ended before any of its answer
 *     arrived (`#352 voice-reconnect-on-bridge-error`): a voice service that lost the turn, or a
 *     hang-up while the agent was still taking it in.
 *   * `lost`: they went to the agent as a turn the voice service lost, and the reconnect that
 *     followed could not send them again: the service did not say the turn never started, or the
 *     turn was one already sent again once after an earlier reconnect.
 *   * `gap`: a stream fell further behind than it could replay, so what was posted meanwhile is
 *     unknown. The channel itself is re-read and complete; only the reading-out may have missed it.
 *
 * @param {{channel: string}[]} items the messages, for `ended`, `unanswered` and `lost`
 * @param {"ended" | "unanswered" | "lost" | "gap"} why
 */
function reportUnrelayed(items: { channel: string; }[], why: "ended" | "unanswered" | "lost" | "gap") {
  let detail;
  if (why === "gap") {
    detail = "The live feed fell behind here, so a message posted in a watched channel about then may " +
      "not have been read out. The channels themselves are complete.";
    deps.addDetail("a live stream fell behind during the call; messages posted meanwhile may not have been read out");
  } else {
    if (!items.length) return;
    const channels = [...new Set(items.map((item: { channel: string; }) => deps.channelLabel(item.channel)))];
    const where = channels.length === 1 ? channels[0] : `${channels[0]} and ${channels.length - 1} other channel${channels.length === 2 ? "" : "s"}`;
    const one = items.length === 1;
    if (why === "lost") {
      detail = `${relayCount(items.length)}, in ${where}, went to the agent, but the voice service lost the ` +
        `turn, so ${one ? "it was" : "they were"} never read out.`;
      deps.addDetail(`${relayCount(items.length)} in ${channels.join(", ")} ${one ? "was" : "were"} not read out: ` +
        "the voice service lost the turn");
    } else if (why === "unanswered") {
      detail = `${relayCount(items.length)}, in ${where}, went to the agent, but the call ended before it ` +
        `answered, so ${one ? "it was" : "they were"} never read out.`;
      deps.addDetail(`${relayCount(items.length)} in ${channels.join(", ")} ${one ? "was" : "were"} not read out: ` +
        "the call ended before the agent answered");
    } else {
      detail = `${relayCount(items.length)}, in ${where}, ${one ? "was" : "were"} still waiting when the ` +
        `call ended, so the agent never read ${one ? "it" : "them"} out.`;
      deps.addDetail(`${relayCount(items.length)} in ${channels.join(", ")} ${one ? "was" : "were"} not read out: ` +
        "the call ended first");
    }
  }
  deps.transcriptSeam("not read out", detail);
}


/**
 * A call has connected: it has heard nothing yet, and the stream of every channel starts, so a
 * message in any watched channel reaches it. `#337 voice-relay-silent`.
 */
function beginCallRelay() {
  relayLedger = new Set();
  relayAnswerAwaited = null;
  relayCallOpenedAt = Date.now();
  deps.startWatchAll();
}


/**
 * The call is over. What was still waiting to be read out is said not to have been, and the
 * stream of every channel stops: nothing but a call reads it. `#337 voice-relay-silent`.
 *
 * `#352 voice-reconnect-on-bridge-error`. And so is a relayed turn that went to the agent and drew
 * none of its answer before the call ended: the turn on the wire, on `vibe-talk-v1`
 * (`session.turnInFlight`), or the last relayed turn on the hosted protocol (`relayAnswerAwaited`).
 * A voice service that lost the turn and then ended the session took those messages with it, and
 * the report counted only what was still queued, so nothing said they were gone.
 */
function endCallRelay() {
  const inFlight = session.turnInFlight;
  const hosted = relayAnswerAwaited;
  const unanswered = [
    ...(inFlight && inFlight.kind === "relay" && !inFlight.answered ? inFlight.relays : []),
    ...(hosted && !hosted.outputAt ? hosted.held : []),
  ];
  if (relayCallOpenedAt && unanswered.length) reportUnrelayed(unanswered, "unanswered");
  const queued = session.pendingPrompts.flatMap((prompt: { text: string; kind: "typed" | "notice" | "relay"; relays: { channel: string; }[]; resent?: boolean; }) => (prompt.kind === "relay" ? prompt.relays : []));
  const unsent = [...queued, ...relayPending];
  if (relayCallOpenedAt && unsent.length) reportUnrelayed(unsent, "ended");
  forgetRelayPending();
  relayAnswerAwaited = null;
  if (relayTimer !== null) {
    clearTimeout(relayTimer);
    relayTimer = null;
  }
  relayCallOpenedAt = 0;
  deps.stopWatchAll();
}

return {
  session,
  sendClientEvent,
  get speechHold() { return speechHold; },
  set speechHold(value: typeof speechHold) { speechHold = value; },
  sendVibeTalkFrame,
  canSendText,
  noteTypedTurn,
  sendUserMessage,
  noteComposing,
  rememberComposeDraft,
  storedComposeDraft,
  sendTyped,
  keepComposerInView,
  writingToAgent,
  get hiddenDuringCall() { return hiddenDuringCall; },
  set hiddenDuringCall(value: typeof hiddenDuringCall) { hiddenDuringCall = value; },
  get visibleAt() { return visibleAt; },
  set visibleAt(value: typeof visibleAt) { visibleAt = value; },
  syncScreenWakeLock,
  retryStart,
  keepTurnFold,
  initializePageInstance,
  saveUnsettledTurns,
  recoverUnsettledTurns,
  UNRESPONSIVE_STATUS,
  settleNoticeForRecap,
  reconnectedSessionRefused,
  stop,
  get hasEnded() { return hasEnded; },
  set hasEnded(value: typeof hasEnded) { hasEnded = value; },
  get hasSuspended() { return hasSuspended; },
  set hasSuspended(value: typeof hasSuspended) { hasSuspended = value; },
  renderControls,
  setSpeakerOff,
  onTalk,
  get relayAnswerAwaited() { return relayAnswerAwaited; },
  set relayAnswerAwaited(value: typeof relayAnswerAwaited) { relayAnswerAwaited = value; },
  relayToAgent,
  reportUnrelayed,
};
}
export type VoiceCall = ReturnType<typeof createVoiceCall>;
