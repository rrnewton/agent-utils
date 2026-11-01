// The Settings and Help screens as the DOM draws them: their folds, and in time their boxes. Its public
// interface. `#624 voice-page-modularity`, plan step 16.

export { mountSettingsFolds } from "./folds.ts";
export type { SettingsFolds, SettingsFoldsDependencies } from "./folds.ts";
export { mountHelpFolds } from "./help.ts";
export type { HelpFolds, HelpFoldsDependencies } from "./help.ts";
export { mountChannelHistory } from "./history.ts";
export type { ChannelHistoryBox, ChannelHistoryDependencies } from "./history.ts";
