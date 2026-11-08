// The links in what is on screen, whose messages, and the kinds of link (`#624
// voice-page-modularity`, moved from the legacy script into view/channel): a row's links shown
// in place of its text under Links, the author filter's choice and sentences, and the kind
// buttons under Links. Why each behaves as it does is told where the legacy script held them.
// The kinds kept on the device stay present/rows' owner, instantiated by the page and handed in;
// `authorChoosing` and the kinds' `hidden` keep their names for their other consumers.

import type { Message } from "../../contract/index.ts";
import { linkKind, linksIn } from "../../foundation/index.ts";
import {
  AUTHOR_FILTERS, LINK_KINDS, authorEmptyWords, hiddenKindWords, linkKindCountText, nextAuthorFilter,
} from "../../present/rows/index.ts";
import { createAuthorFilter } from "./authorfilter.ts";
import type { AuthorFilter } from "./authorfilter.ts";

/** One link a row holds, with the kind its address is. */
interface RowLink {
  href: string;
  name: string;
  kind: string;
}

/** A row carrying its drawn links. */
type LinkRow = HTMLElement & {
  messages?: Message[];
  linkText?: string;
  linkView?: {
    key: string; links: RowLink[]; hidden: ReadonlySet<string> | null; raw: boolean | null;
    count: number; node: HTMLElement | null;
  } | null;
};

/** The kinds turned off and whether links are written as addresses (present/rows owns them). */
export interface LinkKindsLike {
  hidden(): ReadonlySet<string>;
  raw(): boolean;
  setShown(kind: string, shown: boolean): void;
  setRaw(on: boolean): void;
}

/** What the row links ask of the page. */
export interface RowLinksDependencies {
  readonly linkKinds: LinkKindsLike;
  byId(id: string): HTMLElement;
  messageLinks(message: Message): Array<{ href: string; name: string }>;
  renderedHtmlOf(message: Message): string;
  rawTextOf(message: Message): string;
  searchOpen(): boolean;
  linksOn(): boolean;
  setLinks(on: boolean): void;
  view(): string;
  currentChannelId(): string | null;
  selectionView(): string;
  selectionThreadId(): string;
  keepingFilterPlace<T>(fn: () => T): T;
  liftingHideRead<T>(fn: () => T): T;
  renderScrollTools(): void;
  inOtherColumn(fn: () => void): void;
  requestVisibleSummaries(): void;
  hidingRead(): boolean;
  ownReadOn(): boolean;
}

/** The row links' commands and questions. */
export interface RowLinks {
  readonly authorFilters: AuthorFilter;
  showRowLinks(row: Element): number;
  takeDownRowLinks(row: Element): number;
  heldLinks(row: Element): RowLink[];
  showingLinks(row: Element): boolean;
  filterPlace(): string;
  setLinksOnly(on: boolean): void;
  authorChoosing(): string;
  setAuthorFilter(mode: string): void;
  cycleAuthorFilter(): void;
  authorEmptySentence(author: string, searching: boolean, pinned: boolean): string;
  setLinkKindShown(kind: string, shown: boolean): void;
  setLinksRaw(on: boolean): void;
  renderLinkKinds(): void;
  renderLinkKindCounts(kinds: Map<string, number>): void;
  hiddenKindNames(counts: Map<string, number>): string;
}

/** No Links view drawn, everyone chosen. */
export function createRowLinks(deps: RowLinksDependencies): RowLinks {
  const el = deps.byId;
  const linkKinds = deps.linkKinds;

  function rowLinkParts(row: LinkRow): Array<Array<{ href: string; name: string }>> {
    if (row.messages) return row.messages.map((m) => deps.messageLinks(m));
    return typeof row.linkText === "string" ? [linksIn(row.linkText) as Array<{ href: string; name: string }>] : [];
  }

  function rowLinkKey(row: LinkRow): string {
    if (row.messages) return row.messages.map((m) => deps.renderedHtmlOf(m) || deps.rawTextOf(m)).join("\u0000");
    return typeof row.linkText === "string" ? row.linkText : "";
  }

  function rowLinks(row: LinkRow): RowLink[] {
    const byAddress = new Map<string, RowLink>();
    for (const link of rowLinkParts(row).flat()) {
      const held = byAddress.get(link.href);
      if (!held) {
        byAddress.set(link.href, { ...link, kind: linkKind(link.href) as string });
      } else if (held.name === held.href) {
        held.name = link.name;
      }
    }
    return [...byAddress.values()];
  }

  function linkList(links: RowLink[], raw: boolean): HTMLElement {
    const list = document.createElement("div");
    list.className = "row-links";
    if (raw) list.setAttribute("data-raw", "");
    for (const link of links) {
      const anchor = document.createElement("a");
      anchor.className = "row-link";
      if (raw) rawAddressInto(anchor, link.href);
      else anchor.textContent = link.name;
      anchor.setAttribute("href", link.href);
      anchor.setAttribute("target", "_blank");
      anchor.setAttribute("rel", "noopener noreferrer");
      anchor.setAttribute("title", link.href);
      list.append(anchor);
    }
    return list;
  }

  function rawAddressInto(anchor: HTMLElement, href: string): void {
    for (const part of (href.match(/[^/?&#=]*[/?&#=]?/g) || []).filter(Boolean)) {
      if (anchor.firstChild) anchor.append(document.createElement("wbr"));
      anchor.append(document.createTextNode(part));
    }
  }

  function placeAfterBody(row: LinkRow, node: Node): void {
    const kids = [...row.children];
    const body = kids.find((kid) =>
      String((kid as HTMLElement).className || "").split(/\s+/).some((word) => word === "body" || word === "msg-text"));
    if (!body) {
      row.append(node);
      return;
    }
    row.replaceChildren(...kids.flatMap((kid) => (kid === body ? [kid, node] : [kid])));
  }

  function showRowLinks(row: Element): number {
    const linkRow = row as LinkRow;
    const key = rowLinkKey(linkRow);
    let drawn = linkRow.linkView;
    if (!drawn || drawn.key !== key) {
      if (drawn && drawn.node && drawn.node.parentNode === linkRow) linkRow.removeChild(drawn.node);
      drawn = { key, links: rowLinks(linkRow), hidden: null, raw: null, count: 0, node: null };
      linkRow.linkView = drawn;
    }
    if (drawn.hidden !== linkKinds.hidden() || drawn.raw !== linkKinds.raw()) {
      if (drawn.node && drawn.node.parentNode === linkRow) linkRow.removeChild(drawn.node);
      const shown = drawn.links.filter((link) => !linkKinds.hidden().has(link.kind));
      drawn.hidden = linkKinds.hidden();
      drawn.raw = linkKinds.raw();
      drawn.count = shown.length;
      drawn.node = shown.length > 0 ? linkList(shown, linkKinds.raw()) : null;
    }
    if (drawn.node && drawn.node.parentNode !== linkRow) placeAfterBody(linkRow, drawn.node);
    if (drawn.count > 0) linkRow.setAttribute("data-links-view", "true");
    else linkRow.removeAttribute("data-links-view");
    return drawn.count;
  }

  function takeDownRowLinks(row: Element): number {
    const linkRow = row as LinkRow;
    if (linkRow.linkView) {
      if (linkRow.linkView.node && linkRow.linkView.node.parentNode === linkRow) linkRow.removeChild(linkRow.linkView.node);
      linkRow.linkView = null;
    }
    if (linkRow.hasAttribute("data-links-view")) linkRow.removeAttribute("data-links-view");
    return 0;
  }

  const showingLinks = (row: Element): boolean => row.getAttribute("data-links-view") === "true";
  const heldLinks = (row: Element): RowLink[] => ((row as LinkRow).linkView ? (row as LinkRow).linkView?.links as RowLink[] : []);

  const filterPlace = (): string =>
    [deps.view(), deps.currentChannelId(), deps.selectionView(), deps.selectionThreadId() || ""].join("\u0000");

  function setLinksOnly(on: boolean): void {
    if (on === deps.linksOn()) return;
    deps.keepingFilterPlace(() => deps.liftingHideRead(() => {
      deps.setLinks(on);
      el("links-filter").setAttribute("aria-pressed", on ? "true" : "false");
      renderLinkKinds();
      deps.renderScrollTools();
      deps.inOtherColumn(deps.renderScrollTools);
    }));
    deps.requestVisibleSummaries();
  }

  const authorFilters: AuthorFilter = createAuthorFilter({
    lookup: (id: string) => el(id),
    searchOpen: () => deps.searchOpen(),
  } as never);

  const authorChoosing = (): string => authorFilters.inForce() as string;

  function setAuthorFilter(mode: string): void {
    const next = (AUTHOR_FILTERS as readonly string[]).includes(mode) ? mode : "everyone";
    if (next === authorFilters.chosen()) return;
    deps.keepingFilterPlace(() => deps.liftingHideRead(() => {
      authorFilters.set(next as never);
      deps.renderScrollTools();
      deps.inOtherColumn(deps.renderScrollTools);
    }));
    deps.requestVisibleSummaries();
  }

  function cycleAuthorFilter(): void {
    setAuthorFilter(nextAuthorFilter(authorFilters.chosen() as never) as string);
    authorFilters.showChip();
  }

  function authorEmptySentence(author: string, searching: boolean, pinned: boolean): string {
    return authorEmptyWords(author as never, searching, pinned, () => deps.view() === "discord" && deps.hidingRead() && deps.ownReadOn()) as string;
  }

  function setLinkKindShown(kind: string, shown: boolean): void {
    deps.keepingFilterPlace(() => {
      linkKinds.setShown(kind, shown);
      renderLinkKinds();
      deps.renderScrollTools();
      deps.inOtherColumn(deps.renderScrollTools);
    });
  }

  function setLinksRaw(on: boolean): void {
    deps.keepingFilterPlace(() => {
      linkKinds.setRaw(on);
      renderLinkKinds();
      deps.renderScrollTools();
      deps.inOtherColumn(deps.renderScrollTools);
    });
  }

  function renderLinkKinds(): void {
    const up = deps.linksOn() && !el("links-filter").hidden;
    el("link-kinds").hidden = !up;
    if (up) el("screen-main").setAttribute("data-link-kinds", "");
    else el("screen-main").removeAttribute("data-link-kinds");
    for (const { kind, chip } of LINK_KINDS) {
      el(chip).setAttribute("aria-pressed", linkKinds.hidden().has(kind) ? "false" : "true");
    }
    el("link-raw").setAttribute("aria-pressed", linkKinds.raw() ? "true" : "false");
  }

  function renderLinkKindCounts(kinds: Map<string, number>): void {
    for (const { kind, chip } of LINK_KINDS) {
      el(`${chip}-count`).textContent = linkKindCountText(kinds.get(kind) || 0) as string;
    }
  }

  function hiddenKindNames(counts: Map<string, number>): string {
    return hiddenKindWords(linkKinds.hidden(), counts as never) as string;
  }

  return {
    authorFilters, showRowLinks, takeDownRowLinks, heldLinks, showingLinks, filterPlace,
    setLinksOnly, authorChoosing, setAuthorFilter, cycleAuthorFilter, authorEmptySentence,
    setLinkKindShown, setLinksRaw, renderLinkKinds, renderLinkKindCounts, hiddenKindNames,
  };
}
