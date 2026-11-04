// The outbox: what is waiting to be sent, and what became of each send. Its public interface.
// `#624 voice-page-modularity`.

export { OUTGOING_RETRY_ATTEMPTS, nextRetryAt, outgoingRetryWait, retriesItself } from "./retries.ts";
export type { RetryFacts } from "./retries.ts";
export { DISMISSED_SENDS_DAYS, DISMISSED_SENDS_LIMIT, createDismissedSends } from "./dismissed.ts";
export type { DismissedSend, DismissedSends, DismissedSendsDependencies } from "./dismissed.ts";
export { OWN_POST_GRACE_MS, createOwnPosts } from "./own.ts";
export type { OwnPosts, OwnPostsDependencies } from "./own.ts";
export { outgoingFromSaved, savedWire } from "./saved.ts";
export type { OutgoingEntry, OutgoingPart, OutgoingWire } from "./saved.ts";
export { QUICK_DRAFTS_KEY, createChannelDrafts } from "./drafts.ts";
export { createOutboxPump, createSendNumbers } from "./pump.ts";
export type { OutboxPump, SendNumbers } from "./pump.ts";
export { DRAFT_LEDGER_KEY, createDraftLedger, createDraftRound, draftIdentity } from "./draftledger.ts";
export type { DraftEntry, DraftKind, DraftLedger, DraftLedgerDependencies, DraftRound } from "./draftledger.ts";
export type { ChannelDrafts, ChannelDraftsDependencies } from "./drafts.ts";
export {
  SEND_SNIPPET_CHARS,
  SENT_RECEIPT_MS,
  editableSend,
  sendSnippet,
  sentReceiptRetires,
  underDraft,
  unsentText,
  waitingItsTurn,
} from "./state.ts";
export type { ReceiptView, SendState } from "./state.ts";
export { createOutboxStore } from "./store.ts";
export type { OutboxDependencies, OutboxEntry, OutboxStore } from "./store.ts";
export { DRAFTS_KEY, createReplyDrafts } from "./replydrafts.ts";
export type { ReplyDrafts, ReplyDraftsDependencies } from "./replydrafts.ts";
export { createPendingThreads } from "./pending.ts";
export type { PendingSend, PendingThreads, PendingThreadsDependencies } from "./pending.ts";
export { SEND_UNDO_MS, createSendUndo } from "./sendundo.ts";
export type { SendDismissal, SendUndo, SendUndoDependencies } from "./sendundo.ts";
export { createUnconfirmedSends } from "./unconfirmed.ts";
export type { UnconfirmedSends, UnconfirmedSendsDependencies } from "./unconfirmed.ts";
