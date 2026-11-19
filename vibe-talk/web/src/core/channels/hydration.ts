// Hydration pacer (`#1846 vibe-talk-windowed-rendering`, step 5): prepares row content for
// cache entries inside the hydration window (render window + one screenful) off the scroll
// path. Work is sliced to <=8ms via requestIdleCallback (50ms timeout fallback), never runs
// while hidden, and scrolling away drops queued work beyond the window — no backlog grows
// toward history size. A mounted-row miss hydrates that one row synchronously (caller side).

export interface HydrationPacer {
  /** Queue `ids` for paced hydration; ids outside a later `retain` are dropped unhydrated. */
  offer(ids: readonly string[]): void;
  /** Keep only `ids`; everything else queued is cancelled, not deferred. */
  retain(ids: readonly string[]): void;
  /** Ids still queued. */
  pending(): number;
  dispose(): void;
}

export function createHydrationPacer(work: (id: string) => void, opts?: { sliceMs?: number; hidden?: () => boolean }): HydrationPacer {
  const sliceMs = opts?.sliceMs ?? 8;
  const queue: string[] = [];
  const queued = new Set<string>();
  let scheduled = false;
  let disposed = false;
  const g = globalThis as unknown as { requestIdleCallback?: (c: () => void, o: { timeout: number }) => void; setTimeout?: (c: () => void, ms: number) => unknown };
  const ric: ((cb: () => void) => void) | null =
    typeof g.requestIdleCallback === "function" ? (cb) => (g.requestIdleCallback as (c: () => void, o: { timeout: number }) => void)(cb, { timeout: 50 }) : null;
  function schedule(): void {
    if (scheduled || disposed || queue.length === 0) return;
    scheduled = true;
    const run = () => {
      scheduled = false;
      if (disposed) return;
      if (opts?.hidden && opts.hidden()) { schedule(); return; }
      const start = Date.now();
      while (queue.length > 0 && Date.now() - start < sliceMs) {
        const id = queue.shift() as string;
        queued.delete(id);
        work(id);
      }
      if (queue.length > 0) schedule();
    };
    if (ric) ric(run);
    else if (g.setTimeout) g.setTimeout(run, 50);
    else run();
  }
  return {
    offer(ids) {
      for (const id of ids) if (!queued.has(id)) { queued.add(id); queue.push(id); }
      schedule();
    },
    retain(ids) {
      const keep = new Set(ids);
      for (let i = queue.length - 1; i >= 0; i -= 1) {
        if (!keep.has(queue[i] as string)) { queued.delete(queue[i] as string); queue.splice(i, 1); }
      }
    },
    pending: () => queue.length,
    dispose() { disposed = true; queue.length = 0; queued.clear(); },
  };
}
