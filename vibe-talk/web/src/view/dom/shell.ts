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

/** Whether nothing holds the focus: the body has it, as after a load or a control that hid itself. */
export function focusLost(): boolean {
  const held = document.activeElement;
  return !held || held === document.body || held === document.documentElement;
}

/**
 * Give `node`, a control in a list — a message box at its foot, an item of a row's menu — the focus,
 * scrolling only when it is off screen, and then the least that shows it. `#280 scroll-stability`. A
 * browser's own `focus()` scrolls a control it cannot show whole to the MIDDLE of the list, which on a
 * list being read is a jump of half a screen for the sake of one control.
 */
/** A focus reveal, as the scroll owner offers it (`intoView`). */
export type FocusReveal = (element: Element, block: string, inline?: string) => void;

/**
 * The page's focus reveal, handed in once at boot (`setFocusReveal`): the scroll
 * owner's own write, so even this layer's reveal is the owner's (`#1251
 * scroll-single-owner`). Until it is handed in — a fixture, a test that never
 * boots the page — `focusMinimally` reveals as it always did.
 */
let focusReveal: FocusReveal | null = null;

/** Hand the dom layer its focus reveal. Called once, at the page's boot. */
export function setFocusReveal(reveal: FocusReveal): void {
  focusReveal = reveal;
}

export function focusMinimally(node: HTMLElement): void {
  node.focus({ preventScroll: true });
  if (focusReveal) focusReveal(node, "nearest", "nearest");
  else if (typeof node.scrollIntoView === "function") node.scrollIntoView({ block: "nearest", inline: "nearest" });
}
