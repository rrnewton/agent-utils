// The keys to the message box (`#241 voice-composer`, `#248 composer-key`, `#403 composer-key-global`, `#410
// inline-reply-editor`, `#422 composer-key-stay`, `#538 c-key-copy`; `#624 voice-page-modularity`, moved from the
// legacy script into view/shell for the plan's step 17): m and Ctrl+S (Cmd+S) take the reader to the box to write
// in, and ↑ in the channel's empty box selects the newest message. Which box is drawn where, the columns, the
// thread list and the lists are the page's.

import { focusMinimally, nodeWithin, shownNode } from "../dom/index.ts";

export interface WriteKeysDependencies<Pane, Column> {
  /** An element of the page by id, in the column the code is working in. */
  byId(id: string): HTMLElement;
  /** An element of the left column by id, whichever column the code is working in. */
  leftById(id: string): HTMLElement;
  /** Which screen is up. */
  screen(): string;
  /** Which view the main screen shows ("voice", "discord"). */
  view(): string;
  /** Leave Settings or Help, as their Back button does. */
  closeSettings(): void;
  /** Whether the reply's box is on screen (`#410 inline-reply-editor`). */
  replyOnScreen(): boolean;
  /** Whether the list of the channel is the one on screen. */
  channelOnScreen(): boolean;
  /** Whether the thread list covers the left column. */
  threadListOnScreen(): boolean;
  /** Back out of the thread list, to what it was chosen from. */
  threadListBack(): void;
  /** Whether Main channel puts the main channel's box at the foot of the thread list's cards (`#419`). */
  mainRowsOnScreen(): boolean;
  /** Whether `node` is behind the thread list, which covers the left column. */
  behindThreadList(node: unknown): boolean;
  /** Whether `node` is in a column that has gone. */
  inColumnGone(node: unknown): boolean;
  /** Whether `node` is a message box. */
  isComposer(node: unknown): boolean;
  /** Whether the code is working in the right column. */
  inSide(): boolean;
  /** The column the keys moved last. */
  activeColumn(): Column;
  /** The left column. */
  mainColumn(): Column;
  /** Run `act` in `column`, and what it returned. */
  inColumn<T>(column: Column, act: () => T): T;
  /** The channel's list, and the right column's. */
  channelPane(): Pane;
  sidePane(): Pane;
  /** The list's pane `node` is in, or null. */
  paneFor(node: unknown): Pane | null;
  /** The column of `pane`. */
  paneColumn(pane: Pane): Column;
  /** The rows of `pane`'s list. */
  paneRows(pane: Pane): HTMLElement[];
  /** Select `row` of `pane`, brought into view. */
  selectRow(pane: Pane, row: HTMLElement): void;
}

/** The keys. Each returns whether it did anything, as a binding of the keymap does. */
export interface WriteKeys {
  /**
   * m (`#538 c-key-copy`; c before): the box to write in: the channel's (a thread's, in a thread), the reply's
   * on Reply, or on the call's view the agent's, under the transcript (`#241 voice-composer`). Going to the
   * agent's box opens nothing: a conversation starts only when something is sent from it.
   *
   * `#403 composer-key-global`. In two columns it is the box of the column being worked on, which the
   * key chose (`paneFor`), and the right column on its thread cards has none, so the box there is Main's.
   * `#410 inline-reply-editor`. A reply open anywhere is the one box drawn, the columns' own hidden, so it
   * is where writing goes from either column.
   */
  composeFromKeyboard(): boolean;
  /**
   * Ctrl+S (Cmd+S), and Ctrl+Enter (Cmd+Enter) outside a text box: the message box, ready to type in,
   * from anywhere. `#248 composer-key`. The owner: "When I'm using keyboard, I want to jump to text
   * entry and start typing. Google chat uses Cmd-s for that and I'm ok with that. Or maybe Cmd-enter
   * could jump to it if it's not highlighted (or send message if it is highlighted with non-zero chars
   * in it)."
   *
   * The box is the one m chooses (`composeFromKeyboard`): the channel's, the thread's the reader is in
   * (on the right, in two columns, when the keys are there), the reply's on Reply, or the agent's on
   * the call's view. Settings and Help are left first, as their Back leaves them, because the box is on
   * the screen they cover. Returns whether it did anything; the bindings that call it keep the
   * browser's own default for the chord shut either way.
   *
   * `#403 composer-key-global`. The owner, in two columns: "when a message in the right column is
   * highlighted and focus is elsewhere. But if focus was in the left column it worked. It seemed
   * selective. It should be globally applicable." The rule, which Help states: in two columns the box
   * of the column the focus is in; with the focus in neither (the bar, the page itself) the box of the
   * column holding the selection, which is the one the keys moved last (`paneFor`); and Main's where
   * that column has none, as on the thread cards. The thread list is left first too, for the view it
   * was chosen from, because it covers the column whose box it is; and the column is asked again once
   * whatever covered it has gone. In a message box on screen it stays, a reply open in that column or
   * not; on a column's chips, outside its scroller, it is that column's box. Ctrl+S is heard before any
   * element's own listener (`everywhere`).
   *
   * `#422 composer-key-stay`. The owner, 2026-10-22: "when I press Cmd-S even if I have Main messages view
   * on, it jumps to a DIFFERENT view ... I would want to keep my view and just enter the messages here."
   * The rule in every view: the chord never changes the view when the view on screen has a box. Main, All,
   * a thread and two columns always have one. The thread list has one while Main channel puts the main
   * channel's box at the foot of its cards (`#419`), or while a reply is open among them (`#410`): it is
   * left only where it has neither. Settings and Help are screens over the view, not views, and are left.
   */
  writeFromKeyboard(): boolean;
  /** ↑ in the box of the channel while it is empty: the newest message selected, as a preference of Slack has it. */
  newestFromComposer(target: HTMLElement | null | undefined): boolean;
}

export function createWriteKeys<Pane, Column>(deps: WriteKeysDependencies<Pane, Column>): WriteKeys {
  function composeFromKeyboard(): boolean {
    // `#410 inline-reply-editor`. A reply open anywhere is the one box drawn: it is where writing goes,
    // from either column.
    if (deps.replyOnScreen()) {
      deps.leftById("reply-text").focus();
      return true;
    }
    if (deps.screen() === "main" && deps.view() === "voice") {
      const agent = deps.leftById("compose-text");
      if (!shownNode(agent)) return false;
      // Its own focus listener brings the whole box into view (`keepComposerInView`).
      agent.focus();
      return true;
    }
    const box = deps.byId("channel-compose-text");
    if (!deps.channelOnScreen() || deps.behindThreadList(box)) return false;
    if (!shownNode(box)) return deps.inSide() ? deps.inColumn(deps.mainColumn(), composeFromKeyboard) : false;
    // The box is at the foot of the list, and writing in it is going there: as far as shows it.
    focusMinimally(box);
    return true;
  }

  /**
   * Whether the thread list on screen holds a message box of its own (`#422 composer-key-stay`): the main
   * channel's at the foot of the cards while Main channel is on (`placeMainCard`), or a reply open among them.
   */
  function threadListHasBox(): boolean {
    return deps.threadListOnScreen() && (deps.replyOnScreen() || deps.mainRowsOnScreen());
  }

  return {
    composeFromKeyboard,
    writeFromKeyboard() {
      if (deps.screen() === "settings" || deps.screen() === "help") deps.closeSettings();
      if (deps.threadListOnScreen() && !threadListHasBox()) deps.threadListBack();
      const held = document.activeElement;
      // Already in a message box on screen: it stays there, even beside a reply open in its column.
      if (deps.isComposer(held) && shownNode(held) && !deps.behindThreadList(held) && !deps.inColumnGone(held)) return true;
      let pane: Pane | null = null;
      if (deps.screen() === "main") {
        // The chips over a column's foot are outside its scroller and still that column's.
        if (held && nodeWithin(deps.leftById("side-tools"), held)) pane = deps.sidePane();
        else if (held && nodeWithin(deps.leftById("scroll-tools"), held)) pane = deps.channelPane();
        else pane = deps.paneFor(held);
      }
      return deps.inColumn(pane ? deps.paneColumn(pane) : deps.activeColumn(), composeFromKeyboard);
    },
    newestFromComposer(target) {
      // The box of the column the key is in: the thread's on the right, in two columns. `#229`.
      if (!target || target !== deps.byId("channel-compose-text") || String((target as HTMLInputElement).value || "") !== "") return false;
      const pane = deps.inSide() ? deps.sidePane() : deps.channelPane();
      const rows = deps.paneRows(pane);
      if (rows.length === 0) return false;
      deps.selectRow(pane, rows[rows.length - 1] as HTMLElement);
      return true;
    },
  };
}
