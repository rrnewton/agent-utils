// Synced composer drafts (`#1840 vibe-talk-draft-sync-across-devices`): the
// owner's unsent words, kept on their own server so another device can offer
// them. Device storage stays the offline source of truth (see drafts.ts);
// this is only the rendezvous. The clock and the timer are handed in, so a
// test drives the idle sync with no wall-clock.

/** Which composer a synced draft belongs to. */
export type SyncedDraftScope = "main" | "discuss";

/** One draft's address: a channel, its thread ("" is the channel), a composer. */
export interface SyncedDraftKey {
  readonly channel: string;
  readonly thread: string;
  readonly scope: SyncedDraftScope;
}

/** One synced draft, as the server holds it. */
export interface SyncedDraft {
  readonly channel_id: string;
  readonly thread_id: string;
  readonly scope: SyncedDraftScope;
  readonly text: string;
  readonly device_id: string;
  readonly device_label: string;
  readonly updated_ms: number;
  readonly version: number;
}

/** The server side of the sync, as the page's API client provides it. */
export interface DraftSyncTransport {
  getDrafts(channel: string): Promise<{ drafts: SyncedDraft[] }>;
  putDraft(
    channel: string,
    body: {
      thread_id?: string;
      scope: SyncedDraftScope;
      text: string;
      device_id: string;
      device_label: string;
      base_version?: number;
    },
  ): Promise<{ draft: SyncedDraft | null; replaced: SyncedDraft | null }>;
  deleteDraft(
    channel: string,
    body: { thread_id?: string; scope: SyncedDraftScope },
  ): Promise<unknown>;
}

/** What the sync asks of the page. */
export interface DraftSyncDependencies {
  readonly transport: DraftSyncTransport;
  deviceId(): string;
  deviceLabel(): string;
  /** Whether idle typing syncs by itself; the Save button works either way. */
  autosync(): boolean;
  schedule(run: () => void, ms: number): unknown;
  cancel(handle: unknown): void;
  /** A sync landed; `replaced` names the different words it replaced, if any. */
  onSaved(key: SyncedDraftKey, replaced: SyncedDraft | null): void;
}

/** How long typing must sit idle before it syncs by itself. */
export const DRAFT_AUTOSYNC_IDLE_MS = 3000;

/** What a fetched draft means for the box that would show it. */
export type DraftOffer =
  | { readonly kind: "none" }
  | { readonly kind: "adopt"; readonly draft: SyncedDraft }
  | { readonly kind: "offer"; readonly draft: SyncedDraft };

/**
 * Decide what a remote draft means beside the words already in the box:
 * nothing when it is this device's own echo or the same words, an adoption
 * when the box is empty, and an offer — never an overwrite — otherwise.
 */
export function decideDraftOffer(
  remote: SyncedDraft | null,
  localText: string,
  ownDeviceId: string,
): DraftOffer {
  if (!remote || !remote.text.trim()) return { kind: "none" };
  if (remote.text === localText) return { kind: "none" };
  if (remote.device_id === ownDeviceId) return { kind: "none" };
  if (!localText.trim()) return { kind: "adopt", draft: remote };
  return { kind: "offer", draft: remote };
}

/** The composer drafts' cross-device sync. */
export interface DraftSync {
  /** Typing happened: remember the latest words and, when allowed, idle-sync. */
  noteTyped(key: SyncedDraftKey, text: string): void;
  /** The Save draft button: sync these words now, whatever the setting. */
  saveNow(key: SyncedDraftKey, text: string): Promise<void>;
  /** The words went out or were discarded: forget them everywhere. */
  clear(key: SyncedDraftKey): Promise<void>;
  /** Every live draft the server holds for a channel. */
  fetchChannel(channel: string): Promise<SyncedDraft[]>;
  /** Retry what a failed sync kept, latest words per scope only. */
  flushPending(): Promise<void>;
}

/** The sync over `deps`; one pending latest text per scope, nothing more. */
export function createDraftSync(deps: DraftSyncDependencies): DraftSync {
  const pending = new Map<string, { key: SyncedDraftKey; text: string }>();
  const timers = new Map<string, unknown>();
  const idOf = (key: SyncedDraftKey): string =>
    `${key.channel}\n${key.thread}\n${key.scope}`;

  function cancelTimer(id: string): void {
    const handle = timers.get(id);
    if (handle !== undefined) {
      deps.cancel(handle);
      timers.delete(id);
    }
  }

  async function sync(key: SyncedDraftKey, text: string): Promise<void> {
    const id = idOf(key);
    try {
      if (!text.trim()) {
        await deps.transport.deleteDraft(key.channel, {
          thread_id: key.thread || undefined,
          scope: key.scope,
        });
        pending.delete(id);
        return;
      }
      const answer = await deps.transport.putDraft(key.channel, {
        thread_id: key.thread || undefined,
        scope: key.scope,
        text,
        device_id: deps.deviceId(),
        device_label: deps.deviceLabel(),
      });
      // A newer keystroke during the flight keeps its own pending words.
      if (pending.get(id)?.text === text) pending.delete(id);
      deps.onSaved(key, answer.replaced);
    } catch {
      // Offline or refused: the device copy is the draft; retry later.
      pending.set(id, { key, text });
    }
  }

  return {
    noteTyped(key, text) {
      const id = idOf(key);
      pending.set(id, { key, text });
      cancelTimer(id);
      if (!deps.autosync()) return;
      timers.set(
        id,
        deps.schedule(() => {
          timers.delete(id);
          const held = pending.get(id);
          if (held) void sync(held.key, held.text);
        }, DRAFT_AUTOSYNC_IDLE_MS),
      );
    },
    saveNow(key, text) {
      cancelTimer(idOf(key));
      pending.set(idOf(key), { key, text });
      return sync(key, text);
    },
    async clear(key) {
      const id = idOf(key);
      cancelTimer(id);
      pending.delete(id);
      try {
        await deps.transport.deleteDraft(key.channel, {
          thread_id: key.thread || undefined,
          scope: key.scope,
        });
      } catch {
        // Already gone locally; the server copy ages out on its own.
      }
    },
    async fetchChannel(channel) {
      const answer = await deps.transport.getDrafts(channel);
      return answer.drafts;
    },
    async flushPending() {
      for (const held of [...pending.values()]) {
        await sync(held.key, held.text);
      }
    },
  };
}

const DRAFT_DEVICE_KEY = "vibe-talk.draft-device";

/** This device's stable draft identity, kept in device storage. */
export function draftDeviceId(storage: {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}, freshId: () => string): string {
  try {
    const held = storage.getItem(DRAFT_DEVICE_KEY);
    if (held) return held;
    const made = freshId();
    storage.setItem(DRAFT_DEVICE_KEY, made);
    return made;
  } catch {
    return freshId();
  }
}

/** A coarse device label from a platform string, for the continue offer. */
export function draftDeviceLabel(platform: string): string {
  if (/iPhone|Android.*Mobile|Mobile/i.test(platform)) return "phone";
  if (/iPad|Tablet/i.test(platform)) return "tablet";
  return "laptop";
}

/** "3 min ago" beside a draft's server stamp, for the continue offer. */
export function draftAgeWords(updatedMs: number, nowMs: number): string {
  const minutes = Math.max(0, Math.round((nowMs - updatedMs) / 60000));
  if (minutes < 1) return "just now";
  if (minutes === 1) return "1 min ago";
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  return hours === 1 ? "1 hr ago" : `${hours} hr ago`;
}
