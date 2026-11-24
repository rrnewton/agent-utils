// Taps that land before their control is wired (`#2216 vibe-talk-pace-availability-flake`).
//
// The dock's tiles stand in the shell from the first paint, but their listeners mount late
// in the ordered boot (`mountPreferenceSwitches` for Pace, `mountReadControls` for Read):
// a tap in that window — the owner's first second with the page — reached a control that
// could not answer and was lost for good. This recorder stands from the bundle's first
// moments (capture phase, before every mount below it) and remembers the first tap on
// each watched control; the mount that wires the control asks for its tap and replays it,
// and the recorder is stopped once the boot it covers is done.

/** The early taps' commands. */
export interface EarlyTaps {
  /** Whether a tap on `id` landed before its control was wired; consumed by the asking. */
  take(id: string): boolean;
  /** Stop recording: the boot window is over. */
  stop(): void;
}

/** The slice of a document the recorder needs. */
export interface EarlyTapDocument {
  addEventListener(type: string, listener: (event: Event) => void, capture: boolean): void;
  removeEventListener(type: string, listener: (event: Event) => void, capture: boolean): void;
}

/** Record the first tap on each of `ids` (element ids) from now until `stop`. */
export function createEarlyTaps(doc: EarlyTapDocument, ids: readonly string[]): EarlyTaps {
  const seen = new Set<string>();
  const onClick = (event: Event): void => {
    const target = event.target as (Node & { closest?: (selector: string) => unknown }) | null;
    if (!target || typeof target.closest !== "function") return;
    for (const id of ids) {
      if (target.closest(`#${id}`)) {
        seen.add(id);
        break;
      }
    }
  };
  doc.addEventListener("click", onClick, true);
  return {
    take(id: string): boolean {
      const had = seen.has(id);
      seen.delete(id);
      return had;
    },
    stop(): void {
      // A document that cannot remove (a test double) simply keeps a
      // recorder that nothing will ask again.
      if (typeof doc.removeEventListener === "function") doc.removeEventListener("click", onClick, true);
    },
  };
}
