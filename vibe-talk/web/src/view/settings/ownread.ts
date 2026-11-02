// Apply "Mark my own messages as read" to existing messages, and undo exactly that application.
// The Settings command's confirmation and receipt, moved from voice.js for #624 voice-page-modularity.
// The setting and the channel's read state remain with their existing owners.

/** How long a first press of "Apply to existing messages" waits for the second, in milliseconds. */
export const OWN_READ_APPLY_ARM_MS = 10000;

/** Only the fields this command reads from the untyped request's answer. */
interface OwnReadChange {
  readonly channel?: { readonly id?: unknown } | null;
  readonly messages?: readonly unknown[] | null;
}

interface OwnReadPayload {
  readonly changes?: readonly (OwnReadChange | null)[] | null;
}

/** The page capabilities used by the command. */
export interface OwnReadApplyDependencies {
  lookup(id: string): HTMLElement;
  request(path: string, options: { method: string; body?: unknown }): Promise<unknown>;
  setStatus(text: string): void;
  selectedChannel(): string | null;
  reloadChannel(options: { keepPosition: true; readAs: "mutation" }): Promise<unknown>;
  guard(run: () => Promise<unknown>): () => void;
  setTimeout(run: () => void, ms: number): ReturnType<typeof setTimeout>;
  clearTimeout(timer: ReturnType<typeof setTimeout>): void;
}

/** The command's named actions; its confirmation and undo receipt are private. */
export interface OwnReadApply {
  /** First press asks; second press applies to all channels and keeps what can be undone. */
  apply(): Promise<void>;
  /** Restore the held receipt in batches, retaining unfinished work after a failure. */
  undo(): Promise<void>;
  /** Stop waiting for a second press and restore the button's words. */
  disarm(): void;
  /** Retire the pending confirmation timer without drawing. */
  dispose(): void;
}

/** Nothing armed or applied; construction has no page effects. */
export function mountOwnReadApply(deps: OwnReadApplyDependencies): OwnReadApply {
  /** The timer of a first press of "Apply to existing messages" waiting for the second, or null. */
  let ownReadApplyArmed: ReturnType<typeof setTimeout> | null = null;

  /**
   * What the last "Apply to existing messages" marked, by channel, for its Undo; null when there is
   * nothing to put back. `#549 own-read-stored`.
   */
  let ownReadApplied: { channel: string; messages: string[] }[] | null = null;

  /** Put "Apply to existing messages" back to waiting for a first press. */
  function disarmOwnReadApply(): void {
    if (ownReadApplyArmed !== null) deps.clearTimeout(ownReadApplyArmed);
    ownReadApplyArmed = null;
    deps.lookup("own-read-apply").textContent = "Apply to existing messages";
  }

  /**
   * "Apply to existing messages": every message of the reader's this server has decided and that is not
   * Done is marked read, in every channel. Pressed twice, the first press saying what the second does,
   * because it reaches messages the reader may have put back on purpose. `#549 own-read-stored`.
   */
  async function applyOwnReadToExisting(): Promise<void> {
    if (ownReadApplyArmed === null) {
      ownReadApplyArmed = deps.setTimeout(disarmOwnReadApply, OWN_READ_APPLY_ARM_MS);
      deps.lookup("own-read-apply").textContent = "Mark them read";
      deps.lookup("own-read-state").textContent = "This marks read every message of yours this server has seen " +
        "that is not Done, in every channel, including any you put back. Press Mark them read to go ahead.";
      return;
    }
    disarmOwnReadApply();
    let payload: OwnReadPayload | null;
    try {
      payload = await deps.request("/api/v1/own-read/apply", { method: "POST" }) as OwnReadPayload | null;
    } catch (error) {
      deps.lookup("own-read-state").textContent = "Nothing was marked read: the server did not take it. Try again.";
      throw error;
    }
    const changes: readonly (OwnReadChange | null)[] = payload && Array.isArray(payload.changes) ? payload.changes : [];
    const held = changes
      .map((change) => ({
        channel: String(change && change.channel && change.channel.id),
        messages: (change && Array.isArray(change.messages) ? change.messages : []).map(String),
      }))
      .filter((change) => change.messages.length > 0);
    const count = held.reduce((sum, change) => sum + change.messages.length, 0);
    ownReadApplied = count > 0 ? held : null;
    deps.lookup("own-read-undo").hidden = ownReadApplied === null;
    const said = count === 0
      ? "None of your messages was waiting."
      : `Marked ${count} of your message${count === 1 ? "" : "s"} read. Undo puts ${count === 1 ? "it" : "them"} back.`;
    deps.lookup("own-read-state").textContent = said;
    deps.setStatus(said);
    if (count > 0 && deps.selectedChannel()) {
      deps.guard(() => deps.reloadChannel({ keepPosition: true, readAs: "mutation" }))();
    }
  }

  /** Put back exactly what the last "Apply to existing messages" marked. `#549 own-read-stored`. */
  async function undoOwnReadApply(): Promise<void> {
    const held = ownReadApplied;
    if (held === null) return;
    deps.lookup("own-read-undo").hidden = true;
    let count = 0;
    // Each batch leaves the held set as it lands, so an Undo that fails part way keeps the rest for the
    // next press, and says how far it got.
    while (ownReadApplied !== null && ownReadApplied.length > 0) {
      const change = ownReadApplied[0]!;
      // As one restore may carry: the server's `MAX_DISMISS_BATCH`.
      const messages = change.messages.slice(0, 1000);
      try {
        await deps.request(`/api/v1/channels/${encodeURIComponent(change.channel)}/restore`, { method: "POST", body: { messages } });
      } catch (error) {
        deps.lookup("own-read-undo").hidden = false;
        deps.lookup("own-read-state").textContent =
          `Put ${count} back; the rest could not be put back yet. Press Undo to try again.`;
        throw error;
      }
      count += messages.length;
      change.messages = change.messages.slice(messages.length);
      if (change.messages.length === 0) ownReadApplied.shift();
    }
    ownReadApplied = null;
    const said = `Put ${count} of your message${count === 1 ? "" : "s"} back.`;
    deps.lookup("own-read-state").textContent = said;
    deps.setStatus(said);
    if (deps.selectedChannel()) {
      deps.guard(() => deps.reloadChannel({ keepPosition: true, readAs: "mutation" }))();
    }
  }

  return {
    apply: applyOwnReadToExisting,
    undo: undoOwnReadApply,
    disarm: disarmOwnReadApply,
    dispose() {
      if (ownReadApplyArmed !== null) deps.clearTimeout(ownReadApplyArmed);
      ownReadApplyArmed = null;
    },
  };
}
