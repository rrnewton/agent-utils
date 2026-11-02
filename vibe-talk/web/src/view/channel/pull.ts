// Pulling the channel down to refresh it, or up past its newest line (`#188 pull-refresh-bottom` and
// the page's pull-to-refresh; `#624 voice-page-modularity`, moved from the legacy script into
// view/channel). The one owner of the touch on the glass and of the affordance that says what letting
// go will do: why each rule is as it is, is told where the legacy script describes the gesture. What a
// refresh reads, when the list counts as at its newest, and the step back a finger down suspends are
// the page's, passed in.

import { stamp } from "../../present/time/index.ts";
import { scrollsItself } from "../dom/index.ts";
import type { ElementById } from "../dom/index.ts";

// How far past either end the finger has to travel before a release means anything. About a
// centimetre on a phone: beyond the few pixels a resting thumb wanders, and within one stroke of a
// thumb that is already on the glass. The same at both ends, so the gesture feels like one thing.
const PULL_ARM_PX = 64;

// How close to the true bottom the list must be for a pull up to start counting. Not zero, because
// a browser at a fractional device-pixel ratio reports a resting list a fraction of a pixel short
// of `scrollHeight - clientHeight`; not BOTTOM_SLACK_PX, because the pixels inside that slack can
// still scroll.
const PULL_EDGE_SLACK_PX = 2;

// How far the affordance moves however far the finger goes. It follows the finger by less and less
// — half of this at the threshold — so the pull reads as stretching something, not dragging it.
const PULL_RISE_PX = 40;

// How long a pull up leaves its result on the screen: long enough to read "Updated 07:15" at a
// glance, short enough not to sit over the newest line once it has been read.
const PULL_RESULT_MS = 2000;

// How often a release that found a read already in flight looks again — the same interval
// `refreshAfterLiveMutation` waits on for the same lock, and a reply's arrow waits on for All to
// be read behind it (`#204 reply-arrow`).
const PULL_WAIT_MS = 50;

// What the affordance says in each state of the gesture. The reader is told it is armed BEFORE
// they let go, because a gesture that only reports itself afterwards cannot be abandoned. A
// successful pull up says when it was updated instead, so its words are written where it lands.
const PULL_LABELS: Readonly<Record<string, string>> = Object.freeze({
  pull: "Pull to refresh",
  armed: "Release to refresh",
  busy: "Refreshing…",
  failed: "Refresh failed",
});

/** A touch event, as far as the gesture reads it. */
export interface PullTouch {
  readonly touches?: ArrayLike<{ readonly clientX?: unknown; readonly clientY?: unknown }>;
  readonly target?: EventTarget | null;
}

/** The touch on the glass, as the gesture follows it (see `pull` below). */
interface Pull {
  startX: number;
  startY: number;
  anchorY: number | null;
  topOpen: boolean;
  fromNewest: boolean;
  endY: number | null;
  edge: string | null;
  armed: boolean;
  refused: boolean;
}

/** How fresh the channel's list is, as the page last read it. */
export interface PullFreshness {
  readonly freshness: string;
  readonly freshAt: number;
}

/** What the gesture asks of the page. */
export interface PullToRefreshDependencies {
  /** The element with this id, as the page's own lookup finds it. */
  byId<K extends keyof ElementById>(id: K): ElementById[K];
  /** The list the touch is on: the column being worked on's. */
  scrollArea(): HTMLElement;
  /** Whether the reader is on the channel's list (the main screen, the channel view). */
  onChannelList(): boolean;
  /** Whether the channel view is the one shown. */
  inChannelView(): boolean;
  /** Whether `area` is at its newest line, within `slack` pixels where given. */
  atBottom(area: HTMLElement, slack?: number): boolean;
  /** Whether a read of the channel is on the wire. */
  fetchInFlight(): boolean;
  /** Whether a step back through the history is on the wire. */
  olderInFlight(): boolean;
  /** The step back a finger down suspended: taken now, if owed. */
  takeDeferredOlder(): void;
  /** A step back owed is forgotten: the refresh reads instead. */
  undeferOlder(): void;
  /** Read the channel. */
  loadDiscord(options: { reason?: string; keepPosition?: boolean; readAs?: string }): Promise<unknown>;
  /** The id of the newest message the list holds, or null. */
  newestId(): unknown;
  /** Say `text` on the status line. */
  setStatus(text: string): void;
  /** The channel and view being read, as a key: a result for another is not said. */
  channelContextKey(): string;
  /** How fresh the list is. */
  freshness(): PullFreshness;
  /** Draw the list's freshness again, which the affordance stands in for while it shows. */
  renderFreshness(): void;
}

/** The gesture's commands. */
export interface PullToRefresh {
  /** Whether a finger is down on the list (a step back waits for it to lift). */
  fingerDown(): boolean;
  /** Show the gesture's state, or take the affordance away (`null`). */
  renderPull(state: string | null, how?: { edge?: string; travel?: number; label?: string }): void;
  /** Take away an offer to refresh that letting go would no longer act on. */
  clearPullOffer(): void;
  /** A finger landed. */
  pullStart(event: PullTouch | null | undefined): void;
  /** The finger moved. */
  pullMove(event: PullTouch | null | undefined): void;
  /** The finger lifted: an armed pull refreshes. */
  pullEnd(): Promise<void>;
  /** The touch was taken away. */
  pullCancel(): void;
}

/** No finger down. */
export function createPullToRefresh(deps: PullToRefreshDependencies): PullToRefresh {
  /**
   * The TOUCH on the glass right now, or null.
   *
   * Built on every touchstart in the channel view, wherever the list happens to be scrolled to —
   * because rule 1 above is about a finger being down and not about what that finger turns out to
   * mean. `anchorY` is null until the list runs out under it at the top — and stays null for a touch
   * that landed without `topOpen` — and `endY` the same at the bottom, set only for a touch that
   * began `fromNewest`; that is what says whether any of this drag counts as a pull yet. `edge` is
   * the end the travel is pulling past, or null.
   */
  let pull: Pull | null = null;

  /** The timer that takes a pull up's result away again, or null. */
  let pullResultTimer: ReturnType<typeof setTimeout> | null = null;

  /**
   * Show the gesture, or take the affordance away. `null` is "no gesture".
   *
   * `edge` puts it at the end being pulled past, and `travel` is how far the finger has gone past
   * that end: the affordance moves with it, resisted, and its spinner winds up as the pull nears the
   * threshold. Every call replaces whatever the affordance was saying, a pull up's result included.
   * `state` is a key of PULL_LABELS, "done", or null.
   */
  function renderPull(state: string | null, { edge = "top", travel = 0, label = "" }: { edge?: string; travel?: number; label?: string } = {}): void {
    const element = deps.byId("pull-refresh");
    if (pullResultTimer !== null) {
      clearTimeout(pullResultTimer);
      pullResultTimer = null;
    }
    element.hidden = state === null;
    element.setAttribute("data-state", state === null ? "idle" : state);
    element.setAttribute("data-edge", edge);
    element.textContent = state === null ? "" : label || (PULL_LABELS[state] as string);
    const gone = Math.max(0, travel);
    const rise = Math.round(PULL_RISE_PX * gone / (gone + PULL_ARM_PX));
    element.style.setProperty("--pull-rise", `${rise}px`);
    // Only while the finger is still deciding: the in-flight spin starts from zero, and starting it
    // from wherever the pull left the arc would jerk it back once a turn.
    const winding = state === "pull" || state === "armed";
    const turn = winding ? Math.round(270 * gone / PULL_ARM_PX) : 0;
    element.style.setProperty("--pull-turn", `${turn}deg`);
    deps.renderFreshness();
  }

  /**
   * Take the affordance away IF it is describing a drag. A refresh in flight, and the result of one,
   * are reports about the channel rather than about this touch: a tap or a cancelled touch while one
   * is up must not take it down, and a scroll elsewhere in the list must not either.
   */
  function clearPullOffer(): void {
    const state = deps.byId("pull-refresh").getAttribute("data-state");
    if (state === "pull" || state === "armed") {
      renderPull(null);
    }
  }

  /**
   * A touch that lands in a draft long enough to scroll belongs to the draft. The composer sits at
   * the foot of the channel, exactly where a pull up starts, and dragging through a long message to
   * reach its end is the same motion; while the draft scrolls, the list stays at its end, so without
   * this it would read as a pull. A draft that fits is just part of the list and pulls like it.
   */
  function inScrollingDraft(event: PullTouch | null | undefined): boolean {
    const target = event && event.target;
    return Boolean(target) && scrollsItself(target as Element);
  }

  function pullCancel(): void {
    pull = null;
    clearPullOffer();
    // The finger is off the glass however it left, so the suspension is over. A step the reader is
    // owed must not be lost because the browser took the gesture away.
    deps.takeDeferredOlder();
  }

  /** A finger landed. Start following this touch — whether or not it turns out to be a pull. */
  function pullStart(event: PullTouch | null | undefined): void {
    const touches = (event && event.touches) || [];
    // MORE THAN ONE FINGER IS NEVER A PULL. `touches` carries every finger currently on the glass,
    // so this is a second one landing part-way through a drag — a pinch, or a two-thumb scroll.
    // Rebuilding the gesture from `touches[0]` here is what the page used to do, and it silently
    // re-anchored the travel to wherever finger one had got to: a drag stopped just short of the
    // threshold could arm from halfway. Refused for the rest of the touch, suspension still in force.
    if (touches.length > 1) {
      if (pull !== null) {
        pull.refused = true;
        pull.armed = false;
        clearPullOffer();
      }
      return;
    }
    // A LONE finger landing means no other is down, so any gesture still held here belongs to a
    // touch whose end this page never saw — a stale one, and the new touch replaces it. Keeping it
    // would suspend the automatic step back for as long as the page stayed open.
    const point = touches[0];
    pull = null;
    if (!point || !deps.onChannelList() || deps.olderInFlight()) {
      return;
    }
    const area = deps.scrollArea();
    const y = Number(point.clientY) || 0;
    // Parked at the newest line when the finger landed — the condition for a pull UP at all. See
    // `#188 pull-refresh-bottom` above: it is what tells a pull from a fling that reached the end.
    const fromNewest = deps.atBottom(area);
    // A channel read on the wire closes the TOP to this touch, and only the top: the foot's release
    // waits for that read and reports it. See "A READ ALREADY IN FLIGHT IS THE REFRESH" above.
    const topOpen = !deps.fetchInFlight();
    pull = {
      startX: Number(point.clientX) || 0,
      startY: y,
      // Null means "the list has not run out yet". Set to where the finger was at the moment it
      // did, which is where an overscroll actually begins — see rule 2 above. Landing with the list
      // already at its top is that same moment, arriving early.
      anchorY: topOpen && area.scrollTop <= 0 ? y : null,
      topOpen,
      fromNewest,
      // The same, at the bottom: landing with the list already at its very end is that moment too.
      endY: fromNewest && deps.atBottom(area, PULL_EDGE_SLACK_PX) ? y : null,
      edge: null,
      armed: false,
      // Still a touch, so the step back stays suspended under it; it is just never a pull.
      refused: inScrollingDraft(event),
    };
  }

  /** The finger moved. Arm the pull, or decide this drag is something else. */
  function pullMove(event: PullTouch | null | undefined): void {
    const point = pull && event && event.touches && event.touches[0];
    // A point is only read from a touch the gesture follows, so `pull` is held here.
    const current = pull as Pull;
    if (!point || current.refused) {
      return;
    }
    const y = Number(point.clientY) || 0;
    // MOSTLY SIDEWAYS IS NOT A PULL. Without an axis test a drag across the list — a swipe the
    // owner's thumb makes at the edge of the screen for the platform's own back gesture — arms a
    // refresh on whatever downward drift it happens to carry. Refused for the rest of the touch
    // rather than re-tested each move, because a gesture that changes its mind about its own axis
    // is how a horizontal drag arms at the far end of the arc.
    if (Math.abs((Number(point.clientX) || 0) - current.startX) > Math.abs(y - current.startY)) {
      current.refused = true;
      current.armed = false;
      clearPullOffer();
      return;
    }
    const area = deps.scrollArea();
    // Each end keeps its anchor only while the list is still run out at that end. Content above,
    // or below: the browser has somewhere to scroll, so these pixels are a scroll, and any anchor
    // from earlier in this drag is void — the reader went back INTO the history, and the overscroll
    // would have to begin again if they come back out of it. Arriving at an end anchors it HERE:
    // the pull is measured from the edge, and the travel spent reaching it belonged to the scroll.
    if (!current.topOpen || area.scrollTop > 0) {
      current.anchorY = null;
    } else if (current.anchorY === null) {
      current.anchorY = y;
    }
    if (!current.fromNewest || !deps.atBottom(area, PULL_EDGE_SLACK_PX)) {
      current.endY = null;
    } else if (current.endY === null) {
      current.endY = y;
    }
    const down = current.anchorY === null ? 0 : y - current.anchorY;
    const up = current.endY === null ? 0 : current.endY - y;
    // The end already being pulled keeps the gesture while its travel lasts. Otherwise it goes to
    // whichever end the finger is past — on a list too short to scroll, the direction of the drag.
    const keeps = (current.edge === "top" && down > 0) || (current.edge === "end" && up > 0);
    if (!keeps) {
      current.edge = down > 0 ? "top" : up > 0 ? "end" : null;
    }
    if (current.edge === null) {
      // Back past where the pull began, or no end under the finger at all. Re-anchor whichever end
      // the list is still at rather than refuse: the finger has not left, and the next millimetre
      // past that end is the start of a pull. This is what lets the reader change their mind
      // without lifting.
      if (current.anchorY !== null) current.anchorY = y;
      if (current.endY !== null) current.endY = y;
      current.armed = false;
      clearPullOffer();
      return;
    }
    const travelled = current.edge === "top" ? down : up;
    current.armed = travelled >= PULL_ARM_PX;
    renderPull(current.armed ? "armed" : "pull", { edge: current.edge, travel: travelled });
  }

  /** The finger lifted. Only an ARMED pull does anything — but the suspension ends either way. */
  async function pullEnd(): Promise<void> {
    const armed = pull !== null && pull.armed;
    const edge = pull !== null ? pull.edge : null;
    pull = null;
    if (!armed) {
      clearPullOffer();
      // Whatever this touch was, it was not a pull, so the step back it stood in the way of is the
      // reader's again. THIS is what keeps the walk automatic: a reader who drags up to the top and
      // lifts has asked for what is above, and gets it here rather than having to scroll a second
      // time to re-announce it.
      deps.takeDeferredOlder();
      return;
    }
    // An ARMED pull drops it instead: the reader has asked for the newest end of the channel, and
    // answering that by prepending more history is answering the opposite question.
    deps.undeferOlder();
    if (edge === "end") {
      await refreshFromNewest();
      return;
    }
    // Held where it armed, rather than snapping back to the edge as the finger lifts.
    renderPull("busy", { travel: PULL_ARM_PX });
    const before = deps.newestId();
    try {
      // No `keepPosition`: a user-initiated refresh goes to the newest message. See the note above.
      await deps.loadDiscord({ reason: "pull" });
    } finally {
      renderPull(null);
    }
    // ...and SAYS what it found. A refresh that finds nothing looks exactly like a refresh that
    // never happened, which is the half of this the issue is most explicit about.
    deps.setStatus(
      deps.newestId() !== null && deps.newestId() !== before
        ? "refreshed — something new had arrived."
        : "refreshed — nothing new since the last read."
    );
  }

  /**
   * An armed pull up, released. Re-read what is on screen — the channel, the thread list, or the one
   * thread that is open, since `loadDiscord` reads whichever view is up — and then say what it found.
   *
   * The error is re-thrown after the affordance has said "Refresh failed", so that the failure is
   * reported where every other failed read is reported, and the affordance is not the only record.
   */
  async function refreshFromNewest(): Promise<void> {
    renderPull("busy", { edge: "end", travel: PULL_ARM_PX });
    // What the result will be about: this view of this channel, from this moment on.
    const asked = {
      context: deps.channelContextKey(),
      at: Date.now(),
      newest: deps.newestId(),
    };
    let failure: unknown = null;
    try {
      if (deps.fetchInFlight()) {
        // A poll, the live stream's catch-up or the cold start's re-read — on the wire when the
        // finger landed, or begun while it was down. It is reading the view on screen now, which is
        // what the reader asked for; a second read queued behind it would only ask again.
        while (deps.fetchInFlight()) {
          await new Promise((resolve) => setTimeout(resolve, PULL_WAIT_MS));
        }
      } else {
        await deps.loadDiscord({ keepPosition: true, readAs: "pull" });
      }
    } catch (error) {
      failure = error;
    }
    showPullResult(failure !== null, asked);
    if (failure !== null) {
      throw failure;
    }
  }

  /** Say, at the foot of the list, what the refresh a pull up asked for found — then go. */
  function showPullResult(threw: boolean, asked: { context: string; at: number; newest: unknown }): void {
    // Nothing to say over a view the reader has left — another tab, another thread, another channel.
    // The read they pulled for was folded away for when they come back, or dropped, and the
    // freshness on the page now is the one restored for where they went. The freshness pill speaks
    // for that view; "Updated" here would be a time they never refreshed at.
    if (!deps.inChannelView() || deps.channelContextKey() !== asked.context) {
      renderPull(null);
      return;
    }
    // The freshness state is the record of how the read went, whichever read it was — this one, or
    // the one already in flight that it waited on — but CURRENT only for a read that landed since
    // the release. A read that was superseded and never landed leaves the stamp from an older one.
    // Current wins over a throw: a read queued behind a failed one can still land for this view.
    const current = deps.freshness().freshness === "fresh" && deps.freshness().freshAt >= asked.at;
    const failed = !current &&
      (threw || deps.freshness().freshness === "offline" || deps.freshness().freshness === "failed");
    if (!failed && !current) {
      renderPull(null);
      return;
    }
    const arrived = deps.newestId() !== null && deps.newestId() !== asked.newest;
    const label = failed
      ? PULL_LABELS.failed
      : `Updated ${stamp(deps.freshness().freshAt)} · ${arrived ? "new messages" : "nothing new"}`;
    renderPull(failed ? "failed" : "done", { edge: "end", travel: PULL_ARM_PX, label });
    pullResultTimer = setTimeout(() => {
      pullResultTimer = null;
      renderPull(null);
    }, PULL_RESULT_MS);
  }

  return {
    fingerDown: () => pull !== null, renderPull, clearPullOffer, pullStart, pullMove, pullEnd, pullCancel,
  };
}

// What the page and the tests use of it.
export { PULL_ARM_PX, PULL_EDGE_SLACK_PX, PULL_LABELS, PULL_RESULT_MS, PULL_RISE_PX, PULL_WAIT_MS };
