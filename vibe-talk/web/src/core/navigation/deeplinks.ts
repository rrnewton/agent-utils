// The link the page opened by, and what it holds while it is applied (`#462 webapp-deep-links`; `#624
// voice-page-modularity`, moved from the legacy script into core/navigation): the link read from the
// address once, the message a link focused, the address last written, whether opening from the link was
// done or is under way, the reader's own choices counted from the link's first reading, whether a place is
// being arrived at rather than chosen, and the epoch signing out puts up. Showing the link's place,
// landing on its message and writing the address are the page's; why the address follows navigation and
// never leads it is told where the legacy script opens the section.

import { deepLinkRefFrom } from "../../foundation/index.ts";
import type { DeepLinkRef } from "../../foundation/index.ts";
import { DEEP_LINK_VIEWS, deepLinkParams, deepLinkPart } from "./links.ts";

/** What a link asks for: its channel, view, thread and message, each null where it says none. */
export interface DeepLinkWanted {
  readonly channel: string | null;
  readonly view: string | null;
  readonly thread: string | null;
  readonly message: DeepLinkRef | null;
}

/** What the links ask of the page. */
export interface DeepLinksDependencies {
  /** The address's query (`window.location.search`); may throw where the host has none. */
  search(): string;
  /**
   * Whether something other than a link's showing is arriving rather than choosing: a Back or Forward
   * (`Visits.arriving`), or a layout the page itself restores (a turned or resized window).
   */
  arriving(): boolean;
}

/** The links' commands and questions. */
export interface DeepLinks {
  /**
   * The link the page opened by, read from the query once; null for a plain open. A reload standing on an
   * address this tab wrote is read the same way. The reader's choices are counted from this first reading.
   */
  wanted(): DeepLinkWanted | null;
  /** The message a link focused, kept in the address while its place stands, or null. */
  focus(): DeepLinkRef | null;
  setFocus(ref: DeepLinkRef | null): void;
  /** The address last written, or null. */
  written(): string | null;
  noteWritten(address: string): void;
  /** Whether opening from the link is done: at most once a load. */
  applied(): boolean;
  markApplied(): void;
  /** Whether an attempt at opening from the link is under way: one at a time. */
  applying(): boolean;
  setApplying(applying: boolean): void;
  /** Put up by signing out, so an apply still waiting from before it does nothing more. */
  epoch(): number;
  /**
   * Note a choice of view or channel the reader made, for an apply to lose to. A place arrived at
   * (`arrive`, Back, Forward) and a layout the page restores are no choice and are not counted.
   */
  noteReaderMove(): void;
  /** The choices noted so far: what a step of an apply compares across its wait. */
  readerMoves(): number;
  /** Whether the reader has chosen anywhere of their own since the link was first read. */
  readerMoved(): boolean;
  /** True while a place is being arrived at rather than chosen. */
  showing(): boolean;
  /** Run `run` as an arrival, not a choice: Back and Forward, a link's own showing, a jump's. */
  arrive<T>(run: () => T): T;
  /** Signing out: the link, its focus and the choices are forgotten, and no apply runs again. */
  signOut(): void;
}

/** No link read yet. */
export function createDeepLinks(deps: DeepLinksDependencies): DeepLinks {
  /** The link the page opened by, once read; `undefined` until first read. */
  let deepLinkWanted: DeepLinkWanted | null | undefined = undefined;
  /** The message a link focused, kept in the address while its place stands. */
  let deepLinkFocus: DeepLinkRef | null = null;
  /** The query last written, so an address already saying this is not written again. */
  let deepLinkWritten: string | null = null;
  /** Opening from a link, done at most once a load; and one attempt of it at a time. */
  let deepLinkApplied = false;
  let deepLinkApplying = false;
  /**
   * Choices of view the reader made (`noteReaderMove`), and how many stood when the link was first read:
   * a reader who has chosen since the link opened is not chased by it, however the places history happens
   * to read by the time the channel's first read lands. Places arrived at (Back, Forward, the link's own
   * showing) and the channel's opening view are not choices and never count.
   */
  let deepLinkReaderMoves = 0;
  let deepLinkStartMoves: number | null = null;
  /** True while a place is being arrived at rather than chosen. */
  let deepLinkShowing = false;
  /** Put up by signing out, so an apply still waiting from before it does nothing more. */
  let deepLinkEpoch = 0;

  return {
    wanted() {
      if (deepLinkWanted !== undefined) return deepLinkWanted;
      deepLinkWanted = null;
      try {
        const params = deepLinkParams(deps.search() || "");
        const channel = deepLinkPart(params, "c");
        const saidView = deepLinkPart(params, "v");
        const view = saidView && DEEP_LINK_VIEWS.includes(saidView.toLowerCase()) ? saidView.toLowerCase() : null;
        const thread = deepLinkPart(params, "t");
        const messageText = deepLinkPart(params, "m");
        const message = messageText ? deepLinkRefFrom(messageText) : null;
        if (channel || view || thread || message) {
          deepLinkWanted = { channel, view, thread, message };
          // Choices are counted from the link's first reading, wherever that is — the channel's
          // opening view may read it before any apply attempt runs.
          if (deepLinkStartMoves === null) deepLinkStartMoves = deepLinkReaderMoves;
        }
      } catch (_error) {
        // An address that cannot be read opens the page as any plain open does.
      }
      return deepLinkWanted;
    },
    focus: () => deepLinkFocus,
    setFocus(ref) {
      deepLinkFocus = ref;
    },
    written: () => deepLinkWritten,
    noteWritten(address) {
      deepLinkWritten = address;
    },
    applied: () => deepLinkApplied,
    markApplied() {
      deepLinkApplied = true;
    },
    applying: () => deepLinkApplying,
    setApplying(applying) {
      deepLinkApplying = applying;
    },
    epoch: () => deepLinkEpoch,
    noteReaderMove() {
      if (!deps.arriving() && !deepLinkShowing) deepLinkReaderMoves += 1;
    },
    readerMoves: () => deepLinkReaderMoves,
    readerMoved: () => deepLinkStartMoves !== null && deepLinkReaderMoves !== deepLinkStartMoves,
    showing: () => deepLinkShowing,
    arrive(run) {
      const wasShowing = deepLinkShowing;
      deepLinkShowing = true;
      try {
        return run();
      } finally {
        deepLinkShowing = wasShowing;
      }
    },
    signOut() {
      deepLinkWanted = null;
      deepLinkFocus = null;
      deepLinkApplied = true;
      deepLinkEpoch += 1;
      deepLinkStartMoves = null;
    },
  };
}
