// The outbox's composition state (`#193 compose-draft-loss`, `#348 settings-dynamic-lists`;
// `#624 voice-page-modularity`, the step 17 composition root's outbox half, moved from the
// legacy script): the outbox itself, what each send has been observed doing, a saved entry as
// the live outbox holds it, the sends dismissed from it and their Settings view, and the
// dismissal the Undo chip takes back. The stores, the view and the Undo stay their owners';
// this makes them, in the page's order, from what the page hands in.

import { createDismissedSends } from "./dismissed.ts";
import type { LiveOutgoingEntry } from "./loads.ts";
import type { DismissedSends } from "./dismissed.ts";
import { outgoingFromSaved as entryFromSaved } from "./saved.ts";
import type { OutgoingEntry } from "./saved.ts";
import { createSendUndo } from "./sendundo.ts";
import type { SendUndo } from "./sendundo.ts";
import { createOutboxStore } from "./store.ts";
import type { OutboxStore } from "./store.ts";
import type { ClockPort, StoragePort } from "../../ports/index.ts";

/** Settings' Recently dismissed view, as the page uses it (`mountDismissedSends`). */
export interface DismissedViewLike {
  render(): void;
  open(id: string, opener: unknown): void;
  close(refocus?: boolean): boolean;
  trapTab(event: unknown): void;
  copy(): Promise<void>;
  restore(): void;
}

/** What the outbox composition is handed by the page. */
export interface OutboxCompositionDependencies {
  storage(): StoragePort;
  makeRoom(): void;
  keepsOwnPosts(channel: string): boolean;
  now(): number;
  lookup(id: string): unknown;
  channelLabel(id: string): string;
  styled(tag: string, className: string, text: string): unknown;
  writeClipboard(text: string): Promise<boolean>;
  copyRefused(): string;
  putBackDismissed(id: string): void;
  scheduleBackSync(): void;
  mountDismissedSends(deps: {
    lookup(id: string): unknown;
    store(): DismissedSends<LiveOutgoingEntry>;
    channelLabel(id: string): string;
    styled(tag: string, className: string, text: string): unknown;
    writeClipboard(text: string): Promise<boolean>;
    copyRefused(): string;
    putBack(id: string): void;
    scheduleBackSync(): void;
  }): DismissedViewLike;
  clock: ClockPort;
  sendUndoEnded(): void;
  resumeRetries(): void;
}

/** The outbox composition state, as the page calls it. */
export interface OutboxComposition {
  readonly outbox: OutboxStore<LiveOutgoingEntry>;
  readonly outgoingObservations: Map<string, Set<string>>;
  outgoingFromSaved(held: unknown): OutgoingEntry | null;
  readonly dismissedSends: DismissedSends<LiveOutgoingEntry>;
  readonly dismissedSendsView: DismissedViewLike;
  readonly sendUndo: SendUndo;
  resumeOnOnline(): void;
}

/** Make the outbox composition state from the page's storage, lists and send service. */
export function createOutboxComposition(deps: OutboxCompositionDependencies): OutboxComposition {
  /**
   * The outbox: every send this page has not seen through, by id, in the order written, how often it has
   * changed, and the device's copy (web/src/core/outbox `createOutboxStore`, `#624
   * voice-page-modularity`). The entries are this page's objects, changed in place as a send goes on.
   */
  const outbox = createOutboxStore<LiveOutgoingEntry>({
    storage: deps.storage(),
    makeRoom: () => deps.makeRoom(),
    keepsOwnPosts: (channel) => deps.keepsOwnPosts(channel),
  });
  const outgoingObservations = new Map<string, Set<string>>();

  /** A saved entry as the live outbox holds it (web/src/core/outbox `outgoingFromSaved`). */
  function outgoingFromSaved(held: unknown): OutgoingEntry | null {
    return entryFromSaved(held, Date.now);
  }

  /**
   * The sends dismissed from the outbox, newest first, each with when it was dismissed, and the device's
   * copy of them (web/src/core/outbox `createDismissedSends`, `#624 voice-page-modularity`).
   */
  const dismissedSends = createDismissedSends<LiveOutgoingEntry>({
    storage: deps.storage(),
    now: () => deps.now(),
    makeRoom: () => deps.makeRoom(),
    decode: (saved) => outgoingFromSaved(saved) as LiveOutgoingEntry | null,
  });

  /**
   * `#348 settings-dynamic-lists`. Settings' Recently dismissed and the pop-out over it, with the dismissed
   * send the pop-out shows (web/src/view/settings `mountDismissedSends`, `#624 voice-page-modularity`).
   * Made here, beside the sends it lists; its wiring is under "the pop-out over Recently dismissed".
   */
  const dismissedSendsView = deps.mountDismissedSends({
    lookup: (id) => deps.lookup(id),
    store: () => dismissedSends,
    channelLabel: (id) => deps.channelLabel(id),
    styled: (tag, className, text) => deps.styled(tag, className, text),
    writeClipboard: (text) => deps.writeClipboard(text),
    copyRefused: () => deps.copyRefused(),
    putBack: (id) => deps.putBackDismissed(id),
    scheduleBackSync: () => deps.scheduleBackSync(),
  });

  /**
   * The dismissal the Undo chip takes back while it is up: the entry's id, its wait for an automatic
   * send if it had one, its stamp, and the chip's timer (web/src/core/outbox `createSendUndo`, `#624
   * voice-page-modularity`).
   */
  const sendUndo = createSendUndo({ clock: deps.clock, ended: () => deps.sendUndoEnded() });

  /** The browser's own word that the connection is back: the sends waiting go again. */
  function resumeOnOnline(): void {
    deps.resumeRetries();
  }

  return { outbox, outgoingObservations, outgoingFromSaved, dismissedSends, dismissedSendsView, sendUndo, resumeOnOnline };
}
