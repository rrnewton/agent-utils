// Settings restored as kept before anything is drawn (`#425 collapsed-lines` and the screen's other settings, the
// read-aloud speed, the switches that shape the lists, the message box's Enter; `#624 voice-page-modularity`, moved
// from the legacy script's wiring into view/settings for the plan's step 17). Each setting's owner applies it; this
// says the order, once, at load.

export interface StoredSettingsDependencies {
  /** The column the reader last chose. Applied unconditionally: on a phone the stylesheet never consults the value,
   * so there is nothing to branch on here and no second definition of "is this a desktop" to drift. */
  readingWidth(): unknown;
  textSize(): unknown;
  /** `#425 collapsed-lines`. Before anything is drawn, for the same reason: a first list folded at three and refolded
   * at the reader's count a moment later is a jump nobody asked for. */
  collapsedLines(): unknown;
  /** Before the first card is drawn: nothing to draw again yet. */
  cardMaxLines(): unknown;
  quoteReadPercent(): unknown;
  markRead(): unknown;
  readSpeed(): unknown;
  ownRead(): unknown;
  combine(): unknown;
  hideEmpty(): unknown;
  richHeaders(): unknown;
  inAppLinks(): unknown;
  openInView(): unknown;
  afterDone(): unknown;
  summarizeCollapsed(): unknown;
  enterSends(): unknown;
}

/** Restore each setting as kept, in the page's order: the column and the type before anything is drawn into them. */
export function restoreStoredSettings(deps: StoredSettingsDependencies): void {
  deps.readingWidth();
  deps.textSize();
  deps.collapsedLines();
  deps.cardMaxLines();
  deps.quoteReadPercent();
  deps.markRead();
  deps.readSpeed();
  deps.ownRead();
  deps.combine();
  deps.hideEmpty();
  deps.richHeaders();
  deps.inAppLinks();
  deps.openInView();
  deps.afterDone();
  deps.summarizeCollapsed();
  deps.enterSends();
}
