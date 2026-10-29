// Settings kept on the server (`#457 settings-server-side`; `#624 voice-page-modularity`, moved
// from the legacy script): the reconciler between this device's replica and the server's document.
//
// HOW IT SYNCS, in the replication model's terms (`ai_docs/transient/2026-10-23-replication-model.md`):
// the device's storage stays this device's replica — every reader and writer of a setting is
// unchanged, which is what keeps a setting working offline and at first paint. This is the update
// detector and reconciler between that replica and the server: the archive is the state both sides
// last agreed on, a short timer compares the replica against it and pushes what moved, and what
// another device moved arrives on the live streams as a `settings` event (and by re-read on focus
// and reconnect). Per-key last-writer-wins on the SERVER's stamp: a remote entry is applied only
// when its `updated_ms` is newer than the archive's, and a local change still on its way is left
// to land after it, which is the same rule the server applied in arrival order. First load after
// this shipped seeds the server from this device's values where it holds none — and only there, so
// the second device to load can never clobber the first one's choices with its stale copies. A
// failed push keeps the local value, says so quietly, and retries on the timer, because the replica
// still differs from the archive until the server takes it.

import { is } from "../../contract/index.ts";
import type { ClientSettings, Contract } from "../../contract/index.ts";
import type { ClockPort, StoragePort } from "../../ports/index.ts";
import { IDENTITY_KEY, SYNCED_SETTING_KEYS } from "./keys.ts";

/** How often the replica is compared with the archive. A settings write is a human act; a second
 * and a half is prompt, and far cheaper than asking on every keystroke of a dragged slider. */
const SETTINGS_SYNC_MS = 1750;

/** The largest value the server keeps for one setting, in characters to its bytes: the
 * server's `MAX_VALUE_BYTES`, mirrored so an oversized value is left local here rather
 * than refused there in a batch with everything else. */
const SETTINGS_VALUE_MAX = 64 * 1024;

/** Passes between whole re-reads of the document while nothing local moved: about a
 * minute at `SETTINGS_SYNC_MS`, the backstop behind the stream. */
const SETTINGS_REREAD_PASSES = 34;

/** What this page and the server last agreed for one key: the value (null for none held) and the
 * server's stamp for it. */
export interface AgreedSetting {
  readonly value: string | null;
  readonly updatedMs: number;
}

/** One entry the server holds, as its answers and stream frames carry it. */
type ServerEntry = ClientSettings["settings"][number];

/** What the reconciler asks of the page. */
export interface SettingsSyncDependencies {
  /** The device's storage: the replica. */
  readonly storage: StoragePort;
  readonly clock: ClockPort;
  /** A request to the server, decoded against the contract (web/src/core/api `decoded`). */
  decoded<K extends keyof Contract>(
    type: K,
    path: string,
    options?: { method?: string; body?: unknown },
  ): Promise<Contract[K]>;
  /** The token held now, or "" for none. */
  token(): string;
  /** What the token may do, as the server last said: "read", "write", or null for unknown. */
  tokenScope(): string | null;
  /**
   * Put one synced key's kept value into force, as its own control would. Called only after the
   * replica holds the new value.
   */
  apply(key: string): void;
  /** Settings' quiet word about the sync: "" while it is keeping up. */
  status(text: string): void;
  /** The server's settings were read or written: after every read, write and stream frame. */
  landed(): void;
}

/** The reconciler. */
export interface SettingsSync {
  /** What this page and the server last agreed for `key`, if anything. */
  agreed(key: string): AgreedSetting | undefined;
  /** This device's kept value for a synced key, or null where it holds none (or storage is refused). */
  local(key: string): string | null;
  /** Whether the server keeps settings, as its client-config said. */
  setSupported(supported: boolean): void;
  /** Sign-out: the next credential is unproven until proven, and the archive was agreed under this one. */
  forget(): void;
  /**
   * The token is proven: start keeping the replica and the server's document in agreement. True
   * the first time, when the caller makes the first pass and arms the timer (`keepPassing`); on
   * every later call, a pass is made here and false returned.
   */
  start(): boolean;
  /** Arm the self-rescheduling timer: a pass every `SETTINGS_SYNC_MS` after the last ended. */
  keepPassing(): void;
  /** One reconciliation pass. Failures keep every local value and are retried by the timer. */
  syncOnce(): Promise<void>;
  /** Re-read the document, for the moments a missed stream frame could hide in: refocus, reconnect. */
  refresh(): Promise<void>;
  /** One `settings` frame off either live stream. Unreadable frames are dropped. */
  onFrame(frame: { readonly data?: string | null | undefined }): void;
}

/** A reconciler over `deps`, holding nothing agreed yet. */
export function createSettingsSync(deps: SettingsSyncDependencies): SettingsSync {
  /**
   * What this page and the server last agreed, by key. The whole sync is the difference between
   * this and the replica.
   */
  const syncedArchive = new Map<string, AgreedSetting>();

  /** Whether the first read of the server's document has landed: until it has, nothing is pushed,
   * so a load cannot overwrite another device's newer choice with this one's not-yet-checked copy. */
  let settingsSyncReady = false;

  /** Whether a push or read is on the wire now, so the timer never stacks a second one on it. */
  let settingsSyncBusy = false;

  /** False once the server has said it has no such route or no store: settings stay local, unsynced
   * and unretried, rather than asking a question with a permanent answer every second and a half. */
  let settingsSyncSupported = false;

  /** Whether the sync's timer is armed: armed once, by the first proven sign-in. */
  let settingsSyncStarted = false;

  /** Whether the token now held has been proved by a `/client-config` answer. Nothing reads with
   * an unproven token: the sync waits behind the same proof, and signing out un-proves it. */
  let settingsSyncProven = false;

  /**
   * This device's values as the sync first saw them, taken in `start` before the first read goes
   * out. A key that moves after that and before the read lands is a change in flight, protected
   * like any other; one that never moved is a migration candidate, and the server's held value
   * rightly wins over it.
   */
  const settingsAtStart = new Map<string, string | null>();

  /** Reconciliation passes since the document was last re-read whole: a missed stream frame
   * (a lagged settings receiver is skipped, never reset) is healed by a slow re-read, not left
   * to whenever the reader next refocuses the page. */
  let settingsPassesSinceRead = 0;

  /** This device's kept value for a synced key, or null where it holds none. */
  function localSettingValue(key: string): string | null {
    try {
      return deps.storage.getItem(key);
    } catch (_error) {
      return null;
    }
  }

  /** Keep a synced value on this device, or forget it, exactly as the setting's own writer would. */
  function keepLocalSetting(key: string, value: string | null): void {
    try {
      if (value === null) deps.storage.removeItem(key);
      else deps.storage.setItem(key, value);
    } catch (_error) {
      // A browser that refuses storage honours the applied value for this session only.
    }
  }

  /**
   * After a remote value is applied, the archive adopts what this page actually kept. Several
   * settings canonicalize as they apply — a clamped pace, an order this page no longer knows
   * read as the default, a read-aloud voice this device cannot offer kept as its own — and
   * the canonical form is THIS page's reading, not a new choice: pushing it back would move
   * every other device's setting merely because this one opened. A forget is kept a forget:
   * an apply that writes its default back does not resurrect the key.
   */
  function adoptAppliedSetting(key: string, value: string | null, updatedMs: number): void {
    if (value === null) {
      keepLocalSetting(key, null);
      syncedArchive.set(key, { value: null, updatedMs });
      return;
    }
    syncedArchive.set(key, { value: localSettingValue(key), updatedMs });
  }

  /**
   * Take entries the server holds — a read's whole document, a write's answer, a stream frame's
   * changes — into the archive, and into this device where they are newer than what it agreed.
   * With `whole`, a synced key absent from the entries is one the server does not hold.
   */
  function takeServerSettings(entries: readonly ServerEntry[], whole: boolean): void {
    const seen = new Set<string>();
    for (const entry of entries) {
      if (!SYNCED_SETTING_KEYS.includes(entry.key)) continue;
      seen.add(entry.key);
      const agreed = syncedArchive.get(entry.key);
      if (agreed && entry.updated_ms < agreed.updatedMs) continue;
      const local = localSettingValue(entry.key);
      // A local change the server has not taken yet stands: it lands after this entry and wins
      // by the same arrival rule, and taking the remote value now would lose it silently. On
      // the first read there is no agreement yet, so "changed" is against the snapshot.
      const pendingLocal = agreed
        ? local !== agreed.value
        : settingsAtStart.has(entry.key) && local !== settingsAtStart.get(entry.key);
      syncedArchive.set(entry.key, { value: entry.value, updatedMs: entry.updated_ms });
      if (pendingLocal || local === entry.value) continue;
      // `#488 slack-app-agents`. Who is who was kept per device until it synced, so the first read
      // finds each device with choices of its own: merged, the server's winning an account both name,
      // and the merge pushed by the next pass (the archive keeps the server's value, which differs).
      if (!agreed && entry.key === IDENTITY_KEY && local !== null && entry.value !== null) {
        const merged = mergedIdentities(local, entry.value);
        if (merged !== null && merged !== entry.value) {
          keepLocalSetting(entry.key, merged);
          deps.apply(entry.key);
          continue;
        }
      }
      keepLocalSetting(entry.key, entry.value);
      deps.apply(entry.key);
      adoptAppliedSetting(entry.key, entry.value, entry.updated_ms);
    }
    if (whole) {
      settingsPassesSinceRead = 0;
      for (const key of SYNCED_SETTING_KEYS) {
        if (seen.has(key)) continue;
        // Absent from the whole document: the server does not hold it — never set, or
        // forgotten, or purged. A key this page agreed a value for is forgotten here too,
        // which is also what stops the page that forgot it asking again every pass.
        const agreed = syncedArchive.get(key);
        if (!agreed) {
          // No agreement yet: this is the first read, and a local value here is the
          // migration's seed, not something a forget is clearing.
          syncedArchive.set(key, { value: null, updatedMs: 0 });
          continue;
        }
        const local = localSettingValue(key);
        const pendingLocal = local !== agreed.value;
        syncedArchive.set(key, { value: null, updatedMs: agreed.updatedMs });
        if (pendingLocal || local === null) continue;
        keepLocalSetting(key, null);
        deps.apply(key);
        adoptAppliedSetting(key, null, agreed.updatedMs);
      }
    }
    deps.landed();
  }

  async function syncSettingsOnce(): Promise<void> {
    if (settingsSyncBusy || !settingsSyncSupported || !settingsSyncProven || !deps.token()) return;
    settingsSyncBusy = true;
    try {
      if (!settingsSyncReady) {
        const held = await deps.decoded("ClientSettings", "/api/v1/client-settings");
        takeServerSettings(held.settings, true);
        settingsSyncReady = true;
        // The one-time migration: this device's values, offered only where the server holds
        // none. Seeds ride the first push below as their own list, never as sets.
        const seed: ServerEntry[] = [];
        for (const key of SYNCED_SETTING_KEYS) {
          const agreed = syncedArchive.get(key);
          const local = localSettingValue(key);
          if (agreed && agreed.value === null && local !== null) {
            seed.push({ key, value: local, updated_ms: 0 });
          }
        }
        // A read-scope token reads the document and applies it, and writes nothing:
        // asking would only be refused, on every pass, beside work it may do.
        if (seed.length > 0 && deps.tokenScope() !== "read") {
          const answer = await deps.decoded("ClientSettings", "/api/v1/client-settings", {
            method: "PUT",
            body: { set: [], seed },
          });
          // The whole document back: a seed another device beat this one to arrives here as
          // that device's value, and replaces the stale local copy it outraced.
          takeServerSettings(answer.settings, true);
        }
        deps.status("");
        return;
      }
      const set: ServerEntry[] = [];
      let tooLarge = false;
      for (const key of SYNCED_SETTING_KEYS) {
        const agreed = syncedArchive.get(key);
        const local = localSettingValue(key);
        if (!agreed || local !== agreed.value) {
          // One value over the server's limit refused whole would stop every other
          // setting syncing behind it: it stays local, the rest still go.
          if (local !== null && local.length > SETTINGS_VALUE_MAX) {
            tooLarge = true;
            continue;
          }
          set.push({ key, value: local, updated_ms: 0 });
        }
      }
      if (deps.tokenScope() === "read") {
        if (set.length > 0) {
          deps.status("This token may only read, so settings stay on this device.");
        }
        return;
      }
      if (set.length === 0) {
        settingsPassesSinceRead += 1;
        if (settingsPassesSinceRead >= SETTINGS_REREAD_PASSES) {
          const held = await deps.decoded("ClientSettings", "/api/v1/client-settings");
          takeServerSettings(held.settings, true);
        }
        deps.status(tooLarge
          ? "One setting is too large for the server to keep, so it stays on this device."
          : "");
        return;
      }
      deps.status("Settings are being saved to the server…");
      const answer = await deps.decoded("ClientSettings", "/api/v1/client-settings", {
        method: "PUT",
        body: { set, seed: [] },
      });
      takeServerSettings(answer.settings, true);
      deps.status("");
    } catch (error) {
      if (settingsSyncRefused(error)) {
        settingsSyncSupported = false;
        deps.status(
          "This server does not keep settings, so yours stay on this device.");
        return;
      }
      deps.status(
        "Settings are kept on this device for now — saving to the server will retry.");
    } finally {
      settingsSyncBusy = false;
    }
  }

  async function refreshServerSettings(): Promise<void> {
    if (settingsSyncBusy || !settingsSyncSupported || !settingsSyncReady || !settingsSyncProven || !deps.token()) return;
    settingsSyncBusy = true;
    try {
      const held = await deps.decoded("ClientSettings", "/api/v1/client-settings");
      takeServerSettings(held.settings, true);
    } catch (_error) {
      // The timer's next pass and the stream both carry this; a failed re-read says nothing new.
    } finally {
      settingsSyncBusy = false;
    }
  }

  return {
    agreed: (key) => syncedArchive.get(key),
    local: localSettingValue,
    setSupported(supported) {
      settingsSyncSupported = supported;
    },
    forget() {
      settingsSyncProven = false;
      settingsSyncReady = false;
      settingsSyncSupported = false;
      syncedArchive.clear();
      settingsAtStart.clear();
    },
    start() {
      settingsSyncProven = true;
      if (settingsSyncStarted) {
        syncSettingsOnce().catch(() => {});
        return false;
      }
      settingsSyncStarted = true;
      for (const key of SYNCED_SETTING_KEYS) settingsAtStart.set(key, localSettingValue(key));
      return true;
    },
    keepPassing() {
      // Self-rescheduling rather than an interval: a slow pass can never stack a second one on itself.
      const tick = (): void => {
        syncSettingsOnce()
          .catch(() => {})
          .then(() => deps.clock.setTimeout(tick, SETTINGS_SYNC_MS));
      };
      deps.clock.setTimeout(tick, SETTINGS_SYNC_MS);
    },
    syncOnce: syncSettingsOnce,
    refresh: refreshServerSettings,
    onFrame(frame) {
      if (!frame.data) return;
      let payload: unknown = null;
      try {
        payload = JSON.parse(frame.data);
      } catch (_error) {
        return;
      }
      if (!is("ClientSettings", payload)) return;
      takeServerSettings(payload.settings, false);
    },
  };
}

/**
 * `#488 slack-app-agents`. Two devices' who-is-who choices as one, the server's winning an account
 * both name, as the stored JSON; null when either is not a map this page can read.
 */
export function mergedIdentities(local: string, server: string): string | null {
  try {
    const mine: unknown = JSON.parse(local);
    const theirs: unknown = JSON.parse(server);
    const map = (value: unknown): value is Record<string, unknown> =>
      !!value && typeof value === "object" && !Array.isArray(value);
    if (!map(mine) || !map(theirs)) return null;
    return JSON.stringify({ ...mine, ...theirs });
  } catch (_error) {
    return null;
  }
}

/** Whether this refusal is the permanent kind: no such route, or no store to keep settings in. */
function settingsSyncRefused(error: unknown): boolean {
  // No route, no store, a token that may only read, or a write the server will never
  // take as written: asking again in two seconds gets the same answer forever.
  const failed = error as { status?: unknown; code?: unknown } | null | undefined;
  return !!failed && (failed.status === 404 || failed.status === 400 || failed.status === 403 ||
    failed.code === "storage_not_configured");
}
