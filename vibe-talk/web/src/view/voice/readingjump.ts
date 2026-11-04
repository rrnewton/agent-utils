// The way back to the message being read (`#837 voice-page-jump-to-reading`, an owner from the start): a chip
// beside Newest and Bookmark, up while a read plays or is held and none of the rows it is read in stands on
// screen, gone as soon as one does; a tap brings the row back, centred. What is on screen is the browser's to
// say (an IntersectionObserver on the rows, clipped by the lists they scroll in), so the chip follows a scroll
// with no work on the scroll itself. Which rows are the read's is the page's (read-aloud's
// `readingRowsShown`), asked again whenever the read or the lists change (`refresh`).

/** What the chip asks of the page. */
export interface ReadingJumpDependencies {
  /** The chip. */
  chip(): HTMLElement;
  /** The rows the message being read is drawn in, in lists on screen; none while nothing is read. */
  rows(): HTMLElement[];
}

/** The chip's commands. */
export interface ReadingJump {
  /** The read, or the rows drawn for it, may have changed: watch the rows there are now. */
  refresh(): void;
  /** Bring the row being read into view, centred; false where none is drawn. */
  jump(): boolean;
}

/** Nothing watched, the chip down. */
export function createReadingJump(deps: ReadingJumpDependencies): ReadingJump {
  let observer: IntersectionObserver | null = null;
  let watched: HTMLElement[] = [];
  // The rows the browser has said something of, and those of them it said stand on screen.
  const known = new Set<Element>();
  const seen = new Set<Element>();

  function render(): void {
    const chip = deps.chip();
    // Down until the browser has said where each row is: a row just drawn is most likely the one in view.
    const hidden = watched.length === 0 || watched.some((row) => !known.has(row) || seen.has(row));
    if (chip.hidden !== hidden) chip.hidden = hidden;
  }

  function watch(rows: HTMLElement[]): void {
    if (observer === null) {
      if (typeof IntersectionObserver !== "function") return;
      observer = new IntersectionObserver((entries) => {
        for (const entry of entries) {
          known.add(entry.target);
          if (entry.isIntersecting) seen.add(entry.target);
          else seen.delete(entry.target);
        }
        render();
      });
    }
    for (const row of watched) {
      if (rows.includes(row)) continue;
      observer.unobserve(row);
      known.delete(row);
      seen.delete(row);
    }
    for (const row of rows) if (!watched.includes(row)) observer.observe(row);
    watched = rows;
  }

  return {
    refresh() {
      const rows = deps.rows();
      if (rows.length !== watched.length || rows.some((row, at) => row !== watched[at])) watch(rows);
      render();
    },
    jump() {
      const row = watched.find((one) => one.getAttribute("data-reading") === "true") || watched[0];
      if (!row) return false;
      row.scrollIntoView({ block: "center" });
      return true;
    },
  };
}
