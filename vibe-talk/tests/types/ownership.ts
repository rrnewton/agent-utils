// Negative type examples (`#624 voice-page-modularity`, plan §3 "type-check ownership as well as
// imports"): what an owner publishes as read-only, a foreign module cannot write. Each write below
// must be a type error; `@ts-expect-error` fails the check (TS2578) the day one compiles, so a
// record that loses its `readonly` fails `make -C vibe-talk typecheck` here. Each is beside a read of
// the same thing, which must compile, so the file is not passing on an unrelated error.
//
// Checked by tsconfig.types.json, strict and portable like the core. Not under web/src, where a
// comment turning the checker off is refused (web/architecture.mjs).

import type { NavigationSnapshot } from "../../web/src/core/navigation/index.ts";
import { createNavigation } from "../../web/src/core/navigation/index.ts";
import type { BookmarkState, HeldBookmark } from "../../web/src/core/annotations/index.ts";
import type { OutboxEntry, OutboxStore } from "../../web/src/core/outbox/index.ts";
import type { DeviceSetting, SteerOffset } from "../../web/src/core/settings/index.ts";

export function navigationIsReadOnly(snapshot: NavigationSnapshot): string {
  const navigation = createNavigation();
  const now = navigation.snapshot();
  // @ts-expect-error -- the selection changes only through selectChannel.
  now.channelId = "elsewhere";
  // @ts-expect-error -- nor does a subscriber's copy.
  snapshot.channelId = "elsewhere";
  return now.channelId + snapshot.channelId;
}

export function bookmarksAreReadOnly(held: HeldBookmark, state: BookmarkState): number {
  // @ts-expect-error -- a held bookmark's revision is the owner's to move.
  held.revision += 1;
  // @ts-expect-error -- and so is its anchor.
  state.anchor = null;
  return held.revision + state.revision;
}

export function outboxEntriesAreReadOnly(store: OutboxStore<OutboxEntry>): string {
  for (const entry of store.values()) {
    // @ts-expect-error -- an entry's state moves through the store's commands, not by assignment.
    entry.state = "sent";
    // @ts-expect-error -- nor is its id another module's to change.
    entry.id = "other";
    return entry.state;
  }
  return "";
}

export function settingsAreReadOnly(setting: DeviceSetting<boolean>, offset: SteerOffset): number {
  // @ts-expect-error -- a setting's key is fixed where it is defined.
  setting.key = "vibe-talk.other";
  // @ts-expect-error -- a steer offset is a value, not a place to write.
  offset.x = 0;
  return setting.key.length + offset.x;
}
