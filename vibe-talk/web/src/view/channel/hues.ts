// The colour each thread is drawn in (`#396 thread-color-memory`, `#414 thread-hue-spread`, `#427
// server-thread-hues`; `#624 voice-page-modularity`, moved from the legacy script into view/channel): the
// colours of the channel on screen, the server's colours for it and which of them only saved rows carried,
// the device's memory of each channel's colours with its bounds, a colour handed out for a thread the server
// has not coloured, and the one offer per load of the remembered colours to the server. How far apart hues
// stand and which remembered colours are kept are present/rows'; why the server's colour wins and the memory
// is bounded is told where the legacy script opens the section.

import type { StoragePort } from "../../ports/index.ts";
import { newestThreadHues, spreadThreadHue } from "../../present/rows/index.ts";

const THREAD_HUES_KEY = "vibe-talk.thread-hues";
const THREAD_HUES_VERSION = 1;
/**
 * Threads remembered per channel. With 24 the widest arc left is at least 15° wide, so a new thread
 * lands at least 7.5° from a remembered one; it is this cap that keeps that floor. A thread whose
 * neighbours rule that arc out (`#414 thread-hue-spread`) is re-rolled to the farthest colour that
 * clears them, which may be nearer a remembered thread drawn elsewhere.
 */
const THREAD_HUES_PER_CHANNEL = 24;
/** Threads remembered across every channel at once. */
const THREAD_HUES_TOTAL = 480;
/** How stale a thread's "last drawn" may get in a long-open page before it is stored again. */
const THREAD_HUES_SEEN_REFRESH_MS = 60 * 60 * 1000;
/** Where "this channel's colours were offered to the server" is remembered, by scope. */
const THREAD_HUES_SEEDED_KEY = "vibe-talk.thread-hues-seeded";
/** Threads either side of a new one, in the order their roots were posted, whose colours it avoids. */
const THREAD_HUE_NEIGHBOURS = 3;
/** Threads given a colour most recently on this page whose colours a new one avoids too. */
const THREAD_HUE_RECENT = 3;

/** A thread summary, as far as its colour reads it. */
interface HueSummary {
  readonly id: unknown;
  readonly hue?: unknown;
  readonly root?: { readonly timestamp?: unknown; readonly thread?: { readonly id: unknown; readonly hue?: unknown } | null } | null;
}

/** A message, as far as its thread's colour reads it. */
interface HueMessage {
  readonly thread?: { readonly id: unknown; readonly hue?: unknown; readonly is_root?: unknown } | null;
}

/** Every channel's remembered colours, as {scope: {thread id: [hue, lastDrawn]}}. */
type RememberedHues = Record<string, Record<string, [number, number]>>;

/** What the colours ask of the page. */
export interface ThreadHuesDependencies {
  /** The device's storage; any call may throw. */
  readonly storage: StoragePort;
  /** Now, in milliseconds. */
  now(): number;
  /** The channel the directory knows by `id`, or undefined. */
  channelRecord(id: string): { readonly provider?: unknown } | undefined;
  /** The channel chosen now, as a string ("" with none). */
  selected(): string;
  /** Whether the page holds the write token, which offering colours needs. */
  writeToken(): boolean;
  /** What the channel store holds now: its channel, thread summaries and messages. */
  held(): { readonly channel: unknown; readonly threads: readonly unknown[]; readonly messages: readonly unknown[] };
  /** A record's time (`timeOf(record, "timestamp")`). */
  timeOf(record: unknown): number;
  /** A message's thread id (`threadOf`), or null. */
  threadOf(message: unknown): string | null;
  /** Offer `hues` for channel `channel` to the server; answers its reply, rejects where it failed. */
  post(channel: string, hues: readonly object[]): Promise<unknown>;
}

/** The colours' commands and questions. */
export interface ThreadHues {
  /** The colour thread `id` is drawn in on the channel chosen: the server's, the remembered one, or a new one. */
  hue(id: unknown): number;
  /** Put back `channel`'s remembered colours, after storing what was drawn of the last one. */
  restore(channel: unknown): void;
  /** Take the server's colours off a batch of messages and thread summaries, roots included. */
  noteServerIn(messages: readonly unknown[] | null | undefined, summaries: readonly unknown[] | null | undefined,
    channel?: string, provisional?: boolean): void;
  /** Offer this browser's remembered colours for `channel` to the server, once a load. Never fails. */
  seed(channel: unknown): Promise<void>;
}

/** No channel's colours put back yet. */
export function createThreadHues(deps: ThreadHuesDependencies): ThreadHues {
  /** The colours of the channel `threadColorsChannel`, by thread id. */
  const threadColors = new Map<string, number>();
  /** The channel id `threadColors` holds the colours of, or null before any is drawn. */
  let threadColorsChannel: string | null = null;
  /** Threads drawn since that channel's colours were put back, and when each was last drawn. */
  const threadHuesSeen = new Map<string, number>();
  let threadHuesSaveOwed = false;
  /** The server's colours for the channel on screen, by thread id. */
  const serverThreadHues = new Map<string, number>();
  /** The channel id `serverThreadHues` holds the colours of, or null before any arrive. */
  let serverHuesChannel: string | null = null;
  /**
   * Colours in `serverThreadHues` that came only from this device's saved rows, not yet from the
   * server in this load. Provisional, because the saved copy is the one that can be stale: after
   * an operator purge the server colours afresh, and its first read must be able to replace what
   * a reload drew from its cache rather than be refused by it for the whole load.
   */
  const serverHuesProvisional = new Set<string>();
  /** The threads last given a colour on this page, oldest first. Cleared with `threadColors`. */
  const threadHuesGiven: string[] = [];
  /** Scopes offered, or being offered, in this page: one attempt per load, failure included. */
  const threadHuesSeedTried = new Set<string>();
  /**
   * The offer in flight, by scope. Both columns load a channel at once, and the second load must
   * wait for the first's offer rather than read past it — a read that reached the server first
   * would colour those threads itself, and the offer would arrive to find them decided.
   */
  const threadHuesSeeding = new Map<string, Promise<void>>();

  /** Where a channel's colours are kept: by provider as well, since two services may share an id. */
  function threadHueScope(channel: unknown): string {
    const known = deps.channelRecord(String(channel));
    return JSON.stringify([known && typeof known.provider === "string" ? known.provider : "", String(channel)]);
  }

  /**
   * Every channel's remembered colours. Storage is a boundary: anything unreadable is left out, and a
   * refusal to read at all is an empty memory.
   *
   * Null for a record another version of this page wrote, in another window or after a downgrade:
   * it is not this page's to read, nor to overwrite with what this page drew of one channel.
   */
  function readThreadHues(): RememberedHues | null {
    let stored = null;
    try {
      stored = JSON.parse(deps.storage.getItem(THREAD_HUES_KEY) || "null");
    } catch (_error) {
      return Object.create(null) as RememberedHues;
    }
    if (stored !== null && typeof stored === "object" && Number.isInteger(stored.v) &&
        stored.v !== THREAD_HUES_VERSION) return null;
    if (stored === null || typeof stored !== "object" || stored.v !== THREAD_HUES_VERSION ||
        stored.channels === null || typeof stored.channels !== "object" || Array.isArray(stored.channels)) {
      return Object.create(null) as RememberedHues;
    }
    const channels = Object.create(null) as RememberedHues;
    // A clock that ran ahead and was put back left times from the future, which would outrank every
    // thread drawn since and have the newest evicted first. None counts as later than now.
    const now = deps.now();
    for (const [scope, threads] of Object.entries(stored.channels as Record<string, unknown>)) {
      if (threads === null || typeof threads !== "object" || Array.isArray(threads)) continue;
      const kept = Object.entries(threads as Record<string, unknown>).filter(([_id, entry]) =>
        Array.isArray(entry) && entry.length === 2 && Number.isFinite(entry[0]) && entry[0] >= 0 &&
        entry[0] < 360 && Number.isFinite(entry[1])).map(([id, entry]) => {
        const [hue, drawn] = entry as [number, number];
        return [id, [hue, Math.min(drawn, now)]];
      });
      if (kept.length > 0) channels[scope] = newestThreadHues(kept, THREAD_HUES_PER_CHANNEL) as Record<string, [number, number]>;
    }
    return channels;
  }

  /**
   * Store what this page drew of its channel into the remembered colours, as they stand in storage
   * now — another window may have written its own since — trimmed to the bounds.
   */
  function saveThreadHues(): void {
    if (!threadHuesSaveOwed) return;
    threadHuesSaveOwed = false;
    if (!threadColorsChannel) return;
    const channels = readThreadHues();
    if (channels === null) return;
    const scope = threadHueScope(threadColorsChannel);
    const threads = new Map<string, [number, number]>(Object.entries(channels[scope] || {}));
    for (const [id, drawn] of threadHuesSeen) {
      if (threadColors.has(id)) threads.set(id, [threadColors.get(id) as number, drawn]);
    }
    // More threads drawn at once than a channel keeps all share one time, and the ones stored before
    // win the tie: a channel showing more than its cap at once recolours a few of them on a reload.
    channels[scope] = newestThreadHues([...threads], THREAD_HUES_PER_CHANNEL) as Record<string, [number, number]>;
    const every = Object.entries(channels).flatMap(([held, kept]) =>
      Object.entries(kept).map(([id, entry]) => [held, id, entry[1]] as [string, string, number]));
    if (every.length > THREAD_HUES_TOTAL) {
      every.sort((a, b) => b[2] - a[2]);
      for (const [held, id] of every.slice(THREAD_HUES_TOTAL)) {
        delete (channels[held] as Record<string, [number, number]>)[id];
        if (Object.keys(channels[held] as object).length === 0) delete channels[held];
      }
    }
    try {
      deps.storage.setItem(THREAD_HUES_KEY, JSON.stringify({ v: THREAD_HUES_VERSION, channels }));
    } catch (_error) {
      // Private browsing or a full quota: the colours hold for this page, as they always did.
    }
  }

  function restoreThreadHues(channel: unknown): void {
    saveThreadHues();
    // The server's colours for another channel are not this one's (`#427 server-thread-hues`) —
    // unless a read of THIS channel noted them already, which a step back folded before its first
    // row was drawn can have done: those stand.
    if (String(channel) !== serverHuesChannel) {
      serverThreadHues.clear();
      serverHuesProvisional.clear();
      serverHuesChannel = String(channel) || null;
    }
    // Already this channel's: what is on screen keeps its colours, remembered or not.
    if (String(channel) === threadColorsChannel) return;
    threadColors.clear();
    threadHuesSeen.clear();
    threadHuesGiven.length = 0;
    threadColorsChannel = String(channel);
    if (!threadColorsChannel) return;
    for (const [id, [hue]] of Object.entries((readThreadHues() || {})[threadHueScope(channel)] || {})) {
      threadColors.set(id, hue);
    }
    // The server's word over this browser's memory, where a read brought both.
    for (const [id, hue] of serverThreadHues) threadColors.set(id, hue);
  }

  /**
   * Take the server's colour for one thread. The first one taken from the server stands: it never
   * recolours, so a different second value from a read or the stream is not a newer truth but a
   * copy from somewhere stale, and believing it would recolour a thread on screen. A provisional
   * colour — one only this device's saved rows carried — yields to the first the server itself
   * sends in this load, for the purge reason above. Mirrored into `threadColors` at once, so a
   * fallback colour handed out for another thread is measured against it.
   */
  function noteServerThreadHue(id: unknown, hue: number, channel = deps.selected(), provisional = false): void {
    if (!channel || !Number.isFinite(hue) || hue < 0 || hue >= 360) return;
    if (channel !== serverHuesChannel) {
      serverThreadHues.clear();
      serverHuesProvisional.clear();
      serverHuesChannel = channel;
    }
    const key = String(id);
    if (!key) return;
    if (serverThreadHues.has(key) && (provisional || !serverHuesProvisional.has(key))) return;
    serverThreadHues.set(key, hue);
    if (provisional) serverHuesProvisional.add(key);
    else serverHuesProvisional.delete(key);
    if (channel === threadColorsChannel) threadColors.set(key, hue);
  }

  /** The scopes whose colours a server has taken, as {scope: when}. Unreadable is none taken. */
  function readSeededScopes(): Record<string, unknown> {
    try {
      const stored = JSON.parse(deps.storage.getItem(THREAD_HUES_SEEDED_KEY) || "null");
      if (stored && typeof stored === "object" && stored.v === 1 &&
          stored.scopes && typeof stored.scopes === "object" && !Array.isArray(stored.scopes)) {
        return stored.scopes;
      }
    } catch (_error) {
      // Storage refused: the offer is simply made again on a later load, and taken-or-not it
      // changes nothing already decided on the server.
    }
    return Object.create(null) as Record<string, unknown>;
  }

  function markSeededScope(scope: string): void {
    const scopes = readSeededScopes();
    scopes[scope] = deps.now();
    try {
      deps.storage.setItem(THREAD_HUES_SEEDED_KEY, JSON.stringify({ v: 1, scopes }));
    } catch (_error) {
      // As `saveThreadHues`: the session's own record (`threadHuesSeedTried`) still holds.
    }
  }

  /** The offer itself: one POST, its colours adopted. `seed` shares it. */
  async function offerThreadHues(wanted: string, scope: string, remembered: Record<string, [number, number]>): Promise<void> {
    const roots = new Map<string, unknown>();
    const held = deps.held();
    for (const summary of (held.channel === wanted ? held.threads : []) as readonly (HueSummary | null)[]) {
      if (summary && summary.root) roots.set(String(summary.id), summary.root.timestamp);
    }
    const hues = Object.entries(remembered).map(([threadId, [hue, drawn]]) => {
      const offered: { thread_id: string; hue: number; last_drawn_ms: number; root_at?: string } =
        { thread_id: threadId, hue, last_drawn_ms: drawn };
      const rootAt = roots.get(threadId);
      if (typeof rootAt === "string") offered.root_at = rootAt;
      return offered;
    });
    let answer;
    try {
      answer = await deps.post(wanted, hues) as { hues?: unknown } | null | undefined;
    } catch (_error) {
      return;
    }
    markSeededScope(scope);
    if (answer && answer.hues && typeof answer.hues === "object") {
      for (const [id, hue] of Object.entries(answer.hues as Record<string, unknown>)) {
        if (typeof hue === "number") noteServerThreadHue(id, hue, wanted);
      }
    }
  }

  /**
   * The colours of the threads drawn near `key`: those whose roots come just before and after its own
   * in time — where All and Main draw them — and those last given a colour, which is the order any view
   * drew its new threads in. Only threads with a colour count; one given its colour later avoids this
   * one in turn. `beside` is the nearest of them: the closest root with a colour on either side, and
   * the thread given a colour last.
   */
  function threadHueNeighbours(key: string): { near: number[]; beside: number[] } {
    const roots = new Map<string, number>();
    const noteRoot = (id: string | null, at: number): void => {
      if (id && Number.isFinite(at) && !((roots.get(id) as number) <= at)) roots.set(id, at);
    };
    const held = deps.held();
    for (const summary of held.threads as readonly (HueSummary | null)[]) {
      if (summary && summary.root) noteRoot(String(summary.id), deps.timeOf(summary.root));
    }
    for (const message of held.messages as readonly (HueMessage | null)[]) {
      if (message && message.thread && message.thread.is_root) noteRoot(deps.threadOf(message), deps.timeOf(message));
    }
    const near: number[] = [];
    const beside: number[] = [];
    const order = [...roots].sort((a, b) => a[1] - b[1] || (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0)).map(([id]) => id);
    const at = order.indexOf(key);
    if (at >= 0) {
      for (const step of [-1, 1]) {
        let found = 0;
        for (let i = at + step; i >= 0 && i < order.length && found < THREAD_HUE_NEIGHBOURS; i += step) {
          if (!threadColors.has(order[i] as string)) continue;
          near.push(threadColors.get(order[i] as string) as number);
          if (found === 0) beside.push(threadColors.get(order[i] as string) as number);
          found += 1;
        }
      }
    }
    for (const given of threadHuesGiven) if (given !== key && threadColors.has(given)) near.push(threadColors.get(given) as number);
    const last = threadHuesGiven[threadHuesGiven.length - 1];
    if (last !== undefined && last !== key && threadColors.has(last)) beside.push(threadColors.get(last) as number);
    return { near, beside };
  }

  return {
    hue(id) {
      // Whichever path draws a channel's first thread — a reload's saved rows, a read, a change of
      // channel — its remembered colours are in place before that thread is given one.
      // An empty picker — a list that emptied for a moment — is no channel: nothing is put back or
      // stored for it, and the channel it comes back to keeps the colours it had.
      const channel = deps.selected();
      if (channel && channel !== threadColorsChannel) restoreThreadHues(channel);
      const key = String(id);
      const now = deps.now();
      if (channel && !((threadHuesSeen.get(key) as number) > now - THREAD_HUES_SEEN_REFRESH_MS)) {
        threadHuesSeen.set(key, now);
        // Once for everything drawn before the page next waits, not once per thread.
        if (!threadHuesSaveOwed) {
          threadHuesSaveOwed = true;
          void Promise.resolve().then(saveThreadHues);
        }
      }
      // `#427 server-thread-hues`: the server's colour, whenever one has arrived for this thread.
      // What follows — the remembered colours and the widest arc — is only for a thread it has not
      // coloured.
      const assigned = serverHuesChannel === channel ? serverThreadHues.get(key) : undefined;
      if (assigned !== undefined) {
        threadColors.set(key, assigned);
        return assigned;
      }
      if (threadColors.has(key)) return threadColors.get(key) as number;
      // Split the largest unused arc: the next thread gets the most separated available color.
      // Keep allocations while paging and moving between views, so a thread never changes color
      // merely because another one arrived. Unlike hashing IDs, adjacent or colliding IDs do not
      // receive indistinguishable colors.
      const used = [...threadColors.values()].sort((a, b) => a - b);
      let hue = 210;
      let widest = -1;
      for (let i = 0; i < used.length; i += 1) {
        const next = i + 1 < used.length ? used[i + 1] as number : (used[0] as number) + 360;
        if (next - (used[i] as number) > widest) {
          widest = next - (used[i] as number);
          hue = ((used[i] as number) + widest / 2) % 360;
        }
      }
      hue = spreadThreadHue(hue, used, threadHueNeighbours(key));
      threadColors.set(key, hue);
      threadHuesGiven.push(key);
      if (threadHuesGiven.length > THREAD_HUE_RECENT) threadHuesGiven.shift();
      return hue;
    },
    restore: restoreThreadHues,
    noteServerIn(messages, summaries, channel = deps.selected(), provisional = false) {
      for (const summary of (summaries || []) as readonly (HueSummary | null)[]) {
        if (!summary) continue;
        if (typeof summary.hue === "number") noteServerThreadHue(summary.id, summary.hue, channel, provisional);
        if (summary.root && summary.root.thread && typeof summary.root.thread.hue === "number") {
          noteServerThreadHue(summary.root.thread.id, summary.root.thread.hue, channel, provisional);
        }
      }
      for (const message of (messages || []) as readonly (HueMessage | null)[]) {
        if (message && message.thread && typeof message.thread.hue === "number") {
          noteServerThreadHue(message.thread.id, message.thread.hue, channel, provisional);
        }
      }
    },
    async seed(channel) {
      const wanted = String(channel || "");
      if (!wanted || !deps.writeToken()) return;
      const scope = threadHueScope(wanted);
      const inFlight = threadHuesSeeding.get(scope);
      if (inFlight) return inFlight;
      if (threadHuesSeedTried.has(scope) || readSeededScopes()[scope] !== undefined) return;
      const remembered = (readThreadHues() || {})[scope];
      if (!remembered || Object.keys(remembered).length === 0) return;
      threadHuesSeedTried.add(scope);
      const offering = offerThreadHues(wanted, scope, remembered);
      threadHuesSeeding.set(scope, offering);
      try {
        await offering;
      } finally {
        threadHuesSeeding.delete(scope);
      }
    },
  };
}

// What the page and the tests use of it.
export { THREAD_HUES_KEY, THREAD_HUES_PER_CHANNEL, THREAD_HUES_SEEDED_KEY, THREAD_HUES_SEEN_REFRESH_MS, THREAD_HUES_TOTAL,
  THREAD_HUES_VERSION, THREAD_HUE_NEIGHBOURS, THREAD_HUE_RECENT };
