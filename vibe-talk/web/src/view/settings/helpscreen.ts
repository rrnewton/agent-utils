// Help's way in (`#85 voice-desktop-review`; `#624 voice-page-modularity`, moved from the legacy
// script into view/settings beside help.ts): the Help screen shown, opened at one entry. The folds
// themselves are mountHelpFolds'; which screen is up is the page's, handed in.

/** What Help's way in asks of the page. */
export interface HelpScreenDependencies {
  /** Show a screen ("help" for this one). */
  showScreen(name: string): unknown;
  /** Every Help entry folded but `slug`'s; none open for a null slug. */
  openAt(slug: string | null): void;
}

/** Help's way in: its named command. */
export interface HelpScreen {
  /**
   * Open Help, optionally at one entry. A null slug opens the top, which is what the standing
   * "Open help" button at the head of Settings wants.
   */
  showHelp(slug: string | null): void;
}

/** Made once, where the page wires its Help. */
export function mountHelpScreen(deps: HelpScreenDependencies): HelpScreen {
  return {
    showHelp(slug) {
      deps.showScreen("help");
      deps.openAt(slug);
    },
  };
}
