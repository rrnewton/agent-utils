// The settings sync's laziness and small acts (`#457 settings-server-side`; `#624
// voice-page-modularity`, moved from the legacy script into view/settings for the plan's step 17):
// the reconciler and its Settings view made on first use, one synced key put into force, the quiet
// word about it, the re-read on refocus or reconnect, one frame off a live stream, and the start
// once sign-in has proven the token. What the reconciler and the view are is handed in; the
// laziness — never made at load, where a saved token is only a claim — is this owner's.

import type { SettingsSync } from "../../core/settings/index.ts";
import type { SettingsSyncView } from "./sync.ts";

/** The reconciler between this device's replica and the server's document, as the glue reads it. */
export type SettingsSyncLike = SettingsSync;

/** The sync's Settings view, as the glue reads it. */
export type SettingsSyncViewLike = SettingsSyncView;

export interface SettingsSyncGlueDependencies {
  /** Make the reconciler, with the glue's own apply and status acts. */
  createSync(apply: (key: string) => void, status: (text: string) => void): SettingsSyncLike;
  /** Make the Settings view over the glue's reconciler, made when it is. */
  mountView(sync: () => SettingsSyncLike): SettingsSyncViewLike;
}

/** The settings sync, as the page calls it. */
export interface SettingsSyncGlue {
  settingsSync(): SettingsSyncLike;
  settingsSyncView(): SettingsSyncViewLike;
  applySyncedSetting(key: string): void;
  renderSettingsSyncState(text: string): void;
  refreshServerSettings(): Promise<unknown>;
  onSettingsFrame(frame: Parameters<SettingsSync["onFrame"]>[0]): void;
  startSettingsSync(): void;
}

/** Make the settings sync glue; neither owner is made until first use. */
export function createSettingsSyncGlue(deps: SettingsSyncGlueDependencies): SettingsSyncGlue {
  let sync: SettingsSyncLike | undefined;
  let view: SettingsSyncViewLike | undefined;

  function settingsSync(): SettingsSyncLike {
    if (!sync) {
      sync = deps.createSync(
        (key) => applySyncedSetting(key),
        (text) => renderSettingsSyncState(text),
      );
    }
    return sync;
  }

  function settingsSyncView(): SettingsSyncViewLike {
    if (!view) view = deps.mountView(() => settingsSync());
    return view;
  }

  /**
   * Put one synced key's kept value into force, through the same stored-and-apply pair its own
   * control ends in, so a change from another device lands exactly as one made here would. Called
   * only after `localStorage` holds the new value. One key failing must not stop the rest.
   */
  function applySyncedSetting(key: string): void {
    settingsSyncView().apply(key);
  }

  /** Settings' quiet word about the sync: silent while it is keeping up. */
  function renderSettingsSyncState(text: string): void {
    settingsSyncView().status(text);
  }

  /** Re-read the document, for the moments a missed stream frame could hide in: refocus, reconnect. */
  function refreshServerSettings(): Promise<unknown> {
    return settingsSync().refresh();
  }

  /**
   * One `settings` frame off either live stream: another device's change, or this one's own
   * heard back, which the archive's stamps make a no-op. Unreadable frames are dropped, as
   * every stream frame this page cannot read is.
   */
  function onSettingsFrame(frame: Parameters<SettingsSync["onFrame"]>[0]): void {
    settingsSync().onFrame(frame);
  }

  /** Start keeping this device's replica and the server's document in agreement. Called from
   * `signIn` once the token is proven, never before: settings are read with the same credential
   * as everything else, and an unproven one reads nothing. */
  function startSettingsSync(): void {
    settingsSyncView().start();
  }

  return {
    settingsSync, settingsSyncView, applySyncedSetting, renderSettingsSyncState,
    refreshServerSettings, onSettingsFrame, startSettingsSync,
  };
}
