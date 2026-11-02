// What was on screen, kept across a reload (`#189 restore-ui-state`, `#205 channel-view-memory`, `#265
// window-channel-memory`; `#624 voice-page-modularity`, moved from the legacy script into
// core/navigation). The one owner of the UI-state record: reading it back from this window's storage and
// the device's (checked on the way back, the old version brought forward, a damaged one dropped), the
// view the reader chose in each recent channel, whether the page has settled what it reopens on, and
// the place last held in the list. Why each part is kept, and where, is told where the legacy script
// describes the record; what is on screen now, and writing the record, are the page's.

import { is } from "../../contract/index.ts";
import type { ThreadSummary } from "../../contract/index.ts";
import type { StoragePort } from "../../ports/index.ts";

/** The record's key, in the device's storage and this window's. */
const UI_STATE_KEY = "vibe-talk.voice.ui-state";
/** The record's shape; a record of an older shape is brought forward (`fromUiStateV1`), any other dropped. */
const UI_STATE_VERSION = 2;
/** How many channels' chosen views the record keeps: the most recently on screen. */
const UI_STATE_CHANNELS = 50;
/** The most characters the record may take: past it the oldest channels' choices go. */
const UI_STATE_CHARS = 60000;
/** The views a channel reopens in. */
const REOPENED_VIEWS: readonly string[] = Object.freeze(["main", "flat", "thread"]);

/** A thread a channel reopens on: its id, where its Back goes, and its summary for the heading. */
export interface SavedThread {
  readonly id: string;
  readonly origin: string;
  readonly summary: ThreadSummary | null;
}

/** The view the reader chose in one channel. */
export interface ChosenView {
  readonly channel: string;
  readonly channelView: string;
  readonly thread: SavedThread | null;
}

/** Where the reader's eye was in the list: a message, not a pixel offset. */
export interface SavedPlace {
  readonly view: string;
  readonly atNewest: boolean;
  readonly anchor: string | null;
  readonly offset: number;
}

/** The record, as kept. */
export interface UiRecord {
  v: number;
  identity: string;
  view: string;
  todo: boolean;
  read?: unknown;
  channel: string;
  place: SavedPlace | null;
  channels: ChosenView[];
}

/** A place held in the list, under the channel and view it was measured in (`key`). */
export interface HeldPlace {
  readonly key: string;
  readonly place: SavedPlace;
}

type Loose = Record<string, unknown>;

const validSavedThread = (thread: unknown): thread is SavedThread => {
  const held = thread as Loose | null;
  return held !== null && typeof held === "object" &&
    typeof held.id === "string" && held.id !== "" && ["main", "flat", "list"].includes(held.origin as string) &&
    (held.summary === null || is("ThreadSummary", held.summary));
};

const validChosenView = (entry: unknown): entry is ChosenView => {
  const held = entry as Loose | null;
  return held !== null && typeof held === "object" &&
    typeof held.channel === "string" && held.channel !== "" && REOPENED_VIEWS.includes(held.channelView as string) &&
    (held.channelView === "thread" ? validSavedThread(held.thread) : held.thread === null);
};

const validSavedPlace = (place: unknown): boolean => {
  const held = place as Loose | null;
  return held === null || (typeof held === "object" &&
    typeof held.view === "string" && typeof held.atNewest === "boolean" &&
    (held.anchor === null || typeof held.anchor === "string") && Number.isFinite(held.offset));
};

/** Whether `saved` is a record of this shape (its channels checked one by one later, by `keptChoices`). */
function validUiState(saved: unknown): saved is UiRecord {
  const held = saved as Loose | null;
  return held !== null && typeof held === "object" && held.v === UI_STATE_VERSION &&
    typeof held.identity === "string" && ["voice", "discord"].includes(held.view as string) &&
    typeof held.todo === "boolean" && typeof held.channel === "string" &&
    validSavedPlace(held.place) && Array.isArray(held.channels);
}

/**
 * A version 1 record brought forward: one channel's view, kept only where the reader chose it, and its
 * place under the view key `viewKey` makes. Null for one that does not say what version 1 said.
 */
function fromUiStateV1(saved: Loose, viewKey: (view: string, thread: string | null) => string): UiRecord | null {
  if (!(typeof saved.identity === "string" && ["voice", "discord"].includes(saved.view as string) &&
      typeof saved.todo === "boolean" && typeof saved.channel === "string" &&
      REOPENED_VIEWS.includes(saved.channelView as string) && typeof saved.chosen === "boolean" &&
      (saved.channelView === "thread" ? validSavedThread(saved.thread) : saved.thread === null))) return null;
  const thread = saved.thread as SavedThread | null;
  const view = viewKey(saved.channelView as string, thread ? thread.id : null);
  const place = saved.place as Loose | null;
  return {
    v: UI_STATE_VERSION, identity: saved.identity, view: saved.view as string, todo: saved.todo, channel: saved.channel,
    place: place === null ? null : ({ ...place, view } as unknown as SavedPlace),
    channels: saved.chosen ? [{ channel: saved.channel, channelView: saved.channelView as string, thread }] : [],
  };
}

/** The choices of `entries` that are sound, one per channel, the first kept, `UI_STATE_CHANNELS` at most. */
function keptChoices(entries: readonly unknown[]): ChosenView[] {
  const seen = new Set<string>();
  const kept: ChosenView[] = [];
  for (const entry of entries) {
    if (!validChosenView(entry) || seen.has(entry.channel)) continue;
    seen.add(entry.channel);
    kept.push(entry);
  }
  return kept.slice(0, UI_STATE_CHANNELS);
}

/** What the record asks of the page. */
export interface UiStateDependencies {
  /** The device's storage, shared by every window. */
  deviceStorage(): StoragePort;
  /** This window's own storage. */
  windowStorage(): StoragePort;
  /** The credential's fingerprint the record is scoped to, or "" with none. */
  identity(): string;
  /** The key a view (and thread) of a channel is kept under (`viewKey`). */
  viewKey(view: string, thread: string | null): string;
}

/** The record's owner. */
export interface UiState {
  /** The record the page opened with, or null. */
  saved(): UiRecord | null;
  /** Whether what the page reopens on is settled: until it is, the record is not written. */
  settled(): boolean;
  /** It is settled. */
  settle(): void;
  /** The place last held in the list, or null. */
  heldPlace(): HeldPlace | null;
  /** Hold `place`. */
  holdPlace(place: HeldPlace): void;
  /** Forget the record, here and in storage: signing out. */
  forget(): void;
  /** The views chosen under the credential now; a change of credential forgets them. */
  chosenViewsNow(): ChosenView[];
  /** The view chosen in `channel`, or undefined. */
  chosenView(channel: unknown): ChosenView | undefined;
  /** `entry` chosen: first, and the channel's older choice gone. */
  remember(entry: ChosenView): void;
  /** Forget the choices of every channel `gone` says. */
  forgetChosen(gone: (channel: string) => boolean): void;
  /** The choices as held, with no check of the credential (the record being written reads them so). */
  chosenViewsHeld(): ChosenView[];
  /** The choices as the record about to be written keeps them (some dropped for room). */
  setChosen(channels: ChosenView[]): void;
}

/** The record read back now, from this window's storage and the device's. */
export function createUiState(deps: UiStateDependencies): UiState {
  const dropUiRecord = (store: () => StoragePort): void => {
    try {
      store().removeItem(UI_STATE_KEY);
    } catch (_error) {
      // Storage refused: nothing kept to drop.
    }
  };
  const dropUiState = (): void => {
    dropUiRecord(deps.deviceStorage);
    dropUiRecord(deps.windowStorage);
  };
  const readUiRecord = (store: () => StoragePort): UiRecord | null => {
    let raw: string | null = null;
    try {
      raw = store().getItem(UI_STATE_KEY);
    } catch (_error) {
      return null;
    }
    if (raw === null) return null;
    let saved: unknown = null;
    try {
      saved = JSON.parse(raw);
    } catch (_error) {
      saved = null;
    }
    if (saved !== null && typeof saved === "object" && (saved as Loose).v === 1) saved = fromUiStateV1(saved as Loose, deps.viewKey);
    const identity = deps.identity();
    if (identity && validUiState(saved) && saved.identity === identity) {
      saved.channels = keptChoices(saved.channels);
      return saved;
    }
    dropUiRecord(store);
    return null;
  };
  const readUiState = (): UiRecord | null => {
    const device = readUiRecord(deps.deviceStorage);
    const own = readUiRecord(deps.windowStorage);
    if (own === null) return device;
    if (device !== null) {
      own.channels = keptChoices([...own.channels, ...device.channels]);
      own.todo = device.todo;
      own.read = device.read;
    }
    return own;
  };

  let savedUi = readUiState();
  let chosenViews: ChosenView[] = savedUi ? savedUi.channels : [];
  let chosenViewsIdentity = savedUi ? savedUi.identity : "";
  let uiStateSettled = savedUi === null;
  let heldPlace: HeldPlace | null = null;

  const chosenViewsNow = (): ChosenView[] => {
    const identity = deps.identity();
    if (chosenViewsIdentity !== identity) {
      chosenViews = [];
      chosenViewsIdentity = identity;
    }
    return chosenViews;
  };

  return {
    saved: () => savedUi,
    settled: () => uiStateSettled,
    settle() {
      uiStateSettled = true;
    },
    heldPlace: () => heldPlace,
    holdPlace(place) {
      heldPlace = place;
    },
    forget() {
      dropUiState();
      savedUi = null;
      chosenViews = [];
      uiStateSettled = true;
      heldPlace = null;
    },
    chosenViewsNow,
    chosenView: (channel) => chosenViewsNow().find((entry) => entry.channel === String(channel)),
    remember(entry) {
      chosenViews = [entry, ...chosenViewsNow().filter((held) => held.channel !== entry.channel)]
        .slice(0, UI_STATE_CHANNELS);
    },
    forgetChosen(gone) {
      chosenViews = chosenViewsNow().filter((entry) => !gone(entry.channel));
    },
    chosenViewsHeld: () => chosenViews,
    setChosen(channels) {
      chosenViews = channels;
    },
  };
}

// What the page and the tests use of it.
export { REOPENED_VIEWS, UI_STATE_CHANNELS, UI_STATE_CHARS, UI_STATE_KEY, UI_STATE_VERSION, keptChoices, validUiState };
