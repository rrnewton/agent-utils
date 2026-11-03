// Filter changes under way (`#243 filter-keeps-selection`; `#624 voice-page-modularity`, moved from the legacy
// script into view/channel): whether a change is being made, so a filter turned off inside another is placed
// once, by the outer; and the walk back to an anchor older than anything loaded, while it is under way, the
// left column's alone. Each walk is its own token, so one that has been ended, or overtaken by another, can
// tell. Keeping the place, and reading older pages, are the page's (`keepingFilterPlace`,
// `walkToFilterAnchor`).

/** A walk back, and the channel context it was started in. */
export interface FilterWalk {
  readonly context: string;
}

/** The changes' commands and questions. */
export interface FilterChanges {
  /** True while a filter change is under way. */
  changing(): boolean;
  /** Make a change with `fn`, and return its answer; nested changes are one, the outer. */
  whileChanging<T>(fn: () => T): T;
  /** Whether a walk back is under way. */
  walking(): boolean;
  /** A walk back starts in channel context `context`: the one under way from now on. */
  startWalk(context: string): FilterWalk;
  /** Whether `walk` is still the one under way. */
  isWalk(walk: FilterWalk): boolean;
  /** No walk is under way from now on: whether one was. */
  endWalk(): boolean;
}

/** No change, no walk. */
export function createFilterChanges(): FilterChanges {
  let filterChanging = 0;
  let filterWalk: FilterWalk | null = null;

  return {
    changing: () => filterChanging > 0,
    whileChanging(fn) {
      filterChanging += 1;
      try {
        return fn();
      } finally {
        filterChanging -= 1;
      }
    },
    walking: () => filterWalk !== null,
    startWalk(context) {
      const walk = { context };
      filterWalk = walk;
      return walk;
    },
    isWalk: (walk) => filterWalk === walk,
    endWalk() {
      if (filterWalk === null) return false;
      filterWalk = null;
      return true;
    },
  };
}
