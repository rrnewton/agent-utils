/**
 * Fold-by-heading sections (`#1387 discuss-collapsible-sections`, shared with
 * the document reader's `#1237`): each heading in a container folds the
 * section under it — everything up to the next heading of the same or higher
 * level. All sections start collapsed. Folding is view state only.
 */
export interface FoldElement {
  tagName: string;
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

/** Wrap every top-level heading section of `root` in a closed fold. Returns the fold count. */
export function foldSectionsByHeading(root: FoldElement, create: CreateElement): number {
  const kids = Array.from(root.children as ArrayLike<FoldElement>);
  const levels = kids.map(headingLevel).filter((level): level is number => level !== null);
  if (levels.length === 0) return 0;
  const foldLevel = Math.min(...levels);
  let count = 0;
  let current: (FoldElement & { open?: boolean }) | null = null;
  let body: FoldElement | null = null;
  for (const kid of kids) {
    const level = headingLevel(kid);
    if (level !== null && level <= foldLevel) {
      const details = create("details");
      details.open = false;
      const summary = create("summary");
      root.insertBefore(details, kid);
      root.removeChild(kid);
      summary.appendChild(kid);
      details.appendChild(summary);
      body = create("div");
      details.appendChild(body);
      root.appendChild(details);
      current = details;
      count += 1;
    } else if (current !== null && body !== null) {
      root.removeChild(kid);
      body.appendChild(kid);
    }
  }
  return count;
}

/** Open or close every fold under `root`. */
export function setAllSections(root: FoldElement, open: boolean): void {
  for (const kid of Array.from(root.children as ArrayLike<FoldElement>)) {
    const el = kid as FoldElement & { open?: boolean };
    if (el.tagName === "DETAILS") el.open = open;
  }
}
