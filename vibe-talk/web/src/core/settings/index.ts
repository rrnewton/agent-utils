// The device's settings, and apart from them its credentials. Its public interface.
// `#624 voice-page-modularity`.

export { createCredentials } from "./credentials.ts";
export type { Credentials } from "./credentials.ts";
export { IDENTITY_KEY, SYNCED_SETTING_KEYS } from "./keys.ts";
export { createSettingsSync, mergedIdentities } from "./sync.ts";
export type { AgreedSetting, SettingsSync, SettingsSyncDependencies } from "./sync.ts";
export { DEVICE_SETTINGS, createDeviceSettings } from "./device.ts";
export type { AfterDone, DeviceSetting, DeviceSettings, ReadAudioSource, SteerOffset } from "./device.ts";
export {
  DEFAULT_CARD_MAX_LINES,
  DEFAULT_MARK_READ_FIRST_PERCENT,
  DEFAULT_MARK_READ_LONG_WORDS,
  DEFAULT_COLLAPSED_LINES,
  DEFAULT_DISCUSS_WIDTH,
  DEFAULT_MSG_SCALE,
  DEFAULT_QUOTE_READ_PERCENT,
  DEFAULT_READING_CH,
  DEFAULT_TWO_COLUMN_SPLIT,
  MAX_CARD_MAX_LINES,
  MAX_MARK_READ_FIRST_PERCENT,
  MAX_MARK_READ_LONG_WORDS,
  MAX_COLLAPSED_LINES,
  MAX_DISCUSS_WIDTH,
  MAX_MSG_SCALE,
  MAX_QUOTE_READ_PERCENT,
  MAX_READING_CH,
  MAX_READ_SPEED,
  MAX_TWO_COLUMN_SPLIT,
  MIN_CARD_MAX_LINES,
  MIN_MARK_READ_FIRST_PERCENT,
  MIN_MARK_READ_LONG_WORDS,
  MIN_COLLAPSED_LINES,
  MIN_DISCUSS_WIDTH,
  MIN_MSG_SCALE,
  MIN_QUOTE_READ_PERCENT,
  MIN_READING_CH,
  MIN_READ_SPEED,
  MIN_TWO_COLUMN_SPLIT,
  clampCardMaxLines,
  clampMarkReadFirstPercent,
  clampMarkReadLongWords,
  clampCollapsedLines,
  clampDiscussWidth,
  clampMsgScale,
  clampQuoteReadPercent,
  clampReadSpeed,
  clampReadingWidth,
  clampTwoColumnSplit,
} from "./bounds.ts";
export { createCardMaxLines, createCollapsedLines, createMarkReadFirstPercent, createMarkReadLongWords, createQuoteReadPercent } from "./messagelimits.ts";
export type { CardMaxLines, CardMaxLinesDependencies, CollapsedLines, MarkReadNumber, MessageLimitDependencies, QuoteReadPercent } from "./messagelimits.ts";
export { createInAppLinks } from "./inapplinks.ts";
export type { InAppLinks, InAppLinksDependencies } from "./inapplinks.ts";
export { createEnterSends, createSingleKeys } from "./keychoices.ts";
export type { EnterSends, KeyChoiceDependencies, SingleKeys } from "./keychoices.ts";
export { createKeptSwitch } from "./switches.ts";
export type { KeptSwitch, KeptSwitchDependencies } from "./switches.ts";
