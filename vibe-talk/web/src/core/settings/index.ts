// The device's settings, and apart from them its credentials. Its public interface.
// `#624 voice-page-modularity`.

export { createCredentials } from "./credentials.ts";
export type { Credentials } from "./credentials.ts";
export { IDENTITY_KEY, SYNCED_SETTING_KEYS } from "./keys.ts";
export { createSettingsSync, mergedIdentities } from "./sync.ts";
export type { AgreedSetting, SettingsSync, SettingsSyncDependencies } from "./sync.ts";
export { DEVICE_SETTINGS, createDeviceSettings } from "./device.ts";
export type { DeviceSetting, DeviceSettings, ReadAudioSource, SteerOffset } from "./device.ts";
export {
  DEFAULT_CARD_MAX_LINES,
  DEFAULT_COLLAPSED_LINES,
  DEFAULT_MSG_SCALE,
  DEFAULT_READING_CH,
  MAX_CARD_MAX_LINES,
  MAX_COLLAPSED_LINES,
  MAX_MSG_SCALE,
  MAX_READING_CH,
  MAX_READ_SPEED,
  MIN_CARD_MAX_LINES,
  MIN_COLLAPSED_LINES,
  MIN_MSG_SCALE,
  MIN_READING_CH,
  MIN_READ_SPEED,
  clampCardMaxLines,
  clampCollapsedLines,
  clampMsgScale,
  clampReadSpeed,
  clampReadingWidth,
} from "./bounds.ts";
