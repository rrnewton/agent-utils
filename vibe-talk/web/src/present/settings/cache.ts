// What Settings' group for the server's own message cache says (`#208 persistent-cache`, `#595
// cache-full-history`; `#624 voice-page-modularity`, moved from the legacy script into present/settings).
// The status line and what an erase takes are web/src/present/notices's; the group is
// web/src/view/settings's.

import type { MessageCacheStatus } from "../../contract/index.ts";
import { SERVER_CACHE_NO_AGE_LIMIT, nounCount } from "../notices/index.ts";

/** The switches the group sends: on, off (which erases), and erase (which carries on). */
type ServerCacheAct = "on" | "off" | "erase";

/** What the line says while a request is out. */
const SERVER_CACHE_SENDING: { readonly [act in ServerCacheAct]: string } = Object.freeze({
  on: "Turning it on…",
  off: "Turning it off and erasing its file…",
  erase: "Erasing its file…",
});

/** The question before each erasing act, and its two buttons. */
const SERVER_CACHE_ASK: { readonly [act in "off" | "erase"]: { readonly ask: (what: string) => string; readonly yes: string; readonly no: string } } =
  Object.freeze({
    off: {
      ask: (what: string) => `Turn saving off and erase ${what} now?`,
      yes: "Turn off and erase",
      no: "Keep saving",
    },
    erase: {
      ask: (what: string) => `Erase ${what} now? Saving carries on in an empty file.`,
      yes: "Erase now",
      no: "Keep them",
    },
  });

/**
 * Whether the server says it holds every message of every channel it caches back to its beginning,
 * so of the one on screen too. The status counts channels and never names one, so a server still
 * gathering one channel says this of none. `#595 cache-full-history`.
 */
function serverHoldsWholeHistory(serverCache: MessageCacheStatus | null): boolean {
  const history = serverCache && serverCache.available ? serverCache.history : null;
  return Boolean(history && history.mode === "full" && history.channels > 0 &&
    history.complete === history.channels);
}

/** How long the server keeps messages, as its help entry says it. */
const serverCacheKeepsWords = (status: MessageCacheStatus): string =>
  status.max_days === SERVER_CACHE_NO_AGE_LIMIT ? "their whole history" : nounCount(status.max_days, "day");

/** Why the line says nothing changed: the server did not confirm the switch. */
const serverCacheUnconfirmed = (why: string | undefined): string => `The server did not confirm that: ${why}`;

export { SERVER_CACHE_ASK, SERVER_CACHE_SENDING, serverCacheKeepsWords, serverCacheUnconfirmed, serverHoldsWholeHistory };
export type { ServerCacheAct };
