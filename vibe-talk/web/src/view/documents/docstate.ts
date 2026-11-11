// Per-document state (`#1265 documents-star-hide`): which documents the owner has
// starred and which they have hidden from the list. It lives where per-item state
// already lives — one synced setting (`#457 settings-server-side`, as a thread's
// card choice does in `present/rows/packing.ts`): a JSON map in device storage
// under `DOC_STATES_KEY`, mirrored to the server by the settings sync, so phone
// and desktop converge on the server's copy with no new API. This module owns the
// value's shape only: parsing, asking, toggling and keeping it bounded. Storage
// itself is handed in, so the shape is testable without a device.

/** The synced setting's key: on `SYNCED_KEYS` server-side and the page's list too. */
export const DOC_STATES_KEY = "vibe-talk.voice.doc-states";

/** How many documents' state is kept: the most recently changed, far beyond a list's window. */
export const DOC_STATES_KEPT = 500;

/** One document's state: starred, hidden, or (where neither) not in the map at all. */
export interface DocState {
  readonly starred: boolean;
  readonly hidden: boolean;
}

/** Every document's state, by canonical key, most recently changed last. */
export type DocStates = ReadonlyMap<string, DocState>;

/** The storage the state is kept in: the device's, as the page hands it. */
export interface DocStateStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

/** Parse a kept value into states; anything unreadable or flagless is no state. */
export function parseDocStates(raw: string | null): Map<string, DocState> {
  const states = new Map<string, DocState>();
  if (!raw) return states;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return states;
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return states;
  for (const [key, flags] of Object.entries(parsed as Record<string, unknown>)) {
    if (!flags || typeof flags !== "object" || Array.isArray(flags)) continue;
    const record = flags as { s?: unknown; h?: unknown };
    const state = { starred: record.s === 1, hidden: record.h === 1 };
    if (state.starred || state.hidden) states.set(key, state);
  }
  return states;
}

/** The kept form of `states`: only the flags each document actually carries. */
export function serializeDocStates(states: DocStates): string {
  const out: Record<string, { s?: 1; h?: 1 }> = {};
  for (const [key, state] of states) {
    if (!state.starred && !state.hidden) continue;
    out[key] = { ...(state.starred ? { s: 1 as const } : {}), ...(state.hidden ? { h: 1 as const } : {}) };
  }
  return JSON.stringify(out);
}

/** Document `key`'s state in `states`, or neither flag where it has none. */
export function docStateOf(states: DocStates, key: string): DocState {
  return states.get(key) ?? { starred: false, hidden: false };
}

/**
 * `states` with document `key`'s one flag set as `on`, most recently changed:
 * the entry moves to the end, an entry left with neither flag is dropped, and
 * the map is cut to the newest `DOC_STATES_KEPT` so the synced value stays
 * well under the server's size limit.
 */
export function withDocFlag(
  states: DocStates,
  key: string,
  flag: "starred" | "hidden",
  on: boolean,
): Map<string, DocState> {
  const next = new Map(states);
  const current = docStateOf(next, key);
  const updated = { ...current, [flag]: on };
  next.delete(key);
  if (updated.starred || updated.hidden) next.set(key, updated);
  while (next.size > DOC_STATES_KEPT) {
    const oldest = next.keys().next().value as string;
    next.delete(oldest);
  }
  return next;
}

/** The live store over `storage`: every read is the kept value, every toggle writes it back. */
export interface DocStatesStore {
  /** The states as kept now. */
  states(): Map<string, DocState>;
  /** Star or unstar document `key`. */
  setStarred(key: string, on: boolean): void;
  /** Hide or unhide document `key`. */
  setHidden(key: string, on: boolean): void;
  /** Read again from storage, as a synced value from another device lands. */
  reload(): Map<string, DocState>;
}

/** A store over `storage`, or over memory alone where no storage is handed in. */
export function createDocStatesStore(storage?: DocStateStorage | null): DocStatesStore {
  let memory = new Map<string, DocState>();
  const read = (): Map<string, DocState> => {
    if (!storage) return new Map(memory);
    try {
      return parseDocStates(storage.getItem(DOC_STATES_KEY));
    } catch {
      return new Map(memory);
    }
  };
  const write = (states: Map<string, DocState>): void => {
    memory = new Map(states);
    if (!storage) return;
    try {
      storage.setItem(DOC_STATES_KEY, serializeDocStates(states));
    } catch {
      // Storage refused: the in-memory copy still serves this session.
    }
  };
  return {
    states: read,
    setStarred(key, on) {
      write(withDocFlag(read(), key, "starred", on));
    },
    setHidden(key, on) {
      write(withDocFlag(read(), key, "hidden", on));
    },
    reload: read,
  };
}
