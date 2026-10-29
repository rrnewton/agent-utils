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
export { createChannelDrafts } from "./drafts.ts";
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
