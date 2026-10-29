// The outbox: what is waiting to be sent, and what became of each send. Its public interface.
// `#624 voice-page-modularity`.

export { OUTGOING_RETRY_ATTEMPTS, nextRetryAt, outgoingRetryWait, retriesItself } from "./retries.ts";
export type { RetryFacts } from "./retries.ts";
export { DISMISSED_SENDS_DAYS, DISMISSED_SENDS_LIMIT, createDismissedSends } from "./dismissed.ts";
export type { DismissedSend, DismissedSends, DismissedSendsDependencies } from "./dismissed.ts";
