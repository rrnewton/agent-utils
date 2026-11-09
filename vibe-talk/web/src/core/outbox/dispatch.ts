// Sending what the outbox holds (`#195 send-resilience`, `#277 send-order`, `#448
// send-dedupe`, `#621 send-double-check`, `#1052 post-to-main`; `#624
// voice-page-modularity`, moved from the legacy script into core/outbox; mu-readcore):
// queueing in the order written, one attempt at a time for a destination, placing a reply
// whose thread is still to be found out, the POST and what became of a lost answer, and the
// automatic retries those attempts arm. The page supplies the transport-shaped API call,
// the clock and timers, the credential, the rendering, and placing a reply's target; this
// owner holds every attempt, wait, queued credential and automatic-retry hold.

import { nextRetryAt, retriesItself as retryDecision } from "./retries.ts";
import { createOutboxPump, createSendNumbers } from "./pump.ts";
import { editableSend as editableDecision, waitingItsTurn as waitingDecision } from "./state.ts";
import type { OutboxStore } from "./store.ts";
import type { OutgoingPart, OutgoingWire } from "./saved.ts";
import type { CancelSignal, CancellationPort, ClockPort, TimerHandle } from "../../ports/index.ts";

const OUTGOING_TIMEOUT_MS = 60000;
// `#448 send-dedupe`. After a send whose answer never came back, how long the page keeps asking
// the server what became of it (`askOutgoingOutcome`), each ask waiting up to OUTGOING_OUTCOME_WAIT_S
// for a post still in flight; and how often a slow send asks how far it has got.
const OUTGOING_OUTCOME_MS = 120000;
const OUTGOING_OUTCOME_WAIT_S = 20;
const OUTGOING_PROGRESS_MS = 2000;

/** What a composer or reply box asks the outbox to send: the legacy request shape, unchanged. */
export interface OutgoingRequest {
  channel: string;
  threadId?: string | null;
  replyTo?: string | null;
  text: string;
  marksRead?: string | null;
  placement?: "unknown";
  /** Land top-level answering `replyTo` by edge only (`#1052 post-to-main`). */
  postToMain?: boolean;
  /** Post the words exactly as typed, marked raw for what carries them on (`#1135 raw-message-page`). */
  raw?: boolean;
  inPlace?: string;
  inCards?: boolean;
}

/** A live send as dispatch reads and changes it, in place in the shared outbox store. */
export interface DispatchEntry {
  id: string;
  channel: string;
  threadId?: string | null;
  postToMain?: boolean;
  raw?: boolean;
  placement?: "unknown" | null;
  replyTo?: string | null;
  marksRead?: string | null;
  originView: string;
  text: string;
  remaining: string;
  state: string;
  phase: string;
  createdAt: number;
  postedCount: number;
  parts: OutgoingPart[];
  observedParts: string[];
  keyedOnly: boolean;
  mayHavePosted: boolean;
  continueIn: string | null;
  wire: OutgoingWire | null;
  sentAt: number;
  sendStartedAt: number;
  serverMs: number | null;
  upstreamMs: number | null;
  serverTick: boolean;
  upstreamTick: boolean;
  tickFailed: boolean;
  showTimes: boolean;
  detail: string;
  retriesUsed?: number;
  progress?: { posted: number; parts: number } | null;
  confirming?: boolean;
  resolveOnly?: { detail: string } | null;
  echoed?: boolean;
  inPlace?: string;
  inCards?: boolean;
}

/** A request body for one attempt. */
export interface OutgoingBody {
  text: string;
  reply_to?: string;
  thread_id?: string;
  post_to_main?: boolean;
  raw?: boolean;
  idempotency_key?: string;
}

/** A send failure as dispatch branches on it. */
export type DispatchFailure = Error & {
  network?: boolean;
  timedOut?: boolean;
  status?: number;
  code?: string;
  gatewayStatus?: number;
  removed?: boolean;
};

/** An automatic send held for its time, or for the connection. */
export interface OutgoingRetryHold {
  at: number;
  offline: boolean;
  unsent: boolean;
  identity: unknown;
  timer: TimerHandle | null;
}

/** Whether the reader follows a confirmed send, decided before it is drawn. */
export interface SentFollow {
  here: boolean;
  pinned: boolean;
  sidePinned: boolean;
}

/** What dispatch asks of the page. */
export interface OutboxDispatchDependencies {
  store: OutboxStore<DispatchEntry>;
  observations: Map<string, Set<string>>;
  now(): number;
  clock: ClockPort;
  cancellation: CancellationPort;
  random(): number;
  api(path: string, options?: { method?: string; body?: OutgoingBody; signal?: CancelSignal }): Promise<unknown>;
  token(): string;
  identity(): unknown;
  online(): boolean;
  channelWritable(channel: string): boolean;
  originView(): string;
  persist(): void;
  renderOutgoing(): void;
  renderRows(): void;
  placeReplyTarget(entry: DispatchEntry): Promise<string | null>;
  postsOnce(channel: string): boolean;
  serviceOf(channel: string): string;
  readableFailure(error: DispatchFailure, service: string): string;
  readableCause(detail: unknown, service: string): string;
  refusalWords(refusal: unknown, service: string): string;
  redact(text: string): string;
  threadOf(part: OutgoingPart): string | null;
  noteOwnSent(ids: string[]): void;
  armSentReceipt(entry: DispatchEntry): void;
  selectedChannel(): string;
  inDiscord(): boolean;
  followsNewest(side: boolean): boolean;
  noteSelfAuthor(authorId: unknown): void;
  noteStartsRead(parts: readonly OutgoingPart[]): void;
  appendPart(part: OutgoingPart): void;
  scrollToNewest(side: boolean): void;
  markSentRead(entry: DispatchEntry): void;
  reportColumnReply(entry: DispatchEntry): void;
  columnReplies: Map<string, unknown>;
  quietly(run: () => void): void;
}

/** Dispatch's commands and questions for the page and the other outbox owners. */
export interface OutboxDispatch {
  queue(request: OutgoingRequest, column?: unknown): Promise<void>;
  retry(id: string, automatic?: boolean): Promise<void> | undefined;
  schedule(entry: DispatchEntry): Promise<void>;
  blocker(entry: DispatchEntry): DispatchEntry | null;
  credential(): string;
  noteQueuedUnder(id: string): void;
  retryOf(id: string): OutgoingRetryHold | undefined;
  retryPending(id: string): boolean;
  testKey(id: string): boolean;
  retriesItself(entry: DispatchEntry): boolean;
  editable(entry: DispatchEntry): boolean;
  waitingItsTurn(entry: DispatchEntry): boolean;
  attempting(id: string): boolean;
  cancelRetry(id: string): void;
  hold(entry: DispatchEntry, hold: { at: number; offline: boolean; unsent: boolean }): void;
  drop(id: string): void;
  resumeRetries(): void;
  stop(): void;
  pump(): void;
}

interface DispatchJob {
  credential: string;
  stopped: boolean;
  cancel: (() => void) | null;
  attemptOver: boolean;
}

interface PostAnswer {
  state?: unknown;
  error?: unknown;
  detail?: unknown;
  refusal?: unknown;
  posted?: unknown;
  thread_id?: unknown;
  unsent?: unknown;
  uncertain?: unknown;
  retryable?: unknown;
  resumable?: unknown;
  parts?: unknown;
  server_ms?: unknown;
  timing?: { upstream_ms?: unknown; server_ms?: unknown } | null;
}

/** What the server accepts as an idempotency key; the outbox's own ids always are one. */
const IDEMPOTENCY_KEY = /^[A-Za-z0-9_-]{1,64}$/;

/** Dispatch over `deps`, with no attempt in progress and nothing held to go again. */
export function createOutboxDispatch(deps: OutboxDispatchDependencies): OutboxDispatch {
  const outbox = deps.store;
  const now = () => deps.now();
  const outgoingObservations = deps.observations;
  const columnReplies = deps.columnReplies;
  // By entry id, the attempt in progress — a reply being placed, or a POST — and nothing else: a send
  // waiting its turn has no job. See "one destination, in the order written".
  const outgoingJobs = new Map<string, DispatchJob>();
  // By entry id, whoever is waiting for its next attempt to end, and the credential it was queued
  // under. Never saved. See `scheduleOutgoingMessage`.
  const outgoingWaiters = new Map<string, Array<() => void>>();
  const outgoingQueuedUnder = new Map<string, string>();
  const outgoingPump = createOutboxPump();
  /**
   * Entries waiting to go out again on their own, by id: when (`at`), or when the connection returns
   * (`offline`); whether nothing has been attempted yet (`unsent`); the credential it was armed
   * under (`identity`); and its timer. Never saved: see `planOutgoingRetry`.
   */
  const outgoingRetries = new Map<string, OutgoingRetryHold>();
  const outgoingSequence = createSendNumbers();

  const persistOutgoingMessages = () => deps.persist();
  const renderOutgoingEverywhere = () => deps.renderOutgoing();
  const channelPostsOnce = (channel: string) => deps.postsOnce(channel);

  function currentCredential(): string {
    try {
      return deps.token();
    } catch (_error) {
      return "";
    }
  }

  function retriesItself(entry: DispatchEntry): boolean {
    return retryDecision(entry, channelPostsOnce);
  }

  function attempting(id: string): boolean {
    return outgoingJobs.has(id);
  }

  function editable(entry: DispatchEntry): boolean {
    return editableDecision(entry, attempting(entry.id));
  }

  function waitingItsTurn(entry: DispatchEntry): boolean {
    return waitingDecision(entry, attempting(entry.id));
  }

  /**
   * Put `request` in the outbox and send it. `column` is given for a reply sent from a column, which
   * the reader is not shown the outbox row of when it goes, and so is told how it went
   * (`reportColumnReply`, `#277 reply-in-column`). Never saved: a reload shows the outbox as it is.
   */
  function queueOutgoingMessage(request: OutgoingRequest, column: unknown = null): Promise<void> {
    let id: string;
    do {
      id = `outgoing-${now().toString(36)}-${outgoingSequence.next()}-${deps.random().toString(36).slice(2, 9)}`;
    } while (outbox.has(id));
    const entry: DispatchEntry = {
      id, ...request, originView: deps.originView(), createdAt: now(),
      remaining: request.text.trim(), state: "sending", phase: "queued",
      parts: [], observedParts: [], postedCount: 0, detail: "", keyedOnly: true, mayHavePosted: false,
      continueIn: null, wire: null, sentAt: 0,
      // `#621 send-double-check`: two-phase durations from the Send tap, and tick states.
      sendStartedAt: now(), serverMs: null, upstreamMs: null,
      serverTick: false, upstreamTick: false, tickFailed: false, showTimes: false,
    };
    outbox.add(entry);
    if (column) columnReplies.set(id, column);
    return scheduleOutgoingMessage(entry);
  }

  /**
   * Send a failed or unconfirmed entry again: because the reader tapped Retry, or — `automatic` —
   * because its wait for an automatic retry ran out.
   */
  function retryOutgoingMessage(id: string, automatic = false): Promise<void> | undefined {
    const entry = outbox.get(id);
    if (!entry || entry.state === "sending" || entry.state === "sent" || !entry.remaining) return undefined;
    cancelOutgoingRetry(id);
    // A person deciding starts the automatic schedule again, should this attempt fail too.
    entry.retriesUsed = automatic ? (entry.retriesUsed || 0) + 1 : 0;
    if (!deps.channelWritable(entry.channel)) {
      entry.detail = "This channel is read-only.";
      persistOutgoingMessages();
      renderOutgoingEverywhere();
      return undefined;
    }
    entry.state = "sending";
    entry.phase = "queued";
    entry.detail = "";
    // `#621 send-double-check`: a new attempt starts its ticks and clock again.
    entry.sendStartedAt = now();
    entry.serverMs = null;
    entry.upstreamMs = null;
    entry.serverTick = false;
    entry.upstreamTick = false;
    entry.tickFailed = false;
    entry.showTimes = false;
    outgoingObservations.delete(entry.id);
    return scheduleOutgoingMessage(entry);
  }

  // --- one destination, in the order written ---------------------------------------------------------
  //
  // `#277 send-order`. The owner: "when it retries it ends up sending messages OUT OF ORDER. I.e. I
  // write A then B but B lands first." Messages to one destination were serialised only while a POST
  // was on the wire: when A's POST failed and A went to wait for its automatic retry, the lane was
  // free, B went, and A's retry landed after it. Order is now kept by the outbox itself, in the order
  // its entries were written (the order of `outbox`, which is the order it is saved in, so
  // a reload keeps it):
  //
  //   * A message goes only when every earlier message to the same destination — the channel, and the
  //     thread in it or none — has been SENT. One still sending, waiting to retry, waiting for the
  //     connection, or failed and waiting for the reader holds every later one to that destination,
  //     whose row says what it is waiting behind and why. When that one is sent the queue drains in
  //     order; when it is dismissed, or edited back into its box, the next one goes.
  //   * Never two POSTs on the wire to one destination at once, whatever their order — a send put back
  //     from Dismiss waits for the one already going out.
  //   * Other destinations are not held: a thread does not wait for the main channel or another thread.
  //   * A reply whose thread is still to be found out (`placeReplyTarget`) may be bound for any
  //     destination in its channel, so whatever was written after it in that channel waits for it
  //     until it is placed, and then for its delivery only if both go to the same place. A ticked
  //     post-to-main (`#1052 post-to-main`) is not one of those: its destination is already known,
  //     so it is never placed and holds only its own top-level lane.
  //   * The rest of a post cut in parts, after a 207, goes where the server said the parts after the
  //     first went (`continueIn`, `#263 post-split-per-provider`), so it is in that thread's lane as
  //     well as its own: a message written later into that thread waits for it too.
  //   * A message with nothing left to send — a 207 that handed nothing back — can post nothing more,
  //     so it holds nothing up.
  //
  // Each entry keeps its own id and so its own idempotency key across every attempt (`#195
  // send-resilience`): waiting behind another message changes when it goes, never what it is.

  /** Whether `entry`'s destination is still to be found out before it can post. */
  function needsPlacing(entry: DispatchEntry): boolean {
    return entry.placement === "unknown" && entry.postToMain !== true;
  }

  /** The outbox lane of one destination: a channel, and the thread in it or none. */
  function outgoingDestination(channel: string, threadId: string | null | undefined): string {
    return JSON.stringify([channel, threadId || null]);
  }

  /**
   * The lanes `entry` posts in: its own destination, and the thread the rest of it continues in where
   * that is another. Null for a reply still to be placed, which may post in any lane of its channel.
   */
  function outgoingLanes(entry: DispatchEntry): string[] | null {
    if (needsPlacing(entry)) return null;
    const lanes = [outgoingDestination(entry.channel, entry.threadId)];
    if (entry.continueIn && entry.continueIn !== entry.threadId) {
      lanes.push(outgoingDestination(entry.channel, entry.continueIn));
    }
    return lanes;
  }

  /**
   * The message `entry` has to wait for before it can go, or null when it can go now: by the rules
   * above, the first earlier message in one of its lanes not yet sent, an earlier reply in its channel
   * not yet placed, or a POST in one of its lanes already on the wire. For a reply still to be placed,
   * only an earlier one not yet placed either: one read places both.
   */
  function outgoingBlocker(entry: DispatchEntry): DispatchEntry | null {
    const lanes = outgoingLanes(entry);
    let earlier = true;
    for (const other of outbox.values()) {
      if (other === entry) {
        earlier = false;
        continue;
      }
      if (other.channel !== entry.channel || other.state === "sent" || !other.remaining) continue;
      if (needsPlacing(other)) {
        if (earlier) return other;
        continue;
      }
      const otherLanes = outgoingLanes(other);
      if (lanes === null || otherLanes === null || !otherLanes.some((lane) => lanes.includes(lane))) continue;
      if (earlier || (other.state === "sending" && other.phase === "posting")) return other;
    }
    return null;
  }

  /**
   * Put `entry`, just queued or retried, in the outbox's order to go, without making the composer wait
   * for its destination. Answers a promise settled when this attempt is over — sent, failed, or no
   * longer in the outbox — which may be long after it was queued: it waits behind what was written
   * before it.
   */
  function scheduleOutgoingMessage(entry: DispatchEntry): Promise<void> {
    outgoingQueuedUnder.set(entry.id, currentCredential());
    const settled = new Promise<void>((resolve) => {
      outgoingWaiters.set(entry.id, [...(outgoingWaiters.get(entry.id) || []), resolve]);
    });
    persistOutgoingMessages();
    renderOutgoingEverywhere();
    pumpOutgoing();
    return settled;
  }

  /** Let whoever waits for `id`'s attempt go: it is over, or it is out of the outbox. */
  function settleOutgoing(id: string): void {
    const waiters = outgoingWaiters.get(id) || [];
    outgoingWaiters.delete(id);
    outgoingQueuedUnder.delete(id);
    for (const resolve of waiters) resolve();
  }

  /** Start every queued message that has nothing to wait for any more, in the order written. */
  function pumpOutgoing(): void {
    // Starting one can finish it at once — signed out, offline — and that asks for a pump of its own, which
    // makes this one go round again (`outgoingPump`).
    outgoingPump.pump(() => {
      for (const entry of [...outbox.values()]) {
        if (outbox.get(entry.id) !== entry || entry.state !== "sending" ||
            entry.phase !== "queued" || outgoingJobs.has(entry.id) || outgoingBlocker(entry)) continue;
        startOutgoing(entry);
      }
    });
  }

  /** One attempt for a message whose turn it is: placing a reply, or the POST. */
  function startOutgoing(entry: DispatchEntry): void {
    // Under the credential it was queued under: a send made by one person is not made by another.
    const credential = outgoingQueuedUnder.has(entry.id) ? outgoingQueuedUnder.get(entry.id)! : currentCredential();
    const job: DispatchJob = { credential, stopped: false, cancel: null, attemptOver: false };
    outgoingJobs.set(entry.id, job);
    const placing = needsPlacing(entry);
    const over = () => {
      if (outgoingJobs.get(entry.id) === job) outgoingJobs.delete(entry.id);
      // An attempt that ended with no outcome is not started again, and again: only a reply it placed
      // is still to be sent, behind whatever is ahead of it where it goes.
      if (entry.state === "sending" && !(placing && !needsPlacing(entry))) {
        Object.assign(entry, { state: "failed", phase: "", detail: entry.detail || "This message was not sent." });
        persistOutgoingMessages();
      }
      if (entry.state !== "sending" || outbox.get(entry.id) !== entry) settleOutgoing(entry.id);
      pumpOutgoing();
    };
    void (placing ? placeOutgoingMessage(entry, job) : dispatchOutgoingMessage(entry, job)).then(over, over);
  }

  /**
   * Whether `entry` cannot go now, saying why on the entry: the sign-in it was queued under is gone,
   * its channel is read-only, or the device is offline — `#195 send-resilience`: held for the
   * connection instead of failed. Nothing has left the device, so sending it when the connection
   * returns cannot post it twice, through any provider, keyed or not.
   */
  function outgoingCannotGo(entry: DispatchEntry, job: DispatchJob): boolean {
    if (!job.credential || currentCredential() !== job.credential) {
      entry.state = "failed";
      entry.detail = "Sign-in changed before this message was sent. Sign in and retry when ready.";
      return true;
    }
    if (!deps.channelWritable(entry.channel)) {
      entry.state = "failed";
      entry.detail = "This channel is read-only.";
      return true;
    }
    if (!deps.online()) {
      entry.state = "failed";
      entry.detail = "This device is offline.";
      holdOutgoingMessage(entry, { at: now(), offline: true, unsent: true });
      return true;
    }
    return false;
  }

  /**
   * Find out where a reply to a message the stream left unplaced goes (`placeReplyTarget`), and leave
   * it queued for that destination; `pumpOutgoing` posts it when its turn there comes.
   */
  async function placeOutgoingMessage(entry: DispatchEntry, job: DispatchJob): Promise<void> {
    if (job.stopped) return;
    // When this attempt started: the next one's wait counts from here (`planOutgoingRetry`).
    const startedAt = now();
    try {
      if (outgoingCannotGo(entry, job)) return;
      entry.phase = "placing";
      persistOutgoingMessages();
      renderOutgoingEverywhere();
      let thread: string | null = null;
      try {
        thread = await deps.placeReplyTarget(entry);
      } catch (error) {
        if (job.stopped) return;
        const failure = error as DispatchFailure;
        // Nothing has been posted, so this is a plain failure the reader can retry, never the
        // "unconfirmed" of a POST whose answer was lost.
        entry.state = "failed";
        // `#315 removed-reply-target`. Removed upstream: an answer, and the reader's to Edit or Dismiss.
        if (failure.removed) {
          entry.detail = "The message you answered was removed from the channel, so this was not posted anywhere.";
          return;
        }
        const cause = "The message you answered came in live, and whether it is in a thread could not " +
          `be found out, so this was not posted anywhere: ${failure.message}.`;
        // `#277 send-order`. A read that failed for a moment — timed out, lost its connection, met the
        // chat service's 5xx — goes again on its own, as a lost send does: posting nothing, it cannot
        // post anything twice, and every message written after this reply in its channel waits for
        // it. A message the read does not carry is an answer, and is left to the reader.
        const transient = Boolean(failure.network || failure.timedOut || (failure.status || 0) >= 500 ||
          failure.status === 408 || (failure.gatewayStatus || 0) >= 500);
        const again = transient && planOutgoingRetry(entry, startedAt, true);
        entry.detail = deps.redact(again ? cause : `${cause} Retry once the channel can be read.`);
        return;
      }
      if (job.stopped) return;
      entry.threadId = thread;
      entry.placement = null;
    } finally {
      if (entry.state === "sending" && !job.stopped) entry.phase = "queued";
      else entry.phase = "";
      persistOutgoingMessages();
      deps.renderRows();
    }
  }

  /**
   * The POST of `body` for `entry`'s attempt `job`, bounded by OUTGOING_TIMEOUT_MS and stopped by
   * `job.cancel`: its answer, or, after a lost answer, the post's own answer found without posting
   * (`askOutgoingOutcome`, `#448 send-dedupe`). Null when the attempt was stopped. Throws what the
   * POST threw otherwise. `armed` is handed the timeout's handle, for the caller to clear.
   */
  async function postOutgoing(
    entry: DispatchEntry,
    job: DispatchJob,
    body: OutgoingBody,
    controller: ReturnType<CancellationPort["create"]>,
    armed: (handle: TimerHandle | null) => void,
  ): Promise<PostAnswer | null> {
    if (body.idempotency_key) watchOutgoingProgress(entry, job, body);
    let timer: TimerHandle | null = null;
    const interruption = new Promise<never>((_resolve, reject) => {
      job.cancel = () => {
        if (controller) controller.abort();
        reject(new Error("The send was interrupted before delivery was confirmed."));
      };
      timer = deps.clock.setTimeout(() => {
        if (controller) controller.abort();
        const late = new Error("The send timed out before delivery was confirmed.") as DispatchFailure;
        late.timedOut = true;
        reject(late);
      }, OUTGOING_TIMEOUT_MS);
    });
    armed(timer);
    const sendTapAt = now();
    entry.sendStartedAt = entry.sendStartedAt || sendTapAt;
    let askedOutcome = false;
    try {
      const payload = await Promise.race([
        deps.api(`/api/v1/channels/${encodeURIComponent(entry.channel)}/reply?ack=accepted`, {
          method: "POST", body, ...(controller ? { signal: controller.signal } : {}),
        }) as Promise<PostAnswer | null>,
        interruption,
      ]);
      if (job.stopped) return null;
      // `#621 send-double-check`: phase one answered; phase two via the outcome long-poll.
      if (payload && payload.state === "accepted") {
        entry.serverMs = Number.isFinite(Number(payload.server_ms)) ? Number(payload.server_ms) : now() - sendTapAt;
        entry.serverTick = true;
        entry.tickFailed = false;
        persistOutgoingMessages();
        renderOutgoingEverywhere();
        const acceptedAt = now();
        askedOutcome = true;
        const found = await askOutgoingOutcome(entry, job, body);
        if (job.stopped) return null;
        if (!found) {
          const late = new Error("The send timed out before delivery was confirmed.") as DispatchFailure;
          late.timedOut = true;
          throw late;
        }
        entry.upstreamMs = found.timing && Number.isFinite(Number(found.timing.upstream_ms))
          ? Number(found.timing.upstream_ms) : now() - acceptedAt;
        entry.upstreamTick = !found.error;
        if (found.timing && Number.isFinite(Number(found.timing.server_ms))) entry.serverMs = Number(found.timing.server_ms);
        return found;
      }
      // Older server: one answer carried both phases; its split is unknown, so the
      // whole elapsed time is upstream-side and the server phase stays unknown.
      if (payload) {
        entry.serverTick = true;
        entry.upstreamMs = now() - sendTapAt;
        entry.upstreamTick = payload.error !== "partially_posted";
      }
      return payload;
    } catch (error) {
      if (timer !== null) deps.clock.clearTimeout(timer);
      armed(null);
      if (job.stopped) return null;
      // `#448 send-dedupe`. The answer was lost — the page stopped waiting, the connection dropped,
      // or something in front of the server answered for it — while the post may still be landing.
      // The owner saw a long message "fail", pressed Retry, and it arrived twice. So the server is
      // asked what became of it before anything is decided, posting nothing.
      const found = !askedOutcome && body.idempotency_key && answerLost(error as DispatchFailure)
        ? await askOutgoingOutcome(entry, job, body) : null;
      if (job.stopped) return null;
      if (!found) throw error;
      return found;
    }
  }

  /** Post one outbox entry whose turn it is. */
  async function dispatchOutgoingMessage(entry: DispatchEntry, job: DispatchJob): Promise<void> {
    if (job.stopped) return;
    let timer: TimerHandle | null = null;
    const controller = deps.cancellation.create();
    // Whether this attempt failed in a way that could succeed later — a timeout, a lost connection,
    // the provider's 5xx — rather than with an answer. Only such a failure goes again on its own.
    let transient = false;
    // When this attempt started: the next one's wait counts from here (`planOutgoingRetry`).
    const startedAt = now();
    // The post's answer when it was found without posting (`settleFromServer`), and whether such an
    // ask found nothing, which leaves the entry, and what was said of it, as they were.
    let payloadFound: PostAnswer | null = null;
    let unchanged = false;
    try {
      if (outgoingCannotGo(entry, job)) return;
      entry.phase = "posting";
      persistOutgoingMessages();
      renderOutgoingEverywhere();
      const body: OutgoingBody = { text: entry.remaining };
      // `#263 post-split-per-provider`. The rest of a post whose first part went to the conversation
      // continues where the server said the parts after it went — the first part's thread, for a
      // provider that threads a long message under its first part — and without the reply target,
      // which only the first part carries.
      let replyTo = entry.replyTo && !entry.continueIn ? entry.replyTo : null;
      let threadId = entry.continueIn || entry.threadId || null;
      // `#1052 post-to-main`: asked to land top-level, answering by edge only. The rest of a post
      // cut in parts continues in the new root's thread as ever, so this rides only until then.
      let postToMain = entry.postToMain === true && !entry.continueIn;
      // `#1135 raw-message-page`: asked to go exactly as typed. As post-to-main, the marker rides
      // only until a post cut in parts continues elsewhere, so a remainder is never delivered raw
      // on its own.
      let raw = entry.raw === true && !entry.continueIn;
      // `#448 send-dedupe`. Where these words go is decided by the first attempt that sends them, and
      // every later one says the same, across reloads: the server answers a repeat under the key with
      // the post it made, and a page that has since drawn the reply into the thread it started must
      // not ask for it somewhere else. Only the unsent rest of a 207 is new words, placed afresh.
      if (entry.wire && entry.wire.text === entry.remaining) {
        ({ replyTo, threadId } = entry.wire);
        postToMain = entry.wire.postToMain === true && !entry.continueIn;
        raw = entry.wire.raw === true && !entry.continueIn;
      } else {
        entry.wire = { text: entry.remaining, replyTo, threadId, postToMain: entry.postToMain === true, raw: entry.raw === true };
      }
      if (replyTo) body.reply_to = replyTo;
      if (threadId) body.thread_id = threadId;
      if (postToMain) body.post_to_main = true;
      if (raw) body.raw = true;
      // The entry's id names this post on every attempt, across reloads, so the server can tell a
      // retry from a second message. `#448 send-dedupe`: sent to every provider, since the server
      // itself answers a repeat of a post that landed, or is landing, with that post. Only a provider
      // that honours it upstream (`channelPostsOnce`) also covers a part whose own answer was lost,
      // which is what an automatic retry needs; one attempt anywhere else and the entry can never
      // again prove a retry safe.
      if (IDEMPOTENCY_KEY.test(entry.id)) body.idempotency_key = entry.id;
      if (!channelPostsOnce(entry.channel) || !body.idempotency_key) entry.keyedOnly = false;
      entry.progress = null;
      // `#468 send-dedupe`. Only asking what became of an earlier attempt (`settleFromServer`):
      // nothing is posted, and with no record the entry is left as it was.
      if (entry.resolveOnly) {
        const found = body.idempotency_key ? await askOutgoingOutcome(entry, job, body) : null;
        if (job.stopped) return;
        if (!found) {
          Object.assign(entry, { state: "unconfirmed", detail: entry.resolveOnly.detail });
          unchanged = true;
          return;
        }
        entry.resolveOnly = null;
        payloadFound = found;
      }
      let payload = payloadFound;
      if (!payload) payload = await postOutgoing(entry, job, body, controller, (handle) => { timer = handle; });
      if (job.stopped) return;
      // `#621 send-double-check`: the outcome route says the post failed upstream, with why.
      if (payload && payload.error === "post_failed") {
        entry.state = "unconfirmed";
        entry.mayHavePosted = true;
        entry.tickFailed = true;
        entry.upstreamTick = false;
        outgoingObservations.delete(entry.id);
        entry.detail = deps.redact(payload.detail
          ? deps.readableCause(payload.detail, deps.serviceOf(entry.channel))
          : "The remaining delivery could not be confirmed.");
        // Only an attempt this page made retries on its own; an outcome found for an
        // earlier attempt (`resolveOnly`) leaves the send to the reader.
        transient = !payloadFound;
        return;
      }
      if (payload && payload.error === "partially_posted") {
        entry.postedCount += Math.max(0, Number(payload.posted) || 0);
        // Where the unsent text belongs, when the server says: the thread the parts already posted
        // went into. Absent means the conversation, or an older server, and changes nothing.
        if (typeof payload.thread_id === "string" && payload.thread_id && entry.postedCount > 0 &&
            payload.thread_id !== entry.threadId) {
          entry.continueIn = payload.thread_id;
        }
        // An adapter can lose an acknowledgement after delivery. Even a 207 is not proof that the
        // last attempted part failed to arrive; its unsent suffix is the ONLY safe retry candidate.
        entry.remaining = typeof payload.unsent === "string" ? payload.unsent : "";
        // `#444 send-refusal-reason`. Unless the server says the part that failed was answered, not
        // lost (`uncertain` false): then it is certainly not in the channel, and this is a send that
        // was not sent. Calling it unconfirmed — "Retrying cannot post it twice" — sent the owner
        // back to Retry four times over a refusal that the same words meet every time. An older
        // server does not say, and its 207 stays unconfirmed.
        const answered = payload.uncertain === false;
        entry.tickFailed = true;
        entry.upstreamTick = false;
        entry.state = answered ? "failed" : "unconfirmed";
        if (!answered || entry.postedCount > 0) entry.mayHavePosted = true;
        outgoingObservations.delete(entry.id);
        entry.detail = deps.redact(deps.refusalWords(payload.refusal, deps.serviceOf(entry.channel)) || (payload.detail
          ? deps.readableCause(payload.detail, deps.serviceOf(entry.channel))
          : "The remaining delivery could not be confirmed."));
        // Said by the server, which knows whether the provider answered or merely failed to; an
        // older server does not say, and its 207 is left to the reader.
        transient = payload.retryable === true && Boolean(entry.remaining);
        // Also the server's to say: whether sending the unsent text again repeats the part that
        // failed as the same request, under the words and key it had. Where it does not — a code
        // block cut across that part, a part repeated among those posted — no later attempt can be
        // recognised as this one, so this entry can never again prove a retry safe. Not said, by an
        // older server, is not proof either.
        if (payload.resumable !== true) entry.keyedOnly = false;
        return;
      }
      const parts = (payload && Array.isArray(payload.parts) && payload.parts.length
        ? payload.parts : payload && payload.posted ? [payload.posted] : []) as Array<OutgoingPart | null>;
      if (!parts.length || parts.some((part) => !part || !part.id ||
          (part.channel_id && String(part.channel_id) !== entry.channel))) {
        throw new Error("The server did not confirm which message was sent.");
      }
      entry.parts = parts.map((part) => ({
        ...part!, id: String(part!.id), channel_id: entry.channel, content: String(part!.content || ""),
      }));
      // `#430 threads-main-own-sent`. Sent from here: the read filters keep it in sight from now on.
      deps.noteOwnSent(entry.parts.map((part) => part.id));
      // `#263 post-split-per-provider`. The rest of a long post may have continued in its first
      // part's thread, where it is drawn rather than here; the receipt for it says which thread.
      if (!entry.threadId) entry.continueIn = entry.parts.slice(1).map((part) => deps.threadOf(part)).find(Boolean) || entry.continueIn;
      const observed = outgoingObservations.get(entry.id) || new Set<string>();
      entry.observedParts = entry.parts.filter((part) => observed.has(part.id)).map((part) => part.id);
      outgoingObservations.set(entry.id, new Set(entry.observedParts));
      entry.state = "sent";
      entry.upstreamTick = true;
      entry.tickFailed = false;
      if (entry.upstreamMs === null) entry.upstreamMs = Math.max(0, now() - (entry.sendStartedAt || now()) - (entry.serverMs || 0));
      entry.detail = "";
      // `#500 sent-receipts-pile-up`. From now its receipt is shown for a while, then goes.
      entry.sentAt = now();
      deps.armSentReceipt(entry);
      // Whether the reader follows is decided BEFORE anything this answer changes is drawn. The first
      // message sent can tell the page who "me" is, and then every row of mine is drawn again as mine
      // (`noteSelfAuthor`), narrower, where a line may wrap onto one more: a reader on the newest line
      // was left that much above it, and the message they had just sent arrived below them, unseen.
      // Not while a filter change holds a message in place (`#243 filter-keeps-selection`), which the
      // page's follow answers for.
      const here = entry.channel === deps.selectedChannel();
      const pinned = here && deps.inDiscord() && deps.followsNewest(false);
      // `#229 desktop-two-column`. The right column's reader on its newest line follows it too.
      const sidePinned = here && deps.followsNewest(true);
      deps.noteSelfAuthor(entry.parts[0]!.author_id);
      // `#549 own-read-stored`. What he just said starts read, as the server recorded it before answering.
      deps.noteStartsRead(entry.parts);
      if (here) {
        for (const part of entry.parts) deps.appendPart(part);
        if (pinned) deps.scrollToNewest(false);
        if (sidePinned) deps.scrollToNewest(true);
      }
      // `#256 reply-marks-read`. Sent, so what it marks is read: what it answers, or the root of the
      // thread it starts (`#300 reply-marks-read`). Only here: a send that failed, or is unconfirmed,
      // answered nothing yet.
      if (entry.marksRead) deps.markSentRead(entry);
    } catch (error) {
      if (job.stopped) return;
      const failure = error as DispatchFailure;
      entry.tickFailed = true;
      entry.upstreamTick = false;
      if (entry.upstreamMs === null && entry.serverTick) {
        entry.upstreamMs = Math.max(0, now() - (entry.sendStartedAt || now()) - (entry.serverMs || 0));
      }
      entry.state = (failure.status || 0) >= 400 && (failure.status || 0) < 500 && failure.status !== 408
        ? "failed" : "unconfirmed";
      if (entry.state === "unconfirmed") entry.mayHavePosted = true;
      outgoingObservations.delete(entry.id);
      // The page's own network sentence tells the reader to retry when the connection recovers,
      // which a row that may retry by itself must not say.
      entry.detail = deps.redact(failure.network
        ? "The connection to vibe-talk failed before delivery was confirmed."
        : (failure.gatewayStatus || 0) >= 500 ? `vibe-talk did not answer (HTTP ${failure.gatewayStatus}).`
          : deps.readableFailure(failure, deps.serviceOf(entry.channel)));
      // Never a refusal: a 4xx is an answer, and asking again asks the same question. A gateway's
      // own 5xx page in front of this server is as unanswered as the server's own 5xx.
      transient = Boolean(failure.network || failure.timedOut || (failure.status || 0) >= 500 || failure.status === 408 ||
        (failure.gatewayStatus || 0) >= 500);
    } finally {
      if (timer !== null) deps.clock.clearTimeout(timer);
      job.cancel = null;
      job.attemptOver = true;
      entry.phase = "";
      entry.progress = null;
      entry.confirming = false;
      entry.resolveOnly = null;
      if (transient && !job.stopped) planOutgoingRetry(entry, startedAt);
      persistOutgoingMessages();
      deps.renderRows();
      // `#277 reply-in-column`. After the rows, the outbox's among them: what is said points at one. Not
      // for a send that signing out stopped, which is the sign-in screen's to explain.
      if (columnReplies.has(entry.id) && !job.stopped && !unchanged) deps.reportColumnReply(entry);
    }
  }

  // --- what became of a send whose answer was lost ---------------------------------------------------
  //
  // `#448 send-dedupe`. The owner sent a long message to a Slack channel, cut in three parts; it took
  // twelve seconds and the page showed it as failed: "that long message appeared to fail to send the
  // first time, and then on retry appeared to succeed, but apparently that caused message duplication."
  // The server had posted every part, under a key no Slack request carries. Now the server remembers
  // each keyed post itself (src/post_keys.rs), and:
  //
  //   * A send whose answer never came back — the page's own timeout, a dropped connection, a gateway's
  //     page in place of the server's answer — asks `POST .../reply/outcome`, which posts nothing and
  //     waits for a post still in flight. The post's own answer is then this attempt's answer: Sent, or
  //     the 207 of a post that landed in part. Only a post the server has no record of is left
  //     unconfirmed, as before, since that proves nothing either way.
  //   * Retry, by hand or on its own, carries the same key to every provider, and the server answers a
  //     repeat of a post that landed, or is landing, with that post.
  //   * While a send is slow, the row says which part is going ("Posting part 2 of 3…"), asked of the
  //     same route every OUTGOING_PROGRESS_MS, so a send that takes seconds never looks stuck.

  /** Whether `error` means the server's answer was lost rather than given. */
  function answerLost(error: DispatchFailure | null | undefined): boolean {
    // A gateway's 5xx is one whether its page is HTML (`gatewayStatus`) or JSON that is not this
    // server's error body (`code` "error", see `api`).
    return Boolean(error && (error.network || error.timedOut || (error.gatewayStatus || 0) >= 500 ||
      error.gatewayStatus === 408 || ((error.status || 0) >= 500 && error.code === "error")));
  }

  /** One ask of the outcome route for `body`'s post: its answer, `202` posting, or a thrown error. */
  function outgoingOutcome(entry: DispatchEntry, body: OutgoingBody, wait: number, signal: CancelSignal | null): Promise<PostAnswer | null> {
    return deps.api(`/api/v1/channels/${encodeURIComponent(entry.channel)}/reply/outcome?wait=${wait}`, {
      method: "POST", body, ...(signal ? { signal } : {}),
    }) as Promise<PostAnswer | null>;
  }

  /**
   * Ask what became of `entry`'s post until it has an answer, for at most OUTGOING_OUTCOME_MS: the
   * post's own answer (a 200's or a 207's body), or null where the server has no record of it, cannot
   * be reached, or the attempt was stopped.
   */
  async function askOutgoingOutcome(entry: DispatchEntry, job: DispatchJob, body: OutgoingBody): Promise<PostAnswer | null> {
    const until = now() + OUTGOING_OUTCOME_MS;
    entry.confirming = true;
    renderOutgoingEverywhere();
    try {
      while (!job.stopped && now() < until) {
        const controller = deps.cancellation.create();
        let timer: TimerHandle | null = null;
        // Each ask ends: a connection gone dead under it would otherwise hold this message, and every
        // one written after it to the same place, until the reader signed out.
        const stop = new Promise<never>((_resolve, reject) => {
          const end = () => {
            if (controller) controller.abort();
            reject(new Error("The ask went unanswered."));
          };
          job.cancel = end;
          timer = deps.clock.setTimeout(end, (OUTGOING_OUTCOME_WAIT_S + 10) * 1000);
        });
        let answer: PostAnswer | null;
        try {
          answer = await Promise.race([
            outgoingOutcome(entry, body, OUTGOING_OUTCOME_WAIT_S, controller && controller.signal), stop,
          ]);
        } catch (_error) {
          return null;
        } finally {
          if (timer !== null) deps.clock.clearTimeout(timer);
        }
        if (job.stopped) return null;
        if (answer && answer.state === "failed") {
          return { error: "post_failed", detail: String(answer.detail || ""), timing: answer.timing || null, uncertain: true };
        }
        if (!answer || answer.state !== "posting") return answer;
        noteOutgoingProgress(entry, answer);
      }
      return null;
    } finally {
      entry.confirming = false;
    }
  }

  /** Take a `202` posting answer's count of parts onto `entry`'s row. */
  function noteOutgoingProgress(entry: DispatchEntry, answer: PostAnswer): void {
    const posted = Math.max(0, Number(answer.posted) || 0);
    const parts = Math.max(0, Number((answer as { parts?: unknown }).parts) || 0);
    if (entry.progress && entry.progress.posted === posted && entry.progress.parts === parts) return;
    entry.progress = { posted, parts };
    renderOutgoingEverywhere();
  }

  /**
   * While `job`'s POST of `body` is on the wire, ask every OUTGOING_PROGRESS_MS how far it has got, one
   * ask at a time. Ends with the attempt. A failed ask changes nothing: it is only for the row's words.
   */
  function watchOutgoingProgress(entry: DispatchEntry, job: DispatchJob, body: OutgoingBody): void {
    const tick = () => {
      if (job.stopped || job.attemptOver || entry.confirming) return;
      outgoingOutcome(entry, body, 0, null).then((answer) => {
        if (!job.stopped && !job.attemptOver && answer && answer.state === "posting") noteOutgoingProgress(entry, answer);
      }, () => {}).finally(() => {
        if (!job.stopped && !job.attemptOver) deps.clock.setTimeout(tick, OUTGOING_PROGRESS_MS);
      });
    };
    deps.clock.setTimeout(tick, OUTGOING_PROGRESS_MS);
  }

  // --- sending again on its own ---------------------------------------------------------------------
  //
  // `#195 send-resilience`. The incident this answers: a slow chat service timed out two sends, each
  // was left as "Delivery unconfirmed. Check history before retrying", and the same words went
  // through from the service's own app a minute later. A send whose answer was lost may or may not
  // have landed, so sending it again is only safe where the second request is recognised as the
  // first. The server says where that is (`idempotent_posts_supported`): there each entry's id goes
  // with every attempt as its idempotency key, the server binds it to each part's text, and the
  // provider posts it at most once. After a partial post that holds for the unsent remainder only
  // where the server says the remainder is `resumable` — the part that failed goes again as the same
  // words under the same key. There, and only there, a transient failure goes again after 3 s, 6 s,
  // 12 s and then every 20 s, six times in all, sooner when the connection returns or the page comes
  // back into view, and then stops and offers Retry. Everywhere else the only automatic send is one
  // that never left the device — held while it knew it was offline, or a reply whose read to find out
  // which thread it goes in failed for a moment (`#277 send-order`), which posted nothing.
  //
  // `#277 send-order`. The waits were 5 s, 15 s and 45 s, counted from when the attempt FAILED. On a
  // slow bridge that was the wrong clock twice over: an attempt it takes half a minute to fail had
  // already waited that long, and a third wait of 45 s came after a failure that took a moment, when
  // a short wait would have found the bridge answering again — and every message written behind it
  // waits as long. So each wait now counts from when the failed attempt started (an attempt that took
  // longer than the wait goes again after the floor), and none is longer than OUTGOING_RETRY_MAX_MS.
  // The row says what it is waiting for and when; a message behind it says what it waits behind.
  //
  // Never after a reload: a reopened page has no record of what its last attempt did, and a send the
  // reader did not watch go out should not go out again on a page they did not ask to send from.
  // The decision itself — whether a send may go again, and when — is retries.ts's (`nextRetryAt`).

  /**
   * After a transient failure of the attempt that started at `startedAt`: arm the next automatic
   * attempt, or leave the row to the reader. Answers whether one was armed.
   *
   * `unposted`: the attempt posted nothing — finding out where a reply goes (`placeOutgoingMessage`)
   * — so going again is safe whether or not the provider honours a key, and spends one of the same
   * OUTGOING_RETRY_ATTEMPTS. `#277 send-order`.
   */
  function planOutgoingRetry(entry: DispatchEntry, startedAt: number, unposted = false): boolean {
    const at = nextRetryAt(entry, startedAt, now(), channelPostsOnce, unposted);
    if (at === null) return false;
    holdOutgoingMessage(entry, { at, offline: !deps.online(), unsent: false });
    return true;
  }

  /** Hold `entry` for an automatic send: at `at`, or when the connection returns. */
  function holdOutgoingMessage(entry: DispatchEntry, { at, offline, unsent }: { at: number; offline: boolean; unsent: boolean }): void {
    cancelOutgoingRetry(entry.id);
    const delay = Math.max(0, at - now());
    const waiting: OutgoingRetryHold = { at, offline, unsent, identity: deps.identity(), timer: null };
    if (!offline) waiting.timer = deps.clock.setTimeout(() => fireOutgoingRetry(entry.id), delay);
    outgoingRetries.set(entry.id, waiting);
  }

  function cancelOutgoingRetry(id: string): void {
    const waiting = outgoingRetries.get(id);
    if (!waiting) return;
    if (waiting.timer !== null) deps.clock.clearTimeout(waiting.timer);
    outgoingRetries.delete(id);
  }

  /** The wait is over. Still offline: wait for the connection instead. */
  function fireOutgoingRetry(id: string): void {
    const waiting = outgoingRetries.get(id);
    const entry = outbox.get(id);
    if (!waiting || !entry) return;
    if (waiting.timer !== null) deps.clock.clearTimeout(waiting.timer);
    waiting.timer = null;
    if (!deps.online()) {
      waiting.offline = true;
      renderOutgoingEverywhere();
      return;
    }
    // Signed in as someone else since: this is not a send that person made.
    if (waiting.identity !== deps.identity()) {
      outgoingRetries.delete(id);
      entry.detail = "Sign-in changed before this message was sent again.";
      persistOutgoingMessages();
      renderOutgoingEverywhere();
      return;
    }
    // A hold for a connection was never an attempt, so it spends none of the automatic ones.
    const automatic = !waiting.unsent;
    deps.quietly(() => { void retryOutgoingMessage(id, automatic); });
  }

  /** The connection is back, or the page is in view again after its timers were held. */
  function resumeOutgoingRetries(): void {
    for (const [id, waiting] of [...outgoingRetries]) {
      const due = waiting.offline ? deps.online() : waiting.at <= now();
      if (due) fireOutgoingRetry(id);
    }
  }

  function stopOutgoingSends(): void {
    // Nothing goes out on its own for a credential that is going away.
    for (const id of [...outgoingRetries.keys()]) cancelOutgoingRetry(id);
    // Every message on its way, and every one waiting its turn behind another (`#277 send-order`),
    // which has no job of its own to stop.
    for (const entry of outbox.values()) {
      if (entry.state !== "sending") continue;
      const job = outgoingJobs.get(entry.id);
      if (job) job.stopped = true;
      const attempted = entry.phase === "posting";
      entry.state = attempted ? "unconfirmed" : "failed";
      if (attempted) entry.mayHavePosted = true;
      outgoingObservations.delete(entry.id);
      entry.detail = attempted ? "Sign-in changed before delivery was confirmed."
        : "Sign-in changed before this message was sent.";
      if (job && job.cancel) job.cancel();
      entry.phase = "";
      settleOutgoing(entry.id);
    }
    persistOutgoingMessages();
    renderOutgoingEverywhere();
  }

  /**
   * Take an entry out of the outbox, durably, in every column — and let the next message to its
   * destination go, which was waiting behind it (`#277 send-order`).
   */
  function dropOutgoing(id: string): void {
    cancelOutgoingRetry(id);
    outbox.remove(id);
    outgoingObservations.delete(id);
    settleOutgoing(id);
    persistOutgoingMessages();
    pumpOutgoing();
    renderOutgoingEverywhere();
  }

  return {
    queue: queueOutgoingMessage,
    retry: retryOutgoingMessage,
    schedule: scheduleOutgoingMessage,
    blocker: outgoingBlocker,
    credential: currentCredential,
    noteQueuedUnder: (id) => { outgoingQueuedUnder.set(id, currentCredential()); },
    retryOf: (id) => outgoingRetries.get(id),
    retryPending: (id) => outgoingRetries.has(id),
    testKey: (id) => IDEMPOTENCY_KEY.test(id),
    retriesItself,
    editable,
    waitingItsTurn,
    attempting,
    cancelRetry: cancelOutgoingRetry,
    hold: holdOutgoingMessage,
    drop: dropOutgoing,
    resumeRetries: resumeOutgoingRetries,
    stop: stopOutgoingSends,
    pump: pumpOutgoing,
  };
}







