// The call's shared records. Controllers pass these by reference; asynchronous callbacks see
// the same session object for the lifetime of the page (#624 voice-page-modularity).
import type { Recap } from "../../core/call/index.ts";

export type CallAudioContext = AudioContext & { readonly sinkId?: string; };
export type Timer = ReturnType<typeof setTimeout>;
export interface Prompt { text: string; kind: "typed" | "notice" | "relay"; relays: {channel: string}[]; resent?: boolean; }
export interface Notice { text: string; kind: "notice"; relays: {channel: string}[]; }
export interface TurnInFlight extends Prompt { resent: boolean; after: number | null; echoed: boolean; turn: number | null; answered: boolean; }
export interface Resume { text?: string; transport?: string; included?: number; dropped?: number; words?: number | null; profileRecap?: number; }
export interface ReconnectAttempt { said: string; resend: "relay" | "typed" | "none" | "unsent"; notice: Notice | null; recap: {resume: Resume | null; why: string | null} | null; seam: Element | null; }
export interface FoldEntry { fold: HTMLElement; [key: string]: unknown; }
export interface VoiceRow extends HTMLLIElement { foldEntry?: FoldEntry; linkText?: string; }
export interface LiveTurn { key: string; who: string; settled: string; pending: string; li: VoiceRow | null; atMs: number; stored: boolean; echo: boolean; cut: boolean; savedCopy: boolean; takenElsewhere: boolean; pageEcho?: boolean; agentEcho?: boolean; nearAgent?: boolean; audibleAtStart?: boolean; undecided?: boolean; }
export interface StartupTiming { origin: number; chat: boolean; marks: Record<string, number>; sent: boolean; socket: {socket_timeout: boolean; socket_close_code?: number} | null; }
export interface CallHealth { chat: boolean; openedAt: number | null; turns: number; silentRun: number; heard: boolean; peak: number; audioMs: number; text: boolean; greetingPending: boolean; errored: boolean; lastPeak: number; lastAudioMs: number; unresponsive: boolean; episodeReported: boolean; reported: Set<string>; noReplyTimer: Timer | null; endPosted: boolean; }

export interface CallSession {
  socket: WebSocket | null;
  connected: boolean;
  failed: boolean;
  muted: boolean;
  speakerOff: boolean;
  audio: CallAudioContext | null;
  stream: MediaStream | null;
  node: ScriptProcessorNode | null;
  source: MediaStreamAudioSourceNode | null;
  playAt: number;
  playLead: number;
  playing: AudioBufferSourceNode[];
  protocol: string;
  providerName: string;
  inputRate: number;
  outputRate: number;
  lastTranscriptKey: string | null;
  conversationId: string | null;
  /** The server-minted conversation id for this call (`#1303 voice-transcript-share`), when the voice-session answer carried one. */
  serverConversationId?: string | null;
  chat: boolean;
  vendorSentAudioInChat: boolean;
  v1Ready: boolean;
  capturePaused: boolean;
  captureResumeTimer: Timer | null;
  audioSegmentActive: boolean;
  waitingForGreeting: boolean;
  waitingForAudioEnd: boolean;
  typedTurnInFlight: boolean;
  replyArriving: boolean;
  spokenTurnOpen: boolean;
  spokenTurnTimer: Timer | null;
  spokenTurnCapTimer: Timer | null;
  spokenTurnOpenedAt: number;
  spokenTurnActiveAt: number;
  spokenTurnHeard: boolean;
  lastCompletedTurn: number | null;
  pendingPrompts: Prompt[];
  turnInFlight: TurnInFlight | null;
  interruptPending: boolean;
  playbackWaitTimer: Timer | null;
  pageMessageHeldNoted: boolean;
  cutForTyped: boolean;
  micSpeechAt: number;
  ownerWordsAt: number;
  speechHold: {since: number; timer: Timer | null} | null;
  speechHolds: number;
  speechHoldMaxMs: number;
  speechHoldCapped: number;
  profileRecap: {id: number; timer: Timer | null; abandoned(served: boolean | null): void} | null;
  armedRecap: number | null;
  lastTextAt: number;
  lastAudioEndAt: number;
  lastInterruptAt: number;
  liveTurns: Map<string, LiveTurn>;
  noTextRows: Map<number, VoiceRow>;
  anonTurn: number;
  timing: StartupTiming | null;
  health: CallHealth | null;
  playEndMs: number;
  answerPlayEndMs: number;
  echoCancellation: boolean;
  greetingPlayedOut: boolean;
  greetingPromised: boolean;
  agentSaid: Map<string, {words: string[]; atMs: number}>;
  echoExposedAt: number;
  sessionStarts: number;
  documentNoticeSent: boolean;
  pendingResume: Resume | null;
  recapAck: string | null;
  recapSeam: Element | null;
  reconnect: ReconnectAttempt | null;
  reconnects: number;
  lastReconnectAt: number;
  noticeAwaitingRecap: {item: Notice; timer: Timer} | null;
  recordedAny?: boolean;
  recap?: Recap | null;
}
