// The live anchor debug mode (`#2091 vibe-talk-shrink-anchor-policy`, owner
// request 308 (c)): opt-in, page-only, read-only. Off unless the page is opened
// with `?anchordebug=1` or the device holds `vibe-talk.voice.anchor-debug` as
// "1". While on, every reader-caused height change the scroll owner settles is
// judged by the one numeric invariant (`anchorpolicy.anchorInvariantHolds`,
// the same function the tests call); a violation is recorded to the console
// and to the freeze telemetry channel from `#2066` (the freeze-report route;
// timings only, never content: the record carries the action kind, the anchor
// kind, the delta in px and whether the lists are windowed — no ids, no words
// from the screen). The mode draws nothing and changes no behaviour: with it
// off, the scroll owner never calls it.

import { anchorDeltaPx, anchorInvariantHolds } from "./anchorpolicy.ts";
import type { AnchorCheck } from "./anchorpolicy.ts";

/** The device key holding the debug choice, as the read mode's is held. */
export const ANCHOR_DEBUG_KEY = "vibe-talk.voice.anchor-debug";

/** What the debug mode asks of the page. */
export interface AnchorDebugDependencies {
  /** The page's clock (`performance.now`). */
  now(): number;
  /** Post one freeze-report body, as the freeze watch posts its own. */
  send(text: string): Promise<boolean>;
  /** Whether a report may be posted now (write scope, online, in sight). */
  canPost(): boolean;
  /** What the page is showing, in the freeze record's own words. */
  state(): { view: string; screen: string; channelView: string; twoColumns: boolean; windowed: boolean };
  /** The page's location query, for `?anchordebug=1`. */
  query(): string;
  /** A device-held choice by key, where the device holds one. */
  held(key: string): string | null;
}

/** A violation to record: the act, and the invariant's own measurement of it. */
export interface AnchorViolation extends AnchorCheck {
  readonly action: string;
}

/** The debug mode's one command. */
export interface AnchorDebug {
  /** Whether the mode is on (the URL flag or the held choice). */
  enabled(): boolean;
  /** Judge one settled change; record it where the invariant is broken. */
  check(violation: AnchorViolation): void;
}

const VIEWS: readonly string[] = ["discord", "voice", "docs"];
const SCREENS: readonly string[] = ["signin", "main", "settings", "help", "welcome"];
const CHANNEL_VIEWS: readonly string[] = ["main", "flat", "thread", "threads"];

function oneOf(words: readonly string[], word: string, absent: string): string {
  return words.includes(word) ? word : absent;
}

/** Make the debug mode over `deps`. It watches nothing on its own: the scroll owner calls it. */
export function createAnchorDebug(deps: AnchorDebugDependencies): AnchorDebug {
  const startedAt = deps.now();
  let on: boolean | null = null;

  function enabled(): boolean {
    if (on !== null) return on;
    try {
      on = new URLSearchParams(deps.query()).get("anchordebug") === "1"
        || deps.held(ANCHOR_DEBUG_KEY) === "1";
    } catch (_error) {
      on = false;
    }
    return on;
  }

  function check(violation: AnchorViolation): void {
    if (!enabled()) return;
    if (anchorInvariantHolds(violation)) return;
    const delta = anchorDeltaPx(violation);
    const state = deps.state();
    const words = `anchor-violation action=${violation.action} kind=${violation.kind} delta_px=${delta.toFixed(2)} windowed=${state.windowed}`;
    // eslint-disable-next-line no-console
    console.warn(`[anchordebug] ${words}`);
    if (!deps.canPost()) return;
    const event = {
      t: deps.now(),
      kind: "longtask",
      dur_ms: Math.max(1, Math.round(delta)),
      view: oneOf(VIEWS, state.view, "other"),
      screen: oneOf(SCREENS, state.screen, "other"),
      channel_view: oneOf(CHANNEL_VIEWS, state.channelView, state.view === "discord" ? "other" : "none"),
      layout: state.twoColumns ? "two_column" : "single",
      reading: "other",
      windowed: state.windowed,
      visibility: "visible",
      action: "none",
      script: words.slice(0, 256),
    };
    void deps.send(JSON.stringify({ v: 1, startedAt, events: [event] })).catch(() => false);
  }

  return { enabled, check };
}
