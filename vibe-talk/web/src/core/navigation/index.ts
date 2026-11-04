// Where the reader is: the selected channel, its one owner, the places on a channel and their
// addresses. Its public interface. `#624 voice-page-modularity`.

export { createNavigation } from "./navigation.ts";
export type { Navigation, NavigationSnapshot } from "./navigation.ts";
export { placeIsOver, visitKey } from "./places.ts";
export type { ChannelPlace, Place } from "./places.ts";
export { DEEP_LINK_VIEWS, deepLinkParams, deepLinkPart, deepLinkQuery } from "./links.ts";
export { REOPENED_VIEWS, UI_STATE_CHANNELS, UI_STATE_CHARS, UI_STATE_KEY, UI_STATE_VERSION, createUiState, keptChoices, validUiState } from "./uistate.ts";
export type { ChosenView, HeldPlace, SavedPlace, SavedThread, UiRecord, UiState, UiStateDependencies } from "./uistate.ts";
export { PLACES_KEPT, createVisits } from "./visits.ts";
export type { Visits } from "./visits.ts";
export { createDeepLinks } from "./deeplinks.ts";
export type { DeepLinkWanted, DeepLinks, DeepLinksDependencies } from "./deeplinks.ts";
export { createLinkJumps, ownLevel } from "./jumps.ts";
export type { LinkJump, LinkJumps, LinkSpot } from "./jumps.ts";
export { BACK_DEPTH_MAX, BACK_STATE_KEY, BACK_TRAVERSAL_MS, createBackStack } from "./backstack.ts";
export type { BackEntryFacts, BackHistoryPort, BackStack, BackStackDependencies } from "./backstack.ts";
export { COLUMNS_KEY, LAYOUT_BEFORE_KEY, createLayoutChoice } from "./layout.ts";
export type { Layout, LayoutChoice, LayoutChoiceDependencies, ListBefore } from "./layout.ts";
export { createViewPlaces } from "./viewplaces.ts";
export type { ViewPlace, ViewPlaces } from "./viewplaces.ts";
