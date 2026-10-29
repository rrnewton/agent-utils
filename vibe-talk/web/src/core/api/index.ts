// The page's requests to its server: the token, the error taxonomy, the contract check and the
// reader's time limit (`#195 send-resilience`). Its public interface. `#624 voice-page-modularity`.

export { createApi } from "./client.ts";
export type { ApiDependencies, ApiPorts, ApiRequestError, ApiRequestOptions, ServerApi } from "./client.ts";
export { SLOW_WORDS, isSlowFailure, providerWords, readableFailure } from "./failures.ts";
export type { FailureFacts } from "./failures.ts";
