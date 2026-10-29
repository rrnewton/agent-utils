// The outbox: what is waiting to be sent, and what became of each send. Its public interface.
// `#624 voice-page-modularity`.

export { OUTGOING_RETRY_ATTEMPTS, nextRetryAt, outgoingRetryWait, retriesItself } from "./retries.ts";
export type { RetryFacts } from "./retries.ts";
