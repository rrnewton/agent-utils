// The keys for the views, the filters and Undo (`#281 view-keys`, `#525 cmd-0-voice-view`, `#432 read-mode-unify`,
// `#443 undo-key-general`; `#624 voice-page-modularity`, moved from the legacy script into view/shell for the
// plan's step 17): Ctrl+0 to Ctrl+4, h, Shift+P, Shift+L and the author filter's key, and z. The views, the
// filters, the read modes and every undo offer are the page's.

import type { Screens } from "./screens.ts";

/** The four views Ctrl+1 to Ctrl+4 choose, in the picker's order. */
export type ViewChoice = "main" | "flat" | "threads" | "columns";

/** An undo on offer, as far as the key reads it: when it was made, and what it is of. */
export interface UndoOfferStamp {
  /** Its undo stamp; an offer from before the stamps has none, and counts oldest. */
  readonly at?: number;
}

export interface ViewKeysDependencies<Column> {
  /** Which screen is up, the screen Settings was opened over, and the view of the main screen. */
  readonly screens: Pick<Screens, "screen" | "beforeSettings" | "view">;
  /** Whether the code is working in the right column. */
  inSide(): boolean;
  /** The left column. */
  mainColumn(): Column;
  /** Run `act` in `column`, and what it returned. */
  inColumn<T>(column: Column, act: () => T): T;
  /** Leave Settings or Help, as their Back button does. */
  closeSettings(): void;
  /** Press the view switch: from the call's view to the channel's, or back. */
  pressViewSwitch(): void;
  /** The channel chosen, or a value that is not one. */
  selectedChannelId(): unknown;
  /** Whether the list of the channel is the one on screen. */
  channelOnScreen(): boolean;
  /** Choose the view `which` (`chooseView`). */
  chooseView(which: ViewChoice): void;
  /** Settle the focus after the view changes. */
  settleFocus(): void;
  /** Whether the Hide read button is hidden. */
  readModeHidden(): boolean;
  /** The Hide read button's own act. */
  cycleReadMode(): void;
  /** What the status line says of the read mode now. */
  readModeSaid(): string;
  /** Say `text` on the status line. */
  setStatus(text: string): void;
  /** Whether the thread list covers the left column. */
  threadListOnScreen(): boolean;
  /** Whether the search bar is open, and opening it. */
  searchOpen(): boolean;
  setSearchOpen(open: boolean): void;
  /** Whether the Pinned filter is on, and turning it on or off. */
  pinnedOnly(): boolean;
  setPinnedOnly(on: boolean): void;
  /** Whether the Links filter is on, and turning it on or off. */
  linksOnly(): boolean;
  setLinksOnly(on: boolean): void;
  /** The author filter's next step. */
  cycleAuthorFilter(): void;
  /** Give the focus to the channel's list. */
  focusChannelList(): void;
  /** The offer the last Done's Undo chip makes, or null; and taking it back. */
  doneOffer(): UndoOfferStamp | null;
  undoDone(): void;
  /** The offer a send dismissed from the outbox makes, or null; whether its copy is still held; and putting it back. */
  sendOffer(): (UndoOfferStamp & { readonly id: string }) | null;
  sendHeld(id: string): boolean;
  putBackSend(id: string): void;
  /** The offer a removed bookmark makes, or null; the channel on screen; and putting it back. */
  bookmarkOffer(): (UndoOfferStamp & { readonly channel: string }) | null;
  currentChannelId(): string;
  undoBookmarkRemoval(): void;
}

/** The keys. Each returns whether it did anything, as a binding of the keymap does. */
export interface ViewKeys {
  /**
   * Ctrl+0 (Cmd+0 on an Apple keyboard): the voice (call) view, as the view switch goes there.
   * `#525 cmd-0-voice-view`. The owner, 2026-10-25: "Cmd-0 go to voice view. We have these great
   * constant time shortcuts to jump to views Cmd-1 thru Cmd-4.. but what about voice channel?
   * Make that Cmd-0."
   *
   * The rules of Ctrl+1 to Ctrl+4, mirrored: a chord, so it fires in a message box and in the
   * search; from Settings and Help opened over the main screen, which it closes; a held key's
   * repeats taken but choosing nothing; and where it can choose nothing (the sign-in screen)
   * it is left to the browser, whose own Ctrl+0 is its zoom reset — kept from the browser only
   * when the page takes the chord, as 1 to 4 keep its tab switch. No channel need be chosen:
   * the call's view is not a view of one. Already there, it is taken and stays.
   */
  voiceFromKeyboard(repeat: boolean): boolean;
  /**
   * Ctrl+1 to Ctrl+4 (Cmd on an Apple keyboard): Main, All, the thread list, and two columns, the four
   * views in the picker's order (`chooseView`). `#281 view-keys`. The owner, 2026-10-13: "Change the 1 2
   * 3 keystrokes ... so it is Cmd-1 through Cmd-3. It should work irrespective of whether we have the
   * text box selected etc. ... TBH I think two column view should just be Cmd-4 as the 4th view."
   *
   * A chord, so it fires in a message box and in the search as it does anywhere else, and types nothing
   * there; and from Settings and Help, which are left as their Back leaves them — and a reply open in a
   * list is put down as a draft, for any view but two columns, which keep it (`chooseView`, `#277
   * reply-in-column`) — and from the call's view, which gives way to the channel as its switch does. Its
   * default is kept from the browser — in Chromium that is the tab switch, which it hands the page first
   * (`IDC_SELECT_TAB_*` is not among the shortcuts it reserves) — whenever it chose a view, and on a held
   * key's repeats, which choose nothing. Where no view can be chosen (the sign-in screen, no channel) it is
   * left to the browser.
   *
   * THE BARE 1, 2 AND 3 ARE RETIRED. Kept beside the chords they would have had to change meaning — 2
   * was the Threads screen and 3 was All — and a key that silently does the other thing is worse than
   * one that does nothing; a reader who wants a view has one chord for it, from anywhere.
   */
  viewFromKeyboard(which: ViewChoice, repeat: boolean): boolean;
  /**
   * h: the cycle of the Hide read button: Show read, Fold read, Hide read. The button's own act
   * (`cycleReadMode`), so the key, the button's words and the list are one state; and said on the status
   * line, as the button is not where the reader's eyes are. `#432 read-mode-unify`.
   */
  readModeFromKeyboard(): boolean;
  /**
   * Shift+P and Shift+L: the Pinned and Links filters, which live in the search bar. So the bar opens
   * with them, as a filter in force must be on screen (`setSearchOpen`), and the focus goes back to the
   * list, where the reader is going to read what is left.
   */
  filterFromKeyboard(which: string): boolean;
  /**
   * z, and Ctrl+Z (Cmd+Z on a Mac) outside a text field: undo the newest thing that can still be
   * taken back, whatever kind it is — a Done, a send dismissed from the outbox, a bookmark taken
   * away. The owner: "if we have an undo keystroke it should be a general undo action that applies
   * to anything that we can undo and anything that pops up an undo chip." Pressed again it takes the
   * next one back, while its chip is still up.
   */
  undoFromKeyboard(): boolean;
}

export function createViewKeys<Column>(deps: ViewKeysDependencies<Column>): ViewKeys {
  const screens = deps.screens;

  function voiceFromKeyboard(repeat: boolean): boolean {
    if (deps.inSide()) return deps.inColumn(deps.mainColumn(), () => voiceFromKeyboard(repeat));
    const covering = screens.screen() === "settings" || screens.screen() === "help";
    if (!(screens.screen() === "main" || (covering && screens.beforeSettings() === "main"))) {
      return false;
    }
    if (repeat) return true;
    if (covering) deps.closeSettings();
    if (screens.view() !== "voice") deps.pressViewSwitch();
    return true;
  }

  function viewFromKeyboard(which: ViewChoice, repeat: boolean): boolean {
    // A view is the page's, whichever column the keys are in.
    if (deps.inSide()) return deps.inColumn(deps.mainColumn(), () => viewFromKeyboard(which, repeat));
    const channel = String(deps.selectedChannelId() || "");
    const covering = screens.screen() === "settings" || screens.screen() === "help";
    // Settings and Help are left for the screen they were opened over, which is the channel's only when
    // that is the main screen.
    if (!channel || !(screens.screen() === "main" || (covering && screens.beforeSettings() === "main"))) {
      return false;
    }
    if (repeat) return true;
    if (covering) deps.closeSettings();
    if (screens.view() !== "discord") deps.pressViewSwitch();
    if (!deps.channelOnScreen()) return false;
    deps.chooseView(which);
    deps.settleFocus();
    return true;
  }

  function filterFromKeyboard(which: string): boolean {
    // The bar and its Pinned filter are the left column's, whichever column the keys are in. `#229`.
    if (deps.inSide()) return deps.inColumn(deps.mainColumn(), () => filterFromKeyboard(which));
    // Over the thread list on its own they would filter the column behind it. `#281 view-keys`.
    if (!deps.channelOnScreen() || deps.threadListOnScreen()) return false;
    if (!deps.searchOpen()) deps.setSearchOpen(true);
    if (which === "pinned") deps.setPinnedOnly(!deps.pinnedOnly());
    else if (which === "author") deps.cycleAuthorFilter();
    else deps.setLinksOnly(!deps.linksOnly());
    deps.focusChannelList();
    return true;
  }

  /**
   * `#443 undo-key-general`. Every undo still on offer, newest first, each with the act its chip
   * performs — the chip's own click, word for word, so the key and the chip can never disagree about
   * what Undo does. An offer counts while its chip is up: a Done while `lastDismissal` stands, a
   * dismissed send while its copy is held, a removed bookmark in the channel it was removed in (its
   * chip is drawn nowhere else, and undoing another channel's removal from here would move a line
   * the reader is not looking at). Stamped as each was made (`nextUndoAt`); an offer a build from
   * before the stamps left standing has none, and counts oldest.
   */
  function liveUndos(): Array<() => void> {
    const held: Array<{ at: number; run: () => void }> = [];
    const done = deps.doneOffer();
    if (done !== null) {
      held.push({ at: done.at || 0, run: () => deps.undoDone() });
    }
    const sendOffer = deps.sendOffer();
    if (sendOffer && deps.sendHeld(sendOffer.id)) {
      held.push({ at: sendOffer.at || 0, run: () => {
        const offer = deps.sendOffer();
        if (offer) deps.putBackSend(offer.id);
      } });
    }
    const bookmarkOffer = deps.bookmarkOffer();
    if (bookmarkOffer && bookmarkOffer.channel === deps.currentChannelId()) {
      held.push({ at: bookmarkOffer.at || 0, run: () => deps.undoBookmarkRemoval() });
    }
    return held.sort((a, b) => b.at - a.at).map(({ run }) => run);
  }

  return {
    voiceFromKeyboard,
    viewFromKeyboard,
    readModeFromKeyboard() {
      if (!deps.channelOnScreen() || deps.readModeHidden()) return false;
      deps.cycleReadMode();
      deps.setStatus(deps.readModeSaid());
      return true;
    },
    filterFromKeyboard,
    undoFromKeyboard() {
      if (!deps.channelOnScreen()) return false;
      const [newest] = liveUndos();
      if (!newest) deps.setStatus("Nothing to undo.");
      else newest();
      return true;
    },
  };
}
