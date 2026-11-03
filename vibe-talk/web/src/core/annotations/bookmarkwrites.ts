// Writing a staged bookmark change to the server (`#240 reading-bookmark`, `#420 bookmark-clear`; `#624
// voice-page-modularity`, moved from the legacy script into core/annotations): in order behind the Done
// writes, conditional on the revision held where asked, not sent where the Done write it follows failed,
// settled into the held bookmark with the server's answer, and what came of it said. The requests, the
// write queue, drawing the line and saying are the page's.

import type { BookmarkAnchor, BookmarkResponse } from "../../contract/index.ts";
import type { BookmarkMove, Bookmarks } from "./bookmarks.ts";

/** How a bookmark write ended: saved, refused because another device moved it, failed, or not sent. */
export type BookmarkOutcome = "saved" | "moved" | "failed" | "cancelled";

/** A failure as the page's API client throws it. */
interface Refusal {
  readonly status?: number;
  readonly body?: unknown;
  readonly detail?: string;
  readonly message?: string;
}

/** What the writes ask of the page. */
export interface BookmarkWritesDependencies {
  /** The held bookmarks and this page's writes of them (`createBookmarks`). */
  readonly bookmarks: Bookmarks;
  /** Run `write` in order behind the Done writes (`queueInboxWrite`). */
  queue<T>(write: () => Promise<T>): Promise<T>;
  /** Put `channel`'s bookmark at `body` (the anchor, and `if_revision` where conditional). */
  put(channel: string, body: object): Promise<BookmarkResponse>;
  /** Take `channel`'s bookmark away, if still at `revision` where one is given. */
  remove(channel: string, revision: number | null): Promise<BookmarkResponse>;
  /** Whether a refusal's body is a bookmark answer (`VibeTalkContract.is("BookmarkResponse", ...)`). */
  isAnswer(body: unknown): boolean;
  /** The channel on screen. */
  channel(): string;
  /** The anchor `channel`'s line is drawn at now, or null (`bookmarkAnchor`). */
  anchor(channel: string): BookmarkAnchor | null;
  /** Whether two anchors are the same place (`sameAnchor`). */
  same(one: BookmarkAnchor | null, other: BookmarkAnchor | null): boolean;
  /** Draw the line and the chip again (`redrawBookmark`). */
  redraw(): void;
  /** Keep the device's copy of the channel, where the left column is being worked on (`saveChannelScope`). */
  save(): void;
  /** Say `text` on the status line. */
  status(text: string): void;
  /** Say a failure where every failure is said (`showError`). */
  error(text: string): void;
}

/** The writes' one command. */
export interface BookmarkWrites {
  /**
   * Write a staged change, in order behind the Done writes. `conditional` sends the revision held when
   * the write goes, so it is refused if another device moved the bookmark since this page last heard.
   * `after`, when given, is the Done write this one follows: when that fails, this one is not sent and
   * the change is taken back with it. Never throws: a failure is taken back and said.
   */
  send(move: BookmarkMove, options?: { conditional?: boolean; after?: Promise<boolean> | null; quiet?: boolean }):
    Promise<BookmarkOutcome>;
}

/** Nothing written yet. */
export function createBookmarkWrites(deps: BookmarkWritesDependencies): BookmarkWrites {
  return {
    send(move, { conditional = false, after = null, quiet = false } = {}) {
      const { channel } = move;
      return deps.queue(async () => {
        let outcome: BookmarkOutcome = "failed";
        let answer: BookmarkResponse | null = null;
        let problem: Refusal | null = null;
        try {
          if (after && !(await after)) {
            outcome = "cancelled";
          } else {
            const revision = conditional ? deps.bookmarks.revision(channel) : null;
            answer = move.anchor
              ? await deps.put(channel, { ...move.anchor, ...(revision === null ? {} : { if_revision: revision }) })
              : await deps.remove(channel, revision);
            outcome = "saved";
          }
        } catch (error) {
          const refusal = error as Refusal | null;
          if (refusal && refusal.status === 409 && deps.isAnswer(refusal.body)) {
            answer = refusal.body as BookmarkResponse;
            outcome = "moved";
          } else {
            problem = refusal;
          }
        } finally {
          deps.bookmarks.writeEnded(channel);
        }
        // The server's revision taken, and its anchor where the change is still this one's; taken back
        // where the write failed (`Bookmarks.settle`).
        const settled = deps.bookmarks.settle(move, answer);
        // Gone with the credential that made it (`forgetBookmarks`): nothing to settle, or to say.
        if (!settled) return outcome;
        const { mine } = settled;
        const shownBefore = settled.before;
        const now = deps.anchor(channel);
        if (!deps.same(shownBefore, now) && channel === deps.channel()) deps.redraw();
        // Kept with the device's copy of the channel, for a page opened offline.
        if (channel === deps.channel()) deps.save();
        // Short, for a phone's status line; the reason a write failed goes where every failure does.
        if (outcome === "moved" && mine && !quiet) {
          deps.status("Bookmark moved on another device.");
        } else if (outcome === "failed" && problem && !quiet && mine) {
          // Only for the change still shown: one made since (Undo, say) has its own write, which says.
          deps.status("The bookmark was not saved.");
          deps.error(`The bookmark could not be saved: ${problem.detail || problem.message}`);
        }
        return outcome;
      });
    },
  };
}
