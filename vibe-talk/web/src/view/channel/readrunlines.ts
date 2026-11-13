// The read modes' lines and places (`#221 read-modes`, `#432 read-mode-unify`; `#624
// voice-page-modularity`, moved from the legacy script into view/channel): whether Hide read
// is leaving read messages out, the collapsed run's line and where each row stands in a run,
// and the rows a redraw replaced. The runs themselves are readcore's; the modes are
// present/rows'.

import { READ_RUN_PEEL_MIN, READ_RUN_PEEL_SIZE, readRunText } from "../../present/rows/index.ts";
import { idsOf, rowMessages } from "./rows.ts";

/** Which edge of a folded run a peel reveals: its oldest (`first`) or newest (`last`) messages. */
export type ReadRunPeelEdge = "first" | "last";

/** A row carrying its read-run line and days. */
type RunRow = HTMLElement & {
  readRunLine?: HTMLButtonElement | null;
  readRunPeels?: HTMLElement | null;
  readRunPeelFirst?: HTMLButtonElement | null;
  readRunPeelLast?: HTMLButtonElement | null;
  runDays?: string;
};

/** What the read-run lines ask of the page. */
export interface ReadRunLinesDependencies {
  byId(id: string): HTMLElement;
  leftById(id: string): HTMLElement;
  inThisColumn<A extends unknown[]>(fn: (...args: A) => void): (...args: A) => void;
  selection(): { view: string; threadId: unknown };
  threads(): Array<{ id: unknown; root?: { id: unknown } | null }>;
  hiding(): boolean;
  suspended(): boolean;
  threading(): boolean;
  eachColumnRenderCached(): void;
  threadIdOf(message: unknown): string | null;
  selectionThread(): { root?: { id: unknown } | null } | null;
  openReadRun(row: Element): void;
  peelReadRun(row: Element, edge: ReadRunPeelEdge): void;
  readRunRows(row: Element): Element[];
}

/** The read-run lines' commands and questions. */
export interface ReadRunLines {
  hidingRead(): boolean;
  liftingHideRead<T>(change: () => T): T;
  readRunLine(row: Element): HTMLButtonElement;
  readRunWords(row: Element): string;
  runDaysOf(row: Element): string;
  readRunCount(row: Element): number;
  placeInReadRun(row: Element, role: string | null, count: number): void;
  /** Show or hide `row`'s peel buttons for a still-folded count of `count` (the cards' own wording). */
  placeReadRunPeel(row: Element, count: number): void;
  isOpenThreadRoot(row: Element, rootId: string): boolean;
  openThreadRootId(): string | null;
  foldedOneIds(key: string): Set<string>;
  readonly foldedOnes: Map<string, Set<string>>;
  liveUnpackedRow(row: Element): HTMLElement | null;
  liveCardRow(row: Element): HTMLElement | null;
}

/** Nothing folded away by hand. */
export function createReadRunLines(deps: ReadRunLinesDependencies): ReadRunLines {
  const leftEl = deps.leftById;

  const foldedOnes = new Map<string, Set<string>>();

  function foldedOneIds(key: string): Set<string> {
    return foldedOnes.get(key) || new Set<string>();
  }

  function isOpenThreadRoot(row: Element, rootId: string): boolean {
    if (deps.selection().view !== "thread" || !deps.selection().threadId) return false;
    const thread = String(deps.selection().threadId);
    return rowMessages(row).some((message) => {
      const held = message as { thread?: { is_root?: boolean } };
      return String(message.id) === rootId || String(message.id) === thread ||
        (deps.threadIdOf(message) === thread && held.thread?.is_root === true);
    });
  }

  function openThreadRootId(): string | null {
    if (deps.selection().view !== "thread" || !deps.selection().threadId) return null;
    const summary = deps.threads().find((held) => String(held.id) === String(deps.selection().threadId)) || deps.selectionThread();
    return summary && summary.root ? String(summary.root.id) : null;
  }

  const hidingRead = (): boolean => deps.hiding() && !(deps.threading() && deps.suspended());

  function liftingHideRead<T>(change: () => T): T {
    const was = hidingRead();
    const result = change();
    if (hidingRead() !== was) deps.eachColumnRenderCached();
    return result;
  }

  function readRunLine(row: Element): HTMLButtonElement {
    const held = row as RunRow;
    if (held.readRunLine) return held.readRunLine;
    const line = document.createElement("button");
    line.className = "read-run";
    line.setAttribute("type", "button");
    line.addEventListener("click", deps.inThisColumn((event: Event) => {
      if (event) event.stopPropagation();
      const focused = document.activeElement === line && line.matches(":focus-visible");
      deps.openReadRun(row);
      const now = (row as Element).parentNode ? row as HTMLElement : liveUnpackedRow(row) || liveCardRow(row);
      if (focused && now) {
        now.setAttribute("tabindex", "-1");
        now.focus({ preventScroll: true });
      }
    }));
    row.replaceChildren(line, ...row.children);
    held.readRunLine = line;
    ensurePeels(row as HTMLElement);
    return line;
  }

  function peelButton(row: HTMLElement, edge: ReadRunPeelEdge): HTMLButtonElement {
    const button = document.createElement("button");
    button.className = "read-run-peel";
    button.setAttribute("type", "button");
    button.setAttribute("data-peel", edge);
    button.textContent = `(${edge} ${READ_RUN_PEEL_SIZE})`;
    button.setAttribute("aria-label", edge === "first"
      ? `Show first ${READ_RUN_PEEL_SIZE} messages of this folded run`
      : `Show last ${READ_RUN_PEEL_SIZE} messages of this folded run`);
    button.setAttribute("title", edge === "first"
      ? `Show the first ${READ_RUN_PEEL_SIZE} of these read messages`
      : `Show the last ${READ_RUN_PEEL_SIZE} of these read messages`);
    button.addEventListener("click", deps.inThisColumn((event: Event) => {
      if (event) event.stopPropagation();
      const focused = document.activeElement === button && button.matches(":focus-visible");
      deps.peelReadRun(row, edge);
      if (!focused) return;
      const anchor = (row as Element).parentNode ? row as HTMLElement : liveUnpackedRow(row) || liveCardRow(row);
      const list = anchor && anchor.parentNode ? [...anchor.parentNode.children] as HTMLElement[] : [];
      const head = list.find((one) => one.getAttribute("data-read-run") === "head") as RunRow | undefined;
      const peelsShown = head && head.readRunPeels && !head.readRunPeels.hidden;
      const next = head ? (peelsShown ? (edge === "first" ? head.readRunPeelFirst : head.readRunPeelLast) : head.readRunLine) : null;
      const target = next && !next.hidden ? next : head || anchor;
      if (target) {
        target.setAttribute("tabindex", "-1");
        target.focus({ preventScroll: true });
      }
    }));
    return button;
  }

  function ensurePeels(row: HTMLElement): HTMLElement {
    const held = row as RunRow;
    if (held.readRunPeels) return held.readRunPeels;
    const box = document.createElement("span");
    box.className = "read-run-peels";
    box.hidden = true;
    const first = peelButton(row, "first");
    const last = peelButton(row, "last");
    box.append(first, last);
    held.readRunPeels = box;
    held.readRunPeelFirst = first;
    held.readRunPeelLast = last;
    row.append(box);
    return box;
  }

  function placeReadRunPeel(row: Element, count: number): void {
    const held = row as RunRow;
    const show = count >= READ_RUN_PEEL_MIN && row.getAttribute("data-read-run") === "head";
    if (!show) {
      if (held.readRunPeels && !held.readRunPeels.hidden) held.readRunPeels.hidden = true;
      return;
    }
    const box = ensurePeels(row as HTMLElement);
    if (box.hidden) box.hidden = false;
  }

  function readRunWords(row: Element): string {
    const held = row as RunRow;
    return held.readRunLine ? String(held.readRunLine.textContent) : "";
  }

  const runDaysOf = (row: Element): string => String((row as RunRow).runDays || "");

  const readRunCount = (row: Element): number =>
    deps.readRunRows(row).reduce((sum, one) => sum + idsOf(one).length, 0);

  function placeInReadRun(row: Element, role: string | null, count: number): void {
    const held = row as RunRow;
    if (role === null) {
      if (row.hasAttribute("data-read-run")) row.removeAttribute("data-read-run");
      if (held.hidden) held.hidden = false;
      if (held.readRunLine && !held.readRunLine.hidden) held.readRunLine.hidden = true;
      if (held.readRunPeels && !held.readRunPeels.hidden) held.readRunPeels.hidden = true;
      return;
    }
    row.setAttribute("data-read-run", role);
    held.hidden = role === "member";
    if (role !== "head") {
      if (held.readRunPeels && !held.readRunPeels.hidden) held.readRunPeels.hidden = true;
      return;
    }
    const line = readRunLine(row);
    const words = readRunText(count, runDaysOf(row)) as string;
    if (line.textContent !== words) line.textContent = words;
    line.setAttribute("title", `Show ${count === 1 ? "this read message" : `these ${count} read messages`}`);
    line.hidden = false;
    placeReadRunPeel(row, count);
  }

  function liveCardRow(row: Element): HTMLElement | null {
    const card = row.hasAttribute("data-card");
    if (!card && !row.hasAttribute("data-main-row")) return null;
    const id = row.getAttribute("data-id");
    return ([...leftEl("side-cards").children] as HTMLElement[]).find((one) =>
      one.getAttribute("data-id") === id && one.hasAttribute(card ? "data-card" : "data-main-row")) || null;
  }

  function liveUnpackedRow(row: Element): HTMLElement | null {
    const thread = row.getAttribute("data-unpack-thread");
    const [first] = idsOf(row);
    if (thread === null || !first) return null;
    return ([...leftEl("side-cards").children] as HTMLElement[]).find((one) =>
      one.getAttribute("data-unpack-thread") === thread && idsOf(one).includes(first)) || null;
  }

  return {
    hidingRead, liftingHideRead, readRunLine, readRunWords, runDaysOf, readRunCount,
    placeInReadRun, placeReadRunPeel, isOpenThreadRoot, openThreadRootId, foldedOneIds, foldedOnes, liveUnpackedRow, liveCardRow,
  };
}
