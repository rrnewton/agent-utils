// Listeners a mount adds and takes down again (`#624 voice-page-modularity`, plan step 17): each owner's mount
// listens through one of these, and its unmount takes down every listener it added, with the options it was added
// with, so the ordered boot's teardown is a plain fold over the mounts.

/** A ResizeObserver, as far as a mount uses one. */
export interface SizeWatcher {
  observe(target: Element): void;
  disconnect(): void;
}

/** Watching sizes, as the page offers it to a mount: whether the browser can, and a watcher calling `report`. */
export interface SizeWatching {
  canWatchSizes(): boolean;
  sizeWatcher(report: () => void): SizeWatcher;
}

/** The listeners one mount added. */
export interface Listening {
  /** Add `listener` for `type` on `target`, as `addEventListener` does, remembered for `unmount`. */
  listen<Kind extends Event>(target: EventTarget, type: string, listener: (event: Kind) => void, options?: AddEventListenerOptions): void;
  /** Take down every listener added, once; a second call does nothing. */
  unmount(): void;
}

export function createListening(): Listening {
  const taken: (() => void)[] = [];
  return {
    listen<Kind extends Event>(target: EventTarget, type: string, listener: (event: Kind) => void, options?: AddEventListenerOptions): void {
      const heard = listener as EventListener;
      target.addEventListener(type, heard, options);
      taken.push(() => target.removeEventListener(type, heard, options));
    },
    unmount() {
      for (const take of taken.splice(0)) take();
    },
  };
}
