// The keys for the search, Settings, Help, Back and Escape (`#224 keyboard-shortcuts`, `#344 search-shortcut`,
// `#281 view-keys`, `#277 reply-in-column`; `#624 voice-page-modularity`, moved from the legacy script into
// view/shell for the plan's step 17): what "/", Ctrl+/, ",", "?" and Escape do on each screen, and where Back
// goes from the screen that is up. The screens, the search, the menus, the reply, the threads and the lists
// they act on are the page's.

import { childByClass, nodeWithin, shownNode } from "../dom/index.ts";
import type { Screens } from "./screens.ts";

/** Where the keys act in a list: its pane and the row, either of them possibly null. */
export interface KeyPlace<Pane> {
  readonly pane: Pane | null;
  readonly row: HTMLElement | null;
}

export interface ScreenKeysDependencies<Pane> {
  /** Which screen is up, the screen Help returns to, and setting where it returns. */
  readonly screens: Pick<Screens, "screen" | "helpReturn" | "setHelpReturn">;
  /** Show the screen `name`. */
  showScreen(name: string): void;
  /** Leave Settings or Help, as their Back button does. */
  closeSettings(): void;
  /** Show the Help screen at its entry `entry`. */
  showHelp(entry: string): void;
  /** An element of the page by id. */
  byId(id: string): HTMLElement;
  /** Whether the search is open. */
  searchOpen(): boolean;
  /** Open or close the search. */
  setSearchOpen(open: boolean): void;
  /** The names of the menus open over the page (a ⋯ menu, the pace popover, the prompts tray, ...). */
  openMenus(): Iterable<string>;
  /** Close the open menu `name`; whether it was open. */
  closeOpenMenu(name: string): boolean;
  /** Close the open ⋯ menu on Escape; whether there was one. */
  closeRowMenuOnEscape(event: unknown): boolean;
  /** Whether the reply's box is docked in a list. */
  replyDocked(): boolean;
  /** Whether the keys are in the column the reply is open in. */
  replyKeysHere(): boolean;
  /** Put the reply down. */
  closeReply(): void;
  /** Whether the thread list covers the left column. */
  threadListOnScreen(): boolean;
  /** Back out of the thread list, to what it was chosen from. */
  threadListBack(): void;
  /** Whether the channel's view is on screen. */
  channelOnScreen(): boolean;
  /** The view the channel's list shows ("thread" in a thread). */
  selectionView(): string;
  /** Whether the code works in the right column and that column shows the thread cards. */
  sideShowsCards(): boolean;
  /** Leave the thread on screen. */
  leaveThread(): void;
  /** The pane and row the keys act on from `target`. */
  keyContext(target: HTMLElement): KeyPlace<Pane>;
  /** Select `row` of `pane`; `bring` false leaves the scroll where it is. */
  selectRow(pane: Pane, row: HTMLElement, bring: boolean): void;
  /** Settle the focus after the search closes. */
  settleFocus(): void;
  /** Whether `node` is a message box. */
  isComposer(node: HTMLElement): boolean;
  /** Give the focus to the list of `pane`. */
  focusList(pane: Pane | null): void;
  /** The list's pane `node` is in, or null. */
  paneFor(node: HTMLElement): Pane | null;
  /** Forget a channel being picked by its keys. */
  forgetChannelPick(): void;
  /** Put out a highlighted thread card on screen; whether there was one (`#277 thread-card-focus`). */
  putOutLitCard(): boolean;
  /** Let go of a thread selected in All by its strip; whether there was one (`#299 all-thread-strip`). */
  clearAllThreadFocus(): boolean;
}

/** The keys. Each returns whether it did anything, as a binding of the keymap does. */
export interface ScreenKeys {
  /**
   * "/": the search, with its field ready to type in. It was Ctrl+S (Cmd+S) as well until `#248
   * composer-key` gave that chord to the message box; the glass over the list is the other way in, and
   * Ctrl+/ (Cmd+/) the way in from anywhere (`searchFromAnywhere`). Settings and Help are left
   * first, as their Back button leaves them: the search is over the list they cover. Anywhere else off
   * the main screen there is nothing to search, and nothing happens. Pressed again while the search is
   * open, the field takes the focus back with its text selected, ready to be typed over.
   */
  searchFromKeyboard(): boolean;
  /**
   * Ctrl+/ (Cmd+/): the search from anywhere, whatever holds the focus. `#344 search-shortcut`. The owner:
   * "want reliable cmd-/ for search ... Sometimes just pressing '/' doesn't work". A bare "/" is a character
   * in a field, the type-to-find of a picker, a step of a slider, and nothing with single keys off, so
   * after a send (the focus stays in the box) or a channel chosen with the mouse (it stays on the picker)
   * it searched nothing. The chord has none of those exceptions.
   *
   * An open ⋯ menu, the pace popover or the prompts tray closes first, as the back gesture closes them, so
   * the back stack stays in step (`#304 back-gesture-in-app`). A reply open in a list stays open: the
   * search is over the list it is in. So does a message's details sheet, which is part of its row, not
   * something over the list. Then it is "/" (`searchFromKeyboard`). True wherever it is pressed, on sign-in
   * too, where there is nothing to search: the chord is never the browser's.
   */
  searchFromAnywhere(): boolean;
  /**
   * "," and Ctrl+, (Cmd+,): Settings, from the screens the gear is on (the main one and sign-in), and
   * back out of Settings or Help to where the reader was, as Back takes them. Every control in Settings
   * keeps its value the moment it changes, so leaving is all that "save and exit" needs.
   */
  settingsFromKeyboard(): boolean;
  /** Leave Help: back to Settings, or to wherever `?` was pressed (`helpReturn`). */
  closeHelp(): void;
  /**
   * "?" and Ctrl+? (Cmd+?, which is Cmd+Shift+/): the list of every key, the Keyboard entry of the Help
   * screen, and Escape back to where it was pressed with the place of the reader kept. From the screens
   * the gear reaches. Ctrl+/ (Cmd+/) was this until `#344 search-shortcut` gave it to the search.
   */
  keysFromKeyboard(): boolean;
  /**
   * Whether the reply's box is on screen: open in a list of the main screen (`#410 inline-reply-editor`).
   * c and Ctrl+S write in it, and Back puts it down — before it leaves a thread.
   */
  replyOnScreen(): boolean;
  /**
   * Where Back goes from the screen that is up, taken; false when there is no Back to take. The thread
   * list's is what it was chosen from (`threadListBack`, `#281 view-keys`): it covers the left column,
   * so a thread there is not on screen to leave.
   *
   * `#277 reply-in-column`. A reply open in a column is that column's Back, not the other's: Escape on
   * the other column's list does there what it does with no reply open, and leaves the reply, out of
   * sight of the keys, as it is. Escape runs in the column of the keys (`paneColumn`).
   */
  goBack(): boolean;
  /**
   * What Escape does: the first that applies of the proposal's section 5.2. An open ⋯ menu, the pace
   * popover, the prompts tray, or a details sheet closes; then the search; then a message box with text
   * in it is left, the draft kept and the focus back on the list (question 4), while an empty one goes
   * Back at once, there being nothing to keep; a picker on the bar is left for the list, whatever it
   * holds (`isBarPicker`); then a highlighted thread card on screen is put out (`#277
   * thread-card-focus`); then a thread selected in All by its strip is let go (`#299 all-thread-strip`);
   * then Back: out of Settings, Help or Reply, out of the thread list to what it
   * was chosen from, or out of a thread — to the thread list, for one opened from it (`#281 view-keys`).
   * With none of those it does nothing: in particular it marks nothing read, which is the proposal's
   * answer to the clients whose Escape does.
   */
  onEscape(event: unknown, target: HTMLElement): boolean;
}

export function createScreenKeys<Pane>(deps: ScreenKeysDependencies<Pane>): ScreenKeys {
  const screens = deps.screens;

  function searchFromKeyboard(): boolean {
    if (screens.screen() === "settings" || screens.screen() === "help") deps.closeSettings();
    if (screens.screen() !== "main") return false;
    if (!deps.searchOpen()) deps.setSearchOpen(true);
    const field = deps.byId("search-field") as HTMLInputElement;
    field.focus();
    if (typeof field.select === "function") field.select();
    return true;
  }

  function closeHelp(): void {
    deps.showScreen(screens.helpReturn());
  }

  function replyOnScreen(): boolean {
    return deps.replyDocked() && screens.screen() === "main";
  }

  function goBack(): boolean {
    if (screens.screen() === "settings") deps.closeSettings();
    else if (screens.screen() === "help") closeHelp();
    else if (replyOnScreen() && deps.replyKeysHere()) deps.closeReply();
    else if (deps.threadListOnScreen()) deps.threadListBack();
    // Not over the right column's cards, whose column still holds the thread it showed last: that Back
    // was the X with nothing to close, and put the cards back at their foot. `#277 thread-card-focus`.
    else if (deps.channelOnScreen() && deps.selectionView() === "thread" && !deps.sideShowsCards()) deps.leaveThread();
    else return false;
    return true;
  }

  /** Close the details sheet on the message the keys act on. Returns whether one was open. */
  function closeDetailsOn(target: HTMLElement): boolean {
    const { pane, row } = deps.keyContext(target);
    const sheet = row ? childByClass(row, "msg-details") : null;
    if (!pane || !row || !sheet) return false;
    const inside = nodeWithin(sheet, document.activeElement);
    row.removeChild(sheet);
    if (inside) deps.selectRow(pane, row, false);
    return true;
  }

  /**
   * A picker on the bar of the main screen: the channel's, or the thread's. Every single key on one is
   * its own (`keepsOwnKeys`), so Escape is the way off it, back to the list, as from a message box
   * with text in it: the first Escape leaves the picker and the next goes Back.
   */
  function isBarPicker(node: HTMLElement | null | undefined): boolean {
    return screens.screen() === "main" && Boolean(node) && ["discord-channel", "thread-select"].includes(String((node as HTMLElement).id || ""));
  }

  return {
    searchFromKeyboard,
    searchFromAnywhere() {
      for (const name of deps.openMenus()) deps.closeOpenMenu(name);
      searchFromKeyboard();
      return true;
    },
    settingsFromKeyboard() {
      if (screens.screen() === "settings" || screens.screen() === "help") {
        deps.closeSettings();
        return true;
      }
      if (screens.screen() === "main" || screens.screen() === "signin") {
        deps.showScreen("settings");
        return true;
      }
      return false;
    },
    closeHelp,
    keysFromKeyboard() {
      if (screens.screen() !== "help") {
        if (!["main", "signin", "settings"].includes(screens.screen())) return false;
        screens.setHelpReturn(screens.screen());
      }
      deps.showHelp("keyboard");
      return true;
    },
    replyOnScreen,
    goBack,
    onEscape(event, target) {
      if (deps.closeRowMenuOnEscape(event)) return true;
      // The pace popover and the prompts tray, as the back gesture closes them (`#304 back-gesture-in-app`).
      if (deps.closeOpenMenu("menu:pace") || deps.closeOpenMenu("menu:prompts") || deps.closeOpenMenu("menu:actions")) return true;
      // `#348 settings-dynamic-lists`. The pop-out over Recently dismissed, before Settings itself.
      if (deps.closeOpenMenu("menu:dismissed")) return true;
      if (closeDetailsOn(target)) return true;
      if (deps.searchOpen() && screens.screen() === "main") {
        deps.setSearchOpen(false);
        deps.settleFocus();
        return true;
      }
      if (deps.isComposer(target) && shownNode(target)) {
        if (String((target as HTMLInputElement).value || "") === "" && goBack()) return true;
        deps.focusList(deps.paneFor(target));
        return true;
      }
      if (isBarPicker(target)) {
        deps.forgetChannelPick();
        deps.focusList(deps.paneFor(target));
        return true;
      }
      if (deps.putOutLitCard()) return true;
      // `#299 all-thread-strip`. A thread selected in All by its strip is let go before All is left.
      if (deps.clearAllThreadFocus()) return true;
      return goBack();
    },
  };
}
