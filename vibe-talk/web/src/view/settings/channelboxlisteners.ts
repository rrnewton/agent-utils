// The channel box's listeners (`#349 channel-settings-all`, `#355 provider-grouped-channels`, `#232
// upstream-mark-read`, `#602 history-settings-and-limits`, `#39 channel-alias`; `#624
// voice-page-modularity`, moved from the legacy script's wiring into view/settings for the plan's step
// 17): the pickers, Clear unread and History, Rename and Add and Browse, Remove and the alias, the noise
// rules, the voice agent's editors and prefix, and the message cache's switch and erase question. The
// guarded acts are handed in already guarded, as they were evaluated at mount. What each does is the
// page's; mounted once, in the page's order, and taken down again.

import { createListening } from "../dom/index.ts";

export interface ChannelBoxListenersDependencies {
  /** An element of the page by id. */
  lookup(id: string): HTMLElement;
  /** A channel chosen on the bar's picker, in this order: the selection's first, the change started, then Ctrl+K's word. */
  selectChannel(): unknown;
  changeSelectedChannel(): unknown;
  channelPicked(): unknown;
  /** The Settings pickers: another service, then another channel (its panels closed, its box drawn again). */
  chooseSettingsService(): unknown;
  changeSettingsChannel(): unknown;
  /** Clear unread and History: a mode chosen only shows what Save would do; Save, Prioritize and Push decide. */
  renderClearChoice(): unknown;
  saveClear(): unknown;
  renderHistoryChoice(): unknown;
  saveHistory(): unknown;
  prioritizeHistory(): unknown;
  pushClear(): unknown;
  /** Rename and Add: opened (their field focused), cancelled (the typing undone), and added. */
  openRename(): unknown;
  cancelRename(): unknown;
  openAddChannel(): unknown;
  cancelAddChannel(): unknown;
  addChannel(): unknown;
  /** Browse: opened, closed, searched as typed and on Enter, paged and retried. */
  openBrowse(): unknown;
  closeBrowse(): unknown;
  scheduleDirectorySearch(): unknown;
  directorySearchKey(event: KeyboardEvent): unknown;
  directoryMore(): unknown;
  directoryRetry(): unknown;
  /** Remove the channel, and the alias saved or cleared. */
  removeChannel(): unknown;
  saveAlias(): unknown;
  clearAlias(): unknown;
  /** The noise rules: one added by its button, or by Enter in its field. */
  addNoiseRule(): unknown;
  noiseRuleKey(event: KeyboardEvent): unknown;
  /** The voice agent: ask-before-send kept, the prompts redrawn as typed and saved or restored, the prefix likewise. */
  saveSendSetting(): unknown;
  renderPrompt(): unknown;
  saveInstructions(): unknown;
  saveSystemPrompt(): unknown;
  restoreSystemPrompt(): unknown;
  renderPrefix(): unknown;
  savePrefix(): unknown;
  resetPrefix(): unknown;
  /** The message cache: switched, Erase asked, and its question answered. */
  cacheSwitched(): unknown;
  askErase(): unknown;
  answerEraseYes(): unknown;
  answerEraseNo(): unknown;
}

/** The listeners, mounted. */
export interface ChannelBoxListeners {
  /** Take every listener down again. */
  unmount(): void;
}

/** Mount the channel box's listeners, in the page's order. */
export function mountChannelBoxListeners(deps: ChannelBoxListenersDependencies): ChannelBoxListeners {
  const { listen, unmount } = createListening();
  const el = (id: string): HTMLElement => deps.lookup(id);
  // A channel the reader chose on the picker is the selection's first, before anything that reads it
  // hears of the change.
  listen<Event>(el("discord-channel"), "change", () => deps.selectChannel());
  listen<Event>(el("discord-channel"), "change", () => deps.changeSelectedChannel());
  // `#224 keyboard-shortcuts`. After the change above has started: a channel chosen from Ctrl+K.
  listen<Event>(el("discord-channel"), "change", () => deps.channelPicked());
  // `#355 provider-grouped-channels`. Another service: its channels, its facts, its Add, Browse and list.
  listen<Event>(el("settings-service"), "change", () => deps.chooseSettingsService());
  listen<Event>(el("settings-channel"), "change", () => deps.changeSettingsChannel());
  // `#232 upstream-mark-read`. Choosing a mode only shows what Save would do; Save is the decision.
  listen<Event>(el("upstream-clear-mode"), "change", () => deps.renderClearChoice());
  listen<Event>(el("upstream-clear-save"), "click", () => deps.saveClear());
  listen<Event>(el("channel-history-mode"), "change", () => deps.renderHistoryChoice());
  listen<Event>(el("channel-history-save"), "click", () => deps.saveHistory());
  listen<Event>(el("channel-history-prioritize"), "click", () => deps.prioritizeHistory());
  listen<Event>(el("upstream-clear-push"), "click", () => deps.pushClear());
  listen<Event>(el("rename-channel"), "click", () => deps.openRename());
  listen<Event>(el("cancel-rename"), "click", () => deps.cancelRename());
  listen<Event>(el("open-add-channel"), "click", () => deps.openAddChannel());
  listen<Event>(el("cancel-add-channel"), "click", () => deps.cancelAddChannel());
  listen<Event>(el("add-channel"), "click", () => deps.addChannel());
  listen<Event>(el("open-browse-channels"), "click", () => deps.openBrowse());
  listen<Event>(el("close-browse-channels"), "click", () => deps.closeBrowse());
  listen<Event>(el("channel-directory-search"), "input", () => deps.scheduleDirectorySearch());
  listen<KeyboardEvent>(el("channel-directory-search"), "keydown", (event) => deps.directorySearchKey(event));
  listen<Event>(el("channel-directory-more"), "click", () => deps.directoryMore());
  listen<Event>(el("channel-directory-retry"), "click", () => deps.directoryRetry());
  listen<Event>(el("remove-channel"), "click", () => deps.removeChannel());
  listen<Event>(el("save-alias"), "click", () => deps.saveAlias());
  listen<Event>(el("add-noise-rule"), "click", () => deps.addNoiseRule());
  listen<Event>(el("voice-ask-before-send"), "change", () => deps.saveSendSetting());
  listen<Event>(el("voice-instructions"), "input", () => deps.renderPrompt());
  listen<Event>(el("voice-system-prompt"), "input", () => deps.renderPrompt());
  listen<Event>(el("save-voice-instructions"), "click", () => deps.saveInstructions());
  listen<Event>(el("save-voice-system-prompt"), "click", () => deps.saveSystemPrompt());
  listen<Event>(el("restore-voice-system-prompt"), "click", () => deps.restoreSystemPrompt());
  listen<Event>(el("voice-post-prefix"), "input", () => deps.renderPrefix());
  listen<Event>(el("save-voice-post-prefix"), "click", () => deps.savePrefix());
  listen<Event>(el("reset-voice-post-prefix"), "click", () => deps.resetPrefix());
  // `#208 persistent-cache`. Off and Erase only ask; the question's own button is what sends.
  listen<Event>(el("message-cache-enabled"), "change", () => deps.cacheSwitched());
  listen<Event>(el("message-cache-erase"), "click", () => deps.askErase());
  listen<Event>(el("message-cache-confirm-yes"), "click", () => deps.answerEraseYes());
  listen<Event>(el("message-cache-confirm-no"), "click", () => deps.answerEraseNo());
  listen<KeyboardEvent>(el("noise-rule-new"), "keydown", (event) => deps.noiseRuleKey(event));
  listen<Event>(el("clear-alias"), "click", () => deps.clearAlias());
  return { unmount };
}
