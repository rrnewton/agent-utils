// Settings' message preferences and the readouts for recap and live delivery
// (`#624 voice-page-modularity`). Feature values, redraw algorithms, and transport state remain
// with their owners. These controls draw their applied values and call the same feature commands.

import {
  DEVICE_SETTINGS,
  MAX_CARD_MAX_LINES,
  MAX_COLLAPSED_LINES,
  MAX_QUOTE_READ_PERCENT,
  MIN_CARD_MAX_LINES,
  MIN_COLLAPSED_LINES,
  MIN_QUOTE_READ_PERCENT,
  clampCardMaxLines,
  clampCollapsedLines,
  clampQuoteReadPercent,
} from "../../core/settings/index.ts";
import type { DeviceSettings } from "../../core/settings/index.ts";
import {
  MESSAGE_PREFERENCE_WORDS,
  cardMaxLinesSavedWords,
  collapsedLinesSavedWords,
  connectionDetailWords,
  liveStateWords,
  quoteReadPercentSavedWords,
  resumeStateWords,
  resumeUnavailableWords,
} from "../../present/settings/index.ts";
import type { LiveReadout, ResumeReadout } from "../../present/settings/index.ts";

/** The channel and message owners' commands; none is called during construction. */
export interface MessagePreferencesDependencies {
  lookup(id: string): HTMLElement;
  deviceSettings(): DeviceSettings;
  status(text: string): void;
  persistCollapsedLines(n: number): boolean;
  changeCollapsedLines(value: number): number;
  persistCardMaxLines(n: number): boolean;
  changeCardMaxLines(value: number): number;
  applyQuoteReadPercent(value: unknown): number;
  applyMarkReadOnReply(on: boolean): void;
  applyMarkReadQuoteGuard(on: boolean): void;
  applyMarkReadFoldGuard(on: boolean): void;
  applyMarkReadNonquote(on: boolean): void;
  applyMarkReadFinalGuard(on: boolean): void;
  applyMarkReadFirstGuard(on: boolean): void;
  applyMarkReadFirstPercent(value: unknown): number;
  applyMarkReadLongWords(value: unknown): number;
  applyInAppLinks(on: boolean): void;
  redrawMessageLinks(): void;
  applySummarizeCollapsed(on: boolean): void;
  applyRichHeaders(on: boolean): void;
  applyMarkOwnRead(on: boolean): void;
  applyHideEmptyMessages(on: boolean): void;
  /** Existing hide-empty redraw/read algorithm, run after its value is applied and before its status. */
  redrawHiddenEmptyMessages(): void;
  applyCombineMessages(on: boolean): void;
  regroupChannelRows(): void;
  chooseOpenInView(view: string): void;
  storedOpenInView(): string;
  chooseAfterDone(choice: string): void;
  storedAfterDone(): string;
}

/** A value rendered by its feature, and changed through that feature's command. */
export interface BooleanPreferenceView {
  render(on: boolean): void;
  changed(): void;
}

/** Slider presentation, saving feedback, and the two buttons which use the slider's own path. */
export interface MessageLimitView {
  render(n: number): void;
  saved(n: number): void;
  step(direction: 1 | -1): void;
}

export interface QuoteReadPreferenceView {
  render(n: number): void;
  changed(value: unknown): number;
  step(direction: 1 | -1): void;
}

export interface MarkReadPreferenceView {
  render(enabled: boolean, quoteGuard: boolean, foldGuard: boolean, nonquote: boolean, finalGuard: boolean, firstGuard: boolean): void;
  changed(): void;
  quoteGuardChanged(): void;
  foldGuardChanged(): void;
  nonquoteChanged(): void;
  finalGuardChanged(): void;
  firstGuardChanged(): void;
  renderFirstPercent(n: number): void;
  firstPercentChanged(value: unknown): number;
  stepFirstPercent(direction: 1 | -1): void;
  renderLongWords(n: number): void;
  longWordsChanged(value: unknown): number;
  stepLongWords(direction: 1 | -1): void;
}

export interface MessagePreferences {
  readonly collapsedLines: MessageLimitView;
  readonly cardMaxLines: MessageLimitView;
  readonly quoteReadPercent: QuoteReadPreferenceView;
  readonly markRead: MarkReadPreferenceView;
  readonly inAppLinks: BooleanPreferenceView;
  readonly summaries: BooleanPreferenceView;
  readonly richHeaders: BooleanPreferenceView;
  readonly markOwnRead: BooleanPreferenceView;
  readonly hideEmpty: BooleanPreferenceView;
  readonly combine: BooleanPreferenceView;
  readonly openInView: { render(): void; changed(): void };
  readonly afterDone: { render(): void; changed(): void };
}

/** No setting is read, applied, kept, or listened to until the page calls its existing path. */
export function mountMessagePreferences(deps: MessagePreferencesDependencies): MessagePreferences {
  function renderCollapsedLines(n: number): void {
    // One root property reaches every list. At zero, the attribute makes CSS hide the folded body:
    // a clamp of zero alone would remove the clamp and show every line.
    document.documentElement.style.setProperty("--collapsed-lines", String(n));
    document.documentElement.setAttribute("data-collapsed-lines", String(n));
    (deps.lookup("collapsed-lines") as HTMLInputElement).value = String(n);
    (deps.lookup("collapsed-lines-smaller") as HTMLButtonElement).disabled = n <= MIN_COLLAPSED_LINES;
    (deps.lookup("collapsed-lines-larger") as HTMLButtonElement).disabled = n >= MAX_COLLAPSED_LINES;
  }

  function sayCollapsedLinesSaved(n: number): void {
    deps.lookup("collapsed-lines-state").textContent = collapsedLinesSavedWords(n, deps.persistCollapsedLines(n));
  }

  /** The slider's own change path, with focus returned to it when the pressed button becomes disabled. */
  function stepCollapsedLines(direction: 1 | -1): void {
    const button = deps.lookup(direction > 0 ? "collapsed-lines-larger" : "collapsed-lines-smaller") as HTMLButtonElement;
    const held = document.activeElement === button;
    deps.changeCollapsedLines(clampCollapsedLines((deps.lookup("collapsed-lines") as HTMLInputElement).value) + direction);
    if (held && button.disabled) deps.lookup("collapsed-lines").focus();
  }

  function renderCardMaxLines(n: number): void {
    // `#528 thread-card-markdown-and-cap`: what a card clamped to K lines is clamped to (web/voice.css).
    document.documentElement.style.setProperty("--card-max-lines", String(n));
    (deps.lookup("card-max-lines") as HTMLInputElement).value = String(n);
    (deps.lookup("card-max-lines-smaller") as HTMLButtonElement).disabled = n <= MIN_CARD_MAX_LINES;
    (deps.lookup("card-max-lines-larger") as HTMLButtonElement).disabled = n >= MAX_CARD_MAX_LINES;
  }

  function sayCardMaxLinesSaved(n: number): void {
    deps.lookup("card-max-lines-state").textContent = cardMaxLinesSavedWords(n, deps.persistCardMaxLines(n));
  }

  /** One line fewer or more, through the feature's scroll-preserving change path. */
  function stepCardMaxLines(direction: 1 | -1): void {
    const button = deps.lookup(direction > 0 ? "card-max-lines-larger" : "card-max-lines-smaller") as HTMLButtonElement;
    const held = document.activeElement === button;
    deps.changeCardMaxLines(clampCardMaxLines((deps.lookup("card-max-lines") as HTMLInputElement).value) + direction);
    if (held && button.disabled) deps.lookup("card-max-lines").focus();
  }

  function renderQuoteReadPercent(n: number): void {
    (deps.lookup("quote-read-percent") as HTMLInputElement).value = String(n);
    (deps.lookup("quote-read-percent-smaller") as HTMLButtonElement).disabled = n <= MIN_QUOTE_READ_PERCENT;
    (deps.lookup("quote-read-percent-larger") as HTMLButtonElement).disabled = n >= MAX_QUOTE_READ_PERCENT;
  }

  /** The share is applied by the reply feature, then kept and read back: private browsing may keep nothing. */
  function quoteReadPercentChanged(value: unknown): number {
    const n = deps.applyQuoteReadPercent(value);
    let kept = false;
    try {
      deps.deviceSettings().keep(DEVICE_SETTINGS.quoteReadPercent, n);
      kept = deps.deviceSettings().keptAs(DEVICE_SETTINGS.quoteReadPercent, n);
    } catch (_error) {
      kept = false;
    }
    deps.lookup("quote-read-percent-state").textContent = quoteReadPercentSavedWords(n, kept);
    return n;
  }

  function stepQuoteReadPercent(direction: 1 | -1): void {
    const button = deps.lookup(direction > 0 ? "quote-read-percent-larger" : "quote-read-percent-smaller") as HTMLButtonElement;
    const held = document.activeElement === button;
    quoteReadPercentChanged(clampQuoteReadPercent((deps.lookup("quote-read-percent") as HTMLInputElement).value) + direction);
    if (held && button.disabled) deps.lookup("quote-read-percent").focus();
  }

  function renderMarkRead(enabled: boolean, quoteGuard: boolean, foldGuard: boolean, nonquote: boolean, finalGuard: boolean, firstGuard: boolean): void {
    (deps.lookup("mark-read-on-reply") as HTMLSelectElement).value = enabled ? "yes" : "no";
    deps.lookup("mark-read-options").hidden = !enabled;
    (deps.lookup("mark-read-quote-guard") as HTMLInputElement).checked = quoteGuard;
    (deps.lookup("mark-read-fold-guard") as HTMLInputElement).checked = foldGuard;
    (deps.lookup("mark-read-nonquote") as HTMLInputElement).checked = nonquote;
    (deps.lookup("mark-read-final-guard") as HTMLInputElement).checked = finalGuard;
    (deps.lookup("mark-read-first-guard") as HTMLInputElement).checked = firstGuard;
    (deps.lookup("quote-read-percent") as HTMLInputElement).disabled = !quoteGuard;
    (deps.lookup("quote-read-percent-smaller") as HTMLButtonElement).disabled = !quoteGuard
      || clampQuoteReadPercent((deps.lookup("quote-read-percent") as HTMLInputElement).value) <= MIN_QUOTE_READ_PERCENT;
    (deps.lookup("quote-read-percent-larger") as HTMLButtonElement).disabled = !quoteGuard
      || clampQuoteReadPercent((deps.lookup("quote-read-percent") as HTMLInputElement).value) >= MAX_QUOTE_READ_PERCENT;
    (deps.lookup("mark-read-first-percent") as HTMLInputElement).disabled = !firstGuard;
    (deps.lookup("mark-read-long-words") as HTMLInputElement).disabled = !firstGuard;
  }

  function markReadSwitches(): [boolean, boolean, boolean, boolean, boolean, boolean] {
    return [
      (deps.lookup("mark-read-on-reply") as HTMLSelectElement).value === "yes",
      (deps.lookup("mark-read-quote-guard") as HTMLInputElement).checked,
      (deps.lookup("mark-read-fold-guard") as HTMLInputElement).checked,
      (deps.lookup("mark-read-nonquote") as HTMLInputElement).checked,
      (deps.lookup("mark-read-final-guard") as HTMLInputElement).checked,
      (deps.lookup("mark-read-first-guard") as HTMLInputElement).checked,
    ];
  }

  function markReadChanged(): void {
    const enabled = (deps.lookup("mark-read-on-reply") as HTMLSelectElement).value === "yes";
    deps.applyMarkReadOnReply(enabled);
    renderMarkRead(...markReadSwitches());
    deps.lookup("mark-read-state").textContent = enabled
      ? "Saved \u2014 a sent reply marks the message it answers read, unless a choice below holds it back."
      : "Saved \u2014 a sent reply leaves the message it answers unread.";
    deps.status(enabled ? "a sent reply marks what it answers read." : "a sent reply leaves what it answers unread.");
  }

  function markReadQuoteGuardChanged(): void {
    const on = (deps.lookup("mark-read-quote-guard") as HTMLInputElement).checked;
    deps.applyMarkReadQuoteGuard(on);
    renderMarkRead(...markReadSwitches());
    deps.status(on ? "a reply quoting too little leaves its message unread." : "a reply marks its message read whatever it quotes.");
  }

  function markReadFoldGuardChanged(): void {
    const on = (deps.lookup("mark-read-fold-guard") as HTMLInputElement).checked;
    deps.applyMarkReadFoldGuard(on);
    deps.status(on ? "a reply sent in Fold read leaves its message unread." : "a reply sent in Fold read marks its message read.");
  }

  function markReadNonquoteChanged(): void {
    const on = (deps.lookup("mark-read-nonquote") as HTMLInputElement).checked;
    deps.applyMarkReadNonquote(on);
    deps.status(on ? "a reply that quotes nothing marks its message read." : "a reply that quotes nothing leaves its message unread.");
  }

  function markReadFinalGuardChanged(): void {
    const on = (deps.lookup("mark-read-final-guard") as HTMLInputElement).checked;
    deps.applyMarkReadFinalGuard(on);
    deps.status(on ? "a quote before the final paragraph leaves its message unread." : "a quote before the final paragraph marks its message read.");
  }

  function markReadFirstGuardChanged(): void {
    const on = (deps.lookup("mark-read-first-guard") as HTMLInputElement).checked;
    deps.applyMarkReadFirstGuard(on);
    renderMarkRead(...markReadSwitches());
    deps.status(on ? "a quote in the first share of a long message leaves it unread." : "a quote in the first share of a long message marks it read.");
  }

  function renderMarkReadFirstPercent(n: number): void {
    (deps.lookup("mark-read-first-percent") as HTMLInputElement).value = String(n);
    (deps.lookup("mark-read-first-percent-smaller") as HTMLButtonElement).disabled = n <= 0;
    (deps.lookup("mark-read-first-percent-larger") as HTMLButtonElement).disabled = n >= 100;
  }

  function markReadFirstPercentChanged(value: unknown): number {
    const n = deps.applyMarkReadFirstPercent(value);
    let kept = false;
    try {
      deps.deviceSettings().keep(DEVICE_SETTINGS.markReadFirstPercent, n);
      kept = deps.deviceSettings().keptAs(DEVICE_SETTINGS.markReadFirstPercent, n);
    } catch (_error) { kept = false; }
    deps.lookup("mark-read-first-percent-state").textContent = kept
      ? `Saved \u2014 the first ${n}% of a long message.`
      : `This browser refused to store it, so it will be forgotten when you reload. Until then the first ${n}% of a long message.`;
    return n;
  }

  function stepMarkReadFirstPercent(direction: 1 | -1): void {
    const button = deps.lookup(direction > 0 ? "mark-read-first-percent-larger" : "mark-read-first-percent-smaller") as HTMLButtonElement;
    const held = document.activeElement === button;
    const now = Number((deps.lookup("mark-read-first-percent") as HTMLInputElement).value) || 0;
    markReadFirstPercentChanged(now + direction);
    if (held && button.disabled) deps.lookup("mark-read-first-percent").focus();
  }

  function renderMarkReadLongWords(n: number): void {
    (deps.lookup("mark-read-long-words") as HTMLInputElement).value = String(n);
    (deps.lookup("mark-read-long-words-smaller") as HTMLButtonElement).disabled = n <= 0;
    (deps.lookup("mark-read-long-words-larger") as HTMLButtonElement).disabled = n >= 5000;
  }

  function markReadLongWordsChanged(value: unknown): number {
    const n = deps.applyMarkReadLongWords(value);
    let kept = false;
    try {
      deps.deviceSettings().keep(DEVICE_SETTINGS.markReadLongWords, n);
      kept = deps.deviceSettings().keptAs(DEVICE_SETTINGS.markReadLongWords, n);
    } catch (_error) { kept = false; }
    deps.lookup("mark-read-long-words-state").textContent = kept
      ? `Saved \u2014 a message over ${n} words is long.`
      : `This browser refused to store it, so it will be forgotten when you reload. Until then a message over ${n} words is long.`;
    return n;
  }

  function stepMarkReadLongWords(direction: 1 | -1): void {
    const button = deps.lookup(direction > 0 ? "mark-read-long-words-larger" : "mark-read-long-words-smaller") as HTMLButtonElement;
    const held = document.activeElement === button;
    const now = Number((deps.lookup("mark-read-long-words") as HTMLInputElement).value) || 0;
    markReadLongWordsChanged(now + direction * 10);
    if (held && button.disabled) deps.lookup("mark-read-long-words").focus();
  }

  return {
    collapsedLines: { render: renderCollapsedLines, saved: sayCollapsedLinesSaved, step: stepCollapsedLines },
    cardMaxLines: { render: renderCardMaxLines, saved: sayCardMaxLinesSaved, step: stepCardMaxLines },
    quoteReadPercent: { render: renderQuoteReadPercent, changed: quoteReadPercentChanged, step: stepQuoteReadPercent },
    markRead: {
      render: renderMarkRead, changed: markReadChanged, quoteGuardChanged: markReadQuoteGuardChanged,
      foldGuardChanged: markReadFoldGuardChanged, nonquoteChanged: markReadNonquoteChanged,
      finalGuardChanged: markReadFinalGuardChanged, firstGuardChanged: markReadFirstGuardChanged,
      renderFirstPercent: renderMarkReadFirstPercent, firstPercentChanged: markReadFirstPercentChanged,
      stepFirstPercent: stepMarkReadFirstPercent, renderLongWords: renderMarkReadLongWords,
      longWordsChanged: markReadLongWordsChanged, stepLongWords: stepMarkReadLongWords,
    },
    inAppLinks: {
      render(on) { (deps.lookup("in-app-links") as HTMLInputElement).checked = on; },
      changed() {
        deps.applyInAppLinks((deps.lookup("in-app-links") as HTMLInputElement).checked);
        deps.redrawMessageLinks();
        deps.status(MESSAGE_PREFERENCE_WORDS.inAppLinks((deps.lookup("in-app-links") as HTMLInputElement).checked));
      },
    },
    summaries: {
      render(on) { (deps.lookup("summarize-collapsed") as HTMLInputElement).checked = on; },
      changed() {
        // The feature owner puts the chip over the list in step with this choice.
        deps.applySummarizeCollapsed((deps.lookup("summarize-collapsed") as HTMLInputElement).checked);
        deps.status(MESSAGE_PREFERENCE_WORDS.summaries((deps.lookup("summarize-collapsed") as HTMLInputElement).checked));
      },
    },
    richHeaders: {
      render(on) {
        (deps.lookup("rich-headers") as HTMLInputElement).checked = on;
        document.documentElement.setAttribute("data-rich-headers", on ? "on" : "off");
      },
      changed() {
        // Nothing to redraw: the root attribute moves every heading in both columns at once.
        deps.applyRichHeaders((deps.lookup("rich-headers") as HTMLInputElement).checked);
        deps.status(MESSAGE_PREFERENCE_WORDS.richHeaders((deps.lookup("rich-headers") as HTMLInputElement).checked));
      },
    },
    markOwnRead: {
      render(on) { (deps.lookup("mark-own-read") as HTMLInputElement).checked = on; },
      changed() {
        // `#549 own-read-stored`. Nothing on screen changes: the setting decides where messages first seen
        // from now on start, and "Apply to existing messages" is the act that reaches the rest.
        deps.applyMarkOwnRead((deps.lookup("mark-own-read") as HTMLInputElement).checked);
      },
    },
    hideEmpty: {
      render(on) { (deps.lookup("hide-empty-messages") as HTMLInputElement).checked = on; },
      changed() {
        deps.applyHideEmptyMessages((deps.lookup("hide-empty-messages") as HTMLInputElement).checked);
        deps.redrawHiddenEmptyMessages();
        deps.status(MESSAGE_PREFERENCE_WORDS.hideEmpty((deps.lookup("hide-empty-messages") as HTMLInputElement).checked));
      },
    },
    combine: {
      render(on) { (deps.lookup("combine-messages") as HTMLInputElement).checked = on; },
      changed() {
        deps.applyCombineMessages((deps.lookup("combine-messages") as HTMLInputElement).checked);
        // REDRAWN FROM WHAT IS ALREADY HERE, not re-read: the rows carry their own messages, so the
        // setting costs no round trip and cannot lose a walk back through the channel. `renderChannelRows`
        // alone would not do — the grouping decides how many ROWS there are, which is a rebuild.
        deps.regroupChannelRows();
        deps.status(MESSAGE_PREFERENCE_WORDS.combine((deps.lookup("combine-messages") as HTMLInputElement).checked));
      },
    },
    openInView: {
      render() { (deps.lookup("open-in-view") as HTMLSelectElement).value = deps.storedOpenInView(); },
      changed() {
        deps.chooseOpenInView((deps.lookup("open-in-view") as HTMLSelectElement).value);
        deps.status(MESSAGE_PREFERENCE_WORDS.openInView(deps.storedOpenInView()));
      },
    },
    afterDone: {
      render() { (deps.lookup("after-done") as HTMLSelectElement).value = deps.storedAfterDone(); },
      changed() {
        deps.chooseAfterDone((deps.lookup("after-done") as HTMLSelectElement).value);
        deps.status(MESSAGE_PREFERENCE_WORDS.afterDone(deps.storedAfterDone()));
      },
    },
  };
}

export interface ResumePreferenceDependencies {
  lookup(id: string): HTMLElement;
  readonly read: ResumeReadout;
  renderControls(): void;
}

export interface ResumePreference {
  render(): void;
  renderSwitch(): void;
}

/** The switch and state line read the voice owner's state; they never reset a recap outcome. */
export function mountResumePreference(deps: ResumePreferenceDependencies): ResumePreference {
  /**
   * Resuming's switch: live only where this server can resume, and otherwise disabled, unticked, with
   * its reason beside it. `#331 resume-on-websocket`: the owner found it a live checkbox on a server
   * that could not resume — a control that did nothing — with only the line above to say so.
   *
   * Unticked while disabled, so it never shows a choice that is not in force; the choice itself is
   * kept, and comes back where the server can act on it.
   */
  function renderResumeSwitch(): void {
    const toggle = deps.lookup("resume-toggle") as HTMLInputElement;
    const why = deps.lookup("resume-unavailable");
    toggle.disabled = !deps.read.allowed;
    toggle.checked = deps.read.allowed && deps.read.wanted();
    why.hidden = deps.read.allowed;
    why.textContent = deps.read.allowed ? "" : resumeUnavailableWords(deps.read.replayUnavailable);
  }

  function renderResumeState(): void {
    const state = deps.lookup("resume-state");
    if (!state) {
      return;
    }
    renderResumeSwitch();
    state.textContent = resumeStateWords(deps.read);
    deps.renderControls();
  }

  return { render: renderResumeState, renderSwitch: renderResumeSwitch };
}

export interface ConnectionReadoutsDependencies {
  lookup(id: string): HTMLElement;
  readonly live: LiveReadout;
}

export interface ConnectionReadouts {
  detail(text: string): void;
  /** Draw only Settings' line. The caller first updates the channel freshness pill, as before. */
  live(): void;
}

/** No live stream or timer is started here. */
export function mountConnectionReadouts(deps: ConnectionReadoutsDependencies): ConnectionReadouts {
  return {
    detail(text) {
      // `#348 settings-dynamic-lists`: one part per line, while the banner keeps one line.
      deps.lookup("settings-detail").textContent = connectionDetailWords(text);
    },
    live() {
      const state = deps.lookup("live-state");
      if (!state) {
        return;
      }
      state.textContent = liveStateWords(deps.live);
    },
  };
}
