// The jumps taken by links to messages, for Back to undo (`#543 in-app-message-links`, `#580
// open-in-chat-jump`; `#624 voice-page-modularity`, moved from the legacy script into core/navigation): the
// jumps taken, the first first; the ticket each jump and each return takes, so a later one ends one still
// under way; how many returns are putting a place back; and the levels a Back would close with the jumps
// among them. Where the reader is, taking a jump, reading its channel and putting the reader back are the
// page's.

import type { DeepLinkRef } from "../../foundation/index.ts";
import type { ChannelPlace } from "./places.ts";

/** Where the reader was when a jump was taken, to come back to: the place, the column, the message their eye was on. */
export interface LinkSpot {
  readonly place: ChannelPlace | null;
  readonly column: "main" | "side";
  readonly pinned: boolean;
  readonly anchor: string | null;
  readonly offset: number;
  readonly focus: DeepLinkRef | null;
  /** Taken from the call's view, which a return goes back to. */
  readonly call?: boolean;
}

/** A jump taken: where it was taken from, the levels open under it, and the levels open when it landed (null while under way). */
export interface LinkJump {
  readonly spot: LinkSpot;
  readonly below: readonly string[];
  landed: string[] | null;
}

/** The jumps' commands and questions. */
export interface LinkJumps {
  /** The last jump taken, the one a Back undoes, or undefined with none. */
  top(): LinkJump | undefined;
  /** Take `jump` over the others: it is the one a Back undoes now. Answers its ticket. */
  take(jump: LinkJump): number;
  /** Undo the last jump: taken off, and answered; undefined with none. */
  undo(): LinkJump | undefined;
  /** Whether `jump` is still taken (not undone, nor forgotten). */
  holds(jump: LinkJump): boolean;
  /** Whether `ticket` is the last taken: a jump or return that holds an older one is over. */
  current(ticket: number): boolean;
  /** A return takes a ticket, ending any jump or return under way. Answers it. */
  nextTicket(): number;
  /** A return is putting a place back, until `returnEnded`. */
  returnStarted(): void;
  returnEnded(): void;
  /**
   * The levels a Back would close, with the jumps among them: the levels open when the last jump was
   * taken, the jump, and what has been opened over it since it landed. While it is under way, nothing it
   * opens is a level of its own; and a level it opened that is closed is forgotten, so a level opened
   * again after it is one over the jump.
   *
   * @param levels what is open, as the page reads it without the jumps
   */
  levels(levels: string[]): string[];
  /** Forget every jump, and end any under way: they were the signed-out reader's. */
  forget(): void;
}

/** Whether level `tag` is one only the reader opens: a screen over the channel, or a menu. */
function ownLevel(tag: string): boolean {
  return tag === "settings" || tag === "help" || tag.startsWith("menu:");
}

/** No jump taken. */
export function createLinkJumps(): LinkJumps {
  /** The jumps taken by links, the first first; the last is the one a Back undoes. */
  let linkJumps: LinkJump[] = [];
  /** Each jump and each return takes a ticket: a later one ends one still under way. */
  let linkJumpTicket = 0;
  /** How many returns are putting a place back now. */
  let linkReturning = 0;

  return {
    top: () => linkJumps[linkJumps.length - 1],
    take(jump) {
      linkJumps.push(jump);
      return ++linkJumpTicket;
    },
    undo: () => linkJumps.pop(),
    holds: (jump) => linkJumps.includes(jump),
    current: (ticket) => ticket === linkJumpTicket,
    nextTicket: () => ++linkJumpTicket,
    returnStarted() {
      linkReturning += 1;
    },
    returnEnded() {
      linkReturning -= 1;
    },
    levels(levels) {
      const top = linkJumps[linkJumps.length - 1];
      if (!top) return levels;
      // Screens and menus the reader opens while the jump loads are theirs, never the jump's.
      if (top.landed === null) return [...top.below, "jump", ...levels.filter(ownLevel)];
      let common = 0;
      while (common < top.landed.length && top.landed[common] === levels[common]) common += 1;
      // Not while a return under it is putting a place back: what is open then is on its way.
      if (common < top.landed.length && linkReturning === 0) top.landed = top.landed.slice(0, common);
      return [...top.below, "jump", ...levels.slice(common)];
    },
    forget() {
      linkJumps = [];
      linkJumpTicket += 1;
    },
  };
}

// What the page and the tests use of it.
export { ownLevel };
