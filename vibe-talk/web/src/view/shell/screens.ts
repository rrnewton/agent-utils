// Which screen is up and which view (`#85 voice-desktop-review`, `#224 keyboard-shortcuts`; `#624
// voice-page-modularity`, moved from the legacy script into view/shell): the screen shown (sign-in, the main
// screen, Settings, Help), the screen Settings' way out goes back to, where Back on Help goes, and the view
// the main screen shows (the call's or the channel's). Showing and hiding the screens and panes, and what goes
// with each change, are view/shell's and the page's (`createScreenShowing`, `showView`).

/** The screens: three sections stacked in one grid cell, and `hidden` picks. */
const SCREENS: readonly string[] = Object.freeze(["signin", "main", "settings", "help"]);

/** The screens' commands and questions. */
export interface Screens {
  /** The screen shown now. */
  screen(): string;
  /** The screen Settings' way out goes back to: never Settings or Help themselves. */
  beforeSettings(): string;
  /** Where Back on Help goes: Settings, unless `?` opened it from elsewhere. */
  helpReturn(): string;
  /** Help is being opened from `screen`, which its Back goes back to. */
  setHelpReturn(screen: string): void;
  /**
   * Screen `name` is now the one shown, `previous` the one shown as the change began. Help is reached from
   * Settings and returns to it, so neither becomes the place Settings goes back to (`#85
   * voice-desktop-review`); and Help left by any way at all forgets where `?` opened it from (`#224
   * keyboard-shortcuts`), so a way out added later leaves no stale answer.
   */
  arrive(name: string, previous: string): void;
  /** The view the main screen shows: "voice" for the call's, "discord" for the channel's. */
  view(): string;
  /** View `name` is now the one shown. */
  setView(name: string): void;
}

/** On the sign-in screen, the call's view behind it. */
export function createScreens(): Screens {
  let screenBeforeSettings = "signin";
  let currentScreen = "signin";
  /**
   * Where Back on Help goes: Settings, which is where Help is opened from, unless `?` opened it
   * (`keysFromKeyboard`). Put back to Settings whenever Help is left (`arrive`).
   */
  let helpReturn = "settings";
  let currentView = "voice";

  return {
    screen: () => currentScreen,
    beforeSettings: () => screenBeforeSettings,
    helpReturn: () => helpReturn,
    setHelpReturn(screen) {
      helpReturn = screen;
    },
    arrive(name, previous) {
      if (name !== "settings" && name !== "help") {
        screenBeforeSettings = name;
      }
      if (previous === "help" && name !== "help") helpReturn = "settings";
      currentScreen = name;
    },
    view: () => currentView,
    setView(name) {
      currentView = name;
    },
  };
}

// What the page and the tests use of it.
export { SCREENS };
