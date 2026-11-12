// The control bar (`#58 control-bar`, `#59 text-entry-button`, `#60 canned-prompt-buttons`,
// `#83 channel-selector-in-bar`, `#129 message-search`, `#197 floating-search`, `#206
// pin-message`, `#211 link-filter`, `#212 link-filter`, `#251 author-filter`, `#241
// voice-composer`, `#820 voice-page-action-menu`, `#880 voice-page-menu-placement`; `#624
// voice-page-modularity`, moved from the legacy script into view/shell for the plan's step 17):
// which view each member of the pack belongs on, and what is on the bar right now. The bar is
// one strip with two possible parents; where it is (`barPlacement`) stays Settings'. The
// screens, the menus and the search are handed in; the words are the page's.

/** What the control bar is handed by the page. */
export interface ControlBarDependencies {
  el(id: string): HTMLElement;
  screen(): string;
  view(): string;
  actionMenuOpen(): boolean;
  renderActionMenu(): void;
  screenHasTitle(screen: string): boolean;
  readAloudAgent(): unknown;
  columnsOffered(): boolean;
  promptsOpen(): boolean;
  setPromptsOpen(open: boolean): void;
  setActionMenuOpen(open: boolean): void;
  searchOpen(): boolean;
  renderLinkKinds(): void;
  barWhere(): string;
}

/** The control bar, as the page calls it. */
export interface ControlBar {
  renderControlBar(): void;
}

/**
 * WHICH VIEW each member of #bar-pack belongs on. A table, and every member needs a line in it.
 *
 * The bar is one strip on a 375px phone and the pack is the part of it that scrolls, so a member
 * that is on screen where it cannot act is not free — it costs the reachability of the members
 * beside it, which is the whole of `#83 channel-selector-in-bar`. The rule is therefore per member
 * and it follows WHERE THE MEMBER'S EFFECT LANDS:
 *
 *   * the channel picker names what the channel view is reading, and on the call view it names a
 *     channel you are not looking at;
 *   * Sumry and Blockers go out through `sendUserMessage`, which draws the line into the
 *     TRANSCRIPT and needs a live call. Offering them over the channel means tapping a button
 *     whose whole result appears on a screen you are not on. Writing INTO the channel is a
 *     different act with its own control — the reply on the row, `#51 reply-view`. Writing to the
 *     agent is not on the bar at all since `#241 voice-composer`: its box is in the transcript.
 *
 * There is no default. A member missing from this table is hidden everywhere and the page suite
 * says which one, because a silent default is how a member ends up on a view nobody chose for it —
 * and because the fit check below is only as good as this table is complete.
 */
export const PACK_VIEWS: Record<string, string[]> = {
  // `#1355 phone-bar-discuss-slot`: #audio-source is no longer a pack member; it lives
  // in the hidable reader row (#control-pane) and renderControlBar states it below.
  "discord-channel": ["discord"],
  // `gchat-thread-selector`. Main, All, or one thread of the channel picked beside it.
  "thread-select": ["discord"],
  // `#229 desktop-two-column`. One column or two, on the channel. Drawn only on a desk wide enough
  // for two (web/voice.css), so on a phone it costs the strip nothing.
  "column-toggle": ["discord"],
  // ONE LINE FOR ALL THE CANNED PROMPTS, which is the point of the tray: they used to be derived
  // into this table one entry each, and every prompt added took another slot off a strip already
  // priced to the edge of a 375px phone. The answer is the same for all of them anyway — every
  // one goes out through `sendUserMessage`, which draws into the transcript and needs a live call.
  "prompts-open": ["voice"],
  // `read-new`. What happens to an arriving channel message DURING A CALL, so it belongs where the
  // call is. It is on the strip at all because the tray gave the slot back — the owner asked for
  // this control in the same breath as asking for the tray, and the arithmetic is why one had to
  // precede the other.
  "read-new": ["voice"],
  // `catchup-side-agent`. Discuss summarizes the channel or thread on screen, so it
  // belongs on the channel view and nowhere else.
  "discuss-open": ["discord"],
};

/** Make the control bar from the page's screens, menus and search. */
export function createControlBar(deps: ControlBarDependencies): ControlBar {

  /**
   * What is ON the bar right now, and whether the header is worth a row.
   *
   * ONE function owns both, because they are the same question asked twice: `#59` and `#60` extend
   * this and nothing else. It used to live inline in `showScreen`, which is how a second caller ends
   * up with a different opinion about whether the gear is reachable.
   */
  function renderControlBar(): void {
    const main = deps.screen() === "main";
    // PER MEMBER, not the bar as a whole. The gear is reachable from the sign-in screen today and
    // must stay so — hiding the bar wholesale off the main screen would take it away.
    //
    // `#241 voice-composer`. NOTHING HERE DEPENDS ON WHETHER YOU ARE WRITING. `#59
    // text-entry-button` made text entry a mode of this bar, which hid the switch, the gear and every
    // other member while it was on; the owner's report was exactly that — "when I press Type to type
    // to the agent... then the UI shifts and I lose the toggle". The message box is in the call's
    // transcript now, so the switch is on the main screen in every state, a call and a draft included.
    deps.el("view-switch").hidden = !main;
    // `#820 voice-page-action-menu`. The list's actions: Expand or Collapse all, on the call's transcript as on
    // the channel, and Summaries and Hide reader on the channel. Beside the switch, on the main screen as it is
    // (`#880 voice-page-menu-placement`).
    deps.el("action-menu-open").hidden = !main;
    deps.el("reader-toggle").hidden = deps.view() !== "discord";
    deps.el("autoplay-toggle").hidden = deps.view() !== "discord";
    // `catchup-side-agent`. Discuss in the menu, on the channel's view as reading is.
    deps.el("discuss-menu-open").hidden = deps.view() !== "discord";
    // `#1265 documents-star-hide`. Show hidden, on the Documents view only.
    deps.el("docs-show-hidden").hidden = deps.view() !== "docs";
    if (deps.actionMenuOpen()) deps.renderActionMenu();
    // `#1355 phone-bar-discuss-slot`: the audio toggle in the reader row, on the views
    // it served on the bar, and only where a configured agent makes it a choice.
    deps.el("audio-source").hidden = !main || !["discord", "docs"].includes(deps.view()) || deps.readAloudAgent() === null;
    deps.el("open-settings").hidden = deps.screenHasTitle(deps.screen());
    // The pack, by the same rule and as a LOOP rather than by name: `#60 canned-prompt-buttons` adds
    // members here, and a member added later is decided by the same line as the rest.
    //
    // WHICH VIEW each member belongs on is read out of `PACK_VIEWS` — a table, so that adding a
    // member is an entry rather than a branch, and so that "does the bar fit on a 375px phone"
    // is a question something can be asked. A member with no entry is a mistake rather than a
    // default: see the loop below.
    for (const member of deps.el("bar-pack").children as unknown as HTMLCollectionOf<HTMLElement>) {
      const views = PACK_VIEWS[member.id] || [];
      const unavailable = (member.id === "audio-source" && deps.readAloudAgent() === null) ||
        (member.id === "column-toggle" && !deps.columnsOffered());
      member.hidden = unavailable || !main || !views.includes(deps.view());
    }
    // A menu whose button has gone must go with it. Leaving Voice or leaving the main screen takes
    // the opener off the strip, and a tray left standing under a button that is no longer there is an
    // orphan floating over the channel — and, worse, still clickable.
    if (deps.el("prompts-open").hidden && deps.promptsOpen()) {
      deps.setPromptsOpen(false);
    }
    // `#820 voice-page-action-menu`. The same for the channel's action menu.
    if (deps.el("action-menu-open").hidden && deps.actionMenuOpen()) deps.setActionMenuOpen(false);
    const members = [
      deps.el("open-settings"),
      deps.el("action-menu-open"),
      deps.el("view-switch"),
      ...(deps.el("bar-pack").children as unknown as HTMLCollectionOf<HTMLElement>),
    ];
    // `#129 message-search`. On the main screen and nowhere else: there is nothing to filter on
    // Settings or Help, and on the sign-in screen there is nothing at all. Not
    // suppressed by the bar's placement either: since `#197 floating-search`
    // the glass and its field float over the top of the LIST, so they never compete with the bar
    // for width — in the dock or in the header.
    deps.el("search-toggle").hidden = deps.screen() !== "main";
    deps.el("search-field").hidden = deps.el("search-toggle").hidden || !deps.searchOpen();
    deps.el("search-count").hidden = deps.el("search-field").hidden;
    // `#206 pin-message`. Beside the glass while the bar is open, and over the channel only: a pin is
    // a channel message, and the call's transcript has none to filter to.
    deps.el("pinned-filter").hidden = deps.el("search-field").hidden || deps.view() !== "discord";
    // `#211 link-filter`. Beside Pinned while the bar is open, and over the call view as well: it
    // filters what the search filters, and a turn can hold a link as a channel message can.
    deps.el("links-filter").hidden = deps.el("search-field").hidden;
    // `#251 author-filter`. Whose messages, left of Links, over the call view as well: a turn is yours
    // or the agent's as a channel message is.
    deps.el("author-filter").hidden = deps.el("search-field").hidden;
    // `#212 link-filter`. The kinds of link under it go with it.
    deps.renderLinkKinds();
    // The bar used to yield the header row to an open search field when the reader had moved it up
    // there, because the two were sharing one 375px row. The field is not in that row any more, so
    // the bar stays where the reader put it, searching or not.
    deps.el("control-bar").hidden = members.every((member) => member.hidden);
    // The header collapses when it holds nothing. With the bar at the bottom that is the ordinary
    // case on the main screen, and an empty 2.4rem strip across the top of a phone is exactly the
    // real estate `#58 control-bar` exists to reclaim. It is a hide of the whole grid row, so the
    // body grows into it rather than leaving a band of empty panel.
    //
    // `#129 message-search` put its glass in the strip's corner and so kept the row on every main
    // screen, on the argument that a strip holding a control is not an empty one. The owner's verdict
    // on the result was "wasteful": a full-width band bought for one small icon. `#197
    // floating-search` floats the glass over the list beside the freshness pill instead, and the
    // header is back to costing a row only when it has a title, a way back, or the bar to carry.
    deps.el("topbar").hidden =
      deps.el("close-settings").hidden &&
      deps.el("topbar-title").hidden &&
      (deps.barWhere() !== "top" || deps.el("control-bar").hidden);
  }


  return { renderControlBar };
}
