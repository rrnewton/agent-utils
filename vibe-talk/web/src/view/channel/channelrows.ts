// Drawing a channel row (`#624 voice-page-modularity`, moved from the legacy script into
// view/channel): empty messages left out, split posts drawn as one, a row's frame and everything
// on it — fold, Reply, its ⋯ menu, Done — the menu's closing, marking read through a row, copying
// a message or its address, and the channel's summary line. Why each behaves as it does is told
// where the legacy script held them. Opening a reply stays the page's (`openReply`, injected);
// the reply box is not this owner's.

import type { Message } from "../../contract/index.ts";
import {
  BLANK_TEXT, combinedContent, isNoise, joinsGroup, reactionText, rowEdited,
  rowReactions, upstreamAppUrlOf, upstreamUrlOf,
} from "../../present/rows/index.ts";
import { classWords, copyText, nodeWithin, setClassWord } from "../dom/index.ts";
import { editedTitle, messageDate, messageTimeLabel, shortLocalTime } from "../../present/time/index.ts";
import { createRowMenus } from "./rowmenu.ts";
import type { RowMenus } from "./rowmenu.ts";
import { renderedText } from "./text.ts";
import { patchRows } from "./rows.ts";
import { EDITED_MARK, timeNode } from "./time.ts";

/** Most ids one dismissal request carries; the server refuses a larger batch. */
export const DISMISS_BATCH = 1000;

/** A drawn channel row, carrying what drawing it put on it. */
type ChannelRow = HTMLElement & {
  messages?: Message[];
  replyButton?: HTMLButtonElement;
  foldEntry?: unknown;
};

/** What drawing a row asks of the page. */
export interface ChannelRowsDependencies {
  byId(id: string): HTMLElement;
  inThisColumn<A extends unknown[]>(fn: (...args: A) => void): (...args: A) => void;
  threadOf(message: unknown): string | null;
  heldThreads(): Array<{ root?: { id?: unknown } | null; reply_count?: number } | null>;
  heldMessages(): Message[];
  hideEmptyOn(): boolean;
  combineOn(): boolean;
  listedMessages(): Message[];
  preservingScroll(fn: () => void): void;
  renderChannelRows(): void;
  renderScrollTools(): void;
  renderBodyInto(body: HTMLElement, messages: Message[]): void;
  headingOf(message: Message): unknown;
  restatedMarked(first: Element): boolean;
  headingLength(heading: unknown, extra: number): string;
  headingPieces(heading: unknown): Node;
  clipboard(): unknown;
  replyArrow(li: HTMLElement): Node;
  pinned(): boolean;
  main(): boolean;
  foldable(li: ChannelRow, meta: Element, body: HTMLElement, text: unknown, id: string, also: string[]): unknown;
  tapRow(li: HTMLElement, fold: unknown): void;
  sayRowShortcut(el: Element, name: string): void;
  openReply(messages: Message[], li: HTMLElement, quote: unknown): void;
  takeQuote(button: Element): unknown;
  draftMarker(messages: Message[], li: HTMLElement): Node;
  replyingTo(message: Message): boolean;
  markReplying(li: HTMLElement, on: boolean): void;
  addThreadDecoration(acts: HTMLElement, message: Message, li: HTMLElement, way: boolean): void;
  providerName(): string;
  upstreamReadMark(): boolean;
  scheduleBackSync(): void;
  guardQuietly<T>(fn: () => T): (...args: unknown[]) => Promise<unknown>;
  markReadUpstream(id: string): Promise<unknown>;
  storedSlackOpen(): string;
  isPinnedId(id: unknown): boolean;
  togglePin(messages: Message[]): Promise<unknown>;
  toggleBookmarkAt(li: HTMLElement): void;
  copyDeepLink(message: Message): Promise<unknown>;
  toggleArchived(ids: string[], li: HTMLElement): Promise<unknown>;
  markNotNoise(ids: string[]): Promise<unknown>;
  swipeable(li: HTMLElement, messages: Message[]): void;
  searchable(li: HTMLElement, ...parts: unknown[]): void;
  anchorsIn(body: HTMLElement): Array<{ href: string }>;
  drawLinkedOn(li: HTMLElement, ids: string[]): void;
  idsOf(row: Element): string[];
  archived(): Set<string>;
  onTheirWay(): string[];
  flashDone(row: HTMLElement): void;
  unflashDone(row: HTMLElement): void;
  dismissMessages(body: { messages: string[] }, bump: unknown): Promise<unknown>;
  throughBump(row: HTMLElement): unknown;
  doneSaved(ids: string[], holds: () => boolean): Promise<boolean>;
  doneHeldHere(ids: string[]): boolean;
  selectedChannelId(): string;
  setStatus(text: string): void;
  currentChannelId(): string | null;
  channels(): Array<{ id?: unknown; provider?: unknown }>;
  providers(): { has(key: string): boolean; size: number };
  serviceLabel(key: string): string;
  providerOfChannel(id: unknown): { name?: unknown } | null;
  chatServiceName(): string;
  copyRefused: string;
  replyButtonTitle: string;
  historyReach(): { start: boolean; topLevel: boolean; whole: boolean; threads: number; full: number };
  closeOpenMenu(name: string): boolean;
}

/** The row drawer's commands and questions. */
export interface ChannelRows {
  readonly rowMenus: RowMenus;
  emptyMessage(message: Message | null | undefined): boolean;
  mayLeadToReplies(message: Message): boolean;
  hiddenEmpty(message: Message): boolean;
  glom(messages: Message[]): Message[][];
  regroupChannelRows(rebuild?: boolean): void;
  channelRowFrame(messages: Message[]): { li: ChannelRow; meta: HTMLElement; body: HTMLElement; content: string; text: string; reactions: HTMLElement | null };
  editedMark(messages: Message[]): HTMLElement | null;
  reactionStrip(messages: Message[]): HTMLElement | null;
  discordNode(messages: Message[]): ChannelRow;
  closeRowMenuOutside(event: { target?: unknown } | null): void;
  closeRowMenuOnEscape(event: { key?: string } | null): boolean;
  markReadThroughHere(row: HTMLElement, upstreamId: string): Promise<void>;
  storedSlackOpen(): string;
  upstreamServiceName(message: Message | null): string;
  copyUpstreamLink(url: unknown, service: string): Promise<boolean>;
  copyMessageText(text: unknown): Promise<boolean>;
  writeClipboardText(value: string): Promise<boolean>;
  channelSummary(count: number, complete: unknown, label: string): string;
}

/** No menu open, nothing drawn. */
export function createChannelRows(deps: ChannelRowsDependencies): ChannelRows {
  const el = deps.byId;

  function emptyMessage(message: Message | null | undefined): boolean {
    if (!message || !BLANK_TEXT.test(String(message.content || ""))) return false;
    if (Array.isArray(message.reactions) && message.reactions.some((reaction) => reaction && (reaction as { count?: number }).count as number > 0)) return false;
    return !mayLeadToReplies(message);
  }

  function mayLeadToReplies(message: Message): boolean {
    const thread = message.thread as { is_root?: boolean; reply_count?: number; root_message_id?: unknown } | null | undefined;
    if (thread && thread.is_root === true && thread.reply_count !== 0) return true;
    const id = String(message.id);
    const own = thread && thread.is_root === true ? deps.threadOf(message) : null;
    return deps.heldThreads().some((summary) => summary && summary.root && String(summary.root.id) === id &&
        summary.reply_count !== 0) ||
      deps.heldMessages().some((held) => {
        const heldThread = held.thread as { is_root?: boolean; root_message_id?: unknown } | null | undefined;
        return heldThread && heldThread.is_root !== true &&
          (String(heldThread.root_message_id) === id || (own !== null && deps.threadOf(held) === own));
      });
  }

  function hiddenEmpty(message: Message): boolean {
    return deps.hideEmptyOn() && emptyMessage(message);
  }

  function glom(messages: Message[]): Message[][] {
    const groups: Message[][] = [];
    for (const message of messages) {
      if (hiddenEmpty(message)) continue;
      const last = groups[groups.length - 1];
      if (last && joinsGroup(last[last.length - 1] as Message, message, deps.combineOn())) {
        last.push(message);
      } else {
        groups.push([message]);
      }
    }
    return groups;
  }

  function regroupChannelRows(rebuild = false): void {
    const list = el("discord-log");
    const messages = deps.listedMessages();
    deps.preservingScroll(() => {
      // `rebuild`: every row is built again, as when the links switch rewires each body as it
      // is built (`#543`); a kept row would keep what the old switch gave it.
      patchRows(list, glom(messages), (group) => discordNode(group), [],
        rebuild ? () => false : (drawn, incoming) => drawn === incoming);
      deps.renderChannelRows();
    });
    deps.renderScrollTools();
  }

  function channelRowFrame(messages: Message[]): { li: ChannelRow; meta: HTMLElement; body: HTMLElement; content: string; text: string; reactions: HTMLElement | null } {
    const message = messages[0] as Message;
    const li = document.createElement("li") as ChannelRow;
    li.className = "discord-message";
    li.setAttribute("data-id", String(message.id));
    li.setAttribute("data-ids", messages.map((m) => String(m.id)).join(" "));
    li.messages = messages;
    if ((message as { reply_to?: unknown }).reply_to) {
      li.setAttribute("data-reply-to", String((message as { reply_to?: unknown }).reply_to));
    }
    li.setAttribute(
      "data-answers",
      messages
        .map((m) => ((m as { reply_to?: unknown }).reply_to ? String((m as { reply_to?: unknown }).reply_to) : ""))
        .filter(Boolean)
        .join(" ")
    );
    li.setAttribute("data-author-id", String((message as { author_id?: unknown }).author_id || ""));
    li.setAttribute("data-author", String(message.author || ""));
    li.setAttribute("data-author-bot", (message as { author_is_bot?: unknown }).author_is_bot ? "true" : "false");
    const meta = document.createElement("div");
    meta.className = "meta";
    if ((message as { reply_to?: unknown }).reply_to) {
      const said = document.createElement("span");
      said.className = "reply-said sr-only";
      said.textContent = "reply. ";
      meta.append(said);
    }
    const author = document.createElement("span");
    author.textContent = (message as { author_is_bot?: unknown }).author_is_bot ? `${message.author} (bot)` : String(message.author);
    const timeLabel = messageTimeLabel(message);
    const timeSaid = shortLocalTime(message);
    const stamp = timeNode("msg-time", messageDate(message), timeLabel, timeSaid);
    author.className = "msg-author";
    const pinChip = document.createElement("span");
    pinChip.className = "pin-chip";
    pinChip.hidden = true;
    pinChip.textContent = "Pinned";
    const edited = editedMark(messages);
    meta.append(author, stamp, ...(edited ? [edited] : []), pinChip);
    const body = document.createElement("div");
    body.className = "body";
    deps.renderBodyInto(body, messages);
    const heading = deps.headingOf(message);
    const firstBody = body.children[0];
    if (heading && firstBody && deps.restatedMarked(firstBody)) {
      firstBody.setAttribute("data-rh", "");
      li.setAttribute("data-rh-len", deps.headingLength(heading, [...timeLabel].length - [...timeSaid].length));
      meta.append(deps.headingPieces(heading));
    }
    li.append(meta, body);
    const text = renderedText(body);
    const reactions = reactionStrip(messages);
    if (reactions) li.append(reactions);
    return { li, meta, body, content: combinedContent(messages), text, reactions };
  }

  function editedMark(messages: Message[]): HTMLElement | null {
    const edited = rowEdited(messages) as { at: Date | null } | null;
    if (edited === null) return null;
    const mark = document.createElement(edited.at === null ? "span" : "time");
    mark.className = EDITED_MARK;
    mark.textContent = "(edited)";
    if (edited.at !== null) {
      mark.setAttribute("datetime", edited.at.toISOString());
      mark.setAttribute("title", editedTitle(edited.at));
    } else {
      mark.setAttribute("title", "Edited");
    }
    return mark;
  }

  function reactionStrip(messages: Message[]): HTMLElement | null {
    const tallies = rowReactions(messages) as Array<{ custom?: boolean; own?: boolean; count: number }>;
    if (tallies.length === 0) return null;
    const strip = document.createElement("div");
    strip.className = "reactions";
    strip.setAttribute("role", "group");
    strip.setAttribute("aria-label", "Reactions");
    for (const tally of tallies) {
      const chip = document.createElement("span");
      chip.className = ["reaction", tally.custom ? "reaction-custom" : "", tally.own ? "reaction-own" : ""]
        .filter(Boolean).join(" ");
      const emoji = document.createElement("span");
      emoji.className = "reaction-emoji";
      emoji.textContent = reactionText(tally as never);
      const count = document.createElement("span");
      count.className = "reaction-count";
      count.textContent = String(tally.count);
      chip.append(emoji, count);
      if (tally.own) {
        chip.title = "You reacted";
        const mine = document.createElement("span");
        mine.className = "sr-only";
        mine.textContent = "including you";
        chip.append(mine);
      }
      strip.append(chip);
    }
    return strip;
  }

  /**
   * `#1063 collapsed-card-reaction-overlay`. The most chips a collapsed card's strip may hold and
   * still be tried on its elision line: past this the strip keeps its own row at draw time, before
   * any measuring. A custom emoji's chip (its `:name:` as text) is never tried either: its width
   * is the name's, and names run long.
   */
  const REACTION_OVERLAY_MAX_CHIPS = 4;

  /** The clear room, in CSS pixels, the overlay keeps between the elision note and the chips. */
  const REACTION_OVERLAY_GAP_PX = 8;

  /**
   * Whether `strip` may be tried overlaid at all: few enough chips, and none of them a custom
   * emoji's. The measured pass below has the last word; this only keeps a strip that cannot
   * plausibly share one line from ever being drawn over text, even for a frame.
   */
  /** Whether `element` wears the class `word` (the page's elements are read by `className`). */
  const wears = (element: Element, word: string): boolean => classWords(element).includes(word);

  function reactionOverlayCandidate(strip: HTMLElement): boolean {
    const chips = [...strip.children];
    return chips.length >= 1 && chips.length <= REACTION_OVERLAY_MAX_CHIPS &&
      !chips.some((chip) => wears(chip, "reaction-custom"));
  }

  /** The one watcher for every overlaid strip's row, made where the browser has one to give. */
  let reactionOverlayWatch: ResizeObserver | null = null;

  /**
   * `#1063 collapsed-card-reaction-overlay`. Measure an overlaid strip against what it shares
   * the card's foot with, and put it back on its own row — by taking `reactions-overlay` off it,
   * which is all the separate row ever was — if it covers anything: the elision note (with
   * `REACTION_OVERLAY_GAP_PX` to spare), the clamped text's last drawn line (the words
   * themselves, read with a range, since the body's box is the whole width however short that
   * line falls), or any other drawn block under the text (a summary line, a reply count, a
   * search excerpt). Only ever takes the overlay away, never gives it, so a row cannot flicker
   * between the two: a redraw decides again. No layout to read (no width yet, or no layout
   * engine behind the page) is no evidence either way, and settles nothing.
   *
   * The strip is first pinned with its top on the elision line's top, replacing the stylesheet's
   * foot-of-the-card anchor: the chips are a few pixels taller than that line, and what does not
   * fit in it belongs in the card's bottom padding below, never over the clamped text above. The
   * pin is redone at every measuring, so a width or fold change that moves the line moves it.
   */
  function settleReactionOverlay(li: ChannelRow, strip: HTMLElement, elided: HTMLElement): void {
    if (!wears(strip, "reactions-overlay")) return;
    if (li.getAttribute("data-collapsed") !== "true" || elided.hidden) return;
    strip.style.top = `${elided.offsetTop}px`;
    strip.style.bottom = "auto";
    const row = li.getBoundingClientRect();
    const drawn = strip.getBoundingClientRect();
    if (row.width < 1 || drawn.width < 1 || drawn.height < 1) return;
    const covers = (box: DOMRect, spare = 0): boolean =>
      Math.min(drawn.right, box.right) - Math.max(drawn.left - spare, box.left) > 0.5 &&
      Math.min(drawn.bottom, box.bottom) - Math.max(drawn.top, box.top) > 0.5;
    let covered = drawn.left < row.left - 0.5 || drawn.right > row.right + 0.5;
    const note = [...elided.children].find((span) => span.getBoundingClientRect().width > 0);
    if (!note) covered = true;
    else if (covers(note.getBoundingClientRect(), REACTION_OVERLAY_GAP_PX)) covered = true;
    if (!covered) {
      for (const block of li.children) {
        if (block === strip || block === elided || wears(block, "meta")) continue;
        if (!block.getClientRects().length) continue;
        if (wears(block, "body")) {
          if (typeof document.createRange !== "function") continue;
          const bodyBox = block.getBoundingClientRect();
          const range = document.createRange();
          range.selectNodeContents(block);
          // The clamp's hidden lines still have boxes, below the body's own: only a line the
          // reader can see, ending under the chips, is covered text.
          covered = [...range.getClientRects()].some((line) => line.top < bodyBox.bottom - 0.5 && covers(line));
        } else {
          covered = covers(block.getBoundingClientRect());
        }
        if (covered) break;
      }
    }
    if (covered) {
      setClassWord(strip, "reactions-overlay", false);
      strip.style.top = "";
      strip.style.bottom = "";
      if (reactionOverlayWatch) reactionOverlayWatch.unobserve(li);
    }
  }

  /**
   * Put a collapsed card's strip on its elision line (`#1063 collapsed-card-reaction-overlay`):
   * mark it for the stylesheet, then, once the row is on the page and whenever its width
   * changes, let `settleReactionOverlay` measure it. A browser with no frames settles nothing
   * and keeps the overlay only where the stylesheet's own guards allow it.
   */
  function overlayReactions(li: ChannelRow, strip: HTMLElement, elided: HTMLElement): void {
    setClassWord(strip, "reactions-overlay", true);
    if (typeof requestAnimationFrame === "function") {
      requestAnimationFrame(() => settleReactionOverlay(li, strip, elided));
    }
    if (typeof ResizeObserver === "function") {
      if (!reactionOverlayWatch) {
        reactionOverlayWatch = new ResizeObserver((seen) => {
          for (const { target } of seen) {
            const row = target as ChannelRow;
            const held = [...row.children].find((kid) => wears(kid, "reactions-overlay")) as HTMLElement | undefined;
            const line = [...row.children].find((kid) => wears(kid, "elided")) as HTMLElement | undefined;
            if (held && line) settleReactionOverlay(row, held, line);
            else if (reactionOverlayWatch) reactionOverlayWatch.unobserve(row);
          }
        });
      }
      reactionOverlayWatch.observe(li);
    }
  }

  const rowMenus: RowMenus = createRowMenus();

  function discordNode(messages: Message[]): ChannelRow {
    const message = messages[0] as Message;
    const { li, meta, body, content, text, reactions } = channelRowFrame(messages);
    if ((message as { reply_to?: unknown }).reply_to && !deps.pinned() && !deps.main()) li.replaceChildren(deps.replyArrow(li), ...li.children);
    const fold = deps.foldable(li, meta, body, text, String(message.id), messages.slice(1).map((m) => String(m.id)));
    deps.tapRow(li, fold);
    if (fold) deps.sayRowShortcut(fold as Element, "fold");
    if (reactions) li.append(reactions);
    // `#1063 collapsed-card-reaction-overlay`: a folded card is laid out as if it had no
    // reactions, and a small strip goes over its elision line instead of taking a line of its
    // own. The fold's entry holds that line; with no line to share, the strip keeps its row.
    const elided = (li.foldEntry as { elided?: HTMLElement | null } | undefined)?.elided;
    if (fold && reactions && elided && reactionOverlayCandidate(reactions)) {
      overlayReactions(li, reactions, elided);
    }
    const reply = document.createElement("button");
    reply.className = "reply-button";
    reply.setAttribute("type", "button");
    reply.setAttribute("title", deps.replyButtonTitle);
    reply.textContent = "Reply";
    reply.addEventListener("click", deps.inThisColumn(() => deps.openReply(messages, li, deps.takeQuote(reply))));
    deps.sayRowShortcut(reply, "reply-button");
    const acts = document.createElement("span");
    acts.className = "row-acts";
    meta.append(acts);
    acts.append(deps.draftMarker(messages, li), reply);
    if (deps.replyingTo(messages[0] as Message)) deps.markReplying(li, true);
    li.replyButton = reply;
    deps.addThreadDecoration(acts, message, li, deps.pinned() || deps.main());
    const upstreamRead = document.createElement("button");
    upstreamRead.className = "upstream-read-button";
    upstreamRead.setAttribute("type", "button");
    const service = deps.providerName() || "the chat service";
    upstreamRead.setAttribute(
      "title",
      `Move ${service}'s own read marker for the whole space to this message, so ${service} shows ` +
        "it and everything before it as read. This app's Done is separate."
    );
    upstreamRead.textContent = `In ${service} only · whole space`;
    upstreamRead.setAttribute("aria-label", `Mark read in ${service} only · whole space`);
    upstreamRead.hidden = !deps.upstreamReadMark();
    const upstreamBoundary = messages[messages.length - 1] as Message;
    const more = document.createElement("span");
    more.className = "row-more";
    const moreButton = document.createElement("button");
    moreButton.className = "row-more-button";
    moreButton.setAttribute("type", "button");
    moreButton.setAttribute("aria-label", "More options for this message");
    moreButton.setAttribute("aria-expanded", "false");
    moreButton.setAttribute("title", "More options");
    moreButton.textContent = "⋯";
    const menu = document.createElement("span");
    menu.className = "row-more-menu";
    menu.hidden = true;
    // `#1228 menu-above-chips`. While open the menu renders in the top layer, above the floating
    // chips, the read strip, the freshness pill and the composer. Inside the list it could never
    // get there: the scroller is one stacking context (contain and mask in the stylesheet) and a
    // read row is another (opacity and filter), so a z-index on the menu competed only inside its
    // row. The top layer has no such parent. Where it is unavailable the menu stays in the list on
    // the overlay scale's menu layer (09-to-do-view.css) — still above the chips wherever the two
    // stacking contexts meet, and the closed menu's place either way.
    menu.setAttribute("popover", "manual");
    type TopLayerMenu = HTMLElement & {
      showPopover?: () => void;
      hidePopover?: () => void;
      matches?: (selector: string) => boolean;
    };
    const topMenu = menu as TopLayerMenu;
    const topLayerOpen = (): boolean => {
      try {
        return typeof topMenu.matches === "function" && topMenu.matches(":popover-open");
      } catch {
        return false;
      }
    };
    // Place the top-layer menu from its row: right-aligned to the row, under its meta line, flipped
    // above the line where below would leave the viewport. Viewport coordinates, because the top
    // layer is fixed; nothing is placed where the row has no layout (the page suite's fake DOM),
    // which reads only the hidden state.
    const placeTopMenu = (): void => {
      if (typeof window === "undefined" || typeof menu.getBoundingClientRect !== "function") return;
      const rowBox = li.getBoundingClientRect();
      const metaBox = meta.getBoundingClientRect();
      const menuBox = menu.getBoundingClientRect();
      if (!rowBox.width || !menuBox.height) return;
      const viewWidth = document.documentElement.clientWidth;
      const viewHeight = document.documentElement.clientHeight;
      const gap = 4;
      const edge = 8;
      const width = Math.min(menuBox.width || rowBox.width, Math.max(0, viewWidth - edge * 2));
      let left = rowBox.right - (width || menuBox.width);
      left = Math.min(Math.max(left, edge), Math.max(edge, viewWidth - (width || menuBox.width) - edge));
      let top = metaBox.bottom + gap;
      if (top + menuBox.height > viewHeight - edge && metaBox.top - menuBox.height - gap >= edge) {
        top = metaBox.top - menuBox.height - gap;
      }
      top = Math.min(Math.max(top, edge), Math.max(edge, viewHeight - menuBox.height - edge));
      menu.style.left = `${left}px`;
      menu.style.top = `${top}px`;
      menu.style.right = "auto";
    };
    // The top-layer menu stays where it opened while the list scrolls behind it, as a native
    // popover does; a scroll or a resize leaves it where the reader opened it, and the next
    // outside tap, Escape or Back closes it. (Following the row would move it between a tap's
    // measuring and its landing; closing on scroll would let the scroll that positioned the row
    // close the menu it opened.)
    let detachTopMenu: (() => void) | null = null;
    const watchTopMenu = (): void => {
      detachTopMenu = null;
    };
    const setMenu = (open: boolean): void => {
      if (open) {
        menu.hidden = false;
        if (typeof topMenu.showPopover === "function") {
          try {
            if (!topLayerOpen()) topMenu.showPopover();
            placeTopMenu();
            watchTopMenu();
          } catch {
            // The in-list fallback above: shown by hidden alone, on the menu layer.
          }
        }
      } else {
        detachTopMenu?.();
        if (typeof topMenu.hidePopover === "function" && topLayerOpen()) {
          try {
            topMenu.hidePopover();
          } catch {
            // Already down; hidden below is the state the page reads.
          }
        }
        menu.style.left = "";
        menu.style.top = "";
        menu.style.right = "";
        menu.hidden = true;
      }
      moreButton.setAttribute("aria-expanded", open ? "true" : "false");
      if (open) rowMenus.opened({ root: more, close: closeMenu, button: moreButton });
      else rowMenus.closed(closeMenu);
      deps.scheduleBackSync();
    };
    const closeMenu = (): void => setMenu(false);
    moreButton.addEventListener("click", deps.inThisColumn(() => setMenu(menu.hidden)));
    deps.sayRowShortcut(moreButton, "row-more-button");
    upstreamRead.addEventListener("click", deps.inThisColumn(() => {
      setMenu(false);
      deps.guardQuietly(() => deps.markReadUpstream(String(upstreamBoundary.id)))();
    }));
    const copy = document.createElement("button");
    copy.className = "row-copy-button";
    copy.setAttribute("type", "button");
    copy.setAttribute("title", "Copy this message's text to the clipboard");
    copy.textContent = "Copy text";
    deps.sayRowShortcut(copy, "row-copy-button");
    copy.addEventListener("click", deps.inThisColumn(() => {
      setMenu(false);
      deps.guardQuietly(() => copyMessageText(content))();
    }));
    const upstreamUrl = upstreamUrlOf(message as never);
    let openLink: HTMLAnchorElement | null = null;
    let copyLink: HTMLButtonElement | null = null;
    if (upstreamUrl !== null) {
      const linkService = upstreamServiceName(message);
      const appUrl = upstreamAppUrlOf(message as never);
      const opensInApp = appUrl !== null && storedSlackOpen() === "app";
      const opensAt = opensInApp ? (appUrl as string) : upstreamUrl;
      openLink = document.createElement("a");
      openLink.className = "row-open-link-button";
      openLink.setAttribute("href", opensAt);
      if (!opensInApp) {
        openLink.setAttribute("target", "_blank");
        openLink.setAttribute("rel", "noopener noreferrer");
      }
      openLink.setAttribute("title", `Open this message in ${linkService}: ${opensAt}`);
      openLink.textContent = `Open in ${linkService}`;
      deps.sayRowShortcut(openLink, "row-open-link-button");
      openLink.addEventListener("click", deps.inThisColumn(() => setMenu(false)));
      openLink.addEventListener("keydown", (event: KeyboardEvent) => {
        if (event.key === " " && !event.ctrlKey && !event.metaKey && !event.altKey) event.preventDefault();
      });
      openLink.addEventListener("keyup", deps.inThisColumn((event: KeyboardEvent) => {
        if (event.key !== " " || event.ctrlKey || event.metaKey || event.altKey) return;
        event.preventDefault();
        (openLink as HTMLAnchorElement).click();
      }));
      copyLink = document.createElement("button");
      copyLink.className = "row-copy-link-button";
      copyLink.setAttribute("type", "button");
      copyLink.setAttribute("title", `Copy this message's address in ${linkService} to the clipboard: ${upstreamUrl}`);
      copyLink.textContent = `Copy ${linkService} link`;
      deps.sayRowShortcut(copyLink, "row-copy-link-button");
      copyLink.addEventListener("click", deps.inThisColumn(() => {
        setMenu(false);
        deps.guardQuietly(() => copyUpstreamLink(upstreamUrl, linkService))();
      }));
    }
    const readThrough = document.createElement("button");
    readThrough.className = "row-read-through-button";
    readThrough.setAttribute("type", "button");
    readThrough.textContent = deps.upstreamReadMark() ? `Through here · also in ${service}` : "Through here";
    readThrough.setAttribute(
      "aria-label",
      deps.upstreamReadMark() ? `Mark read through here · also in ${service}` : "Mark read through here"
    );
    readThrough.setAttribute(
      "title",
      "Mark this message and every message above it in this list as Done here" +
        (deps.upstreamReadMark() ? `, and move ${service}'s own read marker for the whole space.` : ".")
    );
    deps.sayRowShortcut(readThrough, "row-read-through-button");
    readThrough.addEventListener("click", deps.inThisColumn(() => {
      setMenu(false);
      deps.guardQuietly(() => markReadThroughHere(li, String(upstreamBoundary.id)))();
    }));
    const pinItem = document.createElement("button");
    pinItem.className = "row-pin-button";
    pinItem.setAttribute("type", "button");
    pinItem.setAttribute(
      "title",
      "Pin this message on this server, for every device; find it again with the Pinned filter beside " +
        `the search glass. ${service}'s own pinned messages are not changed.`
    );
    pinItem.textContent = messages.some((m) => deps.isPinnedId(m.id)) ? "Unpin message" : "Pin message";
    deps.sayRowShortcut(pinItem, "row-pin-button");
    pinItem.addEventListener("click", deps.inThisColumn(() => {
      setMenu(false);
      deps.guardQuietly(() => deps.togglePin(messages))();
    }));
    const bookmarkItem = document.createElement("button");
    bookmarkItem.className = "row-bookmark-button";
    bookmarkItem.setAttribute("type", "button");
    bookmarkItem.setAttribute(
      "title",
      "Your place in this channel, kept on this server for every device: a line above this message. " +
        "Nothing is marked Done."
    );
    bookmarkItem.textContent = "Bookmark above";
    deps.sayRowShortcut(bookmarkItem, "row-bookmark-button");
    bookmarkItem.addEventListener("click", deps.inThisColumn(() => {
      setMenu(false);
      deps.toggleBookmarkAt(li);
    }));
    const readGroup = document.createElement("span");
    readGroup.className = "row-more-group";
    readGroup.setAttribute("role", "group");
    readGroup.setAttribute("aria-label", "Mark read");
    const readCaption = document.createElement("span");
    readCaption.className = "row-more-caption";
    readCaption.setAttribute("aria-hidden", "true");
    readCaption.textContent = "Mark read";
    readGroup.append(readCaption, readThrough, upstreamRead);
    const deepCopy = document.createElement("button");
    deepCopy.className = "row-copy-deep-link-button";
    deepCopy.setAttribute("type", "button");
    deepCopy.setAttribute("title", "Copy a link that opens this app on this message");
    deepCopy.textContent = "Copy vibe-talk link";
    deps.sayRowShortcut(deepCopy, "row-copy-deep-link-button");
    deepCopy.addEventListener("click", deps.inThisColumn(() => {
      setMenu(false);
      deps.guardQuietly(() => deps.copyDeepLink(message))();
    }));
    menu.append(copy, ...(openLink ? [openLink] : []), ...(copyLink ? [copyLink] : []), deepCopy, pinItem, bookmarkItem, readGroup);
    more.append(moreButton, menu);
    acts.append(more);
    const done = document.createElement("button");
    done.className = "done-button";
    done.setAttribute("type", "button");
    done.setAttribute("title", "Mark as dealt with here. This does not change the source chat service.");
    done.textContent = "Done";
    const ids = messages.map((m) => String(m.id));
    done.addEventListener("click", deps.inThisColumn(() => deps.guardQuietly(() => deps.toggleArchived(ids, li))()));
    deps.sayRowShortcut(done, "done-button");
    const rescue = document.createElement("button");
    rescue.className = "not-noise-button";
    rescue.setAttribute("type", "button");
    rescue.setAttribute("title", "This is a real message. Stop reading it automatically; the rule stays as it is.");
    rescue.textContent = "Not noise";
    rescue.hidden = !messages.every((m) => isNoise(m as never));
    rescue.addEventListener("click", deps.inThisColumn(() => deps.guardQuietly(() => deps.markNotNoise(ids))()));
    acts.append(rescue, done);
    deps.swipeable(li, messages);
    deps.searchable(li, message.author, text, ...deps.anchorsIn(body).map((link) => link.href));
    deps.drawLinkedOn(li, messages.map((one) => String(one.id)));
    return li;
  }

  function closeRowMenuOutside(event: { target?: unknown } | null): void {
    const menu = rowMenus.open() as { root: Node; close(): void } | null;
    if (menu && !nodeWithin(menu.root as never, event && event.target as never)) menu.close();
  }

  function closeRowMenuOnEscape(event: { key?: string } | null): boolean {
    if (!event || event.key !== "Escape") return false;
    return deps.closeOpenMenu("menu:row");
  }

  async function markReadThroughHere(row: HTMLElement, upstreamId: string): Promise<void> {
    const thread = row.getAttribute("data-unpack-thread");
    const main = row.hasAttribute("data-main-row");
    const rows = [...((row.parentNode || el("discord-log")) as HTMLElement).children]
      .filter((one) => (main ? one.hasAttribute("data-main-row") : thread === null || one.getAttribute("data-unpack-thread") === thread));
    const at = rows.indexOf(row);
    if (at < 0) return;
    const covered = rows.slice(0, at + 1).flatMap((one) => deps.idsOf(one));
    const ids = covered.filter((id) => !deps.archived().has(id));
    const channel = String(deps.selectedChannelId());
    const onTheirWay = [...deps.onTheirWay()];
    const waitingFor = [...new Set([...covered, ...onTheirWay])];
    if (ids.length) {
      deps.flashDone(row);
      try {
        await deps.dismissMessages({ messages: ids }, deps.throughBump(row));
      } catch (error) {
        deps.unflashDone(row);
        throw error;
      }
    }
    let upstream = false;
    const service = deps.providerName() || "the chat service";
    const count = `${ids.length} message${ids.length === 1 ? "" : "s"}`;
    if (deps.upstreamReadMark() && upstreamId) {
      if (!(await deps.doneSaved(waitingFor, () => String(deps.selectedChannelId()) === channel && deps.doneHeldHere(waitingFor)))) {
        if (String(deps.selectedChannelId()) === channel) {
          const why = "a Done it covers was refused or undone";
          deps.setStatus(ids.length > 0 && deps.doneHeldHere(ids) ? `Marked ${count} read here, not in ${service}: ${why}.`
            : `Not marked read in ${service}: ${why}.`);
        }
        return;
      }
      await deps.markReadUpstream(upstreamId);
      upstream = true;
    }
    deps.setStatus(upstream ? `Marked ${count} read here and in ${service}.` : `Marked ${count} read.`);
  }

  function storedSlackOpen(): string {
    return deps.storedSlackOpen();
  }

  function upstreamServiceName(message: Message | null): string {
    for (const id of [message && (message as { channel_id?: unknown }).channel_id, deps.currentChannelId()]) {
      if (!id) continue;
      const channel = deps.channels().find((candidate) => String(candidate.id) === String(id));
      const key = channel && typeof channel.provider === "string" ? channel.provider : null;
      if (key && deps.providers().has(key)) return deps.serviceLabel(key);
      const provider = deps.providerOfChannel(id);
      if (provider && typeof provider.name === "string" && (provider.name as string).trim()) return (provider.name as string).trim();
    }
    return deps.providers().size > 1 ? "the chat service" : deps.chatServiceName();
  }

  async function copyUpstreamLink(url: unknown, service: string): Promise<boolean> {
    const copied = await writeClipboardText(String(url || ""));
    deps.setStatus(copied ? `Copied the link in ${service}.` : deps.copyRefused);
    return copied;
  }

  async function copyMessageText(text: unknown): Promise<boolean> {
    const copied = await writeClipboardText(String(text || ""));
    deps.setStatus(copied ? "Copied the message text." : deps.copyRefused);
    return copied;
  }

  function writeClipboardText(value: string): Promise<boolean> {
    return copyText(value, () => deps.clipboard() as never) as Promise<boolean>;
  }

  function channelSummary(count: number, complete: unknown, label: string): string {
    const reach = deps.historyReach();
    if (complete === true && reach.start && reach.topLevel && !reach.whole) {
      const lacking = reach.threads - reach.full;
      return `${label} — back to the first message; ${lacking} thread${lacking === 1 ? " here has" : "s here have"} ` +
        "replies not loaded";
    }
    if (complete !== true || !reach.whole) {
      return `${label} — the most recent messages; older ones are not loaded`;
    }
    if (count === 0) {
      return `no messages in ${label}`;
    }
    return `${count} message${count === 1 ? "" : "s"} from ${label}`;
  }

  return {
    rowMenus, emptyMessage, mayLeadToReplies, hiddenEmpty, glom, regroupChannelRows,
    channelRowFrame, editedMark, reactionStrip, discordNode, closeRowMenuOutside,
    closeRowMenuOnEscape, markReadThroughHere, storedSlackOpen, upstreamServiceName,
    copyUpstreamLink, copyMessageText, writeClipboardText, channelSummary,
  };
}
