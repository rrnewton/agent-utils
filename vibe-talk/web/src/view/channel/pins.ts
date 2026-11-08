// Keeping the channel's pins current, and changing them (`#206 pin-message`; `#624 voice-page-modularity`,
// moved from the legacy script into view/channel): the list read only when a channel read names a revision
// it is not at, or the filter is about to show it, and once more when a read landed meanwhile named another;
// a pin or unpin shown at once, sent in order, rolled back for what the server did not answer, and said;
// what a pin keeps of a message, and a message drawn from what its pin kept. The list itself is
// web/src/core/annotations `createPins`; drawing the rows and the filter is the page's.

import type { Message, Pin, PinChangeResponse, PinsResponse } from "../../contract/index.ts";
import { createPins } from "../../core/annotations/index.ts";
import type { Pins } from "../../core/annotations/index.ts";
import { inTimeOrder, threadOf } from "../../core/channels/index.ts";
import { renderedHtmlOf } from "../../present/rows/index.ts";
import { childByClass, setAttributeIfChanged, setHiddenIfChanged, setTextIfChanged } from "../dom/index.ts";
import { idsOf, rowMessages } from "./rows.ts";

/** A message as a pin keeps it. */
type PinnedMessage = Pick<Message, "id" | "author" | "author_id" | "author_is_bot" | "content" | "content_html" |
  "heading" | "upstream_url" | "timestamp" | "thread">;

/** A failure as the page's API client throws it. */
interface Refusal {
  readonly detail?: string;
  readonly message?: string;
  readonly network?: boolean;
}

/** What keeping the pins asks of the page. */
export interface PinSyncDependencies {
  /** The list and this page's unanswered changes (`createPins`). */
  readonly pins: Pins;
  /** The channel on screen, or "". */
  channel(): string;
  /** Forget the held list for `channel` (another channel, or another credential), and what the filter drew. */
  forget(channel: string): void;
  /** Read `channel`'s pins. */
  read(channel: string): Promise<PinsResponse>;
  /**
   * Pin message `id` in `channel`, or unpin it with null, in the order writes are queued in. `request`
   * builds the pin's body, called as the write's turn comes, so it reads the message as it is then.
   */
  write(channel: string, id: string, request: (() => object) | null): Promise<PinChangeResponse>;
  /** Draw every row's pin, and the filter's list, again (`renderPins`). */
  drawn(): void;
  /** Run `run` with its failures caught quietly (`guardQuietly`). */
  guard(run: () => Promise<void>): void;
  /** Whether the browser says it is online (`navigator.onLine !== false`). */
  online(): boolean;
  /** Say `text` on the status line. */
  status(text: string): void;
  /** The chat service as a sentence names it. */
  service(): string;
  /** Now, in milliseconds. */
  now(): number;
}

/** Keeping the pins' commands and questions. */
export interface PinSync {
  /** A read said what revision this channel's pins are at: read the list when the one held is not at it. */
  noteRevision(revision: unknown): void;
  /** Read the list now if this channel's has never been read: the filter is about to show it. */
  ensure(): void;
  /** Read this channel's pins, and draw what changed. */
  load(): Promise<void>;
  /** Pin every message of a row, or unpin them, said either way. */
  toggle(messages: readonly PinnedMessage[]): Promise<void>;
  /** A message drawn from what pin `pin` kept, for a pin older than anything loaded. */
  kept(pin: Pin, channel: string): Message;
  /** Whether `message` was drawn from what its pin kept (`kept`), rather than from the message itself. */
  isKept(message: unknown): boolean;
}

/** Nothing read yet. */
export function createPinSync(deps: PinSyncDependencies): PinSync {
  /** Rows of the filter drawn from what a pin kept, rather than from the message itself. */
  const keptByPin = new WeakSet<object>();

  /** Whether this message is pinned in the channel on screen. */
  const isPinnedId = (id: unknown): boolean => deps.pins.isPinned(deps.channel(), id);

  function noteRevision(revision: unknown): void {
    const channel = deps.channel();
    if (typeof revision !== "number" || !channel) return;
    if (deps.pins.channel() !== channel) deps.forget(channel);
    if (deps.pins.revision() === revision) return;
    // One read at a time: a revision noted while one is out is checked against what it brings back.
    const flight = deps.pins.inFlight();
    if (flight && flight.channel === channel) {
      flight.noted = revision;
      return;
    }
    deps.guard(loadPins);
  }

  async function loadPins(): Promise<void> {
    const channel = deps.channel();
    if (!channel) return;
    if (deps.pins.channel() !== channel) deps.forget(channel);
    const read = deps.pins.startRead(channel);
    try {
      const payload = await deps.read(channel);
      if (!deps.pins.isCurrent(read) || deps.channel() !== channel) return;
      applyPinsList(payload);
    } catch (error) {
      if (!deps.pins.isCurrent(read) || deps.channel() !== channel) return;
      // SAID, by the filter that would have shown them; a channel read is not failed for it.
      const refusal = error as Refusal;
      deps.pins.failed(refusal.detail || refusal.message || "the server did not answer");
      deps.drawn();
    } finally {
      deps.pins.endRead(read);
    }
    // A read that landed meanwhile named a revision this list is not at: read once more.
    if (read.noted !== null && read.noted !== deps.pins.revision() && deps.channel() === channel) await loadPins();
  }

  /** Take a list the server sent as the truth, with this page's unanswered changes laid over it. */
  function applyPinsList(payload: PinsResponse): void {
    deps.pins.apply(payload);
    deps.drawn();
  }

  /** What a pin keeps of `message`, as this page will show it until the server says what it kept. */
  function provisionalPin(message: PinnedMessage): Pin {
    return {
      message_id: String(message.id),
      author: String(message.author || ""),
      author_id: String(message.author_id || ""),
      author_is_bot: message.author_is_bot === true,
      content: String(message.content === null || message.content === undefined ? "" : message.content),
      // Drawn as the message is until the server's own copy of the pin arrives; never sent.
      content_html: renderedHtmlOf(message),
      ...(message.heading ? { heading: message.heading } : {}),
      ...(message.upstream_url ? { upstream_url: message.upstream_url } : {}),
      truncated: false,
      timestamp: String(message.timestamp || ""),
      thread_id: threadOf(message),
      thread_root: Boolean(message.thread && message.thread.is_root === true),
      pinned_at_ms: deps.now(),
    } as Pin;
  }

  /** What a pin write sends: the row as this page holds it. Its id is the path's. */
  function pinRequest(message: PinnedMessage): object {
    const pin = provisionalPin(message);
    return {
      author: pin.author, author_id: pin.author_id, author_is_bot: pin.author_is_bot, content: pin.content,
      timestamp: pin.timestamp, thread_id: pin.thread_id, thread_root: pin.thread_root,
    };
  }

  /** A message drawn from what its pin kept, for a pin older than anything loaded. */
  function keptMessage(pin: Pin, channel: string): Message {
    const message: Message = {
      id: pin.message_id,
      channel_id: channel,
      author: pin.author,
      author_id: pin.author_id,
      author_is_bot: pin.author_is_bot,
      timestamp: pin.timestamp,
      spoken_time: "",
      reply_to: null,
      // An ellipsis where the server cut it, so a shortened text does not read as the whole message.
      // The server renders the snapshot with the same ellipsis after it. `#217 markdown-blocks`.
      content: pin.truncated ? `${pin.content}…` : pin.content,
    } as Message;
    if (renderedHtmlOf(pin) !== "") message.content_html = pin.content_html;
    // `#242 rich-headers`. Found by the server in the same text, ellipsis and all.
    if (pin.heading) message.heading = pin.heading;
    // `#408 copy-upstream-link`. Formed by the server as the pin was served, as a loaded
    // message's is, so a row drawn from a pin alone offers the link too.
    if (typeof pin.upstream_url === "string" && pin.upstream_url !== "") message.upstream_url = pin.upstream_url;
    if (pin.thread_id) {
      message.thread = {
        id: pin.thread_id, root_message_id: null, is_root: pin.thread_root, reply_count: null, reply_count_exact: false,
      } as Message["thread"];
    }
    keptByPin.add(message);
    return message;
  }

  return {
    noteRevision,
    ensure() {
      const channel = deps.channel();
      if (!channel) return;
      if (deps.pins.channel() !== channel) deps.forget(channel);
      const flight = deps.pins.inFlight();
      if (deps.pins.revision() === null && !(flight && flight.channel === channel)) deps.guard(loadPins);
    },
    load: loadPins,
    async toggle(messages) {
      const channel = deps.channel();
      if (!channel || messages.length === 0) return;
      if (deps.pins.channel() !== channel) deps.forget(channel);
      const pinning = !messages.some((message) => isPinnedId(message.id));
      const act = pinning ? "pinned" : "unpinned";
      if (!deps.online()) {
        deps.status(`Offline — nothing was ${act}. Try again once the connection is back.`);
        return;
      }
      const before = new Map(messages.map((message) => [String(message.id), deps.pins.held(String(message.id))]));
      for (const message of messages) {
        const id = String(message.id);
        deps.pins.stage(id, pinning ? provisionalPin(message) : null);
      }
      deps.drawn();
      const unpinned: string[] = [];
      let revision: unknown = null;
      let answered = 0;
      try {
        for (const message of messages) {
          const id = String(message.id);
          // In the queue the archive writes use, so a run of taps reaches the server in order.
          const answer = await deps.write(channel, id, pinning ? () => pinRequest(message) : null);
          answered += 1;
          if (deps.pins.channel() !== channel) continue;
          // At the bound the server unpinned the oldest pin to make room, and named it.
          deps.pins.answered(id, answer.pin, answer.unpinned);
          for (const gone of answer.unpinned) unpinned.push(String(gone));
          revision = answer.revision;
        }
      } catch (error) {
        if (deps.pins.channel() === channel) {
          // Back to what it was — for the messages the server did not answer for. Any it did are
          // its truth now, and the list is read again to be sure of them.
          for (const message of messages.slice(answered)) {
            const id = String(message.id);
            deps.pins.rollback(id, before.get(id));
          }
          deps.drawn();
          if (answered > 0) deps.guard(loadPins);
        }
        const refusal = error as Refusal;
        deps.status(refusal.network
          ? `Offline — that message was not ${act}. Try again once the connection is back.`
          : `That message was not ${act}: ${refusal.detail || refusal.message}`);
        return;
      }
      if (deps.pins.channel() !== channel) return;
      deps.drawn();
      // The list's revision has moved; read it again unless this change is all that moved it.
      noteRevision(revision);
      const service = deps.service();
      if (!pinning) deps.status("Unpinned.");
      else if (unpinned.length > 0) {
        deps.status(`Pinned. The oldest pin here was unpinned to stay at ${deps.pins.limit() || "the limit of"} pins.`);
      } else deps.status(`Pinned here, not in ${service}.`);
    },
    kept: keptMessage,
    isKept: (message) => typeof message === "object" && message !== null && keptByPin.has(message),
  };
}

// The pinned messages themselves on the page (`#206 pin-message`; `#624 voice-page-modularity`,
// moved from the legacy script into view/channel): the held list, each column's filter record,
// the sync instantiated over them, every row's chip, the filter's list and its toggle. The list
// itself is core/annotations' (`createPins`); keeping it current is PinSync above.


/** A column's record of its Pinned filter. */
export interface PinsView {
  only: boolean;
  drawn: string;
}

/** What the pinned messages ask of the page. */
export interface PinnedMessagesDependencies {
  byId(id: string): HTMLElement;
  leftById(id: string): HTMLElement;
  currentChannelId(): string | null;
  activePins(): PinsView;
  readPins(channel: string): Promise<unknown>;
  writePin(channel: string, id: string, request: (() => object) | null): Promise<unknown>;
  guard(run: () => Promise<void>): void;
  providerName(): string;
  eachColumn(fn: () => void): void;
  preservingScroll(fn: () => void): void;
  renderChannelRows(): void;
  renderScrollTools(): void;
  threading(): boolean;
  heldThreads(): Array<{ root?: Message | null }>;
  heldMessages(): Message[];
  timelineMessages(): Message[];
  combineOn(): boolean;
  hideEmptyOn(): boolean;
  scope(): string;
  isNoise(message: Message): boolean;
  renderedHtmlOf(message: Message): string;
  headingOf(message: Message): unknown;
  editedDrawn(message: Message): unknown;
  upstreamUrlOf(message: Message): string | null;
  glom(messages: Message[]): Message[][];
  discordNode(messages: Message[]): HTMLElement;
  whilePinned<T>(fn: () => T): T;
  keepingFilterPlace<T>(fn: () => T): T;
  renderChannelNavigation(): void;
  renderOlderControl(): void;
  requestVisibleSummaries(): void;
  setStatus(text: string): void;
  online(): boolean;
  settlePendingFilterPlace(): void;
  currentStoreChannel(): string;
  reactionsDrawn(message: Message): unknown;
}

/** The pinned messages' commands and questions. */
export interface PinnedMessages {
  readonly pinStore: Pins;
  freshPinsView(): PinsView;
  pinsView(): PinsView;
  isPinnedId(id: unknown): boolean;
  forgetPins(channel?: string): void;
  notePinsRevision(revision: unknown): void;
  ensurePins(): void;
  drawRowPin(row: Element): void;
  renderPins(): void;
  pinnedMessages(): Message[];
  syncPinnedList(): void;
  setPinnedOnly(on: boolean, keepPlace?: boolean): void;
  togglePin(messages: Message[]): Promise<unknown>;
}

/** No pins read, filter off. */
export function createPinnedMessages(deps: PinnedMessagesDependencies): PinnedMessages {
  const el = deps.byId;
  const leftEl = deps.leftById;

  const pinStore: Pins = createPins() as Pins;

  function freshPinsView(): PinsView {
    return { only: false, drawn: "" };
  }

  function pinsView(): PinsView {
    return deps.activePins();
  }

  const pinSync: PinSync = createPinSync({
    pins: pinStore,
    channel: () => currentChannelId(),
    forget: (channel: string) => forgetPins(channel),
    read: (channel: string) => deps.readPins(channel) as never,
    write: (channel: string, id: string, request: (() => object) | null) => deps.writePin(channel, id, request) as never,
    drawn: () => renderPins(),
    guard: (run: () => Promise<void>) => deps.guard(run),
    online: () => deps.online(),
    status: (text: string) => deps.setStatus(text),
    service: () => deps.providerName() || "the chat service",
    now: () => Date.now(),
  });

  const currentChannelId = (): string => String(deps.currentChannelId() || "");

  function isPinnedId(id: unknown): boolean {
    return (pinStore as { isPinned(channel: string, id: unknown): boolean }).isPinned(currentChannelId(), id);
  }

  function forgetPins(channel: string = ""): void {
    (pinStore as { forget(channel: string): void }).forget(channel);
    pinsView().drawn = "";
  }

  function notePinsRevision(revision: unknown): void {
    (pinSync as { noteRevision(revision: unknown): void }).noteRevision(revision);
  }

  function ensurePins(): void {
    (pinSync as { ensure(): void }).ensure();
  }

  function drawRowPin(row: Element): void {
    const isPinned = idsOf(row).some(isPinnedId);
    setAttributeIfChanged(row as never, "data-pinned", isPinned ? "true" : "false");
    const chip = childByClass(row as never, "pin-chip") as HTMLElement | null;
    if (chip) {
      setHiddenIfChanged(chip as never, !isPinned);
      setAttributeIfChanged(
        chip as never,
        "title",
        rowMessages(row).some((message) => (pinSync as { isKept(m: Message): boolean }).isKept(message))
          ? "Pinned on this server. This is the copy kept when it was pinned; the message is older " +
              "than what is loaded."
          : "Pinned on this server, for every device. The chat service's own pins are not changed."
      );
    }
    const pinItem = childByClass(row as never, "row-pin-button") as HTMLElement | null;
    if (pinItem) setTextIfChanged(pinItem as never, isPinned ? "Unpin message" : "Pin message");
  }

  function renderPins(): void {
    deps.eachColumn(() => {
      deps.preservingScroll(deps.renderChannelRows);
      deps.renderScrollTools();
    });
    for (const row of leftEl("side-cards").children) {
      if (row.hasAttribute("data-main-row") || row.hasAttribute("data-unpack-thread")) drawRowPin(row);
    }
    if (pinsView().only) deps.settlePendingFilterPlace();
  }

  function pinnedMessages(): Message[] {
    const channel = currentChannelId();
    const store = pinStore as { channel(): string; size(): number; entries(): Array<[string, unknown]> };
    if (store.channel() !== channel || store.size() === 0) return [];
    const held = new Map<string, Message>();
    const loaded = deps.threading() && deps.currentStoreChannel() === channel
      ? [...deps.heldThreads().map((summary) => summary.root).filter(Boolean) as Message[], ...deps.heldMessages(), ...deps.timelineMessages()]
      : [...el("discord-log").children].flatMap((row) => rowMessages(row));
    for (const message of loaded) held.set(String(message.id), message);
    return inTimeOrder(store.entries().map(([id, pin]) => held.get(id) || (pinSync as { kept(pin: unknown, channel: string): Message }).kept(pin, channel)) as never, "timestamp" as never) as Message[];
  }

  function syncPinnedList(): void {
    const list = el("pinned-log");
    if (!pinsView().only) {
      if (list.children.length > 0) list.replaceChildren();
      pinsView().drawn = "";
      return;
    }
    const messages = pinnedMessages();
    const drawn = JSON.stringify([deps.combineOn(), deps.hideEmptyOn(), deps.scope(), deps.threading(),
      ...messages.map((m) => [m.id, (m as { timestamp?: unknown }).timestamp, m.content, deps.renderedHtmlOf(m), m.author, threadOf(m as never), deps.isNoise(m),
        (pinSync as { isKept(m: Message): boolean }).isKept(m), deps.reactionsDrawn(m), (m as { heading?: unknown }).heading || null, deps.editedDrawn(m), deps.upstreamUrlOf(m)])]);
    if (drawn === pinsView().drawn) return;
    pinsView().drawn = drawn;
    deps.whilePinned(() => list.replaceChildren(...deps.glom(messages).map(deps.discordNode)));
  }

  function setPinnedOnly(on: boolean, keepPlace: boolean = true): void {
    if (on === pinsView().only) return;
    const change = (): void => {
      pinsView().only = on;
      el("pinned-filter").setAttribute("aria-pressed", on ? "true" : "false");
      if (on) el("pane-discord").setAttribute("data-pinned-only", "");
      else el("pane-discord").removeAttribute("data-pinned-only");
      deps.renderChannelNavigation();
      deps.renderOlderControl();
      deps.renderChannelRows();
      deps.renderScrollTools();
      if (on) ensurePins();
    };
    if (keepPlace) deps.keepingFilterPlace(change);
    else change();
    deps.requestVisibleSummaries();
  }

  function togglePin(messages: Message[]): Promise<unknown> {
    return (pinSync as { toggle(messages: Message[]): Promise<unknown> }).toggle(messages);
  }

  return {
    pinStore, freshPinsView, pinsView, isPinnedId, forgetPins, notePinsRevision, ensurePins,
    drawRowPin, renderPins, pinnedMessages, syncPinnedList, setPinnedOnly, togglePin,
  };
}
