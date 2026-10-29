// How new a page of the server's is, on the server's own clock (`#473
// replication-c7-server-versions`; `#624 voice-page-modularity`).

/** `#473 replication-c7-server-versions`. How new a page of the server's is, on the server's clock. */
export interface ServerVersions {
  readonly observed: number;
  readonly settledBelow: number | null;
}

/**
 * `#473 replication-c7-server-versions`. How new a page of the server's is, on the server's clock: when
 * its read began (`observed_ms`), and on a newest page the provider time below which its absence of a
 * message proves the message gone (`settled_below_ms`). Null from an older server, which says neither:
 * the page then orders copies by when it sent its reads (`copyOrder`), as before.
 */
export function serverVersions(payload: unknown): ServerVersions | null {
  const page = payload as { observed_ms?: unknown; settled_below_ms?: unknown } | null | undefined;
  if (!page || typeof page.observed_ms !== "number") return null;
  return {
    observed: page.observed_ms,
    settledBelow: typeof page.settled_below_ms === "number" ? page.settled_below_ms : null,
  };
}
