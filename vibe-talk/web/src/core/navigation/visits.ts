// The places the reader has visited on the channel, for Back and Forward (`#423 back-key-consistency`;
// `#624 voice-page-modularity`, moved from the legacy script into core/navigation): the places noted since
// the channel was chosen, which one the reader is at, noting a new one after it as a browser does, a close
// onto the place before taken as a step back, and walking back or forward past places that cannot be shown.
// Reading the place the reader is at and showing a place are the page's; THE RULE and THE SPECIAL CASES are
// told where the legacy script opens the section.

import type { Place } from "./places.ts";
import { placeIsOver, visitKey } from "./places.ts";

/** The most places kept: far more than a reader walks back through, and a bound on what is held. */
const PLACES_KEPT = 100;

/** The places' commands and questions. */
export interface Visits {
  /** Note `place`, the place the reader is at, as the one after the place they were at, unless it is that place. */
  note(place: Place | null): void;
  /**
   * Walk `step` places (-1 Back, +1 Forward) from the one the reader is at, passing over what `showable` says
   * cannot be shown now and `here`, the place the reader is at; `go` is shown the place landed on, as an
   * arrival, which is noted as none. True where a place was landed on.
   */
  walk(step: number, here: string, showable: (place: Place) => boolean, go: (place: Place) => void): boolean;
  /** A Back or Forward is arriving: what it lands on stands for the place it went to. */
  arriving(): boolean;
  /** Which place the reader is at, and how many are noted: what a link applied compares across its wait. */
  position(): { readonly at: number; readonly length: number };
}

/** No place noted. */
export function createVisits(): Visits {
  /** The places visited on this channel, the first first. */
  let visited: Place[] = [];
  /** Which of `visited` the reader is at. */
  let visitedAt = -1;
  /** A Back or Forward is arriving: what it lands on stands for the place it went to, and is noted as none. */
  let placeArriving = false;

  function note(place: Place | null): void {
    if (!place) return;
    if (visited.length > 0 && (visited[0] as Place).channel !== place.channel) {
      visited = [];
      visitedAt = -1;
    }
    if (visitedAt >= 0 && visitKey(visited[visitedAt] as Place) === visitKey(place)) {
      // The same place: what it says of itself kept fresh (a thread's way back, Help's open entry).
      visited[visitedAt] = place;
      return;
    }
    if (placeArriving && visitedAt >= 0) {
      visited[visitedAt] = place;
      return;
    }
    // What was open over the place before closed onto it (Esc, the X, the browser's Back, a Back button):
    // a step back along the history, as Ctrl/Cmd+[ would have been, the place left kept ahead for Forward.
    if (visitedAt > 0 && placeIsOver(visited[visitedAt] as Place) && visitKey(visited[visitedAt - 1] as Place) === visitKey(place)) {
      visitedAt -= 1;
      visited[visitedAt] = place;
      return;
    }
    visited = visited.slice(0, visitedAt + 1);
    visited.push(place);
    if (visited.length > PLACES_KEPT) visited = visited.slice(visited.length - PLACES_KEPT);
    visitedAt = visited.length - 1;
  }

  return {
    note,
    walk(step, here, showable, go) {
      for (let at = visitedAt + step; at >= 0 && at < visited.length; at += step) {
        const place = visited[at] as Place;
        // Passed over: what cannot be shown now, and the place the reader is at, noted twice where a window
        // narrowed and widened again took two columns away and back.
        if (!showable(place) || visitKey(place) === here) continue;
        placeArriving = true;
        try {
          visitedAt = at;
          go(place);
        } finally {
          placeArriving = false;
        }
        return true;
      }
      return false;
    },
    arriving: () => placeArriving,
    position: () => ({ at: visitedAt, length: visited.length }),
  };
}

// What the page and the tests use of it.
export { PLACES_KEPT };
