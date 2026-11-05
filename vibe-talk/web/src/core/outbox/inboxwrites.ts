// Done and its put back, kept on the device until the server has them (`#892 voice-page-optimistic-done`).
// The owner: "Shouldn't that be a completely local operation with no network round trip? This comes back to
// our model of local state plus synchronization." A Done is drawn the moment it is marked; its write goes
// behind it. A write the server never heard — offline, a dropped connection, a proxy answering for a
// restarting server — goes again by itself, in order, for as long as the page is open, and is kept on the
// device so a page closed meanwhile sends it when it next opens. A write the server answered and refused is
// the page's to take back, as it always was.
//
// Sending a Done or a put back again is safe where sending a message is not: the server keeps a set of
// messages Done, and a second write of the same act leaves it where the first did.

import type { StoragePort } from "../../ports/index.ts";
import { storedExactly as storedExactlyIn } from "./storage.ts";

/** Where the device keeps the writes not yet answered, oldest first. */
const INBOX_WRITES_KEY = "vibe-talk.inbox-writes";

/**
 * How old a write kept on the device may be and still be sent when the page opens again, in ms. Older, the
 * other devices have had their say since, and a write from then could undo what was decided there.
 */
const INBOX_WRITE_MAX_AGE_MS = 24 * 60 * 60 * 1000;

/** The waits between attempts of one write: 1 s, doubling, never more than 30 s. */
const INBOX_RETRY_FIRST_MS = 1000;
const INBOX_RETRY_MAX_MS = 30000;

/** Which write: a Done, or a put back. */
export type InboxWriteKind = "dismiss" | "restore";

/** One write not yet answered by the server. */
export interface PendingInboxWrite {
  /** Its name, unique on the device. */
  readonly id: string;
  readonly kind: InboxWriteKind;
  readonly channel: string;
  /** The messages it names, as the server is sent them. */
  readonly messages: readonly string[];
  /** When it was made, in ms. */
  readonly at: number;
  /** Whose it is: the fingerprint of the credential it was made under, never the credential. */
  readonly owner: string;
  /** The page keeping it: its own until it is answered, or until that page has gone (`takeLeftOver`). */
  readonly page: string;
}

/**
 * How long a page may go without a sign of life before the writes it keeps are another page's to send, in ms.
 * A page sending a write signs at every attempt (`beat`), which is at most `INBOX_RETRY_MAX_MS` apart.
 */
const INBOX_PAGE_GONE_MS = 3 * INBOX_RETRY_MAX_MS;

export interface InboxWriteLedgerDependencies {
  readonly storage: StoragePort;
  /** The wall clock, in ms. */
  now(): number;
  /** The device's quota is full: give up the saved channel rows first (`dropMessageCache`). */
  makeRoom(): void;
  /** This page's name among the pages open on the device: one per page load. */
  readonly page: string;
}

/**
 * The writes kept on the device. Several pages may be open on one device, sharing it: each keeps its own
 * writes and leaves the others' alone, and takes another's only once that page has gone quiet.
 */
export interface InboxWriteLedger {
  /** Keep a write made at `at` (by default now), on the device at once. Answers its id. */
  add(kind: InboxWriteKind, channel: string, messages: readonly string[], owner: string, at?: number): string;
  /** The write `id` has been answered, taken or refused: no longer kept. */
  settle(id: string): void;
  /** A sign that this page is still here, and still sending what it keeps. */
  beat(): void;
  /**
   * This page is going (`pagehide`): what it keeps is the next page's to send at once, without waiting for it
   * to go quiet. A page that comes back (`beat`, `add`) keeps its own again.
   */
  leave(): void;
  /**
   * The writes kept by pages that went without answers and have gone quiet since (`INBOX_PAGE_GONE_MS`), oldest
   * first: `owner`'s, and none older than `INBOX_WRITE_MAX_AGE_MS`; the rest of theirs are forgotten. They are
   * handed over once, and the caller keeps each again (`add`) as it sends it. A page still here keeps its own.
   */
  takeLeftOver(owner: string): PendingInboxWrite[];
  /** Forget every write: another credential, whose writes these are not. */
  clear(): void;
  /** How many writes this page keeps now. */
  size(): number;
}

const KINDS: readonly InboxWriteKind[] = ["dismiss", "restore"];

/** What the device holds: every page's writes, and when each page last gave a sign of life. */
interface Kept {
  writes: PendingInboxWrite[];
  beats: Record<string, number>;
}

/** What the device held; anything that is not a write is left out. */
function decoded(raw: string | null): Kept {
  const kept: Kept = { writes: [], beats: {} };
  try {
    const saved: unknown = JSON.parse(raw || "{}");
    if (!saved || typeof saved !== "object") return kept;
    const { writes, beats } = saved as Record<string, unknown>;
    if (beats && typeof beats === "object") {
      for (const [page, at] of Object.entries(beats as Record<string, unknown>)) if (typeof at === "number") kept.beats[page] = at;
    }
    if (!Array.isArray(writes)) return kept;
    kept.writes = writes.flatMap((one): PendingInboxWrite[] => {
      if (!one || typeof one !== "object") return [];
      const { id, kind, channel, messages, at, owner, page } = one as Record<string, unknown>;
      if (typeof id !== "string" || !KINDS.includes(kind as InboxWriteKind) || typeof channel !== "string" ||
          !Array.isArray(messages) || messages.length === 0 || typeof at !== "number" || typeof owner !== "string" ||
          typeof page !== "string") return [];
      return [{ id, kind: kind as InboxWriteKind, channel, messages: messages.map(String), at, owner, page }];
    });
  } catch (_error) {
    // Storage may be absent or hold an interrupted write.
  }
  return kept;
}

/** The ledger: this page keeping nothing yet. */
export function createInboxWriteLedger(deps: InboxWriteLedgerDependencies): InboxWriteLedger {
  let held: PendingInboxWrite[] = [];
  let made = 0;
  let left = false;
  const read = (): Kept => {
    try {
      return decoded(deps.storage.getItem(INBOX_WRITES_KEY));
    } catch (_error) {
      return { writes: [], beats: {} };
    }
  };
  /** Write this page's part over what the device holds now, every other page's part left as it is. */
  const save = (others: (kept: Kept) => PendingInboxWrite[] = (kept) => kept.writes.filter((one) => one.page !== deps.page)): void => {
    const kept = read();
    const writes = [...others(kept), ...held];
    const beats: Record<string, number> = {};
    for (const one of writes) beats[one.page] = kept.beats[one.page] ?? 0;
    if (held.length > 0) beats[deps.page] = left ? 0 : deps.now();
    const encoded = JSON.stringify({ writes, beats });
    if (storedExactlyIn(deps.storage, INBOX_WRITES_KEY, encoded)) return;
    deps.makeRoom();
    storedExactlyIn(deps.storage, INBOX_WRITES_KEY, encoded);
  };
  return {
    add(kind, channel, messages, owner, at = deps.now()) {
      left = false;
      made += 1;
      const id = `${deps.page}-${made}`;
      held.push({ id, kind, channel: String(channel), messages: messages.map(String), at, owner, page: deps.page });
      save();
      return id;
    },
    settle(id) {
      const before = held.length;
      held = held.filter((one) => one.id !== id);
      if (held.length !== before) save();
    },
    beat() {
      left = false;
      if (held.length > 0) save();
    },
    leave() {
      left = true;
      if (held.length > 0) save();
    },
    takeLeftOver(owner) {
      const now = deps.now();
      const gone = (kept: Kept, one: PendingInboxWrite): boolean =>
        one.page !== deps.page && now - (kept.beats[one.page] ?? 0) > INBOX_PAGE_GONE_MS;
      let taken: PendingInboxWrite[] = [];
      save((kept) => {
        taken = kept.writes.filter((one) => gone(kept, one) && one.owner === owner && now - one.at <= INBOX_WRITE_MAX_AGE_MS);
        return kept.writes.filter((one) => one.page !== deps.page && !gone(kept, one));
      });
      return taken;
    },
    clear() {
      held = [];
      save(() => []);
    },
    size: () => held.length,
  };
}

/** What a failed write's error says of itself, as the page's api client makes one. */
export interface WriteFailure {
  /** The request never reached the server: offline, or the connection dropped. */
  readonly network?: boolean;
  /** The answer was not the server's own (a proxy's page) and had this status. */
  readonly gatewayStatus?: number;
  readonly status?: number;
}

/**
 * Whether a write that failed this way may simply go again: the server never answered it, or said it could
 * not then (408, 429, 503, 504). Any other answer is the server's decision, which the page takes back.
 */
export function transientWriteFailure(error: WriteFailure | null | undefined): boolean {
  if (!error) return false;
  if (error.network === true || (Number(error.gatewayStatus) || 0) > 0) return true;
  return [408, 429, 503, 504].includes(Number(error.status) || 0);
}

/** The wait before attempt `attempt` + 1 of a write (`attempt` from 1), in ms. */
export function inboxRetryWait(attempt: number): number {
  return Math.min(INBOX_RETRY_MAX_MS, INBOX_RETRY_FIRST_MS * 2 ** Math.max(0, attempt - 1));
}

export interface RetryDependencies {
  /** Wait `ms`; the page's clock. */
  wait(ms: number): Promise<void>;
  /** Attempt `attempt` failed with `error`, which goes again; said quietly, once or as often as it likes. */
  waiting(error: unknown, attempt: number): void;
}

/**
 * `send`, again after each transient failure (`transientWriteFailure`) until it is answered: its answer, or the
 * server's refusal thrown. Never gives up on its own: the write is the reader's act, drawn already, and the
 * writes behind it wait in order behind it.
 */
export async function retryingWrite<T>(send: () => Promise<T>, deps: RetryDependencies): Promise<T> {
  for (let attempt = 1; ; attempt += 1) {
    try {
      return await send();
    } catch (error) {
      if (!transientWriteFailure(error as WriteFailure)) throw error;
      deps.waiting(error, attempt);
      await deps.wait(inboxRetryWait(attempt));
    }
  }
}

export { INBOX_PAGE_GONE_MS, INBOX_RETRY_FIRST_MS, INBOX_RETRY_MAX_MS, INBOX_WRITES_KEY, INBOX_WRITE_MAX_AGE_MS };
