// The offline message cache's page half (`#18 offline-message-cache`; `#624
// voice-page-modularity`, moved from the legacy script into view/channel): the device's saved
// copy instantiated over the device's storage. The envelope itself is core/channels'
// (`createMessageCache`); the reload that draws it stays the page's.

import { createMessageCache } from "../../core/channels/index.ts";
import type { MessageCache } from "../../core/channels/index.ts";

/** What the offline cache asks of the page. */
export interface OfflineCacheDependencies {
  storage(): unknown;
  token(): unknown;
  channels(): unknown[];
  stillToDo(message: never, done?: unknown): boolean;
}

/** The device's saved copy, as one owner. */
export function createOfflineCache(deps: OfflineCacheDependencies): { cache: MessageCache } {
  const cache = createMessageCache({
    storage: deps.storage(),
    token: deps.token(),
    channels: () => deps.channels(),
    stillToDo: (message: never, done?: unknown) => deps.stillToDo(message, done),
  } as never) as MessageCache;
  return { cache };
}
