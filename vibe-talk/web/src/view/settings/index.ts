// The Settings and Help screens as the DOM draws them: their folds, and in time their boxes. Its public
// interface. `#624 voice-page-modularity`, plan step 16.

export { mountSettingsFolds } from "./folds.ts";
export type { SettingsFolds, SettingsFoldsDependencies } from "./folds.ts";
export { mountHelpFolds } from "./help.ts";
export type { HelpFolds, HelpFoldsDependencies } from "./help.ts";
export { mountChannelHistory } from "./history.ts";
export type { ChannelHistoryBox, ChannelHistoryDependencies } from "./history.ts";
export { mountUpstreamClear } from "./upstream.ts";
export type { UpstreamClearBox, UpstreamClearDependencies } from "./upstream.ts";
export { mountChannelDirectory } from "./directory.ts";
export type { ChannelDirectory, ChannelDirectoryDependencies } from "./directory.ts";
export { mountDismissedSends } from "./dismissed.ts";
export type { DismissedSendsView, DismissedSendsViewDependencies } from "./dismissed.ts";
export { mountServerCache } from "./cache.ts";
export type { ServerCache, ServerCacheDependencies } from "./cache.ts";
export { mountIdentityList } from "./identities.ts";
export type { IdentityList, IdentityListDependencies } from "./identities.ts";
export { mountPairing } from "./pairing.ts";
export type { Pairing, PairingDependencies } from "./pairing.ts";
export { mountReadingWidth, mountTextSize } from "./screen.ts";
export type { ReadingWidth, ScreenSettingDependencies, TextSize } from "./screen.ts";
export { mountBarPlacement } from "./placement.ts";
export type { BarPlacement, BarPlacementDependencies } from "./placement.ts";
