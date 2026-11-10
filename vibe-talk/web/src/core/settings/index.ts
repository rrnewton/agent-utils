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
  DEFAULT_QUOTE_READ_PERCENT,
  DEFAULT_READING_CH,
  DEFAULT_TWO_COLUMN_SPLIT,
  MAX_CARD_MAX_LINES,
  MAX_COLLAPSED_LINES,
  MAX_MSG_SCALE,
  MAX_QUOTE_READ_PERCENT,
  MAX_READING_CH,
  MAX_READ_SPEED,
  MAX_TWO_COLUMN_SPLIT,
  MIN_CARD_MAX_LINES,
  MIN_COLLAPSED_LINES,
  MIN_MSG_SCALE,
  MIN_QUOTE_READ_PERCENT,
  MIN_READING_CH,
  MIN_READ_SPEED,
  MIN_TWO_COLUMN_SPLIT,
  clampCardMaxLines,
  clampCollapsedLines,
  clampMsgScale,
  clampQuoteReadPercent,
  clampReadSpeed,
  clampReadingWidth,
  clampTwoColumnSplit,
} from "./bounds.ts";
export { createCardMaxLines, createCollapsedLines, createQuoteReadPercent } from "./messagelimits.ts";
export type { CardMaxLines, CardMaxLinesDependencies, CollapsedLines, MessageLimitDependencies, QuoteReadPercent } from "./messagelimits.ts";
export { createInAppLinks } from "./inapplinks.ts";
export type { InAppLinks, InAppLinksDependencies } from "./inapplinks.ts";
export { createEnterSends, createSingleKeys } from "./keychoices.ts";
export type { EnterSends, KeyChoiceDependencies, SingleKeys } from "./keychoices.ts";
export { createKeptSwitch } from "./switches.ts";
export type { KeptSwitch, KeptSwitchDependencies } from "./switches.ts";
