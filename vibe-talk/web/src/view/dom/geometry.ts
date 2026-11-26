// Where the page's measured geometry reaches the stylesheet (`#2277 vibe-talk-key-scroll-freezes-11-26`;
// new in view/dom). The chips' room, the box's top edge, the read-aloud strip and the keyboard are
// measured by the page and handed to the stylesheet as custom properties. They used to be written on
// `#frame-body`, `#screen-main` or the root, where every element of the page inherits them: each change
// restyled every row of every list (12.5k elements at 192 rows, about 1 s at 768 rows on a phone), and
// the box's top edge changes on every scroll at the foot. They are now registered `inherits: false`
// (`02-body.css`) and written here, on exactly the elements whose own rules read them, so a change
// restyles those few elements and nothing below them. A browser without `@property` inherits them as
// before: right, just as slow as before.
//
// The census below is the one list of who reads what. A rule that reads one of these names on an
// element missing from its list would see nothing (the property is not inherited), so the page's
// style tests refuse any `var()` of these names outside a rule for a listed consumer.

/** The geometry custom properties the page measures and writes. */
export type GeometryName =
  | "--composer-top"
  | "--scroll-tools-clearance"
  | "--scroll-tools-height"
  | "--read-steer-clearance"
  | "--read-steer-height"
  | "--keyboard-inset";

/**
 * The elements that stand on the chips' foot (`--chips-foot`, worked out in each of their own rules
 * from `--composer-top`, and from `--read-steer-height` while a phone's strip plays): the chips, the
 * pull's report at the end, the status pill, the read-aloud strip and the floating Discuss panel.
 */
export const CHIPS_FOOT_CONSUMERS: readonly string[] = Object.freeze([
  "scroll-tools", "pull-refresh", "status-line", "read-steer", "discuss-panel", "discuss-current-section",
]);

/**
 * Who reads each property, by element id. `live`: written as measured, whatever screen is up.
 * `held`: inside `#screen-main`, written only while that screen shows, so the lists and their boxes
 * keep the room last measured on it while another screen is up (`#280 scroll-stability`).
 */
export const GEOMETRY_CONSUMERS: Readonly<Record<GeometryName, { readonly live: readonly string[]; readonly held: readonly string[] }>> =
  Object.freeze({
    "--composer-top": { live: CHIPS_FOOT_CONSUMERS, held: [] },
    "--scroll-tools-clearance": {
      live: ["status-line"],
      held: ["pull-refresh", "channel-composer", "side-composer", "voice-composer", "pane-discord", "side-pane", "side-list", "scroll-area"],
    },
    "--scroll-tools-height": { live: ["read-steer"], held: [] },
    "--read-steer-clearance": {
      live: ["status-line"],
      held: ["pull-refresh", "channel-composer", "side-composer", "voice-composer", "pane-discord", "side-pane", "side-list"],
    },
    "--read-steer-height": { live: CHIPS_FOOT_CONSUMERS, held: ["screen-main", "scroll-area", "side-scroll"] },
    "--keyboard-inset": { live: ["channel-composer", "side-composer"], held: [] },
  });

/** What writing the geometry asks of the page. */
export interface GeometryDependencies {
  /** The page's element by id (the left column's: these are shell elements), or null. */
  lookup(id: string): HTMLElement | null;
  /** Whether `#screen-main` shows now (the `held` consumers are written only then). */
  mainShown(): boolean;
}

/** Writing the geometry. */
export interface Geometry {
  /** Hand `value` to every element that reads `name`; a value already there is not written again. */
  write(name: GeometryName, value: string): void;
}

export function createGeometry(deps: GeometryDependencies): Geometry {
  function put(id: string, name: GeometryName, value: string): void {
    const node = deps.lookup(id);
    if (!node || !node.style) return;
    if (node.style.getPropertyValue(name) !== value) node.style.setProperty(name, value);
  }

  return {
    write(name, value) {
      const consumers = GEOMETRY_CONSUMERS[name];
      for (const id of consumers.live) put(id, name, value);
      if (consumers.held.length && deps.mainShown()) for (const id of consumers.held) put(id, name, value);
    },
  };
}
