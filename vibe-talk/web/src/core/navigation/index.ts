// Where the reader is: the selected channel, its one owner, the places on a channel and their
// addresses. Its public interface. `#624 voice-page-modularity`.

export { createNavigation } from "./navigation.ts";
export type { Navigation, NavigationSnapshot } from "./navigation.ts";
export { placeIsOver, visitKey } from "./places.ts";
export type { ChannelPlace, Place } from "./places.ts";
export { DEEP_LINK_VIEWS, deepLinkParams, deepLinkPart, deepLinkQuery } from "./links.ts";
export { REOPENED_VIEWS, UI_STATE_CHANNELS, UI_STATE_CHARS, UI_STATE_KEY, UI_STATE_VERSION, createUiState, keptChoices, validUiState } from "./uistate.ts";
export type { ChosenView, HeldPlace, SavedPlace, SavedThread, UiRecord, UiState, UiStateDependencies } from "./uistate.ts";
