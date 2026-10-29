// One task after another (`#624 voice-page-modularity`, moved from the legacy script): each task added
// starts once the one before it has settled, whether it succeeded or failed, and what it answers is
// its own. The reader's writes go through one, so a quick run of Done taps reaches the server in
// order and never occupies every provider-adapter worker at once.

/** A queue of tasks run one after another. */
export interface SerialQueue {
  /** Run `task` after every task added before it has settled; answers what `task` answers. */
  run<T>(task: () => Promise<T>): Promise<T>;
}

/** An empty queue. */
export function createSerialQueue(): SerialQueue {
  let tail: Promise<unknown> = Promise.resolve();
  return {
    run<T>(task: () => Promise<T>): Promise<T> {
      const run = tail.then(task, task);
      tail = run.catch(() => {});
      return run;
    },
  };
}
