// Loading what a previous page left unsent, and what landing reads say of it (`#277
// send-order`, `#500 sent-receipts-pile-up`; `#624 voice-page-modularity`, moved from the
// legacy script into core/outbox; mu-readcore): saved entries restored (never replaying a
// POST on load), parts observed in landing pages, the stream's echo of an own send, an
// unconfirmed send settled from the server, and when a sent receipt retires. Dispatch
// (queue/post/retry) stays with the send service; this owner only loads and notes.
// Entries are stored exactly as `saved.ts` decoded them and persisted from the live objects:
// this layer names no fields of its own, so a field the decode grows rides load and persist unread.

import { outgoingFromSaved } from "./saved.ts";
import type { OutboxStore } from "./store.ts";
import { SENT_RECEIPT_MS } from "./state.ts";
import type { OutgoingEntry } from "./saved.ts";
import { sentReceiptRetires as receiptRetires } from "./state.ts";
import { OWN_POST_GRACE_MS } from "./own.ts";
import type { ClockPort } from "../../ports/index.ts";



/** A saved entry as the live outbox holds it: sending states and page-gained fields included. */
export type LiveOutgoingEntry = Omit<OutgoingEntry, "state"> & {
  state: string;
  detail?: string;
  echoed?: boolean;
  resolveOnly?: { detail: string } | null;
  inPlace?: string;
  inCards?: boolean;
};

/** What loading asks of the page. */
export interface OutboxLoadsDependencies {
  store: OutboxStore<LiveOutgoingEntry>;
  now(): number;
  clock: ClockPort;
  blocker(entry: LiveOutgoingEntry): unknown;
  credential(): unknown;
  /** Note the credential a restored queued entry waits under. */
  noteQueuedUnder(id: string): void;
  keepsOwnPosts(channel: string): boolean;
  selectedChannel(): string;
  storedIn(channel: string): ReadonlySet<string> | null;
  schedule(entry: LiveOutgoingEntry): void;
  retryPending(id: string): boolean;
  testKey(id: string): boolean;
  observations: Map<string, Set<unknown>>;
  onChange(): void;
}

/** Loading's commands. */
export interface OutboxLoads {
  persist(): boolean;
  load(): void;
  observe(messages: readonly { id: unknown; channel_id: unknown }[]): void;
  noteEcho(id: string): void;
  settleFromServer(id: string): void;
  receiptRetires(entry: LiveOutgoingEntry, drawn: ReadonlySet<string>): boolean;
  armSentReceipt(entry: LiveOutgoingEntry, due?: number): void;
}

/** Loading over `deps`. */
export function createOutboxLoads(deps: OutboxLoadsDependencies): OutboxLoads {
  const outbox = deps.store;
  const now = () => deps.now();
  const outgoingBlocker = deps.blocker;
  const currentCredential = deps.credential;
  const outgoingObservations = deps.observations;
  const outgoingRetries = { has: (id: string) => deps.retryPending(id) };
  const IDEMPOTENCY_KEY = { test: (id: string) => deps.testKey(id) };

  function persistOutgoingMessages(): boolean {
    return outbox.save();
  }

function loadOutgoingMessages(): void {
  try {
    const saved = outbox.saved();
    if (!Array.isArray(saved)) return;
    const queued = [];
    let confirmed = false;
    for (const held of saved) {
      const entry = outgoingFromSaved(held, now) as LiveOutgoingEntry | null;
      if (!entry) continue;
      // `#500 sent-receipts-pile-up`. Nor a confirmed send saved by an earlier build, which did not say
      // when it was confirmed, or past the grace (`sentReceiptRetires`): the owner's pile of "Sent."
      // receipts, all of them posted long before, goes on the first load.
      if (entry.state === "sent" && !(now() - entry.sentAt < OWN_POST_GRACE_MS)) {
        confirmed = true;
        continue;
      }
      outbox.add(entry);
      if (entry.state === "sent") armSentReceipt(entry, OWN_POST_GRACE_MS);
      if (held.state === "sending" && held.phase === "queued") queued.push(entry);
    }
    // `#277 send-order`. A message that was waiting behind an earlier one to the same place still
    // is: it never left the device, and it goes once that one has — after a Retry on this page, or
    // a Dismiss or Edit. So does a reply still to be placed that was waiting behind another. One
    // with nothing ahead of it any more is NOT sent on its own: nobody asked this page to send it,
    // so it is left to Retry, as before. In order, so a message restored to the queue holds the ones
    // written after it.
    for (const entry of queued) {
      Object.assign(entry, { state: "sending", phase: "queued", detail: "" });
      if (outgoingBlocker(entry)) {
        deps.noteQueuedUnder(entry.id);
      } else {
        Object.assign(entry, { state: "failed", phase: "", detail: "This page closed before this message was sent." });
      }
    }
    // In particular, never replay a POST merely because Android reopened this page.
    if (outbox.size() || confirmed) persistOutgoingMessages();
  } catch (_error) {
    outbox.refused();
  }
}

function observeOutgoingMessages(messages: readonly { id: unknown; channel_id: unknown }[]): void {
  let changed = false;
  for (const entry of outbox.values()) {
    if (entry.state !== "sending" && entry.state !== "sent") continue;
    if (entry.state === "sending" && entry.phase !== "posting") continue;
    const seen = outgoingObservations.get(entry.id) || new Set(entry.observedParts);
    const partIds = new Set(entry.parts.map((part) => part.id));
    for (const message of messages) {
      const id = String(message.id);
      if (String(message.channel_id) === entry.channel &&
          (entry.state === "sending" || partIds.has(id))) seen.add(id);
    }
    outgoingObservations.set(entry.id, seen);
    const observed = entry.parts.filter((part) => seen.has(part.id)).map((part) => part.id);
    if (observed.length !== entry.observedParts.length) {
      entry.observedParts = observed;
      changed = true;
    }
  }
  if (changed) persistOutgoingMessages();
}

function noteOutgoingEcho(id: string): void {
  const entry = outbox.get(id);
  if (!entry || entry.state === "sent" || entry.echoed) return;
  entry.echoed = true;
  outbox.touch();
  deps.onChange();
}

function settleFromServer(id: string): void {
  const entry = outbox.get(id);
  if (!entry || entry.state !== "unconfirmed" || !IDEMPOTENCY_KEY.test(entry.id) || outgoingRetries.has(id) ||
      !currentCredential()) return;
  entry.resolveOnly = { detail: entry.detail };
  entry.state = "sending";
  entry.phase = "queued";
  deps.schedule(entry);
}

function sentReceiptRetires(entry: LiveOutgoingEntry, drawn: ReadonlySet<string>): boolean {
  return receiptRetires(entry, drawn, {
    now: () => deps.now(),
    keepsOwnPosts: (channel) => deps.keepsOwnPosts(channel),
    selectedChannel: () => deps.selectedChannel(),
    storedIn: (channel) => deps.storedIn(channel),
  });
}

function armSentReceipt(entry: LiveOutgoingEntry, due: number = SENT_RECEIPT_MS): void {
  deps.clock.setTimeout(() => {
    if (outbox.get(entry.id) !== entry || entry.state !== "sent") return;
    // Timers can fire before the wall-clock deadline. Retry that deadline instead of
    // postponing a still-visible receipt straight to the ten-minute fallback.
    if (now() - entry.sentAt < due) {
      armSentReceipt(entry, due);
      return;
    }
    deps.onChange();
    if (outbox.get(entry.id) === entry && due < OWN_POST_GRACE_MS) armSentReceipt(entry, OWN_POST_GRACE_MS);
  }, Math.max(1, due - (now() - entry.sentAt)));
}
  return {
    persist: persistOutgoingMessages,
    load: loadOutgoingMessages,
    observe: observeOutgoingMessages,
    noteEcho: noteOutgoingEcho,
    settleFromServer,
    receiptRetires: sentReceiptRetires,
    armSentReceipt,
  };
}
