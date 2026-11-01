// A row's replies as the DOM draws them (`#624 voice-page-modularity`, moved from the legacy script into
// view/channel; `#228 reply-visible`, `#215 reply-coalesce`, `#267 quote-reply`): the circle's lift and the
// arrows' reach written only where they change, where a gathered reply's stack hangs, a gutter piece put
// or taken, and whether Reply can say "Quote" without moving its header line.

/** Lift `row`'s circle up its side, or put it back on the corner, where that differs. `#228`. */
function setMarkerLift(row: Element, lift: boolean): void {
  if (lift !== row.hasAttribute("data-reply-lift")) {
    if (lift) row.setAttribute("data-reply-lift", "true");
    else row.removeAttribute("data-reply-lift");
  }
}

/** Write one arrow's reach and its distance, and clear the other distance, where they differ. */
function setArrowReach(arrow: HTMLElement, reach: string, property: string, value: string, other: string): void {
  if (arrow.getAttribute("data-reply-reach") !== reach) arrow.setAttribute("data-reply-reach", reach);
  if (arrow.style.getPropertyValue(property) !== value) arrow.style.setProperty(property, value);
  if (arrow.style.getPropertyValue(other) !== "") arrow.style.removeProperty(other);
}

/** Whether `row` holds only replies a gathered thread put in this list, which are not the list's own. */
const fromGatheredThread = (row: Element): boolean => row.hasAttribute("data-gathered-thread");

/**
 * What the row's speaker and shape put its left edge at, in the words web/voice.css keys a gathered
 * reply's inset on (`data-stack-under`): the stack hangs one gutter in from its parent's edge, so the
 * bridge's spine is under the parent and comes out of its foot whoever sent it.
 */
function stackUnder(row: Element): string {
  const who = row.getAttribute("data-who");
  if (row.getAttribute("data-is-reply") === "true") return who === "me" ? "me-reply" : "reply";
  return who === "me" || who === "coder" ? who : "third";
}

/** Put `make()` at the head of `row` when `wanted` and it is not there, and take it away when not. */
function gutterPiece(row: Element, name: string, wanted: boolean, make: (row: Element) => Node): void {
  const held = [...row.children].find((child) => String(child.className).split(/\s+/).includes(name));
  if (wanted && !held) row.replaceChildren(make(row), ...row.children);
  else if (!wanted && held) row.removeChild(held);
}

/**
 * Where everything on a row's header line stands: the line's height, the top of each thing on it
 * measured from the line's own top, and the author's width. Empty where the engine lays nothing out.
 */
function headerLayout(meta: Element | null | undefined): number[] {
  if (!meta || typeof meta.getBoundingClientRect !== "function") return [];
  const line = meta.getBoundingClientRect();
  const kids = meta.children || [];
  const tops: number[] = [];
  for (let at = 0; at < kids.length; at += 1) {
    const kid = kids[at] as Element;
    if (typeof kid.getBoundingClientRect !== "function") continue;
    const box = kid.getBoundingClientRect();
    tops.push(box.top - line.top);
    // `#377 compact-collapsed-rows`. The name gives way on a full line, so a longer label can cut it
    // without moving anything up or down: its width is part of where it stands.
    if (String(kid.className || "").split(" ").includes("msg-author")) tops.push(box.width);
  }
  return [line.height, ...tops];
}

/**
 * `#267 quote-reply`. Whether `button` can read `full` without moving anything on its row's header
 * line from where it stands as `plain`: drawn both ways in one go, which the reader never sees, and
 * measured. Leaves the button as `full`, and its width as `plain` in `quoteWidth`, for
 * `drawReplyLabel` to finish.
 */
function quoteLabelFits(button: HTMLElement & { quoteWidth?: string }, plain: string, full: string): boolean {
  // The header line itself, not the row's controls that Reply is held among (`#377`).
  const held = button.parentNode;
  const meta = (held && String((held as Element).className || "") === "row-acts" ? held.parentNode : held) as Element | null;
  if (!meta || typeof button.getBoundingClientRect !== "function") return true;
  button.removeAttribute("data-quote");
  button.style.width = "";
  button.textContent = plain;
  const before = headerLayout(meta);
  const plainWidth = button.getBoundingClientRect().width;
  button.quoteWidth = `${plainWidth}px`;
  button.setAttribute("data-quote", "true");
  button.textContent = full;
  // `#377 compact-collapsed-rows`. A collapsed row in a narrow list draws no Reply until words of it
  // are selected, and then draws it whole: there is no plain label in place to keep the line as it was.
  // Unless whole it takes the header onto a second line, which a time that says its day leaves less
  // room for (`#729 vt-older-message-dates`): then "Quote", at its own width.
  if (!(plainWidth > 0)) {
    button.quoteWidth = "";
    return (headerLayout(meta)[0] ?? 0) <= (before[0] ?? 0) + 0.5;
  }
  const after = headerLayout(meta);
  return before.length === after.length && before.every((value, at) => Math.abs(value - (after[at] as number)) <= 0.5);
}

export { fromGatheredThread, gutterPiece, headerLayout, quoteLabelFits, setArrowReach, setMarkerLift, stackUnder };
