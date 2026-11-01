// A channel place as an address's query, and back (`#462 webapp-deep-links`): `c` the channel, `v`
// the view, `t` the thread, `m` a focused message. Which link the page opened by, and keeping the
// address in step with the reader, are the page's. Moved here from web/voice.js unchanged but for
// its types (`#624 voice-page-modularity`).

import { DEEP_LINK_PART_MAX, deepLinkRefText } from "../../foundation/index.ts";
import type { DeepLinkRef } from "../../foundation/index.ts";
import type { ChannelPlace } from "./places.ts";

/** The views a link may name (`voice` read only to refuse it: a link starts no call). */
const DEEP_LINK_VIEWS: readonly string[] = Object.freeze(["main", "all", "threads", "thread", "columns", "voice"]);

/**
 * The address's query as a name-to-value map, read without `URLSearchParams`: the page runs in
 * hosts without it (its own test fixture is one), and a query this small wants no parser but
 * splitting on its two separators. A value that will not unescape is left out whole.
 */
function deepLinkParams(search: unknown): Map<string, string> {
  const params = new Map<string, string>();
  for (const pair of String(search || "").replace(/^\?/, "").split("&")) {
    if (!pair) continue;
    const at = pair.indexOf("=");
    const key = at < 0 ? pair : pair.slice(0, at);
    try {
      params.set(decodeURIComponent(key), decodeURIComponent((at < 0 ? "" : pair.slice(at + 1)).replace(/\+/g, " ")));
    } catch (_error) {
      // A malformed escape: that pair says nothing.
    }
  }
  return params;
}

/** One query value from the address, bounded as an id is; null for an absent or absurd one. */
function deepLinkPart(params: ReadonlyMap<string, string>, key: string): string | null {
  const value = params.get(key);
  return typeof value === "string" && value !== "" && value.length <= DEEP_LINK_PART_MAX ? value : null;
}

/** The query for channel place `place` (and a focused message in it), each value escaped. */
function deepLinkQuery(place: ChannelPlace, ref?: DeepLinkRef | null): string {
  const pairs: [string, string][] = [["c", place.channel]];
  if (place.layout === "list") pairs.push(["v", "threads"]);
  else if (place.layout === "two") {
    pairs.push(["v", "columns"]);
    if (place.side) pairs.push(["t", place.side]);
  } else if (place.view === "thread" && place.thread) {
    pairs.push(["v", "thread"], ["t", place.thread]);
  } else if (place.view === "flat") pairs.push(["v", "all"]);
  else pairs.push(["v", "main"]);
  if (ref && ref.channel === place.channel) pairs.push(["m", deepLinkRefText(ref)]);
  return pairs.map(([key, value]) => `${key}=${encodeURIComponent(value)}`).join("&");
}

// What the page and the tests use of it.
export { DEEP_LINK_VIEWS, deepLinkParams, deepLinkPart, deepLinkQuery };
