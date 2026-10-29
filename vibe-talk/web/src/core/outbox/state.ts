// What a send in the outbox can do and say, decided from the entry alone (`#193 compose-draft-loss`,
// `#195 send-resilience`, `#277 send-order`, `#500 sent-receipts-pile-up`; `#624
// voice-page-modularity`, moved from the legacy script; plan step 12). What the page knows beyond the
// entry (an attempt in progress, the rows drawn, the store's channel) is passed in.

import { OWN_POST_GRACE_MS } from "./own.ts";

/**
 * How long a confirmed send's "Sent." receipt is shown where the message itself is not drawn — a reply
 * into a thread, sent from Main or the Threads view's cards — before it goes once the page holds the
 * message (`sentReceiptRetires`); and it goes by OWN_POST_GRACE_MS after confirmation whatever the
 * page holds.
 */
const SENT_RECEIPT_MS = 15000;

/** How much of a message a control quotes to name it. */
const SEND_SNIPPET_CHARS = 28;

/** The fields of an outbox entry these decisions read. */
export interface SendState {
  readonly id: string;
  readonly channel: string;
  readonly state: string;
  readonly phase?: string;
  readonly text: string;
  readonly remaining?: string;
  readonly postedCount?: number;
  readonly mayHavePosted?: boolean;
  readonly sentAt?: number;
  readonly parts: readonly { readonly id: string }[];
  readonly observedParts: readonly string[];
}

/** What a send has not delivered: all of it, or what a partial post left unsent. */
export function unsentText(entry: SendState): string {
  return (entry.postedCount ? entry.remaining : entry.text) as string;
}

/**
 * Whether `entry` is waiting behind another message to its destination, with no attempt of its own
 * in progress (`attempting`): nothing of it is on its way, so it can be taken back.
 */
export function waitingItsTurn(entry: SendState, attempting: boolean): boolean {
  return entry.state === "sending" && entry.phase === "queued" && !attempting;
}

/**
 * Whether Edit may take `entry` back: refused, or never sent — failed, or still waiting its turn
 * behind another message — so that none of its unsent text can be in the channel already. Not a send
 * whose delivery is unconfirmed, or ever was (`mayHavePosted`, kept through a later refusal): its
 * outbox id is the idempotency key that lets Retry go again without posting it twice, and Edit would
 * drop it, so the same words sent again from the box would be a second message wherever the first
 * had landed after all — the `#195 send-resilience` incident. Retry and Dismiss are its ways out.
 */
export function editableSend(entry: SendState, attempting: boolean): boolean {
  return (entry.state === "failed" || waitingItsTurn(entry, attempting)) && entry.mayHavePosted !== true;
}

/** `text` under `draft`, after a blank line, or `text` alone where there is no draft. */
export function underDraft(draft: string | null | undefined, text: string): string {
  const kept = String(draft || "").replace(/\s+$/, "");
  return kept.trim() ? `${kept}\n\n${text}` : text;
}

/** The first few words of a message (`chars` at most), on one line, for a control that has to name it. */
export function sendSnippet(text: unknown, chars: number = SEND_SNIPPET_CHARS): string {
  const words = String(text).replace(/\s+/g, " ").trim();
  if (words.length <= chars) return words;
  let cut = words.slice(0, chars - 1);
  // At the end of a word, where one ends in the second half, rather than in the middle of one.
  const end = words[cut.length] === " " ? cut.length : cut.lastIndexOf(" ");
  if (end > chars / 2) cut = cut.slice(0, end);
  return `${cut.trimEnd()}…`;
}

/** What `sentReceiptRetires` asks of the page, beyond the entry. */
export interface ReceiptView {
  /** The wall clock, in milliseconds. */
  now(): number;
  /** Whether the page's store keeps its own posts in `channel` against a page from older state. */
  keepsOwnPosts(channel: string): boolean;
  /** The selected channel's id. */
  selectedChannel(): string;
  /** The ids of the messages the store holds, where its channel is `channel`; null where it is another. */
  storedIn(channel: string): ReadonlySet<string> | null;
}

/**
 * Whether the receipt of `entry`, a send, retires now: once a read carried every part; once
 * OWN_POST_GRACE_MS has passed since confirmation, whatever the page holds; and, where the store
 * keeps the page's own posts, once it has been shown SENT_RECEIPT_MS and every part is drawn in this
 * column (`drawn`) or held in the selected channel's store.
 */
export function sentReceiptRetires(entry: SendState, drawn: ReadonlySet<string>, view: ReceiptView): boolean {
  if (entry.state !== "sent" || !entry.parts.length) return false;
  if (entry.parts.every((part) => entry.observedParts.includes(part.id))) return true;
  const shown = view.now() - (Number(entry.sentAt) || 0);
  if (shown >= OWN_POST_GRACE_MS) return true;
  if (shown < SENT_RECEIPT_MS || !view.keepsOwnPosts(entry.channel)) return false;
  const here = entry.channel === view.selectedChannel();
  const stored = here ? view.storedIn(entry.channel) : null;
  return here && entry.parts.every((part) => drawn.has(part.id) || Boolean(stored && stored.has(part.id)));
}

export { SEND_SNIPPET_CHARS, SENT_RECEIPT_MS };
