// Where the control bar sits (`#58 control-bar`; `#624 voice-page-modularity`, moved from the legacy
// script; plan step 16, view/settings).
//
// ONE bar, TWO possible parents. `#control-bar-top` sits in the header and `#control-bar-bottom` sits
// in the dock directly above the big buttons; `set` moves the single element between them. Two copies
// would be two sets of ids, and the page would then have to keep two gears and two switches agreeing
// about which screen is up. The bar is declared in the BOTTOM mount in web/voice.html rather than being
// created here, so the default placement is a fact about the served markup: a page whose script dies
// still shows the controls where they belong. What the bar shows is the page's (`renderControlBar`).

import type { StoragePort } from "../../ports/index.ts";
import { DEFAULT_PLACEMENT, PLACEMENTS, placementOf, placementWords } from "../../present/settings/index.ts";

/** Where this device keeps the reader's choice. */
const BAR_PLACEMENT_KEY = "vibe-talk.voice.bar-placement";

/** What the bar's placement asks of the page. */
export interface BarPlacementDependencies {
  /** The element with this id (`el`). */
  lookup(id: string): HTMLElement;
  /** The device's storage (`deviceStorage`); reading or writing it may throw. */
  storage(): StoragePort;
  /** Redraw the bar, which decides what it shows from where it is (`renderControlBar`). */
  renderControlBar(): void;
}

/** The bar's placement: where it is, where it was kept, and the moves. */
export interface BarPlacement {
  /** Where the bar is now. */
  where(): string;
  /** Where the reader last put the bar, validated. */
  stored(): string;
  /** Move the bar. The re-parent is the whole mechanism; everything else follows from it. */
  set(where: unknown): void;
  /** Settings' picker changed: kept, moved, and said. */
  changed(): void;
}

/** The bar at its default place. Made where the page kept the placement. */
export function mountBarPlacement(deps: BarPlacementDependencies): BarPlacement {
  let placement = DEFAULT_PLACEMENT;

  function persist(where: string): boolean {
    try {
      deps.storage().setItem(BAR_PLACEMENT_KEY, where);
    } catch (_error) {
      return false;
    }
    // Read back rather than assume, exactly as the microphone settings do: private browsing accepts
    // setItem and stores nothing, so "Saved" would be a lie discovered only after a reload.
    return deps.storage().getItem(BAR_PLACEMENT_KEY) === where;
  }

  function set(where: unknown): void {
    placement = placementOf(where);
    deps.lookup("control-bar").setAttribute("data-placement", placement);
    deps.lookup(placement === "top" ? "control-bar-top" : "control-bar-bottom").append(deps.lookup("control-bar"));
    deps.renderControlBar();
  }

  return {
    where: () => placement,
    stored() {
      let stored: string | null = null;
      try {
        stored = deps.storage().getItem(BAR_PLACEMENT_KEY);
      } catch (_error) {
        stored = null;
      }
      return PLACEMENTS.includes(stored as string) ? stored as string : DEFAULT_PLACEMENT;
    },
    set,
    changed() {
      const chosen = (deps.lookup("bar-placement") as HTMLSelectElement).value;
      const saved = persist(chosen);
      set(chosen);
      deps.lookup("bar-placement-state").textContent = placementWords(saved, placement);
    },
  };
}
