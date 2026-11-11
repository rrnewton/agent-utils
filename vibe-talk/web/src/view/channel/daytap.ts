// Saying which day a message was sent, on a tap or Enter on its time (`#624
// voice-page-modularity`, moved from the legacy script). Words and order are as they
// were in the script. The day words are present/time's and the titled classes
// view/channel/time's, taken directly; the status line is the page's, handed in.

import { fullLocalAt } from "../../present/time/index.ts";
import { DAY_TITLED, freshenDayTitle } from "./time.ts";

/** What saying a message's day asks of the page. */
export interface DayTapDependencies {
  setStatus(text: string): void;
}

/** Saying a time's full day on the status line. */
export interface DayTap {
  sayTimeDay(node: Element): void;
  onTimeTap(event: Event | null | undefined): void;
  onTimeKey(event: KeyboardEvent | null | undefined): void;
}

/** The day tap over `deps`. */
export function createDayTap(deps: DayTapDependencies): DayTap {
  // `#260 timestamp-hover`. A row's time is the hour and minute; hovering it says the day as well.
  // How a day, a time and an age are worded is web/src/present/time's (`dayLabel`, THE ONE DAY
  // FORMATTER, and its section note); what is left here draws them.

  /** Say a time element's full day on the status line: a tap on a phone, Enter on the keys. */
  function sayTimeDay(node: Element): void {
    const at = new Date(String(node.getAttribute("datetime") || ""));
    if (Number.isNaN(at.getTime())) return;
    deps.setStatus(fullLocalAt(at));
  }

  /**
   * A tap (or click) on a message's time says its full day on the status line: the one press on
   * a phone that says when a message was from, without the details sheet a press-and-hold on
   * the row opens. The press itself is the row's as it always was — at zero collapsed lines the
   * header is all of a row there is to press, and a press there must still open it.
   * `#479 message-time-full-date`.
   *
   */
  function onTimeTap(event: Event | null | undefined): void {
    const node = (event ? event.target : null) as Element | null;
    if (!node || typeof node.getAttribute !== "function" || String(node.tagName).toLowerCase() !== "time") return;
    if (!DAY_TITLED.has(String(node.className))) return;
    freshenDayTitle(event);
    sayTimeDay(node as Element);
  }

  /**
   * Enter or Space on a focused time says its day, as a tap does; every other key is the row's.
   * `#479 message-time-full-date`.
   *
   */
  function onTimeKey(event: KeyboardEvent | null | undefined): void {
    if (!event || (event.key !== "Enter" && event.key !== " ")) return;
    const node = event.target as Element | null;
    if (!node || typeof node.getAttribute !== "function" || String(node.tagName).toLowerCase() !== "time") return;
    if (!DAY_TITLED.has(String(node.className))) return;
    event.stopPropagation();
    event.preventDefault();
    freshenDayTitle(event);
    sayTimeDay(node as Element);
  }

  return { sayTimeDay, onTimeTap, onTimeKey };
}
