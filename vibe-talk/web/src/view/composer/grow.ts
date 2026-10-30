// The message boxes that grow with what is typed (`#340 composer-grow-box`; `#624
// voice-page-modularity`, moved from the legacy script; plan step 16, view/composer). Why a box grows,
// when, and what is kept in view as it does, is told where the legacy script describes "the box that
// grows". What the page knows beyond the boxes (where each list is, keeping a list at its foot, the
// device's settings) is passed in.

/**
 * The boxes that grow, and their Grow checkboxes in the same order. All left-column ids. The reply's
 * too, which is one of the composers in the stream (`#410 inline-reply-editor`).
 */
const COMPOSER_GROW_BOXES = ["compose-text", "channel-compose-text", "side-compose-text", "reply-text"];
const COMPOSER_GROW_IDS = ["compose-grow", "channel-compose-grow", "side-compose-grow", "reply-grow"];

/** The height the stylesheet gives a growing box where the browser cannot size it (`fit`). */
const COMPOSER_GROW_HEIGHT = "--composer-grow-height";

/** What the owner asks of the page. */
export interface ComposerGrowDependencies {
  /** The element with this id in the left column (`leftEl`). */
  lookup(id: string): HTMLElement;
  /** The list `field` is in (`growingBoxList`). */
  listOf(field: HTMLElement): HTMLElement;
  /** Whether `area` is at its foot, within `slack` pixels (`atBottom`). */
  atBottom(area: HTMLElement, slack?: number): boolean;
  /** Put `area` back at its foot (`followFoot`). */
  followFoot(area: HTMLElement): void;
  /** Bring `box` into `area` whole (`keepBoxInView`). */
  keepBoxInView(box: HTMLElement, area: HTMLElement): void;
  /** The element with the focus. */
  activeElement(): Element | null;
  /** How far above its foot a list still counts as at its foot (`BOTTOM_SLACK_PX`). */
  readonly bottomSlack: number;
  /** Whether the browser sizes a field to its content itself (`field-sizing: content`). */
  readonly fieldSizing: boolean;
  /** Keep the reader's choice on the device; may throw where storage is refused. */
  keep(on: boolean): void;
}

/** The owner's read surface and named commands. */
export interface ComposerGrow {
  /** Whether the boxes grow with their text: off unless the reader ticked Grow. */
  on(): boolean;
  /** Grow ticked or unticked by the reader, on the checkbox `grow`: kept, and drawn. */
  apply(on: boolean, grow: HTMLElement): void;
  /** Draw the choice on #screen-main and every checkbox. */
  render(on: unknown): void;
  /** Whether the composer `field` is in draws its Grow checkbox: whether the box grows there at all. */
  shown(field: HTMLElement): boolean;
  /** Where the browser cannot size a box to its text, size it here. */
  fit(field: HTMLElement): void;
  /** `fit` for each of the boxes. */
  fitAll(): void;
  /** A growing box changed size: with the reader typing in it, its whole composer brought into its list. */
  keepInView(field: HTMLElement): void;
}

/** Grow off; nothing measured yet. */
export function createComposerGrow(deps: ComposerGrowDependencies): ComposerGrow {
  /** Whether the boxes grow with their text: off unless the reader ticked Grow. */
  let composerGrow = false;
  /** Each growing box's height at its last report, by id, for `keepInView`. */
  const growingHeights = new Map<string, number>();

  /**
   * The Grow checkbox `grow`, just ticked, if any of it is on screen in its list: with that list, and
   * where in the window it is.
   */
  const growInView = (grow: HTMLElement): { grow: HTMLElement; area: HTMLElement; top: number } | null => {
    const at = COMPOSER_GROW_IDS.indexOf(grow.id);
    if (at < 0) return null;
    const area = deps.listOf(deps.lookup(COMPOSER_GROW_BOXES[at] as string));
    const rect = grow.getBoundingClientRect();
    const top = area.getBoundingClientRect().top;
    if (rect.bottom <= top || rect.top >= top + (area.clientHeight || 0)) return null;
    return { grow, area, top: rect.top };
  };

  const owner: ComposerGrow = {
    on: () => composerGrow,
    apply(on, grow) {
      try {
        deps.keep(on);
      } catch (_error) {
        // A browser that refuses storage still honours the choice for this session.
      }
      const footed = ["scroll-area", "side-scroll"].map((id) => deps.lookup(id)).filter((area) => deps.atBottom(area));
      const ticked = growInView(grow);
      owner.render(on);
      for (const area of footed) deps.followFoot(area);
      if (ticked && !footed.includes(ticked.area)) {
        ticked.area.scrollTop += ticked.grow.getBoundingClientRect().top - ticked.top;
      }
    },
    render(on) {
      composerGrow = Boolean(on);
      const screen = deps.lookup("screen-main");
      if (composerGrow) screen.setAttribute("data-composer-grow", "");
      else screen.removeAttribute("data-composer-grow");
      for (const id of COMPOSER_GROW_IDS) (deps.lookup(id) as HTMLInputElement).checked = composerGrow;
      if (composerGrow) for (const id of COMPOSER_GROW_BOXES) deps.lookup(id).style.removeProperty("height");
      owner.fitAll();
    },
    shown(field) {
      const box = deps.lookup(COMPOSER_GROW_IDS[COMPOSER_GROW_BOXES.indexOf(field.id)] || "");
      return Boolean(box) && typeof box.getClientRects === "function" && box.getClientRects().length > 0;
    },
    fit(field) {
      if (deps.fieldSizing) return;
      if (!composerGrow) {
        field.style.removeProperty(COMPOSER_GROW_HEIGHT);
        return;
      }
      if (!owner.shown(field)) return;
      const area = deps.listOf(field);
      const place = area.scrollTop;
      field.style.setProperty(COMPOSER_GROW_HEIGHT, "auto");
      const style = typeof window.getComputedStyle === "function" ? window.getComputedStyle(field) : null;
      const borders = style ? (parseFloat(style.borderTopWidth) || 0) + (parseFloat(style.borderBottomWidth) || 0) : 0;
      field.style.setProperty(COMPOSER_GROW_HEIGHT, `${Math.ceil((Number(field.scrollHeight) || 0) + borders)}px`);
      area.scrollTop = place;
    },
    fitAll() {
      for (const id of COMPOSER_GROW_BOXES) owner.fit(deps.lookup(id));
    },
    keepInView(field) {
      const height = field.getBoundingClientRect().height;
      const seen = growingHeights.get(field.id);
      growingHeights.set(field.id, height);
      if (!composerGrow || deps.activeElement() !== field || !owner.shown(field)) return;
      const area = deps.listOf(field);
      const grew = seen === undefined ? 0 : height - seen;
      if (grew > 0 && deps.atBottom(area, deps.bottomSlack + grew)) {
        deps.followFoot(area);
        return;
      }
      const composer = ({ "compose-text": "voice-composer", "channel-compose-text": "channel-composer",
        "reply-text": "reply-compose" } as { readonly [id: string]: string | undefined })[field.id] || "side-composer";
      deps.keepBoxInView(deps.lookup(composer), area);
    },
  };
  return owner;
}

export { COMPOSER_GROW_BOXES, COMPOSER_GROW_IDS };
