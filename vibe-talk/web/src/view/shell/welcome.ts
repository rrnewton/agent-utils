// The first-run Welcome (`#1781 vibe-talk-first-run-welcome-channel-default`): a short screen that
// gives a new reader the map of the app in a minute, shown once on first launch and re-openable
// from Settings and Help ever after. Why each surface earns a line, and the order they stand in,
// follows how the app is actually used day to day: the Channel is home, and Voice is one tool
// among several for keeping up with it.
//
// With no channel configured, the same screen is the guided setup: the SETUP guide's steps
// (vibe-talk/SETUP.md, `#1780`) shown where the reader already is, the config snippet to copy,
// and a Check again that re-reads the channel list through the page's existing client-config
// path. Channels come from the server's configuration today, so the Welcome creates nothing:
// it says exactly what to change, and notices when it has changed.

/** Where the welcome's dismissed mark lives on this device. */
export const WELCOME_DISMISSED_KEY = "vibe-talk.welcome.dismissed";

/** The config snippet the guided step offers to copy, placeholder id and all. */
export const WELCOME_CONFIG_SNIPPET = "[[channels]]\nid = \"CHANNEL_ID\"\nlabel = \"My channel\"\nwritable = true";

/** What the Welcome asks of the page. */
export interface WelcomeDependencies {
  /** The page's element by id (`el`). */
  el(id: string): HTMLElement;
  /** Show a screen ("welcome", "main" or "settings"). */
  showScreen(name: string): void;
  /** Whether this device has dismissed the Welcome before. */
  dismissed(): boolean;
  /** Remember on this device that the Welcome was dismissed. */
  markDismissed(): void;
  /** Whether the page holds a saved UI record: a returning reader's own evidence. */
  hasSavedRecord(): boolean;
  /** Whether the page was opened by an explicit link, whose destination outranks the Welcome. */
  hasDeepLink(): boolean;
  /** Whether any channel is configured yet. */
  hasChannel(): boolean;
  /** Copy `text` for the reader; answers whether it was copied. */
  copyText(text: string): Promise<boolean>;
  /** Re-read the channel list from the server; answers whether one is configured now. */
  checkAgain(): Promise<boolean>;
  /** Read the channel now selected, as entering it does. */
  readChannel(): void;
  /** Land the reader on the channel, now the main screen is visible again. */
  landOnChannel(): void;
}

/** The Welcome's commands. */
export interface Welcome {
  /**
   * Show the Welcome on a genuine first run only: no dismissed mark and no saved record.
   * Returns whether it was shown. A returning reader is never blocked by it.
   */
  maybeShowFirstRun(): boolean;
  /** Show the Welcome because the reader asked for it (Settings or Help). */
  showWelcome(): void;
  /** Leave the Welcome for the main screen, and remember that on this device. */
  dismiss(): void;
  /** Copy the guided step's config snippet, and say so on the screen either way. */
  copyConfig(): Promise<void>;
  /** Re-read the channel list; land on the Channel when one has arrived. */
  recheck(): Promise<void>;
}

/** The Welcome, over `deps`. */
export function createWelcome(deps: WelcomeDependencies): Welcome {
  /** Draw the guided step as the channel list now stands: shown only while there is none. */
  function render(): void {
    deps.el("welcome-guide").hidden = deps.hasChannel();
  }

  function showWelcome(): void {
    render();
    deps.showScreen("welcome");
  }

  function dismiss(): void {
    deps.markDismissed();
    deps.showScreen("main");
    deps.landOnChannel();
  }

  return {
    maybeShowFirstRun() {
      if (deps.dismissed() || deps.hasSavedRecord() || deps.hasDeepLink()) return false;
      showWelcome();
      return true;
    },
    showWelcome,
    dismiss,
    async copyConfig() {
      const copied = await deps.copyText(WELCOME_CONFIG_SNIPPET);
      deps.el("welcome-guide-state").textContent = copied
        ? "Copied. Paste it into the server's configuration, then restart."
        : "Copy failed — select the snippet above and copy it by hand.";
    },
    async recheck() {
      deps.el("welcome-guide-state").textContent = "Checking…";
      if (await deps.checkAgain()) {
        deps.readChannel();
        dismiss();
      } else {
        render();
        deps.el("welcome-guide-state").textContent =
          "Still no channel. If the app opens with no channels, the configuration it loaded has none that the startup probe could read — check the steps above, then try again.";
      }
    },
  };
}
