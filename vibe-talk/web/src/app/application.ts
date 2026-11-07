// The ordered boot (`#624 voice-page-modularity`, plan step 17): mount every owner in the
// page's order, run the first drawing and sign-in in theirs, and — when a test or a later
// owner asks for it — take the mounts down again in the reverse of that order. What mounts,
// and what starting does, is the caller's: this file knows only thunks and acts, never a
// screen, a store or an element, so two boots in one process share nothing.

/** One mounted owner, as every current mount already returns it. */
export interface Mounted {
  /** Take down everything the mount added, once. */
  unmount(): void;
}

/**
 * Mount one owner and hand back its teardown. A thunk, not the mounted owner itself, so
 * nothing is constructed — and no dependency object is even built — until `mount()` reaches
 * it in the page's order: legacy wiring builds several dependency objects by calling the
 * owners mounted just before them, and evaluating the list early would read them before
 * they exist (a temporal-dead-zone change, not a tidy-up).
 */
export type MountThunk = () => Mounted;

/**
 * One step of the first drawing and sign-in, in the page's order. It may start work it does
 * not wait for (today's sign-in proof and deep-link opening are both guarded and left
 * running); `start()` neither awaits nor collects what an act returns.
 */
export type StartAct = () => unknown;

/** What a boot is given: the mounts and the start acts, each already in the page's order. */
export interface ApplicationPorts {
  /** Mounted by `mount()`, in array order. Snapshotted when the application is created. */
  readonly mounts: readonly MountThunk[];
  /** Run by `start()`, in array order. Snapshotted when the application is created. */
  readonly starts: readonly StartAct[];
}

/** One boot of the page. Production mounts and starts it once; `dispose()` is for tests. */
export interface Application {
  /**
   * Register the teardown of an owner mounted in place, in the page's textual order, and
   * count the boot as mounted. The legacy script mounts its owners where they have always
   * been mounted — several dependency objects call the owners mounted just before them —
   * so it tracks each one as it goes rather than evaluating a mount list early; the final
   * entry step, which owns the wiring itself, passes thunks through the ports instead.
   * Tracking after disposal takes the owner down at once: a late mount is never leaked.
   */
  track(mounted: Mounted): void;
  /**
   * Mount every owner, in the ports' order, keeping each teardown for `dispose()`. A second
   * call — before or after `start()` — mounts nothing again. If a thunk throws, the owners
   * already mounted are taken down in reverse order and the error is rethrown, so a failed
   * boot leaves no listener behind; the application is then disposed.
   */
  mount(): void;
  /**
   * Run the start acts, in the ports' order. Must follow `mount()`: calling it first throws,
   * because today's sequence reads what the mounts wired. A second call runs nothing again,
   * and a call after `dispose()` runs nothing.
   */
  start(): void;
  /**
   * Take the mounts down in reverse mount order. Idempotent: a second call, or a call before
   * `mount()`, does nothing. Every teardown is attempted even if one throws; the first error
   * is rethrown once the rest have run. After disposal, `mount()` and `start()` do nothing:
   * a disposed boot is not revived, a fresh one is created instead.
   */
  dispose(): void;
}

/** Create one boot from its mounts and start acts. No module-level state is held. */
export function createApplication(ports: ApplicationPorts): Application {
  const mounts = [...ports.mounts];
  const starts = [...ports.starts];
  const teardowns: Mounted[] = [];
  let mounted = false;
  let started = false;
  let disposed = false;

  function takeDownAll(): void {
    let firstError: unknown;
    let failed = false;
    while (teardowns.length > 0) {
      const mountedOwner = teardowns.pop();
      if (mountedOwner === undefined) break;
      try {
        mountedOwner.unmount();
      } catch (error) {
        if (!failed) {
          failed = true;
          firstError = error;
        }
      }
    }
    if (failed) throw firstError;
  }

  return {
    track(mountedOwner: Mounted): void {
      if (disposed) {
        mountedOwner.unmount();
        return;
      }
      teardowns.push(mountedOwner);
      mounted = true;
    },
    mount(): void {
      if (mounted || disposed) return;
      try {
        for (const thunk of mounts) teardowns.push(thunk());
      } catch (error) {
        disposed = true;
        try {
          takeDownAll();
        } catch {
          // The mount error is the one the caller must see; a teardown error while
          // unwinding a failed boot would only hide it.
        }
        throw error;
      }
      mounted = true;
    },
    start(): void {
      if (disposed || started) return;
      if (!mounted) throw new Error("the application must be mounted before it is started");
      started = true;
      for (const act of starts) act();
    },
    dispose(): void {
      if (disposed || !mounted) return;
      disposed = true;
      takeDownAll();
    },
  };
}
