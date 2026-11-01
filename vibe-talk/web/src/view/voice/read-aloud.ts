// Read-aloud transport and its controls, owned together (`#624 voice-page-modularity`).
import { browserSpeechChunks, createPreparedSpeech, joinBytes, readClock, readPieces, readSectionStarts, readTimelineWeights, recoveryWait, sentenceEnds, silentBefore, speechElapsedMs, speechSegments, ticketEnded, ticketTooShortForBlocks, wavLayout } from "../../core/speech/index.ts";
import { READ_NO_SPEECH_MS, cloudFailureText } from "../../present/call/index.ts";
import { DEVICE_SETTINGS, MAX_READ_SPEED, MIN_READ_SPEED, clampReadSpeed } from "../../core/settings/index.ts";
import { cardRootOf, idsOf, rowMessages, rowSeen } from "../../view/channel/index.ts";
import { browserSpeechFailure } from "../../present/notices/index.ts";
import { isNoise } from "../../present/rows/index.ts";
import { childByClass } from "../../view/dom/index.ts";
import type { Message } from "../../contract/index.ts";
import type { ApiRequestOptions } from "../../core/api/index.ts";
import type { DeviceSettings, ReadAudioSource } from "../../core/settings/index.ts";
import type { PreparedBlock, SpeechMoment } from "../../core/speech/index.ts";
import type { AudioPort, SpeechSynthesisPort } from "../../ports/index.ts";
import type { ElementById } from "../dom/index.ts";

interface VoiceConfig {
  readonly playback?: string;
  readonly label?: string;
  readonly chain?: readonly string[];
}
interface PrepareReply {
  readonly voices?: readonly { name: string }[];
  readonly expires_in_seconds?: number;
  readonly prepared?: readonly { message_id: string; url: string; blocks?: readonly PreparedBlock[] }[];
}
type SpeechSegment = ReturnType<typeof speechSegments>[number];
interface Point { x: number; y: number }
interface ReadFrom { part: number; block: number | null; recovery?: number; hold?: boolean }
interface Timeline { startsAt: number | null; endsAt: number; finished: boolean; gaps: readonly (readonly [number, number])[] }
interface ReadPlayer {
  pause(): void;
  play?(): Promise<void>;
  removeAttribute?(name: string): void;
  load?(): void;
  readonly hearsSpeech?: boolean;
  heardSpeech?(): boolean;
  timeline?(): Timeline;
  addEventListener?(type: string, fn: (...details: unknown[]) => void): void;
}
interface TransportPlayer extends ReadPlayer {
  play(): Promise<void>;
  addEventListener(type: string, fn: (...details: unknown[]) => void): void;
}
/** Fields are filled by the selected transport; guards retain the original read ticket and lifetime rules. */
interface Playing {
  id: string;
  audio: ReadPlayer;
  urls: string[];
  parts: string[];
  playback?: string;
  players?: TransportPlayer[];
  segments?: SpeechSegment[];
  segment?: number;
  blocks?: (readonly PreparedBlock[] | null)[];
  streamed?: boolean;
  tickets?: ({ url: string; until: SpeechMoment } | null)[];
  paused: boolean;
  stale?: boolean;
  wasHeld?: boolean;
  utterance?: SpeechSynthesisUtterance | null;
  speechTimer?: ReturnType<typeof setTimeout> | null;
  chunk?: number;
  pauseTransport?: (() => void) | null;
  resumeTransport?: (() => void) | null;
}
interface DrawnBlock { element: Element; kind: string; piece: number; pieces: number; alone?: boolean }
type Position = [Node, number];
interface Sentence { from: Position; to: Position; length: number }

/** Live page dependencies; constructing the owner performs no I/O or event registration. */
export interface ReadAloudDependencies<Column = unknown> {
  el<K extends string>(id: K): K extends keyof ElementById ? ElementById[K] : HTMLElement;
  leftEl(id: string): HTMLElement;
  deviceSettings(): DeviceSettings;
  guardQuietly(fn: () => unknown): () => Promise<unknown>;
  setStatus(text: string): void;
  api(path: string, options?: ApiRequestOptions): Promise<PrepareReply>;
  renderControlBar(): void;
  browserPorts(): {
    readonly audio: AudioPort<typeof AudioContext, HTMLAudioElement>;
    readonly speech: SpeechSynthesisPort<SpeechSynthesis | null, SpeechSynthesisUtterance>;
  };
  /** Fetch at the original call site, using the browser's binary/stream response capability. */
  fetch(input: RequestInfo | URL, init?: RequestInit): Promise<Response>;
  readonly browser: Pick<Navigator, "languages" | "language" | "mediaSession"> | undefined;
  audioAvailable(): boolean;
  clearError(): void;
  heldMessage(id: string): Message | null;
  readonly session: { readonly socket: unknown; readonly chat: boolean };
  showError(text: string, options?: { stay?: boolean }): void;
  eachColumn(fn: () => unknown): void;
  renderChannelRows(): void;
  dismissMessages(body: { messages: string[] }): Promise<void>;
  readonly currentView: string;
  selectedChannelId(): string;
  shownChannelList(): HTMLOListElement;
  readonly threadListShown: boolean;
  sideColumnUp(): boolean;
  sideShowing(): boolean;
  rowOnScreen(row: Element): boolean;
  inColumn<T>(column: Column, fn: () => T): T;
  readonly columns: { readonly side: Column };
  threadForMessageId(id: string): string | null;
  renderControls(): void;
  withThreadQuery(path: string, threadId: string | null): string;
  token(): string;
}

export function createReadAloud<Column>(deps: ReadAloudDependencies<Column>) {


  // --- reading a message aloud ----------------------------------------------------------------
  //
  // The owner's ask: in the channel view, the Talk control becomes READ. Turn it on and a tap on any
  // message reads its full text through the configured speech provider; when the audio finishes,
  // the message archives itself. Not hands-free — a tap per message — but it turns a backlog of long
  // bot messages into something that can be worked through with a thumb and an ear, with the greyed
  // rows showing exactly how far you have got.
  //
  // WHY THE SERVER MAKES THE VENDOR CALL. Reading aloud costs money and needs an ElevenLabs account
  // key. A key this page could use is a key this page could leak, so the browser asks its OWN server
  // for audio and never learns the credential; see `api::speak`.
  //
  // WHY THE ARCHIVE IS ON `ended` AND NOT ON TAP. "Read it" and "I am done with it" are the same act
  // only if the reading actually happened. Archiving on tap would file away a message whose audio
  // failed to fetch, or that the reader stopped two seconds in — which is the one thing this mode
  // must not do, because the archive is how they know what is left.

  /**
   * How fast a message is read, as a PERCENTAGE, or null for "however the agent speaks".
   *
   * Null is a real value and not zero: the owner set his agent to speak faster than default, and the
   * right behaviour with no preference expressed is to match it rather than to impose 100%. The
   * server borrows the agent's pace when this sends nothing.
   *
   * The bounds match the server's own clamp: below half the words stop being words, above double the
   * audio outruns following it, and both ends are a vendor request nobody wanted to pay for.
   */
  /** `#567 pace-stepper`. What one press of − or +, or one arrow key in the box, moves the pace by. */
  const READ_SPEED_STEP = 10;
  /**
   * How long the pace must rest before a read in progress takes it up again at the new pace: three
   * presses of + restart the sentence or block once, not three times.
   */
  const READ_SPEED_SETTLE_MS = 600;
  let readSpeed: number | null = null;

  /** Put the pace on the page and remember it. */
  function applyReadSpeed(value: number | null) {
    const previous = readSpeed;
    readSpeed = value === null ? null : clampReadSpeed(value);
    const shown = readSpeed === null ? 100 : readSpeed;
    deps.el("read-speed-box").value = `${shown}%`;
    deps.el("read-speed-down").disabled = shown <= MIN_READ_SPEED;
    deps.el("read-speed-up").disabled = shown >= MAX_READ_SPEED;
    // A button disabled under the focus would drop it to the page: it goes to the box instead.
    const focused = document.activeElement;
    if ((focused === deps.el("read-speed-down") && deps.el("read-speed-down").disabled) ||
        (focused === deps.el("read-speed-up") && deps.el("read-speed-up").disabled)) {
      deps.el("read-speed-box").focus();
    }
    deps.el("speed-value").textContent =
      readSpeed === null
        ? readAloudPlayback === "browser" ? "Pace: device default" : "Pace: as the agent speaks"
        : `Pace: ${readSpeed}%`;
    deps.el("read-pace-help").textContent = readAloudPlayback === "browser"
      ? "Unset, a message uses the device voice's normal pace. A change applies at once, from the " +
        "sentence being read."
      : "Unset, a message is read at whatever pace the voice agent is configured with. A change " +
        "applies at once, from the block being read (a message read whole starts again).";
    deps.el("read-speed-box").setAttribute("aria-label", readAloudPlayback === "browser"
      ? "reading pace, percent of the device voice's normal pace" : "reading pace, percent of the agent's own");
    deps.el("read-speed-label").textContent = readSpeed === null ? "Pace" : `${readSpeed}%`;
    try {
      deps.deviceSettings().keep(DEVICE_SETTINGS.readSpeed, readSpeed);
    } catch (_error) {
      // A browser that refuses storage still reads at the chosen pace for this session.
    }
    // THE PACE IS BAKED INTO A TICKET. It is chosen at mint time so the tap does not have to send it,
    // which means a ticket minted at the old pace would read at the old pace however the slider now
    // looks — the reader moves the control and hears no difference, the most confusing possible
    // outcome. Throwing them away costs one request and nothing at the vendor.
    if (readSpeed !== previous) {
      forgetPreparedSpeech();
      deps.guardQuietly(prepareSpeech)();
      retakeReadAtPace();
    }
  }

  /** The settle timer of `retakeReadAtPace`; null while none is waiting. */
  let readSpeedSettleTimer: ReturnType<typeof setTimeout> | null = null;

  /**
   * `#567 pace-stepper`. The pace changed while a message is being read: once it rests, the read takes
   * the new pace up where it is. The device voice says the sentence being read again at the new rate
   * (its pause and resume, `#554`); the cloud voice reads the block being read again on tickets at the
   * new pace (a jump to where it is). A held read waits: its resume uses the pace then.
   */
  function retakeReadAtPace() {
    if (readSpeedSettleTimer !== null) clearTimeout(readSpeedSettleTimer);
    readSpeedSettleTimer = setTimeout(() => {
      readSpeedSettleTimer = null;
      const playing = nowPlaying;
      if (playing === null || playing.paused) return;
      if (playing.playback === "browser") {
        if (typeof playing.pauseTransport === "function" && typeof playing.resumeTransport === "function") {
          playing.pauseTransport();
          // A beat between the cancel and the next speak: some engines drop a speak issued in the
          // same moment as a cancel.
          const ticket = readingTicket;
          setTimeout(() => {
            if (ticket === readingTicket && nowPlaying === playing && !playing.paused) playing.resumeTransport!();
          }, 50);
        }
        return;
      }
      if (playing.streamed === true && Array.isArray(playing.segments!)) jumpReading(playing.segment || 0);
    }, READ_SPEED_SETTLE_MS);
  }

  /** `#567 pace-stepper`. One press of − (`-1`) or + (`+1`): to the next step of ten, within the range. */
  function stepReadSpeed(direction: number) {
    const from = readSpeed === null ? 100 : readSpeed;
    const next = direction > 0
      ? Math.floor(from / READ_SPEED_STEP) * READ_SPEED_STEP + READ_SPEED_STEP
      : Math.ceil(from / READ_SPEED_STEP) * READ_SPEED_STEP - READ_SPEED_STEP;
    applyReadSpeed(Math.min(MAX_READ_SPEED, Math.max(MIN_READ_SPEED, next)));
  }

  /**
   * `#567 pace-stepper`. What is typed in the box, taken on Enter or when the box is left: a number,
   * with or without "%", clamped to the range. Anything else puts the pace it had back in the box.
   */
  function commitReadSpeedBox() {
    const value = deps.el("read-speed-box").value.trim();
    // Left as it was shown — tapped into and out of — chooses nothing: an unset pace stays the voice's
    // own, which the box shows as 100%, and a read in progress is not restarted.
    if (value === `${readSpeed === null ? 100 : readSpeed}%`) return;
    const typed = /^(\d{1,4}(?:\.\d+)?)\s*%?$/u.exec(value);
    applyReadSpeed(typed ? Number(typed[1]!) : readSpeed);
  }

  /** What was stored, clamped. Absent means "as the agent speaks", which is not the same as 100%. */
  function storedReadSpeed() {
    return deps.deviceSettings().get(DEVICE_SETTINGS.readSpeed);
  }


  /** Is a tap on a message a request to hear it? Session-only, and only in the channel view. */
  let readingMode = false;

  // Playback is a capability declared by the backend. Older servers use the audio route. A device
  // voice is explicitly selected; it never falls back to a remote speech service.
  let readAloudPlayback = "audio";
  let readAloudLabel = "Voice provider";
  const READ_AUDIO_SOURCE_KEY = DEVICE_SETTINGS.readAudioSource.key;
  let agentReadAloud: VoiceConfig | null = null;
  let readAudioSource: ReadAudioSource = "device";
  let browserVoicesListening = false;
  let waitingForBrowserVoice = false;
  let browserVoiceProblem = "";

  function storedReadAudioSource() {
    // A browser refusing storage reads as no choice (`DEVICE_SETTINGS.readAudioSource.refused`).
    return deps.deviceSettings().get(DEVICE_SETTINGS.readAudioSource);
  }

  // `#552 wit-tts-provider`. When the server's read-aloud backend is Wit.ai,
  // Settings offers its catalogue: a voice the reader picks here rides on
  // every speech preparation, and the server's configured voice is the
  // fallback the empty choice names.
  const WIT_VOICE_KEY = DEVICE_SETTINGS.witVoice.key;
  let witReadAloud: VoiceConfig | null = null;
  let witVoice = "";

  // `#563 read-aloud-fallback-chain`. True when the server's configured
  // chain ends with the device voice: a server read that fails for good is
  // then read by this device instead of stopping.
  let chainDeviceTail = false;

  /**
   * Read `parts` with the device voice as a chain's last resort, when the
   * chain ends with it. False when it does not apply or the device cannot
   * read either, so the caller keeps its ordinary failure.
   *
   * @param {Array<string>} parts
   * @param {string} id
   * @param {number} ticket
   */
  function readWithDeviceTail(parts: string[], id: string, ticket: number) {
    if (!chainDeviceTail || readAloudPlayback !== "audio") return false;
    try {
      readWithBrowserSpeech(parts, id, ticket);
      deps.setStatus("Reading with the device voice.");
      return true;
    } catch (_error) {
      return false;
    }
  }

  function storedWitVoice() {
    // A browser refusing storage reads as the configured voice (`DEVICE_SETTINGS.witVoice.refused`).
    return deps.deviceSettings().get(DEVICE_SETTINGS.witVoice);
  }

  /** Choose the Wit.ai voice for later reads; the configured voice when cleared. */
  function setWitVoice(name: unknown, announce = true) {
    witVoice = String(name || "");
    try {
      deps.deviceSettings().keep(DEVICE_SETTINGS.witVoice, witVoice);
    } catch (_error) {
      // The choice still applies to this page when storage is unavailable.
    }
    deps.el("settings-wit-voice").value = witVoice;
    forgetPreparedSpeech();
    if (announce) {
      deps.setStatus(witVoice ? `Wit.ai voice set to ${witVoice}.` : "Wit.ai will use the configured voice.");
    }
  }

  /** Fill the Wit.ai voice picker from the server's catalogue. */
  async function loadWitVoices() {
    if (!witReadAloud) return;
    try {
      const answer = await deps.api("/api/v1/read-aloud/voices");
      const voices = Array.isArray(answer && answer.voices) ? answer.voices! : [];
      const picker = deps.el("settings-wit-voice");
      picker.replaceChildren();
      const configured = document.createElement("option");
      configured.value = "";
      configured.textContent = "Configured voice";
      picker.append(configured);
      for (const voice of voices) {
        const name = String(voice && voice.name || "");
        if (!name) continue;
        const option = document.createElement("option");
        option.value = name;
        option.textContent = name.replace(/^wit\$/u, "");
        picker.append(option);
      }
      picker.value = witVoice;
      deps.el("wit-voice-state").textContent = voices.length > 0
        ? `${voices.length} Wit.ai voices available.`
        : "Wit.ai listed no voices for this app.";
    } catch (_error) {
      deps.el("wit-voice-state").textContent =
        "Wit.ai voices could not be listed; the configured voice will be used.";
    }
  }

  /** Select where message audio is produced. The toggle itself never waits for either provider. */
  function setReadAudioSource(source: unknown, announce = true) {
    // A stored "agent" from another server-audio backend means the Wit.ai
    // voice on this one: the picker is the same choice under its own name.
    readAudioSource = source === "wit" && witReadAloud ? "wit"
      : source === "agent" && witReadAloud ? "wit"
      : source === "agent" && agentReadAloud ? "agent" : "device";
    stopReading();
    forgetPreparedSpeech();
    const serverSpeech = readAudioSource === "wit" ? witReadAloud
      : readAudioSource === "agent" ? agentReadAloud : null;
    if (serverSpeech) {
      readAloudPlayback = String(serverSpeech.playback || "audio");
      readAloudLabel = String(serverSpeech["label"] || "Agent voice");
    } else {
      readAloudPlayback = "browser";
      readAloudLabel = "Device voice";
      prepareBrowserSpeech();
    }
    try {
      deps.deviceSettings().keep(DEVICE_SETTINGS.readAudioSource, readAudioSource);
    } catch (_error) {
      // The selection still applies to this page when storage is unavailable.
    }
    const agent = readAudioSource !== "device";
    deps.el("audio-source").setAttribute("aria-checked", agent ? "true" : "false");
    deps.el("audio-source").setAttribute(
      "aria-label", agent ? `Read messages with ${readAloudLabel}` : "Read messages with device audio"
    );
    deps.el("audio-source").title = agent
      ? `${readAloudLabel}. Tap to use device audio.`
      : `Device audio. Tap to use ${agentReadAloud ? agentReadAloud["label"]! : "the configured agent"}.`;
    deps.el("settings-audio-source").value = readAudioSource;
    deps.el("settings-agent-audio").toggleAttribute("hidden", agentReadAloud === null || witReadAloud !== null);
    deps.el("settings-agent-audio").toggleAttribute("disabled", agentReadAloud === null || witReadAloud !== null);
    deps.el("settings-wit-audio").toggleAttribute("hidden", witReadAloud === null);
    deps.el("settings-wit-audio").toggleAttribute("disabled", witReadAloud === null);
    deps.el("wit-voice-picker").toggleAttribute("hidden", witReadAloud === null);
    // `#563 read-aloud-fallback-chain`: the configured order, first first.
    const chainOrder = serverSpeech && Array.isArray(serverSpeech.chain) ? serverSpeech.chain : [];
    deps.el("settings-chain-order").toggleAttribute("hidden", chainOrder.length < 2);
    if (chainOrder.length >= 2) {
      deps.el("settings-chain-order").textContent = `Voice order: ${chainOrder.join(" → ")}`;
    }
    // The icons are <svg>, and `hidden` reflects only on HTMLElement: assigning `.hidden` here would
    // set an expando and leave the phone icon showing in agent mode.
    deps.el("audio-device-icon").toggleAttribute("hidden", agent);
    deps.el("audio-agent-icon").toggleAttribute("hidden", !agent);
    if (readingMode) {
      setReadState("ready");
      deps.guardQuietly(prepareSpeech)();
    }
    deps.renderControlBar();
    applyReadSpeed(readSpeed);
    if (announce) deps.setStatus(`Messages will use ${readAloudLabel}.`);
  }

  /** The device's speech engine, or null where it has none that can list voices, speak and cancel (`browserPorts`). */
  function browserSpeechEngine() {
    return deps.browserPorts().speech.engine();
  }

  function browserSpeechVoice() {
    const engine = browserSpeechEngine();
    if (!engine) return null;
    // Some browser voices send text to a service. Only voices explicitly reported as local qualify;
    // the phone's configured speech engine still determines how it generates audio. An empty list
    // must never select the browser's implicit default.
    const voices = engine.getVoices().filter((voice) => voice.localService === true);
    const languages = [
      ...(Array.isArray(deps.browser!.languages) ? deps.browser!.languages : []),
      deps.browser!.language,
    ].map((language) => String(language || "").replace(/_/gu, "-").toLowerCase())
      .filter((language, index, all) => language && all.indexOf(language) === index);
    const tagged = voices.map((voice) => ({
      voice,
      language: String(voice.lang || "").replace(/_/gu, "-").toLowerCase(),
    }));
    // A browser's `default` flag means the engine's default, not "safe for this text". Android may
    // report a downloaded voice for another language as both local and default; selecting it made
    // English messages sound like corrupt noise or like another language. Refuse that fallback.
    for (const language of languages) {
      const exact = tagged.filter((entry) => entry.language === language);
      if (exact.length > 0) return (exact.find((entry) => entry.voice.default) || exact[0]!).voice;
    }
    for (const language of languages) {
      const base = language.split("-")[0]!;
      const compatible = tagged.filter((entry) => entry.language.split("-")[0]! === base);
      if (compatible.length > 0) {
        return (compatible.find((entry) => entry.voice.default) || compatible[0]!).voice;
      }
    }
    return null;
  }

  function prepareBrowserSpeech() {
    const engine = browserSpeechEngine();
    if (!engine) return;
    // Chrome may expose an empty list at first. Populate it ahead of the tap; do not defer speak()
    // until this event, because a later callback may no longer have the user's audio permission.
    if (!browserVoicesListening && typeof engine.addEventListener === "function") {
      engine.addEventListener("voiceschanged", () => {
        if (readAloudPlayback === "browser" && waitingForBrowserVoice && browserSpeechVoice()) {
          waitingForBrowserVoice = false;
          if (readingMode) {
            if (deps.el("error").textContent === browserVoiceProblem) deps.clearError();
            setReadState("ready");
            deps.setStatus("Device voice is ready. Tap a message to hear it.");
          }
        }
      });
      browserVoicesListening = true;
    }
    browserSpeechVoice();
  }

  function browserSpeechProblem() {
    if (!browserSpeechEngine()) {
      return "This browser does not expose device speech. Open this page in Chrome on Android.";
    }
    if (!browserSpeechVoice()) {
      waitingForBrowserVoice = true;
      browserVoiceProblem = "No installed device voice matches this browser's language. In Android Settings, open " +
        "Text-to-speech output and download a voice for that language, then return here and tap the message again.";
      return browserVoiceProblem;
    }
    return "";
  }

  const BROWSER_SPEECH_START_MS = 10000;
  const BROWSER_SPEECH_FINISH_MS = 60000;

  // --- where a read's message is drawn: `#540 read-aloud-desktop` ----------------------------------
  //
  // The owner, 2026-10-26, on a desk: in the thread list the device voice said the message "wasn't
  // loaded", and a read's sentence was lit nowhere. Every lookup a read made named the left column's
  // lists, `#discord-log` and `#pinned-log`, and the thread list draws its messages in the cards' list
  // (unpacked rows, Main's rows among the cards) and the right column in `#side-log`. So a read finds its
  // message, and lights its rows, in every list a message row is drawn in, by the lists' own ids,
  // whichever column is being worked on.

  /** Every list a message row that can be read aloud is drawn in. */
  const READABLE_LISTS = ["discord-log", "pinned-log", "side-log", "side-pinned-log", "side-cards"];

  /** The rows of list `id`, or none where the page has no such list. */
  function listRows(id: string) {
    const list = deps.leftEl(id);
    return list && list.children ? [...list.children] : [];
  }

  /** Every message row drawn for the read whose first message is `id`, in every list. A card is none. */
  function readingRowsOf(id: string) {
    const wanted = String(id);
    return READABLE_LISTS.flatMap(listRows)
      .filter((row) => row.getAttribute("data-id") === wanted && !row.hasAttribute("data-card"));
  }

  /**
   * The message `id` as the page holds it: from any row drawn for it, a card's first message
   * (`readCardAloud`), or the store, for one no list draws now. Null when none has it.
   */
  function readableMessage(id: string) {
    const wanted = String(id);
    for (const row of READABLE_LISTS.flatMap(listRows)) {
      const held = row.hasAttribute("data-card") ? [cardRootOf(row)] : rowMessages(row);
      const found = held.find((message) => message && String(message.id) === wanted);
      if (found) return found;
    }
    return deps.heldMessage(wanted);
  }

  /**
   * `#540 read-aloud-desktop`. A press on a card in reading mode reads the thread's first message, as a
   * copy of the card copies it (`#296 copy-highlighted-message`): the server speaks messages, and a
   * summary is not one. The card is marked as its rows are (`markReadingCard`).
   */
  function readCardAloud(row: Element) {
    const root = cardRootOf(row);
    if (!root) {
      deps.setStatus("This thread's first message is not loaded yet; open the thread to read it.");
      return;
    }
    deps.guardQuietly(() => readAloud([String(root.id)]))();
  }

  /** Card `row` says whether its first message is being read, or asked for. Written only where it changes. */
  function markReadingCard(row: Element) {
    const root = cardRootOf(row);
    const id = root ? String(root.id) : null;
    const reading = id !== null && nowPlaying !== null && nowPlaying!.id === id ? "true" : "false";
    const pending = id !== null && pendingRead === id ? "true" : "false";
    if (row.getAttribute("data-reading") !== reading) row.setAttribute("data-reading", reading);
    if (row.getAttribute("data-pending") !== pending) row.setAttribute("data-pending", pending);
  }

  /** Every card in the cards' list, marked as `markReadingCard` says. */
  function paintReadingCards() {
    for (const row of listRows("side-cards")) {
      if (row.hasAttribute("data-card")) markReadingCard(row);
    }
  }

  /** Speak the cached prepared row on the tap's own call stack, without a network request or await. */
  function readWithBrowserSpeech(parts: string[], id: string, ticket: number) {
    const problem = browserSpeechProblem();
    if (problem) throw new Error(problem);
    if (deps.session.socket && !deps.session.chat) {
      throw new Error("Hang up the voice call before reading messages with the device voice.");
    }
    const text = parts.map((part) => {
      const message = readableMessage(part);
      if (!message) throw new Error("That message is no longer loaded. Refresh the channel and tap it again.");
      // The server prepares the same body for every speech path. Device-local synthesis used to be
      // the exception: it read `content` directly, so markdown and long ids were spoken literally
      // even though Agent audio and Read new both used `spoken_content`.
      return String(message.spoken_content || message.content || "");
    }).join("\n\n");
    const chunks = browserSpeechChunks(text);
    if (chunks.length === 0) throw new Error("This message has no text to read.");
    const engine = browserSpeechEngine()!;
    const voice = browserSpeechVoice()!;
    waitingForBrowserVoice = false;
    deps.clearError();
    // Retain the current utterance as well as its listeners. Some engines otherwise lose events
    // after garbage collection, which would strand the row and its remaining chunks.
    // `audio.pause` is what a STOP reaches it by: cancel, then unpause the engine — an engine left
    // paused by a held read that was stopped would swallow the next read into a paused queue.
    nowPlaying = { id, playback: "browser",
      audio: { pause: () => { engine.cancel(); if (typeof engine.resume === "function") engine.resume(); } }, urls: [],
      utterance: null, speechTimer: null, parts, chunk: 0, paused: false, stale: false,
      pauseTransport: null, resumeTransport: null };
    const current = () => ticket === readingTicket && nowPlaying !== null && nowPlaying!.id === id;
    const fail = (error: unknown) => {
      if (!current()) return;
      // `#434 read-aloud-pause`: as for a streamed part, a failure under a hold is the resume's
      // to repair (the held chunk is spoken again), not the end of a read only paused.
      if (nowPlaying!.paused) {
        nowPlaying = { ...nowPlaying!, stale: true };
        return;
      }
      stopReading();
      setReadState("failed");
      const detail = browserSpeechFailure(error);
      deps.setStatus(detail);
      deps.showError(detail);
    };
    const speakPart = (index: number) => {
      if (!current()) return;
      // Where the read is, at chunk granularity: a resume that cannot simply resume the engine
      // speaks this chunk again, and no other (`#434 read-aloud-pause`).
      nowPlaying!.chunk = index;
      const utterance = deps.browserPorts().speech.utterance(chunks[index]!);
      utterance.voice = voice;
      utterance.lang = voice.lang;
      utterance.rate = readSpeed === null ? 1 : readSpeed / 100;
      const isCurrentPart = () => current() && nowPlaying!.utterance === utterance;
      const watch = (delay: number, error: string) => {
        if (nowPlaying!.speechTimer! !== null) clearTimeout(nowPlaying!.speechTimer!);
        nowPlaying!.speechTimer! = setTimeout(() => {
          if (isCurrentPart()) fail(error);
        }, delay);
      };
      utterance.onstart = () => {
        if (!isCurrentPart()) return;
        watch(BROWSER_SPEECH_FINISH_MS, "speech-timeout");
        // The device voice by name: it may be reading one message in place of the cloud voice
        // (`#548 cloud-read-failures`), which stays the voice selected.
        if (pendingRead === id) deps.setStatus("Reading with Device voice.");
        pendingRead = null;
        setReadState("ready");
        deps.eachColumn(deps.renderChannelRows);
      };
      utterance.onerror = (event) => { if (isCurrentPart()) fail(event.error); };
      utterance.onend = () => {
        if (!isCurrentPart()) return;
        if (index + 1 < chunks.length) {
          speakPart(index + 1);
        } else {
          stopReading();
          setReadState("ready");
          deps.guardQuietly(() => deps.dismissMessages({ messages: parts }))();
        }
      };
      nowPlaying!.utterance = utterance;
      watch(BROWSER_SPEECH_START_MS, "start-timeout");
      try {
        engine.speak(utterance);
      } catch (_error) {
        fail("synthesis-failed");
      }
    };
    // `#434 read-aloud-pause`. The device voice's share of the one pause path. `#554
    // read-pause-reply-resume`: it is a STOP AT THE SENTENCE, never the engine's own pause. Chrome on
    // a phone has none: its `pause()` stops the voice and its `resume()` does nothing (in Chromium,
    // the phone build's speech platform), and the stopped utterance is never ended, so the engine goes on
    // counting it as current and a Resume that asked the engine to resume was silence until the
    // watchdog gave the read up. So the utterance is let go first — the cancel is then not read as
    // this part ending — the engine is emptied, and the watchdog stops with it (an hour on pause is
    // not a timeout). The resume says the sentence it was paused in again, from its start, and goes
    // on: the same on every engine, and after anything the reader did in between (a reply dictated
    // through the keyboard's microphone takes the audio, and can silence an engine's queue).
    nowPlaying!.pauseTransport = () => {
      if (nowPlaying!.speechTimer! !== null) clearTimeout(nowPlaying!.speechTimer!);
      nowPlaying!.speechTimer = null;
      nowPlaying!.utterance = null;
      engine.cancel();
      // An engine cancelled while paused can stay paused (see `audio.pause`).
      if (engine.paused === true && typeof engine.resume === "function") engine.resume();
    };
    nowPlaying!.resumeTransport = () => {
      if (nowPlaying!.utterance !== null) {
        nowPlaying!.utterance = null;
        engine.cancel();
      }
      if (engine.paused === true && typeof engine.resume === "function") engine.resume();
      nowPlaying!.stale = false;
      speakPart(nowPlaying!.chunk || 0);
    };
    // An engine a stopped held read left paused must not swallow this one (see `audio.pause`).
    if (engine.paused === true && typeof engine.resume === "function") engine.resume();
    speakPart(0);
    renderReadTransport();
  }

  /**
   * Where each visible message can be PLAYED from, resolved before the reader taps anything.
   *
   * Message id -> `{url, until}`: a URL that streams that message's audio, and when its ticket may have
   * ended, on two clocks (`speechDeadlineNow`). The server minted these when read-aloud was switched
   * on: it resolved every id on screen against one Discord window, normalised each for speech, and
   * warmed the voice lookup. What a tap then costs is opening this URL.
   *
   * THE URL CARRIES ITS OWN AUTHORITY, which is the only reason any of this works: an `<audio src>`
   * cannot send an `Authorization` header, and without an `<audio src>` the browser will not stream —
   * it has to be handed a complete file. That is the wait being removed.
   *
   * AND THAT AUTHORITY ENDS. `#290 speech-ticket-refresh`: a ticket lives `expires_in_seconds` from when
   * the server minted it, and a URL played after that is a 404 and "that message could not be played".
   * So each entry keeps its own deadline, `until`: when the request that minted it was SENT, plus that
   * lifetime, so the server's work and the answer's journey shorten how long the URL is used and never
   * stretch it past its ticket. On BOTH clocks, and past either is past: the monotonic one cannot be
   * moved back by a clock change, but on a phone it stops while the device sleeps (`performance.now()`
   * does on iOS, and on Android's CLOCK_MONOTONIC), while the server's ticket keeps ageing; the wall
   * clock goes on through sleep. A wall clock set back only loosens its own check, which the monotonic
   * one still holds; one set forward only prepares a row early. Per entry, because rows are
   * prepared at different times — the screen's batch, a tap's own row (`prepareSpeechFor`). An entry
   * past its deadline is read as missing (`preparedEntry`).
   *
   * Empty is not an error. Anything missing falls back to fetching the audio as a blob, which is
   * exactly what this page did before and is still correct, only slower.
   *
   * The owner is web/src/core/speech `createPreparedSpeech` (`#624 voice-page-modularity`), on this page's
   * two clocks.
   *
   * @type {import("../../core/speech/index.ts").PreparedSpeech}
   */
  const preparedSpeech = createPreparedSpeech({ now: () => speechDeadlineNow() });

  /**
   * The prepared entry for message `id` while its ticket may still be good, else null: an entry past
   * its deadline is dropped, and the row is one with nothing prepared, which the screen's preparation
   * asks for again and a tap prepares again once before it reads by the slower route.
   * `#290 speech-ticket-refresh`.
   *
   * @param {string} id
   */
  function preparedEntry(id: string) {
    return preparedSpeech.entry(id);
  }

  /**
   * Resolve everything on screen so a tap does not have to.
   *
   * Called when read-aloud is switched on, and again as rows arrive or scroll into reach. Cheap to
   * repeat: it asks only for rows with nothing prepared, against messages the server has usually
   * just fetched anyway. It spends NOTHING at the vendor — no audio is generated until a tap.
   *
   * A TAP NEVER WAITS FOR THIS. `#191 read-aloud-first-lag`: a tap on a row that has no ticket yet
   * prepares that row on its own (`prepareSpeechFor`) and starts reading it, and this batch carries
   * on behind it.
   *
   * ONE REQUEST AT A TIME, and a tap takes its place. `#375 read-aloud-latency`: the server prepares
   * one thread per request, and this used to send every thread's request at once. On a chat service
   * whose lookups queue behind one upstream slot, ten threads on screen were ten lookups queued
   * behind each other, and the tapped row's own preparation queued behind all ten: the owner's tap
   * waited 46 to 53 seconds and then failed with them. Now the threads go one after another, the
   * rows on screen first, and a tap that needs a preparation cancels the one in flight
   * (`cancelPrepareSpeech`), which starts again once the tap's own request is answered.
   */
  let prepareSpeechInFlight: Promise<void> | null = null;
  /** Cancels the batch in flight; null when none is. */
  let prepareSpeechAbort: AbortController | null = null;

  async function prepareSpeech() {
    if (prepareSpeechInFlight !== null) return prepareSpeechInFlight;
    const abort = typeof AbortController === "function" ? new AbortController() : null;
    const running = prepareSpeechNow(abort ? abort.signal : null);
    prepareSpeechInFlight = running;
    prepareSpeechAbort = abort;
    try {
      return await running;
    } finally {
      if (prepareSpeechInFlight === running) {
        prepareSpeechInFlight = null;
        prepareSpeechAbort = null;
      }
    }
  }

  /**
   * Stop the batch in flight, so the next `prepareSpeech` starts afresh. Its request is closed, which
   * frees the server's lookup for a tap, and nothing it was answered with afterwards is kept.
   */
  function cancelPrepareSpeech() {
    if (prepareSpeechAbort !== null) prepareSpeechAbort.abort();
    prepareSpeechInFlight = null;
    prepareSpeechAbort = null;
  }

  /** @param {AbortSignal | null} signal */
  async function prepareSpeechNow(signal: AbortSignal | null) {
    if (!readingMode || deps.currentView !== "discord") {
      return;
    }
    if (readAloudPlayback === "browser") {
      prepareBrowserSpeech();
      return;
    }
    if (readAloudPlayback !== "audio") return;
    const channel = deps.selectedChannelId();
    if (!channel) {
      return;
    }
    // The rows the reader can see first, then the rest, each in screen order. `#375`.
    const seen: string[] = [];
    const beyond: string[] = [];
    // `#540 read-aloud-desktop`. And the lists the thread list and the right column draw: their rows, and
    // each card's first message, which a press on the card reads.
    // Not the left column's list behind the thread list, which is out of sight there.
    const shown = deps.shownChannelList();
    const lists = [...(deps.threadListShown ? [] : [shown]), ...(deps.sideColumnUp() ? [deps.leftEl("side-cards")] : []),
      ...(deps.sideShowing() ? [deps.leftEl("side-log")] : [])];
    for (const li of lists.flatMap((list) => (list ? [...list.children] : []))) {
      const card = li.hasAttribute("data-card") ? cardRootOf(li) : null;
      // `#196 auto-read-noise`. A placeholder is not prepared: preparing warms a vendor lookup for a
      // message nobody is going to ask to hear. A tap on one still reads it, preparing it then.
      const held = card ? [card] : rowMessages(li);
      let visible = true;
      try {
        // Against the scroller of the column the row is in.
        visible = li.parentNode === shown ? deps.rowOnScreen(li) : deps.inColumn(deps.columns.side, () => deps.rowOnScreen(li));
      } catch (_error) {
        // Nothing laid out to ask: screen order alone.
      }
      (card ? [String(card.id)] : idsOf(li)).forEach((id, index) => {
        if (id && !preparedEntry(id) && !isNoise(held[index]!) && !seen.includes(id) && !beyond.includes(id)) {
          (visible ? seen : beyond).push(id);
        }
      });
    }
    const ids = [...seen, ...beyond];
    if (ids.length === 0) {
      return;
    }
    try {
      await requestPreparedSpeech(channel, ids, signal);
    } catch (_error) {
      // PREPARING IS AN OPTIMISATION AND FAILS LIKE ONE. The tap still works without it, by the
      // older and slower route, so taking the channel away over this would trade a slow feature for
      // a broken one.
    }
  }

  /**
   * Prepare exactly these messages, one request per thread they belong to, and keep their tickets.
   *
   * The requests go ONE AFTER ANOTHER, in the order their threads first appear in `ids`, and each
   * thread's tickets are kept as soon as they arrive. `#375 read-aloud-latency`: sent all at once
   * they queued behind each other upstream anyway, and every one waited for the slowest.
   *
   * @param {string} channel the channel the ids are in.
   * @param {Array<string>} ids message ids, most wanted first.
   * @param {AbortSignal | null} [signal] stops it between requests, and closes the one in flight.
   */
  async function requestPreparedSpeech(channel: string, ids: string[], signal: AbortSignal | null = null) {
    const groups = new Map<string | null, string[]>();
    for (const id of ids) {
      const threadId = deps.threadForMessageId(id);
      if (!groups.has(threadId)) groups.set(threadId, []);
      groups.get(threadId)!.push(id);
    }
    for (const [threadId, messageIds] of groups) {
      if (signal && signal.aborted) return;
      // Before the request goes: the tickets may be minted the moment it arrives, and counting from
      // the answer would let a URL outlive its ticket by however long the answer took. `#290`.
      const requestedAt = speechDeadlineNow();
      const batch = await deps.api(
        `/api/v1/channels/${encodeURIComponent(channel)}/speech/prepare`,
        { method: "POST", body: { ids: messageIds, speed: readSpeed === null ? null : readSpeed / 100,
          ...(readAudioSource === "wit" && witVoice ? { voice: witVoice } : {}),
          ...(threadId ? { thread_id: threadId } : {}) }, ...(signal ? { signal } : {}) }
      );
      // Cancelled while it was answered: its tickets may be for a pace or a screen that has gone.
      if (signal && signal.aborted) return;
      // A batch that says no lifetime gives URLs that are never played: none is not forever.
      const lifetimeMs = (Number(batch && batch.expires_in_seconds) || 0) * 1000;
      const until = { mono: requestedAt.mono + lifetimeMs, wall: requestedAt.wall + lifetimeMs };
      for (const entry of (batch && batch.prepared) || []) {
        // `#376 read-aloud-blocks`. The blocks the server will read the message as, when it is more
        // than one: `[{kind, spoken}]`, in drawing order.
        const blocks = Array.isArray(entry.blocks) && entry.blocks.length > 1 ? entry.blocks : null;
        preparedSpeech.keep(String(entry.message_id), { url: entry.url, until, blocks });
      }
    }
  }

  /**
   * `#191 read-aloud-first-lag`. Prepare the row the reader just tapped, NOW, and on its own.
   *
   * A tap on a row with no ticket used to join the batch for the whole screen and wait for all of
   * it. On the measured first read that was 1.78 s of a five-second wait, and most of it was a chat
   * fetch for rows nobody had asked to hear yet. This asks for the tapped row's messages and nothing
   * else, so the read starts as soon as they are ready, while the batch finishes behind it.
   *
   * Never throws. A failure leaves the row unprepared, and the tap falls back to the older and
   * slower route, as it does when the batch fails.
   *
   * Unlike the batch, it does not ask whether Read is on or which view is showing. The batch runs
   * on its own, so it must check that someone is reading; this runs only inside a tap that is going
   * to read anyway, for the same channel and messages the slower route (`fetchSpeech`) would fetch
   * without those checks. Skipping them reaches nothing that route does not.
   *
   * @param {Array<string>} ids the tapped row's message ids, oldest first.
   */
  async function prepareSpeechFor(ids: string[]) {
    const channel = deps.selectedChannelId();
    if (!channel || readAloudPlayback !== "audio") return;
    const missing = ids.map(String).filter((id) => id && !preparedEntry(id));
    if (missing.length === 0) return;
    // The batch gives way: on the server its lookup would sit ahead of this one. `#375`. The tap
    // starts it again (`readAloud`) once this is answered.
    cancelPrepareSpeech();
    try {
      await requestPreparedSpeech(channel, missing);
    } catch (_error) {
      // As for the batch: the slower route still reads it.
    }
  }

  // `#376 read-aloud-blocks`. The life a ticket must still have for a read in blocks to start on it
  // (`BLOCK_READ_TICKET_MARGIN_MS`, `ticketTooShortForBlocks`) is web/src/core/speech's.

  /**
   * Forget what was prepared. The pace is baked into a ticket, so changing it invalidates them, and a
   * batch still in flight would bring back tickets at the old pace: it is cancelled.
   */
  function forgetPreparedSpeech() {
    cancelPrepareSpeech();
    preparedSpeech.clear();
  }

  /** The message being read right now, and the player reading it. Null when nothing is playing. */
  let nowPlaying: Playing | null = null;

  /**
   * ONE VOICE AT A TIME, and the counter is what makes that true.
   *
   * The owner tapped a message twice and heard TWO copies of it read over each other. `readAloud`
   * stops whatever is playing and then AWAITS the audio — so a second tap arriving during that await
   * found nothing playing to stop, and both fetches went on to build a player. `nowPlaying` was
   * overwritten by the second, leaving the first with no reference and nothing able to pause it: two
   * voices, and only one of them stoppable.
   *
   * A boolean "busy" flag would not fix it either, because the act has to remain INTERRUPTIBLE — the
   * whole point of tapping again is to stop. So every attempt takes a ticket, and any attempt whose
   * ticket is stale by the time its audio arrives throws that audio away instead of playing it. The
   * last tap wins, always, and nothing else ever reaches a speaker.
   */
  let readingTicket = 0;

  /** `#487 read-aloud-recovery`. The try at starting a cut-off read again that is waiting, or null. */
  let readRecoveryTimer: ReturnType<typeof setTimeout> | null = null;

  /**
   * The message a tap has ASKED for, before any audio exists.
   *
   * The whole of the responsiveness complaint. A tap starts a fetch to this server, which starts a
   * request to ElevenLabs, which synthesises the entire message; only when that returns did anything
   * on screen change. The reader tapped and watched nothing happen for seconds — which, before the
   * one-voice-at-a-time fix, is exactly why they tapped again.
   *
   * Set SYNCHRONOUSLY, on the tap, and cleared when the audio starts or the attempt dies. Held apart
   * from `nowPlaying` because "about to be read" and "being read" are different facts and must not
   * look the same: one of them the reader can still change their mind about.
   */
  let pendingRead: string | null = null;

  /**
   * What the Read control is doing, for the reader rather than for the code.
   *
   * `idle` — the mode is off. `ready` — on, nothing in flight. `working` — a read is being fetched.
   * `failed` — the last attempt did not produce audio.
   *
   * There is deliberately no "connected" state. The provider transport is an implementation detail:
   * it may be reused, expire, or reconnect without changing the reader's task. What the reader can
   * actually be told is whether something is in flight now and whether the last one worked, which is
   * what these four say.
   */
  let readState = "idle";

  function setReadState(state: string) {
    readState = state;
    deps.renderControls();
  }

  /** Stop whatever is playing and forget it. Safe to call when nothing is. */
  function stopReading() {
    // Invalidate every read in flight, not only the one that is audible. A fetch that has not come
    // back yet is still going to build a player unless its ticket is stale.
    readingTicket += 1;
    const wasPending = pendingRead !== null;
    pendingRead = null;
    // `#487 read-aloud-recovery`. A read waiting to start again after it was cut off held the media
    // keys and the audio output through the wait, as a jump does; stopped now, it gives them back.
    if (readRecoveryTimer !== null && !readJumping) {
      clearTimeout(readRecoveryTimer);
      readRecoveryTimer = null;
      releaseMediaSession();
      quietReadAloudContext();
    }
    if (nowPlaying === null) {
      // ABORTING A READ THAT NEVER STARTED still has to take the highlight off. This returned early,
      // so a message aborted while its audio was still being fetched stayed lit with nothing coming
      // — the row claimed to be working on something that had already been abandoned.
      if (wasPending) {
        if (readingMode) {
          setReadState("ready");
        } else {
          // `#451 read-strip-followups`: out of the mode nothing else redraws the transport, whose
          // Read said Stop for the fetch just cancelled.
          renderReadTransport();
        }
        deps.eachColumn(deps.renderChannelRows);
      }
      return;
    }
    const { audio, urls, speechTimer, players } = nowPlaying;
    nowPlaying = null;
    // `#376 read-aloud-blocks`. The media keys and the steering strip belong to a read in progress,
    // and a jump to another block of it is still that read.
    if (!readJumping) releaseMediaSession();
    renderReadSteer();
    // `#434 read-aloud-pause`: with nothing playing, Pause/Resume give the Read button back.
    renderReadTransport();
    clearReadingSentence();
    if (speechTimer !== null && speechTimer !== undefined) clearTimeout(speechTimer);
    for (const player of players || [audio]) {
      try {
        player.pause();
        // A paused `<audio>` keeps downloading. Dropping its source closes a streamed response,
        // which is what tells the server to interrupt the agent instead of generating on for
        // nobody.
        if (typeof player.removeAttribute === "function" && typeof player.load === "function") {
          player.removeAttribute("src");
          player.load();
        }
      } catch (_error) {
        // A player that will not pause is not a reason to leave the page in a reading state.
      }
    }
    if (!readJumping) quietReadAloudContext();
    // The object URL holds the audio alive until it is revoked, and this mode fetches one per
    // message: not revoking is a leak that grows with the length of the backlog. ALL of them on a
    // combined row, including the parts that had not been reached — they were all fetched.
    for (const url of urls) {
      if (url && typeof URL !== "undefined" && URL.revokeObjectURL) {
        URL.revokeObjectURL(url);
      }
    }
    if (wasPending && readingMode) setReadState("ready");
    deps.eachColumn(deps.renderChannelRows);
  }

  /** Fetch the audio for one message. Bytes, not JSON, so it cannot go through `api`. */
  async function fetchSpeech(channel: string, id: string) {
    // Nothing at all when no pace was chosen, so the server borrows the agent's. Sending 100 would
    // silently OVERRIDE an agent configured to speak faster, which is the bug this avoids.
    const pace = readSpeed === null ? "" : `?speed=${(readSpeed / 100).toFixed(2)}`;
    const response = await deps.fetch(
      deps.withThreadQuery(`/api/v1/channels/${encodeURIComponent(channel)}/messages/${encodeURIComponent(id)}/speak${pace}`, deps.threadForMessageId(id)),
      { method: "POST", headers: { Authorization: `Bearer ${deps.token()}` } }
    );
    if (!response.ok) {
      // The FAILURE body is JSON even though the success body is audio, so the reason survives.
      let detail = `HTTP ${response.status}`;
      try {
        const payload = JSON.parse(await response.text());
        if (payload && payload.detail) {
          detail = payload.detail;
        }
      } catch (_error) {
        // A non-JSON failure body is not worth a second failure; the status still says something.
      }
      throw new Error(detail);
    }
    return response.blob();
  }

  /**
   * Report only clocks for a prepared read. The ticket already authorizes this one observation. Also one
   * of the two clocks a prepared URL's deadline is kept on (`speechDeadlineNow`): monotonic where the
   * browser has one.
   */
  function speechTimingNow() {
    return typeof performance !== "undefined" && typeof performance.now === "function"
      ? performance.now()
      : Date.now();
  }

  /**
   * Now on both clocks a prepared URL's deadline is kept on (`preparedSpeech`): the monotonic one,
   * which no clock change moves, and the wall clock, which goes on while a phone sleeps.
   * `#290 speech-ticket-refresh`.
   */
  function speechDeadlineNow() {
    return { mono: speechTimingNow(), wall: Date.now() };
  }

  /**
   * A fresh opaque id for one tap. The server speaks one read at a time on a shared session: a
   * request naming a NEW selection interrupts whatever is still being generated, while the parts of
   * one combined row share an id and play in order.
   */
  function speechSelection() {
    const bytes = new Uint8Array(8);
    if (typeof crypto !== "undefined" && typeof crypto.getRandomValues === "function") {
      crypto.getRandomValues(bytes);
    } else {
      for (let at = 0; at < bytes.length; at += 1) bytes[at]! = Math.floor(Math.random() * 256);
    }
    return `tap-${Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("")}`;
  }

  /**
   * `#548 cloud-read-failures`. Tell the server why the page stopped a read, under the read's ticket,
   * so the failure is in its log and diagnostics as well as on the phone. Never fails the page.
   */
  function reportSpeechFailure(url: string | null | undefined, reason: unknown, detail: unknown, block: number | null = null) {
    if (typeof url !== "string" || !url.startsWith("/api/v1/speech/")) return;
    deps.fetch(`${url}/failure${block === null ? "" : `?block=${block}`}`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ reason: String(reason).replace(/[^a-z0-9_-]/gu, "") || "failed", detail: String(detail).slice(0, 300) }),
      keepalive: true,
    }).catch(() => {});
  }

  /**
   * `#548 cloud-read-failures`. Why the server says this read's audio stream failed, when it kept a
   * reason (the voice failed the read after its first audio); null when it kept none, or cannot say.
   *
   * @param {string | undefined} url the read's prepared URL
   * @param {number | null} block the block read, or null for the whole message
   * @returns {Promise<string | null>}
   */
  async function speechStreamFailure(url: string | null | undefined, block: number | null) {
    if (typeof url !== "string" || !url.startsWith("/api/v1/speech/")) return null;
    try {
      const response = await deps.fetch(`${url}/failure${block === null ? "" : `?block=${block}`}`, { cache: "no-store" });
      if (!response.ok) return null;
      const answer = JSON.parse(await response.text());
      const detail = String((answer && answer.detail) || "").trim();
      return detail || null;
    } catch (_error) {
      return null;
    }
  }

  /** The message the last failed cloud read was of, for Read with device voice; null when none. */
  let deviceRetryParts: string[] | null = null;

  /**
   * `#548 cloud-read-failures`. The owner, 2026-10-26: with the voice's speech access expired, the
   * page opened the scrubber and "read" through the message in silence. A cloud read the voice fails —
   * silence, no speech in time, an error answer — STOPS: the scrubber closes, nothing is archived, the
   * reason is said in short words and stays up, and Read with device voice reads that message by the
   * phone's own voice. `report` tells the server (`reportSpeechFailure`).
   */
  function cloudReadFailed(reason: string, detail: unknown, parts: string[], report: ((reason: string, text: string) => void) | null = null) {
    stopReading();
    if (readingMode) setReadState("failed");
    // The server's own word for a turn of silence (`conversation_speech_silent`), or the voice's
    // (`speech_silent`), is the silence this page detects itself, and is said and reported as one.
    if (/speech_silent/u.test(String(detail))) reason = "silent";
    const text = cloudFailureText(reason, detail);
    deps.setStatus(text);
    deps.showError(text, { stay: true });
    deviceRetryParts = Array.isArray(parts) && parts.length > 0 ? parts.slice() : null;
    deps.el("read-on-device").hidden = deviceRetryParts === null || browserSpeechEngine() === null;
    if (report) report(reason, text);
  }

  /**
   * Read with device voice: the message whose cloud read failed, by the device voice, this once. The
   * voice selected stays the cloud's; the switch beside Read changes it for every read.
   */
  function readOnDevice() {
    const parts = deviceRetryParts;
    deviceRetryParts = null;
    deps.clearError();
    if (!parts) return;
    const held = { playback: readAloudPlayback, voiceName: readAloudLabel };
    readAloudPlayback = "browser";
    readAloudLabel = "Device voice";
    try {
      // Everything that reads the selection happens before `readAloud` first awaits.
      deps.guardQuietly(() => readAloud(parts))();
    } finally {
      readAloudPlayback = held.playback;
      readAloudLabel = held.voiceName;
    }
  }

  function reportSpeechPlayback(url: string | null | undefined, timing: Record<string, number>, block: number | null = null) {
    if (typeof url !== "string" || !url.startsWith("/api/v1/speech/")) return;
    // `#376 read-aloud-blocks`. A block is its own request, with its own server timing.
    deps.fetch(`${url}/timing${block === null ? "" : `?block=${block}`}`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(timing),
      keepalive: true,
    }).catch(() => {
      // Telemetry must never turn successful audio into a visible failure.
    });
  }

  /**
   * PLAYING A PREPARED READ AS IT ARRIVES, which an `<audio src>` will not do.
   *
   * A media element buffers a streamed WAV by BYTES before it reports anything: Chromium reads about
   * 225 KB of samples before `loadedmetadata`, whatever their rate. A read arrives at speaking pace —
   * 24 kHz, 16-bit, mono is 48 KB a second — so the reader sat through 4.7 seconds of silence after
   * the server already had audio. The other containers a browser streams wait about as long. So the
   * page fetches the prepared URL itself and schedules each piece of PCM on a Web Audio context as
   * it lands, which is how the voice conversation already plays its answers.
   *
   * It answers to exactly the part of `<audio>` that `readAloud` and `stopReading` use — `play`,
   * `pause`, and `loadeddata` / `playing` / `ended` / `error` — so every rule about tickets, parts,
   * stopping and archiving applies to it unchanged. Pausing ABORTS the fetch, because closing the
   * response is still what tells the server to interrupt the agent.
   */
  // A read arrives at speaking pace, so the audio banked before the first sample is the whole
  // cushion against a network stall. A tenth of a second made every hiccup on a highway audible; half
  // a second costs that much more before the first word and survives the common short dropout.
  const STREAM_START_LEAD_SECONDS = 0.5;
  /** An underrun doubles the lead, up to this, so a jittery network costs one gap rather than many. */
  const STREAM_MAX_LEAD_SECONDS = 4;
  /** A WAV header larger than this is not one this page will wait for. */
  const STREAM_HEADER_LIMIT_BYTES = 64 * 1024;
  /**
   * The first sample at least this loud (of full scale, about -34 dBFS) is where speech begins.
   *
   * `#191 read-aloud-first-lag`. A read's audio opens with silence: in every read measured, speech
   * began 0.55 to 0.8 s after the first frame. `playing` counts that silence as audible, so the
   * page also reports when the first sample this loud was due at the speaker. Quiet room tone sits
   * far below this level and the start of a spoken word far above it.
   */
  const SPEECH_ONSET_LEVEL = 0.02;
  /** How long a report waits, after `playing`, to learn when speech began before it goes anyway. */
  const SPEECH_ONSET_WAIT_MS = 5000;
  /**
   * `#548 cloud-read-failures`. A 10 ms window of a read's PCM whose RMS is above this (of full scale:
   * 500 of 32768) is speech. A voice service whose speech access is denied answers with about 0.85 s
   * of silent PCM and a completed turn, never an error, so loudness is the evidence, not bytes.
   */
  const SPEECH_RMS_LEVEL = 500 / 0x8000;

  let readAloudContext: AudioContext | null = null;
  let readAloudWantsOutput = false;

  /**
   * The context read-aloud plays through, resumed. Null where a fetch cannot be streamed into Web
   * Audio, and the caller then uses `<audio>`. Called from the tap itself as well as from `play`,
   * because a browser only lets a context start inside a gesture.
   */
  function readAloudAudioContext() {
    if (typeof ReadableStream === "undefined" || typeof AbortController !== "function") return null;
    const Context = deps.browserPorts().audio.contextClass();
    if (!Context) return null;
    // A browser may close the context under a long-held read (`#434 read-aloud-pause`): a closed
    // one never resumes, so the next read makes a fresh one rather than failing on it for good.
    if (readAloudContext !== null && readAloudContext.state === "closed") readAloudContext = null;
    if (readAloudContext === null) {
      try {
        readAloudContext = new Context();
      } catch (_error) {
        return null;
      }
    }
    // Not while the reader has paused: the clock stays stopped until they play. `#376`.
    const paused = nowPlaying !== null && nowPlaying!.paused === true;
    readAloudWantsOutput = !paused;
    if (readAloudContext.state === "suspended" && !paused && typeof readAloudContext.resume === "function") {
      readAloudContext.resume().catch(() => {});
    }
    return readAloudContext;
  }

  /** Nothing is being read: let the device's audio output go rather than hold it open in silence. */
  function quietReadAloudContext() {
    readAloudWantsOutput = false;
    if (readAloudContext !== null && readAloudContext.state === "running"
      && typeof readAloudContext.suspend === "function") {
      const context = readAloudContext;
      context.suspend().then(() => {
        // A new read may have asked to resume before this asynchronous suspension completed.
        // Honor the latest intent, including a subsequent Stop or Pause, when it settles.
        if (readAloudWantsOutput && readAloudContext === context && typeof context.resume === "function") {
          context.resume().catch(() => {});
        }
      }).catch(() => {});
    }
  }

  function streamingPlayer(url: string, context: AudioContext, clock = readClock()) {
    const listeners = new Map<string, ((...details: unknown[]) => void)[]>();
    const emit = (type: string, ...details: unknown[]) => {
      for (const fn of listeners.get(type) || []) fn(...details);
    };
    const abort = new AbortController();
    const waiting: AudioBuffer[] = []; // decoded before `play`: a later part, fetched while the one before plays
    const sounding = new Set<AudioBufferSourceNode>();
    let wanted = false;
    let finished = false;
    let over = false; // paused, failed or ended: nothing more is heard or announced
    let loaded = false;
    let announced = false;
    let spoke = false;
    // `#548 cloud-read-failures`. Whether any sample that has arrived, played or not, was speech.
    let voiced = false;
    let drained = false;
    // Where this part's audio begins and ends on the audio clock, as far as it has arrived.
    let firstAt: number | null = null;
    let lastEnd = 0;
    // Where on the audio clock this part fell silent between its first sample and its last, each
    // `[from, to]`: its underruns. `#472 gate-flake-hardening`.
    const gaps: [number, number][] = [];
    let lead = STREAM_START_LEAD_SECONDS;

    const silence = () => {
      over = true;
      abort.abort();
      for (const node of sounding) {
        try {
          node.stop();
        } catch (_error) {
          // Already finished.
        }
      }
      sounding.clear();
      waiting.length = 0;
    };
    // `#487 read-aloud-recovery`. Why, for the read to tell a part that was cut off or whose ticket is
    // gone ("lost": worth starting again) from one refused or malformed (which would only fail again).
    const fail = (why = "refused", detail = "") => {
      if (over) return;
      silence();
      emit("error", why, detail);
    };
    const endIfDone = () => {
      if (over || !finished || !wanted || waiting.length > 0 || sounding.size > 0) return;
      over = true;
      emit("ended");
    };
    // Every sample is scheduled: what plays next can be scheduled after it. Said once.
    const drainIfDone = () => {
      if (over || drained || !finished || !wanted || waiting.length > 0) return;
      drained = true;
      emit("drained");
    };
    const schedule = (buffer: AudioBuffer) => {
      const node = context.createBufferSource();
      node.buffer = buffer;
      node.connect(context.destination);
      const now = context.currentTime;
      if (!announced) {
        // A part's first piece: where the part before it ends, or the cushion ahead of now, whichever
        // is later. `#376 read-aloud-blocks`. Where the voice generates faster than it speaks, the
        // next block is waiting before this one ends and follows it without a gap; where it speaks
        // as it generates, each block's own first-audio wait is a pause at the boundary, and the
        // cushion against a network stall is never spent on it.
        clock.playAt = Math.max(clock.playAt, now + lead);
      } else if (clock.playAt < now + 0.005) {
        // An underrun: start again a longer lead ahead.
        lead = Math.min(STREAM_MAX_LEAD_SECONDS, lead * 2);
        const ranOut = clock.playAt;
        clock.playAt = now + lead;
        if (firstAt !== null) gaps.push([Math.max(ranOut, firstAt), clock.playAt]);
      }
      const playAt = clock.playAt;
      node.start(playAt);
      const startsInMs = Math.round((playAt - now + (context.outputLatency || 0)) * 1000);
      if (!spoke) {
        const onset = loudFrom(buffer);
        if (onset >= 0) {
          spoke = true;
          // In how many milliseconds the first loud sample reaches the speaker. Said NOW, as a
          // number, rather than on a timer: the reader is owed this figure even if they stop first.
          emit("speech", startsInMs + Math.round((onset / buffer.sampleRate) * 1000));
        }
      }
      clock.playAt = playAt + buffer.duration;
      if (firstAt === null) firstAt = playAt;
      lastEnd = clock.playAt;
      sounding.add(node);
      node.onended = () => {
        sounding.delete(node);
        endIfDone();
      };
      if (!announced) {
        announced = true;
        // `playing` is when the first sample is due at the speaker, not when it was queued: on the
        // audio clock, so a pause in between holds it back as long as it holds the sample back.
        announceAt(playAt, startsInMs);
      }
    };
    /**
     * Say `playing` in `inMs`, or, when the reader paused the audio clock in between, when the clock
     * reaches `due`. `#376 read-aloud-blocks`: the highlight follows the voice, not the wall clock.
     */
    const announceAt = (due: number, inMs: number) => {
      let held = false;
      const check = () => {
        if (over) return;
        if (context.state === "suspended") {
          held = true;
          setTimeout(check, 250);
          return;
        }
        const left = held ? Math.round((due - context.currentTime) * 1000) : 0;
        held = false;
        if (left > 20) {
          setTimeout(check, left);
          return;
        }
        emit("playing");
      };
      setTimeout(check, inMs);
    };
    /** The first frame of `buffer` loud enough to be speech, in any channel; -1 when none is. */
    const loudFrom = (buffer: AudioBuffer) => {
      let first = -1;
      for (let channel = 0; channel < buffer.numberOfChannels; channel += 1) {
        const samples = buffer.getChannelData(channel);
        const end = first < 0 ? samples.length : first;
        for (let frame = 0; frame < end; frame += 1) {
          if (Math.abs(samples[frame]!) >= SPEECH_ONSET_LEVEL) {
            first = frame;
            break;
          }
        }
      }
      return first;
    };
    /** Whether any 10 ms window of `buffer`, in any channel, is loud enough to be speech. */
    const voicedIn = (buffer: AudioBuffer) => {
      const window = Math.max(1, Math.round(buffer.sampleRate / 100));
      for (let channel = 0; channel < buffer.numberOfChannels; channel += 1) {
        const samples = buffer.getChannelData(channel);
        for (let start = 0; start < samples.length; start += window) {
          const end = Math.min(samples.length, start + window);
          let energy = 0;
          for (let frame = start; frame < end; frame += 1) energy += samples[frame]! * samples[frame]!;
          if (Math.sqrt(energy / (end - start)) > SPEECH_RMS_LEVEL) return true;
        }
      }
      return false;
    };
    const decode = (bytes: Uint8Array, channels: number, rate: number) => {
      const frames = Math.floor(bytes.length / (2 * channels));
      const buffer = context.createBuffer(channels, frames, rate);
      const view = new DataView(bytes.buffer, bytes.byteOffset, frames * 2 * channels);
      for (let channel = 0; channel < channels; channel += 1) {
        const samples = buffer.getChannelData(channel);
        for (let frame = 0; frame < frames; frame += 1) {
          samples[frame]! = view.getInt16((frame * channels + channel) * 2, true) / 0x8000;
        }
      }
      return buffer;
    };
    const run = async () => {
      let response;
      try {
        response = await deps.fetch(url, { signal: abort.signal, cache: "no-store" });
      } catch (_error) {
        fail("lost");
        return;
      }
      if (over) return;
      if (!response.ok || !response.body || typeof response.body.getReader !== "function") {
        // A ticket the server does not know: it ended, or the server restarted and forgot it.
        if (response.status === 404 || response.status === 410) {
          fail("lost");
          return;
        }
        // `#548 cloud-read-failures`. Refused: the server's reason goes with it, for the reader.
        let detail = "";
        try {
          const answer = await response.json();
          detail = String((answer && (answer.detail || answer.error)) || "");
        } catch (_error) {
          // No reason to be had: the status alone says it was refused.
        }
        if (over) return;
        fail("refused", detail);
        return;
      }
      // `#563 read-aloud-fallback-chain`: a chain names the voice that is
      // speaking this block; tell the reader when it is not the usual one.
      const spokenBy = response.headers && typeof response.headers.get === "function"
        ? response.headers.get("x-read-aloud-voice") : null;
      if (spokenBy && (pendingRead !== null || nowPlaying !== null)) {
        deps.setStatus(`Reading with ${spokenBy}.`);
      }
      const reader = response.body.getReader();
      let head: Uint8Array = /** @type {Uint8Array} */ (new Uint8Array(0));
      let layout = null;
      let carry: Uint8Array = /** @type {Uint8Array} */ (new Uint8Array(0));
      try {
        for (;;) {
          const { value, done } = await reader.read();
          if (over) return;
          if (done) break;
          let bytes: Uint8Array = /** @type {Uint8Array} */ (value);
          if (layout === null) {
            head = joinBytes(head, bytes);
            layout = wavLayout(head);
            if (layout === null) {
              if (head.length > STREAM_HEADER_LIMIT_BYTES) throw new Error("no samples in the audio");
              continue;
            }
            bytes = head.subarray(layout.dataAt);
          }
          // A chunk boundary can split a sample; the remainder waits for the next chunk.
          bytes = joinBytes(carry, bytes);
          const whole = bytes.length - (bytes.length % (2 * layout.channels));
          carry = bytes.slice(whole);
          if (whole === 0) continue;
          const buffer = decode(bytes.subarray(0, whole), layout.channels, layout.rate);
          if (!voiced) voiced = voicedIn(buffer);
          if (!loaded) {
            loaded = true;
            emit("loadeddata");
            if (over) return;
          }
          if (wanted) schedule(buffer);
          else waiting.push(buffer);
        }
      } catch (error) {
        // Includes a response cut off part-way: a read that did not arrive whole did not END, and
        // must not be archived as heard. Cut off is "lost"; audio this page cannot read is not.
        fail(error && (error as { name?: unknown }).name === "TypeError" ? "lost" : "malformed");
        return;
      }
      // A body that stops inside a frame was cut short, or disagrees with its own format.
      if (!loaded || carry.length > 0) {
        fail("malformed");
        return;
      }
      finished = true;
      drainIfDone();
      endIfDone();
    };
    run();
    return {
      url,
      /** This player can tell when speech begins, and says so with a `speech` event. */
      hearsSpeech: true,
      /**
       * Whether any audio that has arrived was speech: a 10 ms window above `SPEECH_RMS_LEVEL`
       * (`#548 cloud-read-failures`; `SPEECH_ONSET_LEVEL` still times where speech begins).
       */
      heardSpeech() {
        return voiced;
      },
      /**
       * `#379 read-aloud-sentences`. Where this part plays on the audio clock: when its first sample
       * is due (null before any is), when what has arrived ends, whether that is all of it, and the
       * silences an underrun left in between (`silentBefore`).
       */
      timeline() {
        return { startsAt: firstAt, endsAt: lastEnd, finished: finished && waiting.length === 0, gaps: gaps.slice() };
      },
      addEventListener(type: string, fn: (...details: unknown[]) => void) {
        if (!listeners.has(type)) listeners.set(type, []);
        listeners.get(type)!.push(fn);
      },
      play() {
        if (!over) {
          wanted = true;
          readAloudAudioContext();
          while (waiting.length > 0) schedule(waiting.shift()!);
          drainIfDone();
          endIfDone();
        }
        return Promise.resolve();
      },
      pause() {
        if (!over) silence();
      },
    };
  }

  /**
   * A player for one streamed part: Web Audio as it arrives where the browser can, else `<audio>`.
   * The parts of one read share `clock`, so each begins where the one before it ends.
   */
  function streamedSpeechPlayer(url: string, clock: { playAt: number }): TransportPlayer {
    const context = readAloudAudioContext();
    return context === null ? deps.browserPorts().audio.element(url) : streamingPlayer(url, context, clock);
  }

  // --- reading a message block by block: `#376 read-aloud-blocks` ---------------------------------
  //
  // The owner listens to long messages in the car. A message of paragraphs and bullets used to be read
  // as one stretch, with no way to see or change where in it the voice was. Now each message is read
  // in the BLOCKS it is drawn as — a paragraph, a heading, a list item, a block quote; a code block or a
  // table as a short description — one request per block, so the block being read can be lit, counted
  // ("2 of 5") and moved between: a tap on another block of the message being read, Previous and Next,
  // and the media keys a steering wheel or a headset sends.
  //
  // THE SERVER DECIDES WHAT EACH BLOCK SAYS, from the message itself, and lists the blocks' kinds in
  // drawing order (`source_blocks` in src/render.rs). The page finds the same blocks in the body it
  // drew by the same rule (`readingBlocks`) and lights block k. When the two lists of kinds differ the
  // read still goes block by block, with the row lit and counted, and no block lit.
  //
  // NO GAP AT A BOUNDARY: the next block is asked for as soon as the current one has audio, so on a
  // provider with one session it is generated the moment the current one is; and it is scheduled on
  // the same audio clock, to begin where the current one ends (`readClock`).

  // `speechSegments`, what one read plays in order, is in web/src/core/speech/ (`#624
  // voice-page-modularity`).

  /** The element each block kind is drawn as: src/render.rs `html_block_kinds`, the same rule. */
  const READING_BLOCK_KINDS = new Map([
    ["p", "paragraph"], ["h1", "heading"], ["h2", "heading"], ["h3", "heading"], ["h4", "heading"],
    ["h5", "heading"], ["h6", "heading"], ["blockquote", "quote"], ["pre", "code"], ["table", "table"],
  ]);

  /**
   * The blocks one drawn body holds, in order, as `{element, kind}`. A `p`, a heading, a block quote,
   * a `pre` and a table are each one block with nothing inside them another; a list item is one
   * block, and the items of the lists nested in it are blocks of their own; lists are walked through.
   * The server walks its rendering by exactly this rule (`html_block_kinds`).
   */
  function readingBlocks(box: Element) {
    const found: { element: Element; kind: string }[] = [];
    const walk = (node: Element, inside: string) => {
      for (const kid of node.children || []) {
        const tag = String((kid as Element).tagName || "").toLowerCase();
        if (tag === "ul" || tag === "ol" || (inside === "blocks" && tag === "div")) {
          walk(kid, "blocks");
          continue;
        }
        if (inside !== "blocks") continue;
        if (tag === "li") {
          found.push({ element: kid, kind: "item" });
          walk(kid, "lists");
          continue;
        }
        const kind = READING_BLOCK_KINDS.get(tag);
        if (kind) found.push({ element: kid, kind });
      }
    };
    walk(box, "blocks");
    return found;
  }

  /** The attribute on the block being read. An attribute, so nothing about the body's classes moves. */
  const READING_BLOCK_ATTRIBUTE = "data-reading-block";

  /**
   * The drawn blocks of message `part` of `row`, one per block the server prepared it as, when they
   * are the blocks it was prepared in; null when it was not prepared in blocks or the two disagree.
   *
   * `#486 read-aloud-pieces`. A prepared block is a PIECE of a drawn one (`element`, `piece` of
   * `pieces`); a server from before pieces lists one per drawn block, each its own element. Each entry
   * here is `{element, kind, piece, pieces, alone}`: the drawn element it is lit as, which of its pieces,
   * and whether it is the message's one block (`read-chunk-balance`).
   */
  function preparedBlocksOf(row: Element, part: number) {
    const body = childByClass(row, "body");
    const box = body && body.children ? body.children[part]! : null;
    const id = idsOf(row)[part]!;
    // The read in progress keeps the blocks it was prepared in; any other row asks what is prepared.
    const playing = nowPlaying !== null && nowPlaying!.blocks! && nowPlaying!.id === row.getAttribute("data-id");
    const entry = id ? preparedSpeech.get(String(id)) : null;
    const listed = playing ? nowPlaying!.blocks![part]! : entry && entry.blocks;
    if (!box || !listed) return null;
    const drawn = readingBlocks(box);
    const elementOf = (block: PreparedBlock, index: number) => (Number.isInteger(block.element) ? block.element as number : index);
    // Every drawn block in order, each listed at least once and only with its own kind.
    let next = 0;
    for (const [index, block] of listed.entries()) {
      const at = elementOf(block, index);
      if (at !== next && at !== next - 1) return null;
      if (!drawn[at] || drawn[at]!.kind !== block.kind) return null;
      next = at + 1;
    }
    if (next !== drawn.length) return null;
    return listed.map((block, index) => ({
      element: drawn[elementOf(block, index)]!.element,
      kind: block.kind,
      piece: Number.isInteger(block.piece) ? block.piece as number : 0,
      pieces: Number.isInteger(block.pieces) ? block.pieces as number : 1,
      // `read-chunk-balance`: the message is one block, which the server cut finer (`readPieces`).
      alone: drawn.length === 1,
    }));
  }

  /** Light the block being read in `row`, and only that one. The row is marked while one is lit. */
  function markReadingBlock(row: Element) {
    if (row.getAttribute("data-reading-marked") !== null) {
      const clear = (node: Element) => {
        for (const kid of node.children || []) {
          if (kid.getAttribute && kid.getAttribute(READING_BLOCK_ATTRIBUTE) !== null) {
            kid.removeAttribute(READING_BLOCK_ATTRIBUTE);
          }
          clear(kid);
        }
      };
      clear(row);
      row.removeAttribute("data-reading-marked");
    }
    const playing = nowPlaying;
    if (playing === null || !playing.segments || playing.id !== row.getAttribute("data-id")) return;
    const segment = playing.segments![playing.segment!]!;
    if (!segment || segment.block === null) return;
    const drawn = preparedBlocksOf(row, segment.part);
    const block = drawn ? drawn[segment.block!]! : null;
    if (!block) return;
    block.element.setAttribute(READING_BLOCK_ATTRIBUTE, "current");
    row.setAttribute("data-reading-marked", "");
  }

  // --- the sentence being read: `#379 read-aloud-sentences` -----------------------------------------
  //
  // Inside the block being read, the sentence being said is lit too. NOT by another request per
  // sentence: on a voice that speaks as it generates, every request boundary is a pause as long as
  // the voice's first audio (`#376`), and a pause at every full stop would be worse than none. The
  // block's audio is one stream, so the sentence is found by its share of the block's characters
  // against how far through the block's audio the reader is: exact at the block's edges, and drifting
  // only as far as one block's sentences are not all said at the same speed. How long the block is
  // comes from its audio once all of it has arrived; until then, from how many characters it says at
  // the speaking rate the voice was last measured at.
  //
  // Lit with the CSS Custom Highlight API, which marks a range of text without touching the body the
  // server rendered. A browser without it lights the block alone, as before.

  /** The highlight's name; web/voice.css styles `::highlight(read-aloud-sentence)`. */
  const SENTENCE_HIGHLIGHT = "read-aloud-sentence";
  /** Seconds of speech per character said, until a block of this voice has been measured (`#47`). */
  const READ_SECONDS_PER_CHAR = 0.068;
  /** How often the sentence is looked for again while a block plays. */
  const SENTENCE_STEP_MS = 150;
  /** The speaking rate the last block that arrived whole was said at, seconds per character. */
  let learnedSecondsPerChar: number | null = null;
  let sentenceTimer: ReturnType<typeof setTimeout> | null = null;
  /** The sentence lit now, as "row part block index", so an unchanged one is not set again. */
  let litSentence = "";
  /** The first text node of each row's sentences when it was lit (`showReadingSentence`). */
  let litSentenceNodes: Node[] = [];

  function sentenceHighlightSupported() {
    return typeof CSS !== "undefined" && CSS !== null && Boolean(CSS.highlights) &&
      typeof Highlight === "function" && typeof document.createRange === "function";
  }

  /**
   * The block of `segment` arrived whole: learn how fast this voice says a character. Only from a
   * block long enough to say something about it.
   */
  function learnSpeakingRate(player: ReadPlayer, segment: SpeechSegment, blocks: (readonly PreparedBlock[] | null)[]) {
    if (!player || typeof player.timeline !== "function" || !segment || segment.block === null) return;
    const listed = blocks[segment.part] && blocks[segment.part]![segment.block!]!;
    const chars = listed && Number(listed.chars);
    const timeline = player.timeline();
    if (!chars || chars < 40 || !timeline.finished || timeline.startsAt === null) return;
    learnedSecondsPerChar = (timeline.endsAt - timeline.startsAt - silentBefore(timeline, timeline.endsAt)) / chars;
  }

  /**
   * The text `element` shows, as `{text, place}`: its text nodes in order, a line break as a line end,
   * and `place(offset)`, where an offset into that text is as `[text node, offset]`. The text of a
   * nested list is not the element's own.
   */
  function shownTextOf(element: Element) {
    const nodes: { node: Node; at: number }[] = [];
    let text = "";
    const walk = (node: Node) => {
      for (const kid of node.childNodes || []) {
        if (kid.nodeType === 3) {
          nodes.push({ node: kid, at: text.length });
          text += kid.textContent;
          continue;
        }
        const tag = String((kid as Element).tagName || "").toLowerCase();
        if (tag === "ul" || tag === "ol") continue;
        // `#486 read-aloud-pieces`: a typed bullet begins a line, and a drawn line break ends one.
        if (tag === "br") {
          text += "\n";
          continue;
        }
        walk(kid);
      }
    };
    walk(element);
    const place = (offset: number): Position => {
      let piece = nodes[0]!;
      for (const candidate of nodes) {
        if (candidate.at > offset) break;
        piece = candidate;
      }
      return [piece!.node, Math.max(0, Math.min(offset - piece!.at, piece!.node.textContent!.length))];
    };
    return { text, place };
  }

  // `sentenceEnds` (src/render.rs `sentence_ranges`) is in web/src/core/speech/ (`#624
  // voice-page-modularity`).

  /**
   * The sentences `element` shows, in order, each as `{from, to, length}`: where it begins and ends as
   * `[text node, offset]`, and how many characters it is. `#486 read-aloud-pieces`: with `within`, only
   * the sentences of that stretch of its text (`shownTextOf`), as `{start, end}`, cut at its edges.
   *
   * @param {Element} element
   * @param {{start: number, end: number} | null} [within]
   */
  function sentencesOf(element: Element, within: { start: number; end: number } | null = null) {
    const { text, place } = shownTextOf(element);
    if (text === "") return [];
    const found: Sentence[] = [];
    let start = within ? within.start : 0;
    for (const end of sentenceEnds(text, start, within ? within.end : text.length)) {
      while (start < end && /\s/.test(text[start]!)) start += 1;
      let last = end;
      while (last > start && /\s/.test(text[last - 1]!)) last -= 1;
      if (last > start) found.push({ from: place(start), to: place(last), length: last - start });
      start = end;
    }
    return found;
  }

  // --- reading a block in pieces: `#486 read-aloud-pieces` ------------------------------------------
  //
  // The owner, 2026-10-23, of a message whose bullets were typed "•" lines in one paragraph: why was it
  // not cut into pieces the controller can step through, with a most size in words, cut at paragraphs
  // and list items first, at sentences next, and evenly by words only as a last resort? So a drawn
  // block is read in pieces, by the rule src/render.rs `read_pieces` cuts the text it was drawn from
  // with (both held to tests/fixtures/read_aloud_pieces.json): a line beginning with a bullet glyph is
  // a piece of its own and the lines between are one; a piece of more than `READ_PIECE_WORDS` words is
  // cut at sentence ends; only one sentence longer than that is cut into equal runs of words. The block
  // stays lit while its pieces are read, and the sentence light keeps inside the piece being read.
  //
  // `read-chunk-balance`: the sentences of a paragraph are cut into BALANCED pieces, against the
  // balance metric (`pieceSkewPasses`), and a message that is one paragraph is cut finer. See
  // src/render.rs, whose section note says why; `balanceSentences` is its `balance_sentences`.

  // The piece math (`READ_PIECE_WORDS`, `readPieces`, `balanceSentences`, `pieceSkewPasses`,
  // `evenRuns`) is a module now: web/src/core/speech/ (`#624 voice-page-modularity`).

  /**
   * Piece `block.piece` of the drawn block `block` as `{start, end}` in its shown text, when the page
   * cuts that block into as many pieces as the server read it in; else null.
   *
   * @param {{element: Element, piece: number, pieces: number, alone?: boolean}} block
   */
  function readPieceOf(block: DrawnBlock) {
    if (block.pieces < 2) return null;
    const pieces = readPieces(shownTextOf(block.element).text, block.alone === true);
    return pieces.length === block.pieces ? pieces[block.piece] || null : null;
  }

  /** Take the sentence light off, and stop looking for the next one. */
  function clearReadingSentence() {
    if (sentenceTimer !== null) clearTimeout(sentenceTimer);
    sentenceTimer = null;
    litSentence = "";
    if (sentenceHighlightSupported()) CSS.highlights.delete(SENTENCE_HIGHLIGHT);
  }

  /** Light the sentence being said in the block being read, and look again shortly. */
  function showReadingSentence() {
    if (!sentenceHighlightSupported()) return;
    if (sentenceTimer !== null) clearTimeout(sentenceTimer);
    sentenceTimer = null;
    const playing = nowPlaying;
    if (playing === null || !playing.segments || playing.segments!.length < 2) {
      clearReadingSentence();
      return;
    }
    sentenceTimer = setTimeout(showReadingSentence, SENTENCE_STEP_MS);
    const segment = playing.segments![playing.segment!]!;
    const player = playing.players![playing.segment!]!;
    // `#540 read-aloud-desktop`. In every row drawn for the read — the thread list's, the right column's —
    // not only the left column's list: the same sentence of the same block in each.
    // The rows the reader can see first: the sentence is timed from the first, and a row drawn from
    // part of the message (a split card's rest) may cut it into other sentences.
    const lit = segment && segment.block !== null ? readingRowsOf(playing.id)
      .sort((one, other) => Number(rowSeen(other)) - Number(rowSeen(one))).map((row) => {
      const drawn = preparedBlocksOf(row, segment.part);
      const block = drawn ? drawn[segment.block!]! : null;
      // `#486 read-aloud-pieces`. A piece of the block lights its own sentences, or itself whole when it
      // is one; where the page cuts the block otherwise, every sentence of the block.
      const piece = block ? readPieceOf(block) : null;
      return block ? { piece, sentences: sentencesOf(block.element, piece) } : null;
    }).filter((one) => one !== null && one!.sentences.length > 0) : [];
    const timeline = player && typeof player.timeline === "function" ? player.timeline() : null;
    const piece = lit.length > 0 ? lit[0]!.piece : null;
    const sentences = lit.length > 0 ? lit[0]!.sentences : [];
    if ((sentences.length < 2 && !piece) || sentences.length < 1 || !timeline || timeline.startsAt === null ||
        readAloudContext === null) {
      if (litSentence !== "") CSS.highlights.delete(SENTENCE_HIGHLIGHT);
      litSentence = "";
      return;
    }
    const listed = playing.blocks![segment.part] && playing.blocks![segment.part]![segment.block!]!;
    const chars = Number(listed && listed.chars) || sentences.reduce((sum, one) => sum + one.length, 0);
    const pace = readSpeed === null ? 1 : readSpeed / 100;
    const perChar = learnedSecondsPerChar === null ? READ_SECONDS_PER_CHAR / pace : learnedSecondsPerChar;
    const arrived = timeline.endsAt - timeline.startsAt - silentBefore(timeline, timeline.endsAt);
    const length = timeline.finished ? arrived : Math.max(arrived, chars * perChar);
    // Nothing past the end of what has arrived has been said: the clock running on into a stall is
    // silence before its gap is known, which only the next piece's arrival records.
    const heard = Math.min(readAloudContext.currentTime, timeline.endsAt);
    const said = heard - timeline.startsAt - silentBefore(timeline, heard);
    const through = Math.min(1, Math.max(0, said / length));
    const total = sentences.reduce((sum, one) => sum + one.length, 0);
    let reached = 0;
    let index = sentences.length - 1;
    for (let at = 0; at < sentences.length; at += 1) {
      reached += sentences[at]!.length;
      if (reached > through * total) {
        index = at;
        break;
      }
    }
    // The rows lit, by their first sentence's text node: a row the poll drew again is lit again.
    const key = `${playing.id} ${segment.part} ${segment.block} ${index}`;
    if (key === litSentence && lit.every((one) => litSentenceNodes.includes(one!.sentences[0]!.from[0]!))) return;
    const ranges = lit.filter((one) => one!.sentences.length === sentences.length).map((one) => {
      const range = document.createRange();
      const { from, to } = one!.sentences[index]!;
      range.setStart(from[0]!, from[1]!);
      range.setEnd(to[0]!, to[1]!);
      return range;
    });
    CSS.highlights.set(SENTENCE_HIGHLIGHT, new Highlight(...ranges));
    litSentence = key;
    litSentenceNodes = lit.map((one) => one!.sentences[0]!.from[0]!);
  }

  /** "2 of 5" for a read of several parts; "" for a read of one, or none. */
  function readingPosition() {
    if (nowPlaying === null || !nowPlaying!.segments || nowPlaying!.segments!.length < 2) return "";
    return `${nowPlaying!.segment! + 1} of ${nowPlaying!.segments!.length}`;
  }

  /** What the author line of the row being read says: "reading", or "reading 2 of 5". */
  function readingLabel() {
    const position = readingPosition();
    return position ? `reading ${position}` : "reading";
  }

  /**
   * Show where in its message the read is: the block lit, "reading 2 of 5" where the author was, and
   * the steering strip. Cheap, so it runs at every boundary rather than redrawing the lists.
   */
  function showReadingPosition() {
    // `#540 read-aloud-desktop`: in every list a row of the read is drawn in, the cards' list included.
    for (const row of READABLE_LISTS.flatMap(listRows)) {
      const reading = nowPlaying !== null && row.getAttribute("data-id") === nowPlaying!.id;
      if (!reading && row.getAttribute("data-reading-marked") === null) continue;
      markReadingBlock(row);
      const named = childByClass(row, "msg-author");
      if (named && reading && row.getAttribute("data-reading") === "true") named.textContent = readingLabel();
    }
    renderReadSteer();
  }

  /**
   * The steering strip, while a read of several parts is playing (`#537`: no Pause): since `#627
   * scrubber-thumb-arrows` the fat Previous and Next section, "k of n", the thin Previous and Next block.
   */
  function renderReadSteer() {
    const strip = deps.el("read-steer");
    if (!strip) return;
    const position = readingPosition();
    const wasHidden = strip.hidden;
    // `scrubber-full-width`. A jump that waits for its block has nothing playing, and the strip
    // stays up through the wait as it stood, as the bar keeps Pause and Stop (`#442`): on a phone it
    // is a row of the layout, and going and coming back would resize the list and put the bar's
    // second row and the chips back for the moment of every Next. So too through the wait before a
    // read cut off is tried again (`recoverRead`). `renderReadLayout` takes it away if either ends
    // without a block playing. (On a desk too: there it no longer blinks out on a Next.)
    const held = position === "" && !wasHidden && (readJumpPending || readRecovering);
    strip.hidden = position === "" && !held;
    renderReadTransport();
    if (!held) renderReadTimeline();
    if (position === "") return;
    deps.el("read-steer-count").textContent = position;
    deps.el("read-next").disabled = nowPlaying!.segment! + 1 >= nowPlaying!.segments!.length;
    const at = nowPlaying!.segment!;
    deps.el("read-section-next").disabled = !readSectionStarts(nowPlaying! as Playing & { segments: SpeechSegment[] }).some((start) => start > at);
    if (wasHidden) placeReadSteer();
  }

  /**
   * `#442 read-strip-order`. ON A DESK THE STRIP CAN BE MOVED, by the grip at its left: the owner
   * asked to drag "that little floating controller" off whatever it covers. Kept as an offset from
   * where it docks (on the chips row), held inside the window, and remembered on this device. The
   * stylesheet draws the grip, and applies the offset, only in the desk's regime; a phone keeps the
   * strip docked under the thumb. Whether the grip is drawn is the one question asked here, so the
   * layout stays the stylesheet's to decide (`#55 voice-desktop-app`).
   */
  /** @type {{ x: number, y: number } | null} */
  let readSteerOffset: Point | null = null;
  function initializeReadSteerOffset() { readSteerOffset = storedReadSteerOffset(); }
  /** @type {{ x: number, y: number, from: { x: number, y: number } } | null} */
  let readSteerDrag: (Point & { from: Point }) | null = null;

  function storedReadSteerOffset() {
    // A browser refusing storage, or a damaged record, reads as docked (`DEVICE_SETTINGS.readSteerOffset.refused`).
    return deps.deviceSettings().get(DEVICE_SETTINGS.readSteerOffset);
  }

  function rememberReadSteerOffset() {
    try {
      deps.deviceSettings().keep(DEVICE_SETTINGS.readSteerOffset, readSteerOffset);
    } catch (_error) {
      // The strip still stays where it was put on this page when storage is unavailable.
    }
  }

  /** The strip can be moved where the stylesheet draws its grip: shown, on a desk. */
  function readSteerMovable() {
    const grip = deps.el("read-steer-grip");
    return !deps.el("read-steer").hidden && typeof grip.getClientRects === "function" && grip.getClientRects().length > 0;
  }

  /**
   * Apply the reader's offset, held so the whole strip stays inside the window, and return what was
   * applied. The docked place is measured, not assumed: the box as drawn less the offset in force.
   */
  function placeReadSteer() {
    const strip = deps.el("read-steer");
    if (readSteerOffset === null || !readSteerMovable()) {
      strip.style.removeProperty("--read-steer-dx");
      strip.style.removeProperty("--read-steer-dy");
      strip.removeAttribute("data-moved");
      return { x: 0, y: 0 };
    }
    const box = strip.getBoundingClientRect();
    const left = box.left - (parseFloat(strip.style.getPropertyValue("--read-steer-dx")) || 0);
    const top = box.top - (parseFloat(strip.style.getPropertyValue("--read-steer-dy")) || 0);
    const root = document.documentElement;
    const hold = (value: number, low: number, high: number) => Math.round(Math.min(Math.max(value, low), Math.max(low, high)));
    const x = hold(readSteerOffset.x, -left, root.clientWidth - box.width - left);
    const y = hold(readSteerOffset.y, -top, root.clientHeight - box.height - top);
    // The held place is the place from here: a drag or a grip key after a layout growth starts from
    // where the strip stands, not from an offset the window no longer allows (`#451`). Storage still
    // changes only where a move ends (`moveReadSteer`), as for a window resize.
    readSteerOffset = { x, y };
    strip.style.setProperty("--read-steer-dx", `${x}px`);
    strip.style.setProperty("--read-steer-dy", `${y}px`);
    strip.setAttribute("data-moved", "true");
    return { x, y };
  }

  /** Move the strip to an offset from its docked place (`null`: dock it) and remember it. */
  function moveReadSteer(offset: Point | null) {
    // A strip gone (the read ended under the drag) or not movable here keeps the place remembered.
    if (!readSteerMovable()) return;
    readSteerOffset = offset;
    if (offset !== null) readSteerOffset = placeReadSteer();
    else placeReadSteer();
    rememberReadSteerOffset();
  }

  function onReadGripDown(event: PointerEvent) {
    if (!event || (event.button !== undefined && event.button !== 0) || !readSteerMovable()) return;
    readSteerDrag = { x: event.clientX, y: event.clientY, from: readSteerOffset || { x: 0, y: 0 } };
    const grip = deps.el("read-steer-grip");
    if (event.pointerId !== undefined && grip.setPointerCapture) grip.setPointerCapture(event.pointerId);
    if (event.preventDefault) event.preventDefault(); // no text selection while it is dragged
  }

  function onReadGripMove(event: PointerEvent) {
    if (!readSteerDrag || !event) return;
    // A release this grip never heard of (outside the window, say) ends the drag here instead.
    if (event.buttons === 0) {
      onReadGripUp();
      return;
    }
    readSteerOffset = {
      x: readSteerDrag.from.x + event.clientX - readSteerDrag.x,
      y: readSteerDrag.from.y + event.clientY - readSteerDrag.y,
    };
    placeReadSteer();
  }

  function onReadGripUp() {
    if (!readSteerDrag) return;
    readSteerDrag = null;
    if (!readSteerMovable()) {
      readSteerOffset = storedReadSteerOffset();
      return;
    }
    // Stored where the drag ended, as the reading-width grip stores its width: once, not per frame.
    moveReadSteer(readSteerOffset);
  }

  // The grip is a control a keyboard can use too: the arrows move it, Shift by more, Home docks it.
  function onReadGripKey(event: KeyboardEvent) {
    // Ctrl, Alt and Meta chords are the page's and the browser's (Ctrl+Home, Alt+Left), not the grip's.
    if (!event || event.ctrlKey || event.altKey || event.metaKey || !readSteerMovable()) return;
    const step = event.shiftKey ? 48 : 12;
    const at = readSteerOffset || { x: 0, y: 0 };
    const moves: Record<string, () => Point | null> = {
      ArrowLeft: () => ({ x: at.x - step, y: at.y }),
      ArrowRight: () => ({ x: at.x + step, y: at.y }),
      ArrowUp: () => ({ x: at.x, y: at.y - step }),
      ArrowDown: () => ({ x: at.x, y: at.y + step }),
      Home: () => null,
    };
    const next = moves[event.key]!;
    if (!next) return;
    if (event.preventDefault) event.preventDefault(); // the arrows would otherwise scroll the list
    moveReadSteer(next());
  }

  /**
   * `#380 read-aloud-timeline`. The read as a line of its blocks: one button a block, as wide as what
   * it says (`chars`, else all alike), the ones heard tinted, the one being read filled. A tap on one
   * reads from there, as a tap on the block itself does. Built once a read, then only marked.
   */
  let readTimelineOf: SpeechSegment[] | null = null;

  function renderReadTimeline() {
    const line = deps.el("read-timeline");
    if (!line) return;
    const playing = nowPlaying;
    if (playing === null || !playing.segments || playing.segments!.length < 2) {
      if (readTimelineOf !== null) line.replaceChildren();
      readTimelineOf = null;
      return;
    }
    if (readTimelineOf !== playing.segments!) {
      readTimelineOf = playing.segments!;
      const listedOf = (segment: SpeechSegment) => (segment.block === null ? null
        : playing.blocks && playing.blocks![segment.part]! && playing.blocks![segment.part]![segment.block!]!);
      const weights = readTimelineWeights(playing.segments!.map((segment) => {
        const listed = listedOf(segment);
        return Number(listed && listed.chars) || 1;
      }));
      line.replaceChildren(...playing.segments!.map((segment, index) => {
        const button = document.createElement("button");
        button.type = "button";
        button.className = "read-timeline-block";
        const listed = listedOf(segment);
        button.setAttribute("style", `--block-weight: ${weights[index]!}`);
        // `read-chunk-balance`. A piece after the first of its drawn block stands closer to the one before:
        // the line reads as the message's blocks, each of its pieces — a first step to the
        // documents-view navigator's levels.
        if (listed && Number.isInteger(listed.piece) && (listed.piece as number) > 0) button.setAttribute("data-continues", "");
        button.setAttribute("aria-label", `Read from block ${index + 1} of ${playing.segments!.length}`);
        button.addEventListener("click", () => { jumpReading(index); });
        return button;
      }));
    }
    [...line.children].forEach((button, index) => {
      button.setAttribute("aria-current", index === playing.segment! ? "true" : "false");
      button.toggleAttribute("data-heard", index < playing.segment!);
    });
  }

  /**
   * Move the read in progress to part `index` of it. The same row, read again from there under a new
   * selection, so the server cuts off what it was saying and begins the block asked for.
   */
  function jumpReading(index: number) {
    const playing = nowPlaying;
    if (playing === null || !playing.segments) return false;
    const at = Math.max(0, Math.min(playing.segments!.length - 1, index));
    const segment = playing.segments![at]!;
    // The read goes on, only from elsewhere: its stop keeps the media keys and the audio output,
    // which a jump from a media key could not take back without a gesture.
    // `#434 read-aloud-pause`: a jump made while held cues the block up HELD — Next on a steering
    // wheel is not a Play the reader never pressed.
    const hold = readingPaused();
    readJumping = true;
    readJumpPending = true;
    try {
      deps.guardQuietly(() => readAloud(playing.parts, { part: segment.part, block: segment.block, hold }))();
    } finally {
      readJumping = false;
    }
    return true;
  }

  /** True while `jumpReading` stops the read it is about to start again elsewhere. */
  let readJumping = false;
  /**
   * `#442 read-strip-order`. True from a jump until the block jumped to plays or fails: the bar keeps
   * Pause and Stop where they were through the wait, rather than giving the thumb's place to a Stop
   * that leaves reading mode for the moment the next block is still being fetched.
   */
  let readJumpPending = false;
  /**
   * `scrubber-full-width`. True from `recoverRead` stopping a read cut off until its retry is under way
   * (then `readJumpPending`) or abandoned: the strip stays up through the wait, as through a jump's.
   */
  let readRecovering = false;

  /** Previous (-1) or Next (+1). Previous on the first block starts it again. */
  function stepReading(delta: number) {
    const playing = nowPlaying;
    if (playing === null || !playing.segments) return false;
    if (delta > 0 && playing.segment! + 1 >= playing.segments!.length) return false;
    return jumpReading(playing.segment! + delta);
  }

  /**
   * `#627 scrubber-thumb-arrows`. Previous (-1) or Next (+1) section. Previous goes to where the section
   * being read began, and from its first block to where the one before began, as a player's Previous
   * chapter does; on the first section it starts it again, as Previous on the first block does.
   */
  function stepSection(delta: number) {
    const playing = nowPlaying;
    if (playing === null || !playing.segments) return false;
    const starts = readSectionStarts(playing as Playing & { segments: SpeechSegment[] });
    const at = playing.segment!;
    if (delta > 0) {
      const next = starts.find((start) => start > at);
      return next === undefined ? false : jumpReading(next);
    }
    const before = starts.filter((start) => start < at);
    return jumpReading(before.length > 0 ? before[before.length - 1]! : 0);
  }

  /**
   * `#434 read-aloud-pause`. ONE STATE PATH for pausing a read. The Read button (Pause while a read
   * plays, Resume while it is held), the media session's play/pause and the keys all come here and
   * nowhere else, so they can never disagree about whether the voice is held (`#537
   * scrub-bar-no-play-pause` took the strip's own Pause/Play away: the bar has it).
   * `renderReadTransport` is its one rendering: every control that says the act reads it.
   */
  function readingPaused() {
    return nowPlaying !== null && nowPlaying!.paused === true;
  }

  /** What every transport control says now, from the one state: strip, Read button, data hooks. */
  function renderReadTransport() {
    const paused = readingPaused();
    const strip = deps.el("read-steer");
    if (strip) strip.setAttribute("data-paused", paused ? "true" : "false");
    const button = deps.el("read-aloud");
    if (button) {
      button.setAttribute("data-paused", paused ? "true" : "false");
      // As `renderControls` states it, kept here for the starts, holds and stops between its draws
      // (`#451 read-strip-followups`).
      button.setAttribute("data-active", readingMode || readInProgress() || pendingRead !== null ? "true" : "false");
    }
    if (nowPlaying !== null || (pendingRead === null && !readJumping)) {
      readJumpPending = false;
      readRecovering = false;
    }
    const label = deps.el("read-aloud-label");
    if (label) {
      // `#451 read-strip-followups`. A read in progress is what makes Read its Pause, in the mode or
      // out of it: s reads one message without entering the mode (`readFromKeyboard`), and that read
      // is paused, resumed and stopped from the bar like any other. A read still being fetched
      // (`pendingRead`) cannot be paused — there is no clock to hold — so there Read is the Stop that
      // cancels it, as it already was in the mode with nothing playing; out of the mode it used to
      // say Read and enter the mode over the fetch.
      label.textContent = readInProgress() ? (paused ? "Resume" : "Pause")
        : readingMode || pendingRead !== null ? "Stop"
        : "Read";
    }
    // `#442 read-strip-order`. While Read is the read's Pause or Resume, the way out stands beside
    // it; with nothing playing, Read is that way out itself and the second button would repeat it.
    // `#451 read-strip-followups`: beside it for a read started with s too, whose one block draws no
    // strip to stop it from.
    const stop = deps.el("read-aloud-stop");
    if (stop && button) stop.hidden = button.hidden || !readInProgress();
    renderReadLayout();
  }

  /**
   * `scrubber-full-width`. The owner, 2026-10-27, of a phone: "I would like the scrubber bar to the
   * full width and the flush with the top of the bottom row of the controls. Those other controls
   * you don't need to access while we are in active playback mode." So while the strip is up the
   * root says whether its read is `playing` or `paused` (`data-read-strip`; absent with no strip),
   * and the stylesheet lays a phone out for it: the strip a full-width row on the dock, and the
   * bar's second row and the chips gone while it plays.
   *
   * PAUSED IS NOT STOPPED. A pause brings the chips back, which float and move nothing, and keeps
   * the rest: the second row coming back would resize the list and lift the strip on every pause and
   * resume, and a pause is the moment the reader reaches for the strip or the row's Reply (the car
   * view's "pause, dictate, reply, resume"). Stop, or the read's end, takes the strip away and puts
   * everything back. A desk keeps its layout: nothing there reads this.
   */
  function renderReadLayout() {
    const strip = deps.el("read-steer");
    if (!strip) return;
    // A strip held up through a jump or a recovery (`renderReadSteer`) goes when that ends with nothing playing.
    if (!strip.hidden && readingPosition() === "" && !readJumpPending && !readRecovering) {
      strip.hidden = true;
      renderReadTimeline();
    }
    const root = document.documentElement;
    if (strip.hidden) root.removeAttribute("data-read-strip");
    // Held through a jump, it says what it said: a held read's jump cues its block up held.
    else if (nowPlaying !== null) root.setAttribute("data-read-strip", readingPaused() ? "paused" : "playing");
  }

  /** A read is playing or held, or moving to another of its blocks (`readJumpPending`). */
  function readInProgress() {
    return nowPlaying !== null || readJumpPending;
  }

  /** Hold the read (`true`) or carry on from where it was held (`false`). The one path. */
  function setReadingPaused(paused: boolean) {
    if (paused) pauseReading();
    else resumeReading();
  }

  /** The toggle every Pause/Play control is. */
  function toggleReadingPaused() {
    setReadingPaused(!readingPaused());
  }

  /**
   * Pause what is being read where it is, and carry on from there. Played through Web Audio, the
   * audio clock simply stops; played by an `<audio>` element, the element pauses; said by the
   * device voice, it stops at the sentence, which the resume says again (`pauseTransport`, its own
   * because its timers are its own; `#554 read-pause-reply-resume`). The position — the block, and
   * where in it the clock had got to, or the sentence — is kept.
   */
  function pauseReading() {
    const playing = nowPlaying;
    if (playing === null || playing.paused) return;
    const audio = playing.audio;
    if (typeof playing.pauseTransport === "function") {
      playing.pauseTransport();
    } else if (audio && audio.hearsSpeech === true) {
      if (readAloudContext === null || typeof readAloudContext.suspend !== "function") return;
      quietReadAloudContext();
    } else if (audio && typeof audio.pause === "function") {
      audio.pause();
    } else {
      return;
    }
    nowPlaying = { ...nowPlaying!, paused: true, wasHeld: true };
    mediaSessionState("paused");
    renderReadTransport();
  }

  /**
   * Whether the read being held can no longer be carried on as it stands: a part failed while it
   * was held, its audio context is gone, or the speech ticket its unplayed parts were asked for
   * under has ended (`#368 speech-ticket-gone`, `#290 speech-ticket-refresh`). A pause may last
   * the rest of a journey; the ticket lasts ten minutes.
   */
  function heldReadStale(playing: Playing) {
    if (playing.stale === true) return true;
    const audio = playing.audio;
    if (audio && audio.hearsSpeech === true &&
        (readAloudContext === null || readAloudContext.state === "closed")) return true;
    if (playing.streamed !== true || !Array.isArray(playing.parts)) return false;
    if (!Array.isArray(playing.tickets)) {
      return playing.parts.some((part) => preparedEntry(part) === null);
    }
    // The tickets AS CAPTURED for this read, for its unheard parts only: what is prepared now may
    // have been re-minted under it (a scroll prepares again), and a fresh entry there says nothing
    // about the dead URL this read would still fetch. A part already heard needs no ticket.
    const now = speechDeadlineNow();
    const ahead = new Set((Array.isArray(playing.segments!) ? playing.segments!.slice(Math.max(0, playing.segment || 0)) : [])
      .map((segment) => segment.part));
    for (const part of ahead) {
      const ticket = playing.tickets[part]!;
      if (!ticket) continue;
      if (ticketEnded(ticket, now)) return true;
      const entry = preparedEntry(playing.parts[part]!);
      if (entry === null || entry.url !== ticket.url) return true;
    }
    return false;
  }

  /**
   * Carry a held read on by reading its held block again: a fresh ticket and a fresh voice
   * session, asked for by the ordinary tap path, which prepares again whatever has ended. The
   * reader hears that one block from its start rather than a failed read.
   */
  function restartHeldBlock(playing: Playing) {
    nowPlaying = { ...playing, paused: false, stale: false };
    renderReadTransport();
    const at = playing.segment!;
    if (Array.isArray(playing.segments!) && playing.segments![at]! && jumpReading(at)) return;
    if (Array.isArray(playing.parts) && playing.parts.length > 0) {
      deps.guardQuietly(() => readAloud(playing.parts, { part: 0, block: null }))();
    }
  }

  // --- a read cut off on the way: `#487 read-aloud-recovery` -----------------------------------------
  //
  // The owner, 2026-10-23: a read "just died spontaneously before finishing the message and clicking it
  // again. Got no response." The server had restarted under it (a deploy), and a restarted server has
  // forgotten every ticket it minted: the read's next request was a 404, the read ended, and every later
  // tap played another URL prepared before the restart, failing the same way. Now a streamed part that
  // fails forgets what was prepared and starts the read again from that part, after a short wait for a
  // server that is coming back, a few times before it says the message could not be played.

  // How long to wait before each try at starting a cut-off read again (`READ_RECOVERY_WAITS_MS`,
  // `recoveryWait`) is web/src/core/speech's.

  /**
   * Start read `parts` again from `from` (`{part, block, recovery}`) after `wait` ms, on fresh tickets.
   * Until then the row shows the read as asked for, and a tap on it, or on any other, is the reader's
   * own choice, which the try then leaves alone.
   *
   * @param {Array<string>} parts
   * @param {{part: number, block: number | null, recovery: number}} from
   * @param {number} wait
   */
  function recoverRead(parts: string[], from: ReadFrom & { recovery: number }, wait: number) {
    // Every ticket minted before this may be as dead as the one that failed.
    forgetPreparedSpeech();
    // `scrubber-full-width`: the strip stays up through the wait, as through a jump's.
    readRecovering = true;
    // Stopped as a jump stops it: the media keys and the audio output stay with the read.
    readJumping = true;
    try {
      stopReading();
    } finally {
      readJumping = false;
    }
    const ticket = readingTicket;
    const id = parts[0]!;
    pendingRead = id;
    setReadState("working");
    deps.eachColumn(deps.renderChannelRows);
    deps.setStatus("Reading aloud was cut off; trying again…");
    if (readRecoveryTimer !== null) clearTimeout(readRecoveryTimer);
    readRecoveryTimer = setTimeout(() => {
      readRecoveryTimer = null;
      if (ticket !== readingTicket || pendingRead !== id) return;
      readJumping = true;
      readJumpPending = true;
      readRecovering = false;
      let attempt;
      try {
        attempt = readAloud(parts, from);
      } finally {
        readJumping = false;
      }
      Promise.resolve(attempt).catch(() => {
        if (pendingRead !== null || nowPlaying !== null) return;
        // Not even prepared or fetched (the server still away): another try, while there are tries;
        // after the last, the media keys and the audio output it held go back.
        const wait = recoveryWait(from.recovery);
        if (wait !== null) {
          recoverRead(parts, { ...from, recovery: from.recovery + 1 }, wait);
        } else {
          releaseMediaSession();
          quietReadAloudContext();
        }
      });
    }, wait);
  }

  function resumeReading() {
    const playing = nowPlaying;
    if (playing === null || !playing.paused) return;
    // What it was held on has gone (see `heldReadStale`): read the held block again instead of
    // resuming onto a dead URL or a closed context and failing the whole read. A transport that
    // repairs itself (the device voice re-speaks its held chunk) is left to it.
    if (typeof playing.resumeTransport !== "function" && heldReadStale(playing)) {
      restartHeldBlock(playing);
      return;
    }
    nowPlaying = { ...playing, paused: false };
    const held = nowPlaying;
    const heldTicket = readingTicket;
    const audio = held.audio;
    const again = () => {
      // The resume itself was refused (a context a browser closed while held, an element whose
      // source went): the same way back as a stale read, if this is still that very read — the
      // attempt's ticket, not only its row, which a fresh read of it would also match.
      if (nowPlaying !== null && nowPlaying!.id === held.id && !nowPlaying!.paused && readingTicket === heldTicket) {
        restartHeldBlock(nowPlaying);
      }
    };
    if (typeof held.resumeTransport === "function") {
      held.resumeTransport();
    } else if (audio && audio.hearsSpeech === true) {
      if (readAloudContext !== null && typeof readAloudContext.resume === "function") {
        readAloudAudioContext();
        const begun = readAloudContext.resume();
        if (begun && typeof begun.catch === "function") begun.catch(again);
      }
    } else if (audio && typeof audio.play === "function") {
      const begun = audio.play();
      if (begun && typeof begun.catch === "function") begun.catch(again);
    }
    mediaSessionState("playing");
    renderReadTransport();
  }

  /**
   * The Media Session: a steering wheel's or a headset's previous, next, play and pause reach the
   * read. A browser routes those keys to a page that is playing a MEDIA ELEMENT, and Web Audio is not
   * one, so a silent looping element plays alongside while a read runs (`readAloudKeeper`).
   */
  let readAloudKeeper: HTMLAudioElement | null = null;

  function mediaSessionOf() {
    return typeof deps.browser !== "undefined" && deps.browser && deps.browser.mediaSession ? deps.browser.mediaSession : null;
  }

  function mediaSessionState(state: MediaSessionPlaybackState) {
    const session = mediaSessionOf();
    if (session === null) return;
    try {
      session.playbackState = state;
    } catch (_error) {
      // Informative only.
    }
    readAloudKeeperWanted = state === "playing";
    if (readAloudKeeper !== null) {
      if (state === "playing") {
        const begun = readAloudKeeper.play();
        if (begun && typeof begun.catch === "function") begun.catch(() => {});
      } else {
        readAloudKeeper!.pause();
      }
    }
  }

  /**
   * Inside the tap that starts any read: make the keeper and play it once, which is what lets a
   * phone's browser play it again later, outside a gesture, when the read turns out to have several
   * parts. Nothing without a Media Session.
   */
  function primeReadAloudKeeper() {
    if (mediaSessionOf() === null) return;
    if (readAloudKeeper === null && deps.audioAvailable() && typeof URL !== "undefined" &&
        typeof URL.createObjectURL === "function" && typeof Blob === "function") {
      readAloudKeeper = deps.browserPorts().audio.element(URL.createObjectURL(silentWav()));
      readAloudKeeper.loop = true;
    }
    if (readAloudKeeper === null || readAloudKeeperWanted) return;
    const begun = readAloudKeeper.play();
    const settle = () => {
      if (!readAloudKeeperWanted) readAloudKeeper!.pause();
    };
    if (begun && typeof begun.then === "function") begun.then(settle, () => {});
    else settle();
  }

  /** Whether the keeper should be playing: a read of several parts is. */
  let readAloudKeeperWanted = false;

  /** One second of silence as a WAV, for the keeper: 8 kHz, 8-bit, mono. */
  function silentWav() {
    const rate = 8000;
    const bytes = new Uint8Array(44 + rate);
    const view = new DataView(bytes.buffer);
    const text = (at: number, word: string) => [...word].forEach((c, i) => { bytes[at + i]! = c.charCodeAt(0); });
    text(0, "RIFF"); view.setUint32(4, 36 + rate, true); text(8, "WAVEfmt ");
    view.setUint32(16, 16, true); view.setUint16(20, 1, true); view.setUint16(22, 1, true);
    view.setUint32(24, rate, true); view.setUint32(28, rate, true); view.setUint16(32, 1, true);
    view.setUint16(34, 8, true); text(36, "data"); view.setUint32(40, rate, true);
    bytes.fill(128, 44);
    return new Blob([bytes], { type: "audio/wav" });
  }

  /** Called inside the tap that starts a read: the only moment a browser lets the keeper start. */
  function claimMediaSession() {
    const session = mediaSessionOf();
    if (session === null) return;
    const handle = (action: MediaSessionAction, fn: MediaSessionActionHandler) => {
      try {
        session.setActionHandler(action, fn);
      } catch (_error) {
        // An action this browser does not know.
      }
    };
    handle("previoustrack", () => { stepReading(-1); });
    handle("nexttrack", () => { stepReading(1); });
    handle("pause", () => { pauseReading(); });
    handle("play", () => { resumeReading(); });
    handle("stop", () => { stopReading(); });
    try {
      if (typeof MediaMetadata === "function") {
        session.metadata = new MediaMetadata({ title: "Reading a message", artist: readAloudLabel });
      }
    } catch (_error) {
      // Metadata is decoration.
    }
    primeReadAloudKeeper();
    mediaSessionState("playing");
  }

  /** Nothing is being read: let the media keys, the lock-screen title and the keeper go. */
  function releaseMediaSession() {
    const session = mediaSessionOf();
    if (session === null) return;
    /** @type {Array<MediaSessionAction>} */
    const actions: MediaSessionAction[] = ["previoustrack", "nexttrack", "pause", "play", "stop"];
    for (const action of actions) {
      try {
        session.setActionHandler(action, null);
      } catch (_error) {
        // An action this browser does not know.
      }
    }
    try {
      session.metadata = null;
    } catch (_error) {
      // Decoration.
    }
    mediaSessionState("none");
  }

  /**
   * Read one ROW aloud, and archive everything in it when the audio finishes.
   *
   * Tapping the message that is already playing STOPS it, so the same gesture is its own cancel and
   * the reader is never stuck listening to something they have finished with.
   *
   * A COMBINED ROW IS READ WHOLE, in parts, in order. The server speaks one Discord message per
   * request — it reads the message's OWN text and will not be handed a string, which is what stops
   * this route from becoming a way to spend the operator's vendor balance on anything at all — so a
   * row of two messages is two requests played back to back. Buffered parts are all fetched before
   * any is played: a gap in the middle of what the reader hears as one message, while the second
   * half is synthesised, is exactly the seam this feature exists to remove. A streamed part is
   * requested as soon as the one before it has audio, which queues it behind that part on the
   * server's session in order, and generation keeps ahead of playback.
   *
   * @param {Array<string>} ids the row's Discord ids, oldest first.
   */
  async function readAloud(ids: string[], from: ReadFrom | null = null) {
    const tapBegan = speechTimingNow();
    const parts = ids.map(String);
    // THE ROW'S IDENTITY is its first message, everywhere: the highlight, the pending mark and the
    // ticket all key on it, so that "which row is speaking" is one question with one answer.
    const id = parts[0]!;
    // A SECOND TAP IS AN ABORT, whether or not the audio has started. Pending counts: the wait is
    // where a mis-tap is noticed — the message is long, the reader realises they did not want it,
    // and the only thing they can do is tap the thing they just tapped. Checking `nowPlaying` alone
    // made that start a SECOND read instead of cancelling the first.
    //
    // `#376 read-aloud-blocks`. Not a JUMP: `from` moves a read to another of its blocks.
    const already = from === null && ((nowPlaying !== null && nowPlaying!.id === id) || pendingRead === id);
    // Also cancels anything still being fetched, so a double tap cannot end in two players.
    stopReading();
    if (already || parts.length === 0) {
      return;
    }
    const channel = deps.selectedChannelId();
    if (!channel) {
      return;
    }
    const ticket = readingTicket;
    // BEFORE the await, and this is the point: everything below is remote, and the reader is owed an
    // answer now rather than when ElevenLabs is finished.
    pendingRead = id;
    setReadState("working");
    // In both columns: the row asked for may be in either. `#229 desktop-two-column`.
    deps.eachColumn(deps.renderChannelRows);
    if (readAloudPlayback !== "audio") {
      try {
        if (readAloudPlayback !== "browser") throw new Error("This speech playback method is not supported by this page.");
        readWithBrowserSpeech(parts, id, ticket);
      } catch (error) {
        if (ticket === readingTicket) {
          stopReading();
          setReadState("failed");
          deps.setStatus((error as Error).message);
        }
        throw error;
      }
      return;
    }
    // Still inside the tap, before anything is awaited: the only moment a browser lets audio start.
    readAloudAudioContext();
    primeReadAloudKeeper();
    // `#376 read-aloud-blocks`. A read in blocks asks for its last block minutes after the tap, so a
    // ticket with little life left is prepared again now rather than failing part-way through.
    for (const part of parts) {
      const entry = preparedEntry(part);
      if (entry && entry.blocks && ticketTooShortForBlocks(entry, speechDeadlineNow())) preparedSpeech.forget(part);
    }
    // THE FAST PATH, and the whole point of preparing: every part already has a URL that streams, so
    // there is nothing to fetch and nothing to wait for. The player is handed the URL and the browser
    // starts playing against a response the server is still writing.
    // `#290 speech-ticket-refresh`: a URL whose ticket has ended is not ready, and is prepared again below.
    let ready = parts.map((part) => preparedEntry(part));
    let streamed = ready.every((entry) => entry && typeof entry.url === "string" && entry.url.length > 0);
    // Turning Read on starts preparing the whole screen in the background. A tap that comes before
    // that reaches this row does NOT wait for it: it prepares this row alone and reads it, and the
    // batch for the rest carries on behind. `#191 read-aloud-first-lag`.
    if (!streamed) {
      await prepareSpeechFor(parts);
      if (ticket !== readingTicket) return;
      ready = parts.map((part) => preparedEntry(part));
      streamed = ready.every((entry) => entry && typeof entry.url === "string" && entry.url.length > 0);
      deps.guardQuietly(prepareSpeech)();
    }
    let blobs;
    if (streamed) {
      blobs = null;
    } else {
    try {
      blobs = await Promise.all(parts.map((part) => fetchSpeech(channel, part)));
    } catch (error) {
      if (ticket === readingTicket) {
        pendingRead = null;
        setReadState("failed");
        deps.eachColumn(deps.renderChannelRows);
      }
      // BOTH places, and this one is why it is caught here at all: `guardQuietly` puts the reason in
      // the standing error panel, which is right for a background failure and wrong for a tap. The
      // reader touched a specific message and is waiting to hear it; the answer belongs on the line
      // under their thumb as well. Rethrown, so the panel still gets it.
      deps.setStatus(`could not read that message aloud: ${(error as Error).message}`);
      throw error;
    }
    }
    // THE TICKET CHECK, and it is the whole fix. Anything that happened while this was in flight --
    // another tap, a stop, leaving the view -- has already moved the ticket on, and this audio is
    // something nobody is waiting for any more. Dropped before a player exists, because a player
    // that exists is a player that can be heard.
    if (ticket !== readingTicket) {
      return;
    }
    // The wait is over: it is being READ now, not merely asked for.
    pendingRead = null;
    // `objectUrls` is what has to be REVOKED later, and a streamed URL is not one of them: it belongs
    // to the server and revoking it would be meaningless. Keeping the two apart is what stops the
    // cleanup either leaking blobs or throwing on something it does not own.
    const objectUrls = streamed ? [] : blobs!.map((blob) => URL.createObjectURL(blob));
    const selection = speechSelection();
    // `#376 read-aloud-blocks`. What is played, in order: every block of every message in the row, or
    // the whole message where it is one block or was not prepared in blocks.
    const segments = streamed ? speechSegments(ready) : parts.map((_part, part) => ({ part, block: null }));
    const sources = streamed
      ? segments.map((segment) => `${ready[segment.part]!.url}?selection=${selection}` +
        (segment.block === null ? "" : `&block=${segment.block}`))
      : objectUrls;
    // Where this read starts: the block a jump asked for, else the first.
    const first = from === null ? 0 : Math.max(0, segments.findIndex((segment) =>
      segment.part === from.part && (from.block === null || segment.block === null || segment.block >= from.block)));
    const clock = readClock();
    // `#487 read-aloud-recovery`. How many times this read has been started again after a part failed
    // on the way, since a part last played to its end.
    let recovery = from !== null && Number.isInteger(from.recovery) ? from.recovery! : 0;
    // The blocks the read was prepared in travel with it, so a pace change that forgets what was
    // prepared does not take the light off the read in progress.
    const blocks = ready.map((entry) => (entry && entry.blocks) || null);
    const players: TransportPlayer[] = [];
    const started = sources.map(() => false);
    const requestBegan = sources.map((): number | null => null);
    const playInvokedAt = sources.map((): number | null => null);
    const loadedAt = sources.map((): number | null => null);
    const audibleAt = sources.map((): number | null => null);
    const speechAt = sources.map((): number | null => null);
    const timingReported = sources.map(() => false);
    // One report per part, sent once it is audible and either speech has begun or the player cannot
    // tell. `#191 read-aloud-first-lag`: the time to the first loud sample, not only the first frame.
    const reportTiming = (at: number) => {
      if (timingReported[at] || audibleAt[at]! === null) return;
      timingReported[at]! = true;
      const invokedAt = playInvokedAt[at]! === null ? audibleAt[at]! : playInvokedAt[at]!;
      const mediaAt = loadedAt[at]! === null ? audibleAt[at]! : loadedAt[at]!;
      // The bare ticket URL: the selection only routes playback.
      reportSpeechPlayback(ready[segments[at]!.part]!.url, {
        tap_to_play_ms: speechElapsedMs(tapBegan, invokedAt),
        request_to_loaded_ms: speechElapsedMs(requestBegan[at]!, mediaAt),
        loaded_to_playing_ms: speechElapsedMs(mediaAt, audibleAt[at]!),
        tap_to_audible_ms: speechElapsedMs(tapBegan, audibleAt[at]!),
        ...(speechAt[at]! === null ? {} : { tap_to_speech_ms: speechElapsedMs(tapBegan, speechAt[at]!) }),
      }, segments[at]!.block);
    };
    // ONE block ahead, never more: the next is asked for once this one is both playing and has
    // audio. A provider that serves reads side by side would otherwise be asked for the whole message
    // at once, each block's first audio asking for the one after it.
    const prefetchAfter = (at: number) => {
      if (streamed && started[at]! && loadedAt[at]! !== null && at + 1 < sources.length && !players[at + 1]) {
        addPlayer(at + 1);
      }
    };
    // `#548 cloud-read-failures`. Where the voice failed this read, reported under the part's ticket.
    const reportFailure = (at: number) => (reason: string, detail: string) => {
      const url = ready[segments[at]!.part] && ready[segments[at]!.part]!.url;
      reportSpeechFailure(url, reason, detail, segments[at]!.block);
    };
    // `#548 cloud-read-failures`. A part with no speech arrived READ_NO_SPEECH_MS after its first audio
    // stops the read: silence slower than `drained` can say so. Counted from the first audio of a part
    // that has been started (`maybeGuardSpeech`), never from the request: a cold session or a busy
    // voice may take seconds to answer, which the server's own first-audio limit judges. Held, the
    // wait waits too. An `<audio>` element cannot hear its samples, and is not judged.
    const guarded = sources.map(() => false);
    const maybeGuardSpeech = (at: number) => {
      if (!streamed || guarded[at]! || !started[at] || loadedAt[at]! === null) return;
      guarded[at]! = true;
      guardSpeech(at);
    };
    const guardSpeech = (at: number) => {
      const check = () => {
        const player = players[at]!;
        if (ticket !== readingTicket || nowPlaying === null || nowPlaying!.id !== id || !player) return;
        if (!("heardSpeech" in player) || player.heardSpeech!()) return;
        if (nowPlaying!.paused) {
          setTimeout(check, READ_NO_SPEECH_MS);
          return;
        }
        cloudReadFailed("no_audio", "", parts, reportFailure(at));
      };
      setTimeout(check, READ_NO_SPEECH_MS);
    };
    const startPlayer = (at: number) => {
      started[at]! = true;
      playInvokedAt[at]! = speechTimingNow();
      const begun = players[at]!.play();
      prefetchAfter(at);
      maybeGuardSpeech(at);
      return begun;
    };
    // The next part, once the one before it is all scheduled or has ended. `play()` answers with a
    // promise a browser is allowed to reject, and this one is not awaited by anybody — an unhandled
    // rejection here would take the next part down silently and leave the row lit with nothing
    // coming.
    const startNext = (at: number) => {
      if (started[at]!) return;
      const next = players[at] || addPlayer(at);
      // Still the same row being read, so nothing on screen changes yet: only the player does.
      nowPlaying = { ...nowPlaying!, audio: next };
      const begun = startPlayer(at);
      if (begun && typeof begun.catch === "function") {
        begun.catch(() => {
          // `#563 read-aloud-fallback-chain`: the device voice is the
          // chain's last resort when the configured order ends with it.
          if (readWithDeviceTail(parts, id, ticket)) return;
          stopReading();
          deps.setStatus("that message could not be played.");
        });
      }
    };
    const addPlayer = (at: number) => {
      requestBegan[at]! = speechTimingNow();
      const audio = streamed ? streamedSpeechPlayer(sources[at]!, clock) : deps.browserPorts().audio.element(sources[at]!);
      players[at]! = audio;
      // Only the Web Audio player hears its own samples; an `<audio>` element cannot.
      const hearsSpeech = "hearsSpeech" in audio && audio.hearsSpeech === true;
      audio.addEventListener("loadeddata", () => {
        if (ticket !== readingTicket || loadedAt[at]! !== null) return;
        loadedAt[at]! = speechTimingNow();
        maybeGuardSpeech(at);
        // The next part is asked for as soon as this one has audio: on a provider with one session
        // it queues behind this one there, and is generated the moment this one is.
        prefetchAfter(at);
      });
      // `#376 read-aloud-blocks`. All of this part is scheduled: the next is scheduled to begin where
      // it ends, so the boundary is as long as the next part's audio is late, and no longer.
      audio.addEventListener("drained", () => {
        if (ticket !== readingTicket || nowPlaying === null || nowPlaying!.id !== id) return;
        // `#540 read-aloud-desktop`. The whole of this part arrived and not one sample of it was speech:
        // the voice answered with silence. Going on raced the light through every block in silence and
        // then archived the message as heard; it stops here, says so, and archives nothing.
        if ("heardSpeech" in audio && !audio.heardSpeech!()) {
          cloudReadFailed("silent", "", parts, reportFailure(at));
          return;
        }
        learnSpeakingRate(audio, segments[at]!, blocks);
        if (at + 1 < sources.length) startNext(at + 1);
      });
      audio.addEventListener("speech", (inMs) => {
        if (!streamed || speechAt[at]! !== null) return;
        speechAt[at]! = speechTimingNow() + Math.max(0, Number(inMs) || 0);
        reportTiming(at);
      });
      audio.addEventListener("playing", () => {
        if (ticket !== readingTicket) return;
        // This part is the one being heard now: the highlight moves to it.
        if (nowPlaying !== null && nowPlaying!.id === id && nowPlaying!.segment! !== at) {
          nowPlaying = { ...nowPlaying!, segment: at };
          showReadingPosition();
        }
        if (audibleAt[at]! !== null || !streamed) return;
        audibleAt[at]! = speechTimingNow();
        if (speechAt[at]! !== null || !hearsSpeech) {
          reportTiming(at);
        } else {
          // Speech has not arrived yet: it usually follows within a second. Not for ever, though.
          setTimeout(() => reportTiming(at), SPEECH_ONSET_WAIT_MS);
        }
      });
      audio.addEventListener("ended", () => {
        // Only if THIS is still the read in progress. The reader may have tapped another message
        // while this was finishing, and archiving on a stale `ended` would file away the wrong one.
        if (ticket !== readingTicket || nowPlaying === null || nowPlaying!.id !== id) {
          return;
        }
        // A part heard to its end: the read is going again, and a later failure has every try.
        recovery = 0;
        if (at + 1 < sources.length) {
          // A part that was not started when this one drained (an `<audio>` element says no such
          // thing) starts now.
          startNext(at + 1);
          return;
        }
        stopReading();
        // EVERY constituent. A row read to the end is a row the reader has heard all of, and
        // archiving only the first would leave its own tail in the queue.
        deps.guardQuietly(() => deps.dismissMessages({ messages: parts }))();
      });
      audio.addEventListener("error", (why: unknown, detail?: unknown) => {
        if (ticket !== readingTicket) {
          return;
        }
        // `#487 read-aloud-recovery`: the streamed player says why it failed; an `<audio>` element
        // hands an event, which says nothing this can act on.
        const lost = /** @type {unknown} */ (why) === "lost";
        // `#548 cloud-read-failures`: an `<audio>` element hands an Event, which names no reason.
        const reason = typeof why === "string" ? why : "refused";
        const said = typeof detail === "string" ? detail : "";
        // `#434 read-aloud-pause`: a part that fails while the read is HELD (its stream timed out
        // under the pause, its ticket went) does not end the read the reader only paused. It is
        // marked stale, and the resume reads the held block again on a fresh ticket.
        if (nowPlaying !== null && nowPlaying!.paused) {
          nowPlaying = { ...nowPlaying!, stale: true };
          return;
        }
        // The same failure delivered just AFTER a resume (a frozen page delivering it late, a part
        // fetched on resume from a captured URL that died under the hold): repair it the same way
        // rather than ending a read whose only fault is having been paused.
        if (nowPlaying !== null && nowPlaying!.wasHeld && heldReadStale(nowPlaying)) {
          restartHeldBlock(nowPlaying);
          return;
        }
        const giveUpOrTryAgain = () => {
          // `#487 read-aloud-recovery`. A streamed part that fails was most often cut off by the server
          // going away (a restart forgets every ticket it minted) or by its connection dropping: the read
          // is started again from that part on fresh tickets, a few times, before it is given up. A part
          // fetched ahead that fails while the one before it is still heard starts the read again from
          // the one being heard, so nothing is skipped.
          const wait = streamed && lost ? recoveryWait(recovery) : null;
          if (wait !== null) {
            const heard = nowPlaying !== null && nowPlaying!.id === id && !started[at] ? nowPlaying!.segment! : at;
            const segment = segments[heard] || segments[at]!;
            recoverRead(parts, { part: segment.part, block: segment.block, recovery: recovery + 1 }, wait);
            return;
          }
          // `#487`: and what was prepared before a lost part with it, so the next tap prepares again
          // rather than playing another URL as dead as this one.
          if (streamed && lost) forgetPreparedSpeech();
          // `#548 cloud-read-failures`. The voice's reason, in short words, and the device voice offered.
          if (streamed) {
            cloudReadFailed(reason, said, parts, reportFailure(at));
            return;
          }
          stopReading();
          deps.setStatus("that message could not be played.");
        };
        // `#548 cloud-read-failures`. Cut off, the read may have been failed by the voice, naming why
        // (speech access denied, say), which another try would only repeat: the server kept why under
        // the ticket (`speechStreamFailure`), and is asked first. A dropped connection or a restarted
        // server has nothing kept, and the read is tried again as before.
        // An `<audio>` element's failure says nothing either, and is asked about the same way.
        if (streamed && (lost || typeof why !== "string")) {
          const entry = ready[segments[at]!.part]!;
          speechStreamFailure(entry && entry.url, segments[at]!.block).then((reason) => {
            if (ticket !== readingTicket) return;
            if (reason) {
              cloudReadFailed("refused", reason, parts, reportFailure(at));
              return;
            }
            giveUpOrTryAgain();
          });
          return;
        }
        // `#563 read-aloud-fallback-chain`: when the configured order ends
        // with the device voice, it takes these parts now instead of the
        // read stopping; without it, the cloud failure flow decides
        // (offer the device voice, try again, give up).
        if (readWithDeviceTail(parts, id, ticket)) return;
        giveUpOrTryAgain();
      });
      return audio;
    };
    if (streamed) {
      addPlayer(first);
    } else {
      sources.forEach((_source, at) => addPlayer(at));
    }
    // The same array the players are added to, so a stop reaches a part requested later.
    // `streamed` and the tickets as captured travel with it: a held read's unplayed parts play
    // from THESE URLs, not from whatever is prepared by resume time, so a resume after a long
    // pause checks these deadlines (`heldReadStale`, `#434`).
    const tickets = ready.map((entry) => (entry && entry.until ? { url: entry.url, until: entry.until } : null));
    nowPlaying = { id, audio: players[first]!, urls: objectUrls, players, parts, segments, segment: first, blocks, streamed, tickets, paused: false };
    setReadState("ready");
    deps.eachColumn(deps.renderChannelRows);
    renderReadSteer();
    if (segments.length > 1) {
      claimMediaSession();
      showReadingSentence();
    } else {
      releaseMediaSession();
    }
    const begun = startPlayer(first);
    // A jump made while held cues this block up held (`#434 read-aloud-pause`): the hold lands in
    // the same tick the player starts, before a streamed part's cushion or an element's first
    // sound can be heard.
    if (from !== null && from.hold === true) pauseReading();
    await begun;
  }
  return {
  initializeReadSteerOffset,
  browserSpeechVoice,
  get preparedSpeech() { return preparedSpeech; },
  get readAloudContext() { return readAloudContext; },
  get readSpeed() { return readSpeed; }, set readSpeed(value: typeof readSpeed) { readSpeed = value; },
  applyReadSpeed,
  stepReadSpeed,
  commitReadSpeedBox,
  storedReadSpeed,
  get readingMode() { return readingMode; }, set readingMode(value: typeof readingMode) { readingMode = value; },
  get readAloudPlayback() { return readAloudPlayback; }, set readAloudPlayback(value: typeof readAloudPlayback) { readAloudPlayback = value; },
  get readAloudLabel() { return readAloudLabel; }, set readAloudLabel(value: typeof readAloudLabel) { readAloudLabel = value; },
  READ_AUDIO_SOURCE_KEY,
  get agentReadAloud() { return agentReadAloud; }, set agentReadAloud(value: typeof agentReadAloud) { agentReadAloud = value; },
  get readAudioSource() { return readAudioSource; }, set readAudioSource(value: typeof readAudioSource) { readAudioSource = value; },
  storedReadAudioSource,
  WIT_VOICE_KEY,
  get witReadAloud() { return witReadAloud; }, set witReadAloud(value: typeof witReadAloud) { witReadAloud = value; },
  get witVoice() { return witVoice; }, set witVoice(value: typeof witVoice) { witVoice = value; },
  get chainDeviceTail() { return chainDeviceTail; }, set chainDeviceTail(value: typeof chainDeviceTail) { chainDeviceTail = value; },
  storedWitVoice,
  setWitVoice,
  loadWitVoices,
  setReadAudioSource,
  browserSpeechProblem,
  readCardAloud,
  markReadingCard,
  paintReadingCards,
  prepareSpeech,
  forgetPreparedSpeech,
  get nowPlaying() { return nowPlaying; }, set nowPlaying(value: typeof nowPlaying) { nowPlaying = value; },
  get pendingRead() { return pendingRead; }, set pendingRead(value: typeof pendingRead) { pendingRead = value; },
  get readState() { return readState; }, set readState(value: typeof readState) { readState = value; },
  setReadState,
  stopReading,
  readOnDevice,
  readAloudAudioContext,
  quietReadAloudContext,
  preparedBlocksOf,
  markReadingBlock,
  readingLabel,
  placeReadSteer,
  moveReadSteer,
  onReadGripDown,
  onReadGripMove,
  onReadGripUp,
  onReadGripKey,
  jumpReading,
  stepReading,
  stepSection,
  renderReadTransport,
  readInProgress,
  toggleReadingPaused,
  readAloud
  };
}
export type ReadAloud = ReturnType<typeof createReadAloud>;
