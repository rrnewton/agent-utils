/**
 * Fold-by-heading sections (`#1387 discuss-collapsible-sections`, shared with
 * the document reader's `#1237`): each heading in a container folds the
 * section under it — everything up to the next heading of the same or higher
 * level. All sections start collapsed unless a kept open set says otherwise
 * (`#1497 discuss-fold-state-persist`). Folding is view state only.
 */
export interface FoldElement {
  tagName: string;
  textContent?: string;
  children: ArrayLike<FoldElement>;
  appendChild(child: FoldElement): unknown;
  insertBefore(child: FoldElement, before: FoldElement | null): unknown;
  removeChild(child: FoldElement): unknown;
}

type CreateElement = (tag: string) => FoldElement & { open?: boolean };

const HEADING = /^H([1-6])$/;

function headingLevel(el: FoldElement): number | null {
  const match = HEADING.exec(el.tagName);
  return match === null ? null : Number(match[1]);
}

/**
 * One section's stable key (`#1497`): its heading's words, runs of space
 * folded to one. A heading with no words is `section`; a repeated heading
 * takes `#2`, `#3`, … in the order the sections stand, so two sections that
 * share a heading still key apart and a kept key names one section only.
 */
function sectionKey(heading: FoldElement, seen: Map<string, number>): string {
  const base = String(heading.textContent ?? "").replace(/\s+/g, " ").trim() || "section";
  const count = (seen.get(base) ?? 0) + 1;
  seen.set(base, count);
  return count === 1 ? base : `${base}#${count}`;
}

/** Wrap every top-level heading section of `root` in a fold, open where `openKeys` names it. Returns the fold count. */
export function foldSectionsByHeading(root: FoldElement, create: CreateElement, openKeys?: Iterable<string>): number {
  const open = openKeys === undefined ? null : new Set(openKeys);
  const kids = Array.from(root.children as ArrayLike<FoldElement>);
  const levels = kids.map(headingLevel).filter((level): level is number => level !== null);
  if (levels.length === 0) return 0;
  const foldLevel = Math.min(...levels);
  const seen = new Map<string, number>();
  let count = 0;
  let current: (FoldElement & { open?: boolean }) | null = null;
  let body: FoldElement | null = null;
  for (const kid of kids) {
    const level = headingLevel(kid);
    if (level !== null && level <= foldLevel) {
      const details = create("details");
      details.open = open !== null && open.has(sectionKey(kid, seen));
      const summary = create("summary");
      root.insertBefore(details, kid);
      root.removeChild(kid);
      summary.appendChild(kid);
      details.appendChild(summary);
      body = create("div");
      details.appendChild(body);
      current = details;
      count += 1;
    } else if (current !== null && body !== null) {
      root.removeChild(kid);
      body.appendChild(kid);
    }
  }
  return count;
}

/** Every fold's key under `root`, in the order the sections stand. */
export function foldSectionKeys(root: FoldElement): string[] {
  const seen = new Map<string, number>();
  const keys: string[] = [];
  for (const kid of Array.from(root.children as ArrayLike<FoldElement>)) {
    if (kid.tagName !== "DETAILS") continue;
    const summary = kid.children[0];
    const heading = summary === undefined ? undefined : summary.children[0];
    if (heading !== undefined) keys.push(sectionKey(heading, seen));
  }
  return keys;
}

/** The keys of the folds now open under `root`: the set a reopen keeps. */
export function openSectionKeys(root: FoldElement): string[] {
  const seen = new Map<string, number>();
  const keys: string[] = [];
  for (const kid of Array.from(root.children as ArrayLike<FoldElement>)) {
    const el = kid as FoldElement & { open?: boolean };
    if (el.tagName !== "DETAILS") continue;
    const summary = el.children[0];
    const heading = summary === undefined ? undefined : summary.children[0];
    if (heading === undefined) continue;
    const key = sectionKey(heading, seen);
    if (el.open === true) keys.push(key);
  }
  return keys;
}

/** Open or close every fold under `root`. */
export function setAllSections(root: FoldElement, open: boolean): void {
  for (const kid of Array.from(root.children as ArrayLike<FoldElement>)) {
    const el = kid as FoldElement & { open?: boolean };
    if (el.tagName === "DETAILS") el.open = open;
  }
}
