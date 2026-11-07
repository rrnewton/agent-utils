// Showing a screen (`#85 voice-desktop-review`, `#224 keyboard-shortcuts`, `#280 scroll-stability`;
// `#624 voice-page-modularity`, moved from the legacy script into view/shell): three sections stacked in
// one grid cell and `hidden` picks, everything that goes with the change — the room past the right
// column's foot, the reader's place in the channel list, a waiting draft's card, the header become a
// title bar, the control pane, a device-voice read, Settings' pop-out and boxes, the jump, the chips,
// the bar, the columns and the focus — in the order the legacy script did it, and the screen itself
// shown LAST (`#280`). Which screen is up, where Settings' and Help's ways out go, and the view the
// main screen shows are the screens' (`screens.ts`); what each thing done here does is the page's.

import { SCREENS } from "./screens.ts";
import type { Screens } from "./screens.ts";

/** What the header calls each screen that is a DESTINATION rather than the app itself. */
const SCREEN_TITLES: Readonly<Record<string, string>> = Object.freeze({ settings: "Settings", help: "Help" });

/** What showing a screen asks of the page. */
export interface ScreenShowingDependencies {
  /** Which screen is up, and a screen arrived at (`Screens.screen`, `Screens.arrive`). */
  readonly screens: Pick<Screens, "screen" | "arrive">;
  /** The page's element by id, in the column being worked on (`el`). */
  el(id: string): HTMLElement;
  /** Close the room past the right column's foot (`closeOverscroll`, `#338 column-overscroll`). */
  closeOverscroll(): void;
  /** Note the reader's message in the channel list, for a reopen (`holdChannelPlace`, `#189 restore-ui-state`). */
  holdChannelPlace(): void;
  /** Put a docked draft's card back at the foot of the main screen (`voiceTranscript().postCardBackToMain`, `#269 approval-card-inline`). */
  postCardBackToMain(): void;
  /** Put a waiting draft on the screen now up (`voiceTranscript().placePostCard`, `#269 approval-card-inline`). */
  placePostCard(): void;
  /** Disarm the transcript's Clear (`voiceTranscript().disarmClear`). */
  disarmClear(): void;
  /** Whether a read aloud is playing in the device's voice (`readAloudPlayback`/`nowPlaying`, `#548`). */
  browserReading(): boolean;
  /** Stop the read aloud (`readAloudOwner().stopReading`). */
  stopReading(): void;
  /** Close Settings' pop-out over Recently dismissed (`dismissedSendsView.close`, `#348 settings-dynamic-lists`). */
  closeDismissedSends(): void;
  /** Draw Settings' Recently dismissed (`dismissedSendsView.render`, `#193 compose-draft-loss`). */
  renderDismissedSends(): void;
  /** Decide the jump to the newest line for the list on screen (`renderJumpNewest`, `#207 scrollback-jump`). */
  renderJumpNewest(): void;
  /** Stand the chips on the channel's message box (`placeChipsOverComposer`, `#245 done-feedback`). */
  placeChipsOverComposer(): void;
  /** Decide what the control bar shows (`renderControlBar`, `#58 control-bar`). */
  renderControlBar(): void;
  /** Decide one column, the thread list, or two (`renderColumns`, `#229 desktop-two-column`). */
  renderColumns(): void;
  /** Open Settings on the service of the channel being read (`channelSettings.pointSettingsAtReading`, `#355 provider-grouped-channels`). */
  pointSettingsAtReading(): void;
  /** Draw the upstream-clear box as it stands (`upstreamClearBox.refresh`, `#232 upstream-mark-read`). */
  refreshUpstreamClear(): void;
  /** Draw the channel-history box as it stands (`channelHistoryBox.refresh`, `#602 history-settings-and-limits`). */
  refreshChannelHistory(): void;
  /** Draw the agent-prefix box as it stands (`agentPrefix.refresh`, `#303 voice-post-prefix`). */
  refreshAgentPrefix(): void;
  /** Draw who is who in the box's channel (`identityList.render`, `#402 who-is-who-channel-box`). */
  renderIdentities(): void;
  /** Whether nothing holds the focus (`focusLost`). */
  focusLost(): boolean;
  /** Give the focus to the list on screen (`settleFocus`, `#224 keyboard-shortcuts`). */
  settleFocus(): void;
  /** Put the back gesture's entries in step with what is open (`scheduleBackSync`, `#304 back-gesture-in-app`). */
  scheduleBackSync(): void;
}

/** Showing a screen's one command. */
export interface ScreenShowing {
  /** Show screen `name`, everything that goes with the change done around it, in order. */
  showScreen(name: string): void;
}

/** Showing as the page has it. */
export function createScreenShowing(deps: ScreenShowingDependencies): ScreenShowing {
  const screens = deps.screens;
  const el = deps.el;

  function showScreen(name: string): void {
    const previous = screens.screen();
    // `#338 column-overscroll`. The room past the right column's foot goes with the main screen: a reader who
    // was in it comes back to the foot.
    if (name !== "main") deps.closeOverscroll();
    // The channel list is about to be hidden, and a hidden list has nothing left to measure: the
    // reader's message is noted now, for a reopen. `#189 restore-ui-state`.
    if (screens.screen() === "main" && name !== "main") deps.holdChannelPlace();
    // `#269 approval-card-inline`. ...and the list about to be shown again is whole when it is first laid out:
    // a draft docked under the screen being left is back at its foot BEFORE it is shown (`postCardBackToMain`).
    if (name === "main" && previous !== "main") deps.postCardBackToMain();
    // `#280 scroll-stability`. Everything going goes FIRST, and the screen coming is shown LAST, once the frame
    // around it is as it will be (below). A browser lays a list on a hidden screen out again the first time it
    // lays the page out after the screen is shown, and clamps the reader's place to the list's window as it is
    // then. Hiding a control that holds the focus is such a time: Chromium blurs it on the spot, and lays the
    // page out to do so. Help's list holds the focus after "?" or Ctrl+?, and Help's Back button after a tap on
    // it, and both were hidden after the main screen was shown and before the control pane was: the list was
    // laid out in a window 66px taller than its own on a 393x852 phone. A reader whose long draft was settled
    // while Help was up was left 58px above the foot, and 200px once the box took back its room, the jump
    // offered. (Settings' ways out let go of the focus before they get here, `closeSettings`.)
    for (const screen of SCREENS) {
      if (screen !== name) el(`screen-${screen}`).hidden = true;
    }
    const main = name === "main";
    // Settings and Reply are both destinations: the header becomes a title bar with a way back, and
    // the controls that act on the screen you have left are absent.
    const destination = Object.prototype.hasOwnProperty.call(SCREEN_TITLES, name);
    // Two separate ways back, because they go to two different places — and because
    // scripts/screenshots.py drives #close-settings by name.
    el("close-settings").hidden = name !== "settings";
    el("close-help").hidden = name !== "help";
    el("topbar-title").hidden = !destination;
    if (destination) {
      el("topbar-title").textContent = SCREEN_TITLES[name] ?? "";
    }
    // Which screen is up, where the stylesheet can see it.
    el("frame-body").setAttribute("data-screen", name);
    // The control pane is a grid ROW inside the dock. Hiding it collapses the row, so the body grows
    // to fill the frame rather than leaving a band of empty pane under a sign-in form.
    //
    // The status line is not touched here any more. It used to be kept visible on every screen so a
    // sign-in failure could say so — but a sign-in failure is shown by the `#error` panel, which is
    // permanent and is on whichever screen is up, and the sign-in screen states what to paste in its
    // own body. `#63 status-line-placement`.
    el("control-pane").hidden = !main;
    // ...and only now the screen itself (`#280 scroll-stability`, above).
    el(`screen-${name}`).hidden = false;
    if (!main) {
      // And a device-voice read Read with device voice started under the cloud voice (`#548`).
      if (deps.browserReading()) deps.stopReading();
      deps.disarmClear();
    }
    // `#348 settings-dynamic-lists`. The pop-out over Recently dismissed is Settings', and goes with it.
    if (name !== "settings") deps.closeDismissedSends();
    // Help is reached FROM settings and returns TO it, so it must not become the thing settings
    // remembers to go back to — that would make the gear's way out lead into the document the
    // reader just left. `#85 voice-desktop-review`.
    // `#224 keyboard-shortcuts`. Help left by any way at all (its Back, Esc, or the search or Settings
    // key, which leave it as Settings' Back does) forgets where `?` opened it from: the next Help, from
    // a `?` in Settings, goes back to Settings. Here rather than in each way out, so that a way out
    // added later cannot leave a stale answer behind (`Screens.arrive`).
    screens.arrive(name, previous);
    // The jump is about the list on the main screen, and is gone with it. `#207 scrollback-jump`. Back on
    // the main screen it is decided at the end, on the list as it is then laid out (below).
    if (!main) deps.renderJumpNewest();
    // ...and so is the channel's message box the chips stand on. `#245 done-feedback`.
    deps.placeChipsOverComposer();
    // LAST, and after `currentScreen` is set: the bar decides what it shows from the screen that is
    // now up, and it is also what collapses the header when nothing is left in it. `#58 control-bar`.
    deps.renderControlBar();
    // `#224 keyboard-shortcuts`. A new screen is a new list for the scroll keys, so its scroller takes
    // the focus, which the control that brought the reader here would otherwise keep (the gear) or
    // drop (a Back button, which has just hidden itself), leaving PgDn and Space to the document. Only
    // on a real change, or with the focus already lost: a sign-in that lands on the screen already up
    // must not pull the reader out of the box they have started typing in.
    // `#229 desktop-two-column`. Back on the main screen, two columns are decided again: the window may
    // have been resized behind Settings.
    if (main) deps.renderColumns();
    // `#269 approval-card-inline`. A waiting draft follows the owner to the screen now up.
    deps.placePostCard();
    // `#207 scrollback-jump` x `#280 scroll-stability`. The jump over the list now shown, decided once the
    // header has collapsed (`renderControlBar`) and the columns and the card are in place. Decided as the
    // screen was un-hidden, it measured the list under the header the screen being left kept, 54px shorter
    // at a desk and 8px on a phone: a reader 40px above the foot read as 94px up, past `SCROLLBACK_PX`, and
    // was offered the jump, whose own room under the box then held it there.
    if (main) deps.renderJumpNewest();
    // `#355 provider-grouped-channels`. Settings opens on the service of the channel being read; not on
    // the way back from Help, which is the same visit, where the reader's choice stands.
    if (name === "settings" && previous !== "settings" && previous !== "help") deps.pointSettingsAtReading();
    // `#232 upstream-mark-read`. The server may have pushed since the box was last drawn.
    if (name === "settings" && previous !== "settings") deps.refreshUpstreamClear();
    // `#602 history-settings-and-limits`. The walk has gone on since the box was last drawn.
    if (name === "settings" && previous !== "settings") deps.refreshChannelHistory();
    // `#303 voice-post-prefix`. Another device may have saved or reset the tag since sign-in.
    if (name === "settings" && previous !== "settings") deps.refreshAgentPrefix();
    // `#193 compose-draft-loss`. Its ages and channel names as they are now, and none past its days.
    if (name === "settings" && previous !== "settings") deps.renderDismissedSends();
    // `#402 who-is-who-channel-box`. Who is who in the box's channel, as the census stands now.
    if (name === "settings" && previous !== "settings") deps.renderIdentities();
    if (name !== previous || deps.focusLost()) deps.settleFocus();
    // `#304 back-gesture-in-app`. A screen opened has an entry to go back through, and one left has none.
    deps.scheduleBackSync();
  }

  return { showScreen };
}

// What the page and the tests use of it.
export { SCREEN_TITLES };
