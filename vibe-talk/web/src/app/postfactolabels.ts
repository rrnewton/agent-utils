// Labels added after the fact (`#2279 vibe-talk-post-facto-labels`, owner requests 380 and
// 382): the page's glue around the label picker (web/src/view/channel/labelpicker.ts), kept out
// of the entry module so the entry only wires it.
//
// Three ways a message's labels change on this page, all folded the same way into the store's
// copy (a NEW object, so the row and its search subject are drawn afresh) and redrawn with the
// place kept (`followPlacement`, the path a live arrival takes; owner rule 370):
//
// - this page's own write: the answer carries the message's labels as served now;
// - another device's write: the `labels` live event carries the same state;
// - a write this page missed (asleep, offline, a dropped stream): every timeline read carries
//   `labels_revision`; when it moves, the channel's complete list
//   (`GET /api/v1/channels/{channel}/message-labels`) is read and applied, and a held message the
//   list no longer names loses its menu labels. The saved copy (`saveChannelScope`) is written
//   after each, so the offline cache holds what the server serves.
//
// A state older than one already applied to a message is ignored, so a late list can never
// undo a newer event.

import { createLabelPicker } from "../view/channel/index.ts";
import type { LabelPicker, LabelPickerEntry, LabelWriteAnswer } from "../view/channel/index.ts";
import { messageIndex, upsertMessage, withLabelState, withoutMenuLabels } from "../core/channels/index.ts";
import type { ChannelCanon, LabelState } from "../core/channels/index.ts";
import type { Message } from "../contract/index.ts";

/** One message's label state as the server sends it (`LiveLabelsEvent`). */
export interface ServedLabelState extends LabelState {
  readonly channel_id: string;
  readonly message_id: string;
  readonly revision: number;
}

/** What the glue asks of the page. */
export interface PostFactoLabelsDependencies {
  byId(id: string): HTMLElement;
  /** The known labels, most used first (the Labels drop-down's rows). */
  entries(): LabelPickerEntry[];
  refreshLabels(): Promise<void>;
  /** The store of the channel on screen (a threading provider's; a flat one keeps no store). */
  canon(): ChannelCanon;
  /** The channel on screen. */
  selectedChannel(): string;
  /** Whether the provider threads (a store to fold into); a flat one's rows hold their messages. */
  threading(): boolean;
  /** A flat provider's drawn rows: every message they hold through `change` (the same object
   * when unchanged), and the changed rows drawn again with the place kept. */
  patchFlatRows(change: (message: Message) => Message): void;
  /** Run `redraw` held on message `id`'s row (liaison, owner rule 370): the scroll owner's
   * `#2076` act hold keeps that row's top where it stands, at the end of the history too, and
   * its growth or shrink moves only what is below it. */
  holdingRow(id: string, redraw: () => void): void;
  isPinned(messages: Message[]): boolean;
  togglePin(messages: Message[]): Promise<unknown>;
  /** The page's exact label write (`PUT`/`DELETE` with `{"name": …}`). */
  writeLabel(channel: string, messageId: string, name: string, present: boolean): Promise<LabelWriteAnswer>;
  /** The channel's complete list of messages with menu labels. */
  readChannelLabels(channel: string): Promise<{ readonly messages: readonly ServedLabelState[]; readonly revision: number }>;
  /** Re-project the view from the store and redraw what changed, keeping the place. */
  followPlacement(): void;
  /** Keep the store's copy for the offline cache. */
  saveChannelScope(): void;
  /** Whether the browser says it is online. */
  online(): boolean;
  narrow(): boolean;
  visibleViewport(): { top: number; height: number; width: number };
  guard(run: () => Promise<void>): void;
}

/** The glue's commands. */
export interface PostFactoLabels {
  readonly picker: LabelPicker;
  /** Open the picker for a row's messages, from its ⋯ menu or long-press sheet. */
  open(messages: Message[], row: HTMLElement, anchor: HTMLElement): void;
  /** A `labels` live event. */
  onLabelsEvent(event: ServedLabelState): void;
  /** A landed timeline read: notes its `labels_revision`, catching up when it moved. */
  noteRead(payload: unknown): void;
  /** A pointer down anywhere: closes the picker when it is outside it. */
  closeOutside(event: { target?: unknown } | null): void;
}

/** Nothing applied yet. */
export function mountPostFactoLabels(deps: PostFactoLabelsDependencies): PostFactoLabels {
  /** Message id → the newest revision applied to it. */
  const applied = new Map<string, number>();
  /** Message id → the newest state applied to it, for the picker's view of a copy a re-read has
   * not replaced yet (a flat provider's rows come back with the next read). */
  const states = new Map<string, ServedLabelState>();
  /** Channel → the labels revision this page last caught up to. */
  const revisions = new Map<string, number>();

  function redraw(): void {
    deps.saveChannelScope();
    deps.followPlacement();
    picker.refresh();
  }

  /** Fold one state into the held copy; true when the copy changed. */
  function fold(canon: ChannelCanon, state: ServedLabelState): boolean {
    const key = String(state.message_id);
    const seen = applied.get(key);
    if (seen !== undefined && seen > state.revision) return false;
    applied.set(key, state.revision);
    states.set(key, state);
    const held = messageIndex(canon).get(key);
    if (!held) return false;
    const next = withLabelState(held, state);
    return upsertMessage(canon, next) !== held;
  }

  /**
   * Fold one state in and redraw, holding the row's place (liaison, owner rule 370): this
   * page's own write (its answer, or its live echo landing first) is the reader's act on the
   * row; another device's is a change to a row this reader may be reading. Either way the row
   * the change grows or shrinks keeps its top where it stands (or, above the screen, the row
   * after it does), and only what is below it moves — never the follow-newest foot.
   */
  function apply(state: ServedLabelState): void {
    // Another channel's copy is brought current by that channel's next read (`noteRead`).
    if (String(state.channel_id) !== String(deps.selectedChannel())) return;
    const key = String(state.message_id);
    const run = (act: () => void): void => deps.holdingRow(key, act);
    const canon = deps.canon();
    if (deps.threading() && String(canon.channel) === String(state.channel_id)) {
      run(() => {
        if (fold(canon, state)) redraw();
      });
      return;
    }
    // A flat provider keeps no store: its drawn rows hold their messages, patched in place (no
    // re-read, so nothing above the list comes and goes). A state already applied (the write's
    // answer and its own live echo carry one revision) draws nothing again.
    const seen = applied.get(key);
    if (seen !== undefined && seen >= state.revision) return;
    applied.set(key, state.revision);
    states.set(key, state);
    run(() => deps.patchFlatRows((message) => (String(message.id) === key ? withLabelState(message, state) : message)));
    picker.refresh();
  }

  function applyList(channel: string, list: { readonly messages: readonly ServedLabelState[]; readonly revision: number }): void {
    if (channel !== String(deps.selectedChannel())) return;
    const named = new Map(list.messages.map((state) => [String(state.message_id), state]));
    if (!deps.threading()) {
      deps.patchFlatRows((message) => {
        const key = String(message.id);
        const state = named.get(key);
        const seen = applied.get(key);
        if (state) {
          if (seen !== undefined && seen > state.revision) return message;
          applied.set(key, state.revision);
          states.set(key, state);
          return withLabelState(message, state);
        }
        return seen !== undefined && seen > list.revision ? message : withoutMenuLabels(message);
      });
      picker.refresh();
      return;
    }
    const canon = deps.canon();
    if (String(canon.channel) !== channel) return;
    let changed = false;
    for (const held of [...canon.messages]) {
      const key = String(held.id);
      const state = named.get(key);
      if (state) {
        changed = fold(canon, state) || changed;
        continue;
      }
      // Not named: its menu labels were taken away while this page was not listening — unless
      // an event newer than the list already gave it some.
      const seen = applied.get(key);
      if (seen !== undefined && seen > list.revision) continue;
      const next = withoutMenuLabels(held);
      if (next !== held && upsertMessage(canon, next) !== held) changed = true;
    }
    if (changed) redraw();
  }

  function noteRead(payload: unknown): void {
    const read = payload as { labels_revision?: unknown; channel?: { id?: unknown } } | null;
    const revision = read?.labels_revision;
    if (typeof revision !== "number") return;
    const channel = String(read?.channel?.id ?? deps.selectedChannel());
    const had = revisions.get(channel);
    if (had === revision) return;
    revisions.set(channel, revision);
    // A server where no label was ever added from a menu has nothing to catch up on.
    if (had === undefined && revision === 0) return;
    deps.guard(async () => {
      applyList(channel, await deps.readChannelLabels(channel));
    });
  }

  const picker = createLabelPicker({
    byId: (id) => deps.byId(id),
    entries: () => deps.entries(),
    refreshLabels: () => deps.refreshLabels(),
    channel: () => String(deps.selectedChannel()),
    held: (id, fallback) => {
      const copy = messageIndex(deps.canon()).get(String(id)) ?? fallback;
      const state = states.get(String(id));
      return state ? withLabelState(copy, state) : copy;
    },
    isPinned: (messages) => deps.isPinned(messages),
    togglePin: (messages) => deps.togglePin(messages),
    write: (channel, messageId, name, present) => deps.writeLabel(channel, messageId, name, present),
    apply: (answer) => apply(answer.event as ServedLabelState),
    online: () => deps.online(),
    narrow: () => deps.narrow(),
    visibleViewport: () => deps.visibleViewport(),
    guard: (run) => deps.guard(run),
  });

  return {
    picker,
    open: (messages, row, anchor) => picker.open(messages, row, anchor),
    onLabelsEvent: (event) => apply(event),
    noteRead,
    closeOutside(event) {
      if (!picker.isOpen()) return;
      const root = deps.byId("label-picker");
      const target = event?.target as Node | null | undefined;
      if (target && typeof root.contains === "function" && root.contains(target)) return;
      picker.close();
    },
  };
}
