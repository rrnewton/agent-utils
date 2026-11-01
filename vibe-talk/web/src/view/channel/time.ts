// A message's time as the element that shows it, and the day its title says on hover (`#624
// voice-page-modularity`, moved from the legacy script into view/channel; `#260 timestamp-hover`,
// `#479 message-time-full-date`). The words are present/time's.

import { childByClass } from "../dom/index.ts";
import { dayTitle, editedTitle } from "../../present/time/index.ts";

/** The classes of the time elements `timeNode` makes, and so the only ones `freshenDayTitle` touches. */
const DAY_TITLED = new Set(["msg-time", "at"]);

/** The class of a row's edited mark, whose title `freshenDayTitle` words as an edit. `#320 slack-edited-mark`. */
const EDITED_MARK = "edited-mark";

/**
 * A message's time, as the element that shows it: `<time datetime>` holding the text `shown`, its
 * day in the title, which the browser draws as its own tooltip on hover. A phone has no hover, and
 * its press-and-hold opens the details sheet, which says the day in the same words. With no instant
 * to go on — a message whose timestamp would not parse, shown by its spoken time — a plain span with
 * no title, because there is no day to tell.
 *
 * `said` is the time as the title says it after the day, where `shown` says more than the time: a
 * row's "Fri 03:23 PM" (`#729 vt-older-message-dates`) is titled "Oct 30, Fri, 03:23 PM", not with
 * its weekday twice. Kept on the element (`data-said`) for the title worded again at hover.
 */
function timeNode(className: string, at: Date | null, shown: string, said: string = shown): HTMLElement {
  const node = document.createElement(at === null ? "span" : "time");
  node.className = className;
  node.textContent = shown;
  if (at !== null) {
    node.setAttribute("datetime", at.toISOString());
    if (said !== shown) node.setAttribute("data-said", said);
    const title = dayTitle(at, said);
    node.setAttribute("title", title);
    // `#479 message-time-full-date`. The same words as its name, so a reader who cannot hover
    // hears the day a hover would have shown; and focusable without a Tab stop of its own —
    // the selected row's time takes the stop with the row (`rowTimeTabStop`), so the keys
    // reach one time at a time, the one on the row they are on.
    node.setAttribute("aria-label", title);
    node.setAttribute("tabindex", "-1");
  }
  return node;
}

/** A row's time element, when it has one that knows its instant. `#479 message-time-full-date`. */
function rowTime(row: Element | null | undefined): Element | null {
  if (!row) return null;
  const node = childByClass(row, "msg-time");
  return node && node.getAttribute("datetime") ? node : null;
}

/**
 * The selected row's time shares its Tab stop: Tab from the row reaches its time, whose tooltip
 * and name say the full date, and Enter there says it on the status line (`sayTimeDay`). Every
 * other row's time stays out of the Tab order, as every other row does (`#239 single-selection`).
 */
function rowTimeTabStop(row: Element | null | undefined, on: boolean): void {
  const node = rowTime(row);
  if (node) node.setAttribute("tabindex", on ? "0" : "-1");
}

/**
 * Every `pointerover` on the page: a time the pointer has just reached has its title worded again,
 * now, before the browser shows it.
 *
 * WHY AT HOVER. A row is drawn once and then kept — `#226 page-energy-profile` stopped redrawing rows
 * that did not change — so a title worded when the row was drawn would still say "Today" the morning
 * after, on a page left open overnight. The browser waits a moment after the pointer arrives before
 * it shows a title, and this rewrites it as the pointer arrives, so the day it shows is the day it
 * is. One element, one date, no layout read, and nothing at all for a pointer over anything else.
 */
function freshenDayTitle(event: Event | null | undefined): void {
  const node = (event ? event.target : null) as Element | null;
  if (!node || typeof node.getAttribute !== "function" || String(node.tagName).toLowerCase() !== "time") return;
  const kind = String(node.className);
  if (!DAY_TITLED.has(kind) && kind !== EDITED_MARK) return;
  const at = new Date(String(node.getAttribute("datetime") || ""));
  if (Number.isNaN(at.getTime())) return;
  const said = node.getAttribute("data-said") ?? String(node.textContent);
  const title = kind === EDITED_MARK ? editedTitle(at) : dayTitle(at, said);
  if (node.getAttribute("title") !== title) node.setAttribute("title", title);
  // `#479 message-time-full-date`. Its name is the same words, freshened with them.
  if (kind !== EDITED_MARK && node.getAttribute("aria-label") !== title) node.setAttribute("aria-label", title);
}

export { DAY_TITLED, EDITED_MARK, freshenDayTitle, rowTime, rowTimeTabStop, timeNode };
