// The device's settings, and apart from them its credentials. Its public interface.
// `#624 voice-page-modularity`.

export { createCredentials } from "./credentials.ts";
export type { Credentials } from "./credentials.ts";
export { IDENTITY_KEY, SYNCED_SETTING_KEYS } from "./keys.ts";
export { createSettingsSync, mergedIdentities } from "./sync.ts";
export type { AgreedSetting, SettingsSync, SettingsSyncDependencies } from "./sync.ts";
export { DEVICE_SETTINGS, createDeviceSettings } from "./device.ts";
export type { AfterDone, Appearance, ColourTheme, DeviceSetting, DeviceSettings, DiscussPanelState, DocumentsReactPolicy, ReadAudioSource, SteerOffset } from "./device.ts";
export { THEME_TOKENS, appearanceOf, effectiveTheme, themeColorFor } from "./appearance.ts";
export { COLOUR_PRESETS, COLOUR_THEME_WORDS, colourThemeOf } from "./colour.ts";
export { DEFAULT_THEME_CONFIG, EXTRA_DARK_TOKENS, THEME_GROUPS, THEME_TOKEN_NAMES, TOKEN_LABELS, THEME_PRESETS, presetsFor, defaultTokensFor, themeConfigOf, themeConfigRaw, tokensForConfig } from "./themeconfig.ts";
export type { DarkStyle, ThemeConfig } from "./themeconfig.ts";
export type { EffectiveTheme } from "./appearance.ts";
export {
  DEFAULT_CARD_MAX_LINES,
  DEFAULT_DATA_REACH_BACK,
  DEFAULT_RENDER_PREFETCH_PX,
  DEFAULT_RENDER_ROW_CAP,
  MAX_DATA_REACH_BACK,
  MAX_RENDER_PREFETCH_PX,
  MAX_RENDER_ROW_CAP,
  MIN_DATA_REACH_BACK,
  MIN_RENDER_PREFETCH_PX,
  MIN_RENDER_ROW_CAP,
  DEFAULT_MARK_READ_FIRST_PERCENT,
  DEFAULT_MARK_READ_LONG_WORDS,
  DEFAULT_COLLAPSED_LINES,
  DEFAULT_DISCUSS_DOCKED_HEIGHT,
  DEFAULT_DISCUSS_WIDTH,
  DEFAULT_MSG_SCALE,
  DEFAULT_QUOTE_READ_PERCENT,
  DEFAULT_READ_KEEP_ALIVE_MS,
  DEFAULT_READ_PREROLL_MS,
  DEFAULT_READ_STEER_WIDTH_PX,
  DEFAULT_READING_CH,
  DEFAULT_TWO_COLUMN_SPLIT,
  MAX_CARD_MAX_LINES,
  MAX_MARK_READ_FIRST_PERCENT,
  MAX_MARK_READ_LONG_WORDS,
  MAX_COLLAPSED_LINES,
  MAX_DISCUSS_DOCKED_HEIGHT,
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
  MIN_DISCUSS_DOCKED_HEIGHT,
  MIN_DISCUSS_WIDTH,
  MIN_MSG_SCALE,
  MIN_QUOTE_READ_PERCENT,
  MIN_READING_CH,
  MIN_READ_SPEED,
  MIN_TWO_COLUMN_SPLIT,
  clampCardMaxLines,
  clampDataReachBack,
  clampMarkReadFirstPercent,
  clampMarkReadLongWords,
  clampCollapsedLines,
  clampRenderPrefetchPx,
  clampRenderRowCap,
  clampDiscussDockedHeight,
  clampDiscussWidth,
  clampMsgScale,
  clampQuoteReadPercent,
  clampReadKeepAliveMs,
  clampReadPrerollMs,
  clampReadSpeed,
  clampReadSteerWidth,
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
export {
  DATA_BYTES_PER_MESSAGE,
  DATA_MAX_BYTES_DESKTOP,
  DATA_MAX_BYTES_PHONE,
  DATA_REACH_DESKTOP,
  DATA_REACH_PHONE,
  RENDER_CAP_DESKTOP,
  RENDER_CAP_PHONE,
  RENDER_PREFETCH_DESKTOP_PX,
  RENDER_PREFETCH_PHONE_PX,
  SCROLL_MOUNT_BUDGET,
  WINDOWED_LISTS_HELP,
  reachBackExcess,
  windowedConfig,
} from "./windowed.ts";
export type { WindowedConfig } from "./windowed.ts";
