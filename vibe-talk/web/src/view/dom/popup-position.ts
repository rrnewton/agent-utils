// Popup placement (`#1544 documents-desktop-control-bar`): a popup centres
// over the button that opened it and is clamped inside the viewport, so a
// button near an edge never pushes its popup off-screen. Pure arithmetic
// first, so the clamp can be unit-checked without a DOM.

/** Left edge (viewport coordinates) centring a popup over its button, clamped. */
export function centredPopupLeft(
  buttonCentre: number,
  popupWidth: number,
  viewportWidth: number,
  margin = 8,
): number {
  const ideal = buttonCentre - popupWidth / 2;
  const max = Math.max(margin, viewportWidth - margin - popupWidth);
  return Math.min(Math.max(ideal, margin), max);
}

/**
 * Centre `popup` over `button` horizontally, clamped to the viewport.
 * Vertical placement stays the stylesheet's (above the bar, or below it in
 * the header). Best effort: a host without layout boxes leaves it alone.
 */
export function centrePopupOver(popup: HTMLElement, button: HTMLElement): void {
  try {
    if (typeof popup.getBoundingClientRect !== "function") return;
    const parent = popup.offsetParent as HTMLElement | null;
    if (parent === null || typeof parent.getBoundingClientRect !== "function") return;
    const buttonRect = button.getBoundingClientRect();
    const popupRect = popup.getBoundingClientRect();
    const viewportWidth = popup.ownerDocument?.documentElement?.clientWidth ?? popupRect.right;
    const left = centredPopupLeft(
      buttonRect.left + buttonRect.width / 2,
      popupRect.width,
      viewportWidth,
    );
    const parentRect = parent.getBoundingClientRect();
    popup.style.right = "auto";
    popup.style.left = `${left - parentRect.left}px`;
  } catch {
    // A host that cannot measure keeps the stylesheet's placement.
  }
}
