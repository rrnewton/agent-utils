// Dynamic toolbar promotion (`#1470 toolbar-dynamic-promotion`): no action is listed twice,
// and menu actions move up to the toolbar while measured room lasts.
//
// THE RULE, from the owner: an action on the toolbar is not in the hamburger menu, and an
// action in the menu is not on the toolbar. Discuss used to break it, drawn in both
// (`#discuss-open` on the bar, `#discuss-menu-open` in the menu). On a desk the bar has
// free room between Discuss and the rest of the bar; that room is spent, in a fixed
// priority order, on the menu's actions: Discuss first, then Fold read, then
// Expand all / Collapse all as one slot, then Summaries. As many as fit are promoted;
// the rest stay in the menu. A phone has the same rule and, having little room, keeps
// mostly the menu.
//
// FIT IS MEASURED, never guessed from the viewport: the pack's own width minus the
// members that are not candidates, re-measured whenever the bar's size changes (a
// ResizeObserver on the bar) and whenever the page redraws the bar (`renderControlBar`
// already runs for dock/undock, one/two columns and every view change). Hysteresis
// holds a boundary width steady: a promoted set is only demoted when it overflows the
// free space by more than the slack below, so a window resting on the boundary does
// not flicker items in and out as it is dragged.
//
// ONE IMPLEMENTATION PER ACTION. Fold read, Expand/Collapse all and Summaries are single
// elements that MOVE between the menu and the pack; they are never cloned, so their
// label, `aria-pressed` state and handler are the same element in either home. Discuss
// has an element in each place by design (a bar button and a menu item with different
// dress), so exactly one of its pair is shown: promoted shows the bar's, demoted shows
// the menu's. Labels and the pressed style are the read-toggle lane's (`#1399`): this
// module writes neither, it only chooses where each element lives.

/** Promotion candidates, highest priority first. Expand all / Collapse all share one slot: they act on the same folding, and promoting one without the other would strand the way back. */
export const PROMOTION_ORDER = Object.freeze(["discuss", "fold-read", "expand-collapse", "summaries"] as const);

/** What one candidate's promotion decision rests on: its measured width and whether it has work to do here. */
export interface PromotionCandidate {
  key: string;
  width: number;
  available: boolean;
}

/**
 * Slack, in pixels, between promoting and demoting: a promoted set that overflows the
 * free space by no more than this keeps its place (the pack scrolls sideways rather
 * than clipping, so a slight overflow costs nothing), while promotion itself needs a
 * true fit. A resize hovering at the boundary therefore settles instead of flickering.
 */
export const PROMOTION_HYSTERESIS_PX = 24;

/**
 * The promotion decision, pure: which candidates are on the toolbar.
 *
 * Candidates arrive in priority order with measured widths; unavailable ones (hidden
 * by the per-view renderers: no work to do on this view) are never promoted and drop
 * out of a promoted set they were in. Demotion runs lowest-priority first, and only
 * past the hysteresis slack; promotion then runs in priority order while each next
 * candidate truly fits what is left. Nothing here touches the DOM, which is what the
 * page unit tests feed measured widths into.
 */
export function decidePromotion(
  freeWidth: number,
  candidates: readonly PromotionCandidate[],
  current: ReadonlySet<string>,
): Set<string> {
  const usable = candidates.filter((candidate) => candidate.available && candidate.width > 0);
  const byKey = new Map(usable.map((candidate) => [candidate.key, candidate]));
  const promoted = new Set([...current].filter((key) => byKey.has(key)));
  const total = (): number =>
    usable.reduce((sum, candidate) => (promoted.has(candidate.key) ? sum + candidate.width : sum), 0);
  // Demote, lowest priority first, while the set overflows by more than the slack.
  // No free space at all demotes everything: a set kept on hysteresis alone would
  // have nowhere to be.
  for (let index = usable.length - 1; index >= 0; index -= 1) {
    if (promoted.size === 0) break;
    const over = total() - freeWidth;
    if (freeWidth > 0 && over <= PROMOTION_HYSTERESIS_PX) break;
    const lowest = usable[index];
    if (lowest) promoted.delete(lowest.key);
  }
  // Promote in priority order: a prefix of the order, so the menu always keeps a
  // contiguous tail and a wider candidate never lets a narrower one jump past it.
  for (const candidate of usable) {
    if (promoted.has(candidate.key)) continue;
    if (total() + candidate.width <= freeWidth) promoted.add(candidate.key);
    else break;
  }
  return promoted;
}

/** The single-element candidates: element ids in the menu, in menu order. Discuss is absent: it is a pair, handled apart. */
const MOVABLE: Readonly<Record<string, readonly string[]>> = Object.freeze({
  "fold-read": Object.freeze(["todo-filter"]),
  "expand-collapse": Object.freeze(["expand-all", "collapse-all"]),
  summaries: Object.freeze(["summarise"]),
});

/** What the promotion controller is handed by the control bar. */
export interface PromotionDependencies {
  el(id: string): HTMLElement;
  screen(): string;
  view(): string;
  /** The menu redraws (its empty line) after items move in or out. */
  renderActionMenu(): void;
}

/** The promotion controller, as the control bar calls it. */
export interface Promotion {
  /** Re-measure the free space and apply the decision. Safe to call often. */
  refresh(): void;
  /** The currently promoted candidate keys. */
  promoted(): ReadonlySet<string>;
  /** Stop watching the bar's size. */
  dispose(): void;
}

/** Make the promotion controller. It starts, as the page does today, with Discuss promoted. */
export function createPromotion(deps: PromotionDependencies): Promotion {
  let promoted: Set<string> = new Set(["discuss"]);
  // Where each movable element sat in the menu, so demotion puts it back in place
  // rather than appending it wherever the menu happens to end.
  const homeNext = new Map<string, HTMLElement | null>();
  for (const ids of Object.values(MOVABLE)) {
    for (const id of ids) {
      const element = deps.el(id);
      homeNext.set(id, (element.nextElementSibling as HTMLElement | null) ?? null);
    }
  }

  /**
   * An element's layout width. A candidate in the closed menu has none (the menu is
   * `display: none`), so it is measured by a synchronous probe: the element and the
   * menu are unhidden under `visibility: hidden` for one read, in the same task, so
   * nothing paints in between. Plain test doubles without layout report 0 and are
   * simply never promoted.
   */
  function measureWidth(element: HTMLElement): number {
    const width = element.offsetWidth;
    if (typeof width === "number" && width > 0) return width;
    const menu = deps.el("action-menu");
    const touched: Array<() => void> = [];
    const reveal = (node: HTMLElement): void => {
      const style = node.style as CSSStyleDeclaration | undefined;
      if (style) {
        const prior = style.visibility;
        style.visibility = "hidden";
        touched.push(() => { style.visibility = prior; });
      }
      if (node.hidden) {
        node.hidden = false;
        touched.push(() => { node.hidden = true; });
      }
    };
    try {
      reveal(element);
      if (element.offsetWidth === 0) reveal(menu);
      return typeof element.offsetWidth === "number" ? element.offsetWidth : 0;
    } catch {
      return 0;
    } finally {
      for (const undo of touched.reverse()) undo();
    }
  }

  /** Whether an element is offered here, as the existing renderers left it: `hidden` is theirs and promotion never writes it for a moved item. */
  function available(element: HTMLElement): boolean {
    return element.hidden !== true;
  }

  function candidates(): PromotionCandidate[] {
    const discussBar = deps.el("discuss-open");
    const list: PromotionCandidate[] = [{
      key: "discuss",
      width: measureWidth(discussBar),
      available: deps.screen() === "main" && deps.view() === "discord",
    }];
    for (const key of PROMOTION_ORDER.slice(1)) {
      const elements = (MOVABLE[key] ?? []).map((id) => deps.el(id));
      const offered = elements.filter((element) => available(element));
      list.push({
        key,
        width: offered.reduce((sum, element) => sum + measureWidth(element), 0),
        available: offered.length > 0,
      });
    }
    return list;
  }

  /** The pack's free width: its own width minus its visible non-candidate members and the gaps between members. */
  function freeWidth(): number {
    const pack = deps.el("bar-pack");
    const packWidth = typeof pack.clientWidth === "number" ? pack.clientWidth : 0;
    if (packWidth <= 0) return 0;
    const candidateIds = new Set(["discuss-open", ...Object.values(MOVABLE).flat()]);
    let used = 0;
    let visible = 0;
    for (const child of Array.from(pack.children as unknown as ArrayLike<HTMLElement>)) {
      if (!child || candidateIds.has(child.id) || child.hidden) continue;
      const width = typeof child.offsetWidth === "number" ? child.offsetWidth : 0;
      if (width > 0) { used += width; visible += 1; }
    }
    // One gap per visible member boundary, read from the stylesheet rather than assumed.
    let gap = 0;
    try {
      const parsed = Number.parseFloat(getComputedStyle(pack).columnGap || "");
      if (Number.isFinite(parsed)) gap = parsed;
    } catch { /* a host without computed styles pays no gap */ }
    return Math.max(0, packWidth - used - gap * Math.max(0, visible - 1));
  }

  function apply(next: Set<string>): void {
    const changed = next.size !== promoted.size || [...next].some((key) => !promoted.has(key));
    promoted = next;
    const pack = deps.el("bar-pack");
    const menu = deps.el("action-menu");
    const canMove = typeof pack.appendChild === "function" && typeof menu.appendChild === "function";
    // Discuss: exactly one of the pair, always. Which one is the whole decision.
    const discussAvailable = deps.screen() === "main" && deps.view() === "discord";
    deps.el("discuss-open").hidden = !(discussAvailable && promoted.has("discuss"));
    deps.el("discuss-menu-open").hidden = !(discussAvailable && !promoted.has("discuss"));
    if (canMove) {
      for (const key of PROMOTION_ORDER.slice(1)) {
        for (const id of MOVABLE[key] ?? []) {
          const element = deps.el(id);
          if (promoted.has(key)) {
            if (element.parentNode !== pack) pack.appendChild(element);
            element.classList?.add("promoted-action");
          } else {
            if (element.parentNode !== menu) {
              const anchor = homeNext.get(id);
              if (anchor && anchor.parentNode === menu) menu.insertBefore(element, anchor);
              else menu.appendChild(element);
            }
            element.classList?.remove("promoted-action");
          }
        }
      }
    }
    if (changed) deps.renderActionMenu();
  }

  function refresh(): void {
    apply(decidePromotion(freeWidth(), candidates(), promoted));
  }

  // Re-measure when the bar itself changes size: a resize, a dock/undock, a column
  // change — anything that gives the pack more or less room. Layout changes that keep
  // the bar's size (a view change, a member appearing) arrive through
  // `renderControlBar`, which calls `refresh` itself.
  let observer: ResizeObserver | null = null;
  if (typeof ResizeObserver === "function") {
    try {
      observer = new ResizeObserver(() => refresh());
      observer.observe(deps.el("control-bar"));
      observer.observe(deps.el("bar-pack"));
    } catch {
      observer = null;
    }
  }

  return {
    refresh,
    promoted: () => new Set(promoted),
    dispose: () => observer?.disconnect(),
  };
}
