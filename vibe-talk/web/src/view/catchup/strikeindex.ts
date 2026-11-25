// The strike index (#2265 vibe-talk-summary-bullet-restyle-everywhere): every
// drawn summary bullet and section, current and earlier, keyed by the
// message ids its citation links name, so a change to one message restyles
// every bullet that cites it with one map lookup, never a walk of every
// bullet. Each draw replaces its own group (the current answer; the
// timeline's earlier summaries), so a redraw never leaves the old elements
// indexed; an element found detached when an event reaches it is dropped
// there. Pure over the structural element shape strikes.ts walks, so it is
// testable without a page.
//
// A message counts as done for the strike when it is read OR deleted
// upstream (owner side, 2026-11-25): a bullet whose cited messages are all
// done is struck, and a citation link naming a deleted message wears
// DELETED_CITE_CLASS (the stylesheet adds a muted "(deleted)" after it, so
// the bullet's own text, search and read-aloud are unchanged).

import {
  FRESH_CLASS,
  STRUCK_BULLET_CLASS,
  STRUCK_SECTION_CLASS,
  citedIdOfAnchor,
  setWord,
  type StrikeElement,
} from "./strikes.ts";

/** A citation link whose message was deleted upstream. */
export const DELETED_CITE_CLASS = "discuss-cite-deleted";

/** The element shape the index holds; `isConnected` as the DOM says it. */
export interface StrikeIndexElement extends StrikeElement {
  readonly isConnected?: boolean;
  children: ArrayLike<StrikeIndexElement>;
}

/** What the strike asks of the page's state NOW. */
export interface StrikeFacts {
  /** Read here (Done, archived, marked read). */
  read(id: string): boolean;
  /** Deleted upstream, as far as this page was told. */
  deleted(id: string): boolean;
}

/** How a pass went: entries (bullets, sections, links) restyled and bullets now struck. */
export interface StrikeTally {
  restyled: number;
  struck: number;
}

/** Options of one group's draw. */
export interface StrikeGroupOptions {
  /** The latest Update's cited ids, green until read (the current answer only). */
  fresh?: ReadonlySet<string>;
  /** Index only under elements this accepts (an earlier summary's body, not a follow-up turn). */
  within?: (el: StrikeIndexElement) => boolean;
}

export interface StrikeIndex {
  /** Drop `group`'s entries and index the freshly drawn `root` (null: the group is empty now). */
  replace(group: string, root: StrikeIndexElement | null, options?: StrikeGroupOptions): void;
  /** Style every entry of `group` from the facts now (right after its draw). */
  strikeGroup(group: string, facts: StrikeFacts): StrikeTally;
  /** Restyle only what cites `ids`: one lookup per id. */
  strikeIds(ids: Iterable<string>, facts: StrikeFacts): StrikeTally;
  /**
   * A change with no id list (server read state landing, a view sync):
   * compare each indexed id's state with the one last styled and restyle
   * only what cites an id that moved. Map work only, no element walk.
   */
  strikeChanged(facts: StrikeFacts): StrikeTally;
  /** The bullets and sections now citing `id`, connected ones only. */
  citing(id: string): StrikeIndexElement[];
  /** How much is indexed. */
  size(): { ids: number; entries: number; links: number };
}

interface Entry {
  readonly el: StrikeIndexElement;
  readonly section: boolean;
  readonly cited: readonly string[];
  readonly group: string;
}

interface Link {
  readonly el: StrikeIndexElement;
  readonly id: string;
  readonly group: string;
}

interface Slot {
  readonly entries: Set<Entry>;
  readonly links: Set<Link>;
}

interface Group {
  readonly entries: Set<Entry>;
  readonly links: Set<Link>;
  readonly fresh: ReadonlySet<string>;
}

/** One id's state for change detection: open, read, or deleted upstream. */
type IdState = 0 | 1 | 2;

const NO_FRESH: ReadonlySet<string> = new Set();

export function createStrikeIndex(): StrikeIndex {
  const byId = new Map<string, Slot>();
  const groups = new Map<string, Group>();
  /** Each indexed id's state as last styled; the change detector. */
  const styled = new Map<string, IdState>();

  const stateOf = (facts: StrikeFacts, id: string): IdState => (facts.deleted(id) ? 2 : facts.read(id) ? 1 : 0);
  const done = (facts: StrikeFacts, id: string): boolean => facts.deleted(id) || facts.read(id);

  function slotOf(id: string): Slot {
    let slot = byId.get(id);
    if (slot === undefined) {
      slot = { entries: new Set(), links: new Set() };
      byId.set(id, slot);
    }
    return slot;
  }

  function forgetIfEmpty(id: string): void {
    const slot = byId.get(id);
    if (slot !== undefined && slot.entries.size === 0 && slot.links.size === 0) {
      byId.delete(id);
      styled.delete(id);
    }
  }

  function dropEntry(entry: Entry): void {
    groups.get(entry.group)?.entries.delete(entry);
    for (const id of entry.cited) {
      byId.get(id)?.entries.delete(entry);
      forgetIfEmpty(id);
    }
  }

  function dropLink(link: Link): void {
    groups.get(link.group)?.links.delete(link);
    byId.get(link.id)?.links.delete(link);
    forgetIfEmpty(link.id);
  }

  /** Style one entry; false (and dropped) when its element left the page. */
  function styleEntry(entry: Entry, facts: StrikeFacts, tally: StrikeTally): boolean {
    if (entry.el.isConnected === false) {
      dropEntry(entry);
      return false;
    }
    const struck = entry.cited.every((id) => done(facts, id));
    const fresh = groups.get(entry.group)?.fresh ?? NO_FRESH;
    const isFresh = !struck && entry.cited.some((id) => fresh.has(id));
    setWord(entry.el, entry.section ? STRUCK_SECTION_CLASS : STRUCK_BULLET_CLASS, struck);
    setWord(entry.el, FRESH_CLASS, isFresh);
    tally.restyled += 1;
    if (struck && !entry.section) tally.struck += 1;
    return true;
  }

  function styleLink(link: Link, facts: StrikeFacts, tally: StrikeTally): void {
    if (link.el.isConnected === false) {
      dropLink(link);
      return;
    }
    setWord(link.el, DELETED_CITE_CLASS, facts.deleted(link.id));
    tally.restyled += 1;
  }

  /** Restyle everything citing `id`, each entry once per pass. */
  function styleId(id: string, facts: StrikeFacts, seen: Set<Entry>, tally: StrikeTally): void {
    const slot = byId.get(id);
    if (slot === undefined) return;
    styled.set(id, stateOf(facts, id));
    for (const entry of [...slot.entries]) {
      if (seen.has(entry)) continue;
      seen.add(entry);
      styleEntry(entry, facts, tally);
    }
    for (const link of [...slot.links]) styleLink(link, facts, tally);
  }

  return {
    replace(group, root, options = {}) {
      const old = groups.get(group);
      if (old !== undefined) {
        for (const entry of [...old.entries]) dropEntry(entry);
        for (const link of [...old.links]) dropLink(link);
      }
      const fresh = options.fresh ?? NO_FRESH;
      const held: Group = { entries: new Set(), links: new Set(), fresh };
      groups.set(group, held);
      if (root === null) return;
      const within = options.within;
      // One walk: each node returns the ids cited at or under it, so a
      // bullet's ids are its subtree's without a second walk per bullet.
      const visit = (node: StrikeIndexElement, inside: boolean): Set<string> => {
        const here = inside || within === undefined || within(node);
        const ids = new Set<string>();
        for (const kid of Array.from(node.children ?? [])) {
          for (const id of visit(kid, here)) if (here) ids.add(id);
        }
        if (!here) return ids;
        const own = citedIdOfAnchor(node);
        if (own !== null) {
          ids.add(own);
          const link: Link = { el: node, id: own, group };
          held.links.add(link);
          slotOf(own).links.add(link);
        }
        const tag = node.tagName.toLowerCase();
        if ((tag === "li" || tag === "details") && ids.size > 0) {
          const entry: Entry = { el: node, section: tag === "details", cited: [...ids], group };
          held.entries.add(entry);
          for (const id of ids) slotOf(id).entries.add(entry);
        }
        return ids;
      };
      visit(root, false);
    },

    strikeGroup(group, facts) {
      const tally: StrikeTally = { restyled: 0, struck: 0 };
      const held = groups.get(group);
      if (held === undefined) return tally;
      const seen = new Set<Entry>();
      const ids = new Set<string>();
      for (const entry of [...held.entries]) {
        seen.add(entry);
        if (styleEntry(entry, facts, tally)) for (const id of entry.cited) ids.add(id);
      }
      for (const link of [...held.links]) {
        styleLink(link, facts, tally);
        ids.add(link.id);
      }
      // An id this draw shares with another group whose state moved since
      // that group was styled restyles there too, so the change detector
      // never records a state some element does not show.
      for (const id of ids) {
        const now = stateOf(facts, id);
        const was = styled.get(id);
        if (was !== undefined && was !== now) styleId(id, facts, seen, tally);
        else styled.set(id, now);
      }
      return tally;
    },

    strikeIds(ids, facts) {
      const tally: StrikeTally = { restyled: 0, struck: 0 };
      const seen = new Set<Entry>();
      for (const id of new Set(Array.from(ids, String))) styleId(id, facts, seen, tally);
      return tally;
    },

    strikeChanged(facts) {
      const tally: StrikeTally = { restyled: 0, struck: 0 };
      const seen = new Set<Entry>();
      for (const id of [...byId.keys()]) {
        if (styled.get(id) === stateOf(facts, id)) continue;
        styleId(id, facts, seen, tally);
      }
      return tally;
    },

    citing(id) {
      const slot = byId.get(String(id));
      if (slot === undefined) return [];
      return [...slot.entries].filter((entry) => entry.el.isConnected !== false).map((entry) => entry.el);
    },

    size() {
      let entries = 0;
      let links = 0;
      for (const held of groups.values()) {
        entries += held.entries.size;
        links += held.links.size;
      }
      return { ids: byId.size, entries, links };
    },
  };
}
