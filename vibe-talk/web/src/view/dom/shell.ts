// Finding the page's elements, and giving one the focus (`#229 desktop-two-column`, `#280
// scroll-stability`; `#624 voice-page-modularity`, moved from the legacy script; plan step 16, view/dom).
// Which column the code works on is the page's; it is passed in.

/**
 * The element with id `id` in the column the code works on: the right column's stand-in for it
 * (`standIns`, the channel pane's ids mapped to each `side-` id) where `side` and it has one, else the
 * element itself. Looked up when asked, in the document as it is then.
 */
export function elementIn(id: string, side: boolean, standIns: { readonly [id: string]: string | undefined }): HTMLElement | null {
  const standIn = side ? standIns[id] : undefined;
  return document.getElementById(standIn || id);
}

/**
 * Give `node`, a control in a list — a message box at its foot, an item of a row's menu — the focus,
 * scrolling only when it is off screen, and then the least that shows it. `#280 scroll-stability`. A
 * browser's own `focus()` scrolls a control it cannot show whole to the MIDDLE of the list, which on a
 * list being read is a jump of half a screen for the sake of one control.
 */
export function focusMinimally(node: HTMLElement): void {
  node.focus({ preventScroll: true });
  if (typeof node.scrollIntoView === "function") node.scrollIntoView({ block: "nearest", inline: "nearest" });
}
