// The place marker this browser kept before the reading bookmark (`#240 reading-bookmark`; `#624
// voice-page-modularity`, moved from the legacy script into core/annotations): the id it left in the
// device's storage, read once as the page starts, and how many channel reads have looked for its message,
// until it is adopted as the bookmark or dropped. Finding its row and moving the bookmark above it are the
// page's.

import type { StoragePort } from "../../ports/index.ts";

/** Where the old place marker kept the id of the message the reader was on. */
const MARKER_KEY = "vibe-talk.voice.place-marker";

/**
 * How many channel reads the old place marker is looked for in before it is dropped unfound: the look is
 * a pass over the list on every redraw, and an id whose message is never loaded again would be looked for
 * in every redraw of every session.
 */
const LEGACY_MARKER_READS = 50;

/** What the old marker asks of the page. */
export interface LegacyMarkerDependencies {
  /** The device's storage: any call may throw. */
  readonly storage: StoragePort;
}

/** The old marker's commands and questions. */
export interface LegacyMarker {
  /** The id the old place marker left in this browser, until it is adopted or dropped; null with none. */
  id(): string | null;
  /** A channel read carried a bookmark while the old marker is still looked for. */
  noteRead(): void;
  /** Whether it has been looked for in `LEGACY_MARKER_READS` channel reads without being found. */
  exhausted(): boolean;
  /** Forget the old place marker, here and in the browser's storage. */
  drop(): void;
}

/** As the device's storage holds it now. */
export function createLegacyMarker(deps: LegacyMarkerDependencies): LegacyMarker {
  /** The channel reads that carried a bookmark while the old place marker was still looked for. */
  let legacyMarkerReads = 0;
  /** The id the old place marker left in this browser, until it is adopted or dropped. */
  let legacyMarker: string | null = null;
  try {
    legacyMarker = deps.storage.getItem(MARKER_KEY);
  } catch (_error) {
    legacyMarker = null;
  }
  return {
    id: () => legacyMarker,
    noteRead() {
      if (legacyMarker !== null) legacyMarkerReads += 1;
    },
    exhausted: () => legacyMarkerReads >= LEGACY_MARKER_READS,
    drop() {
      legacyMarker = null;
      try {
        deps.storage.removeItem(MARKER_KEY);
      } catch (_error) {
        // A browser that refuses storage asks again next time, and the server already has it then.
      }
    },
  };
}

// What the page and the tests use of it.
export { LEGACY_MARKER_READS, MARKER_KEY };
