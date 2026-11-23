// The Appearance choice resolved to the theme in force (`#1845 vibe-talk-appearance-setting`;
// `#624 voice-page-modularity`). Pure: what a stored value means and what the system says are
// both handed in, so the pre-paint script in the shell and this module cannot drift on the
// choice itself — only on the one line each uses to ask the system.

import type { Appearance } from "./device.ts";

/** The theme actually painted: the choice, or the system's where the choice is System. */
export type EffectiveTheme = "light" | "dark";

/** What a kept value means: Light or Dark as kept, anything else System. */
export function appearanceOf(raw: string | null): Appearance {
  return raw === "light" || raw === "dark" ? raw : "system";
}

/** The theme in force for `choice` where the system is dark exactly when `systemDark`. */
export function effectiveTheme(choice: Appearance, systemDark: boolean): EffectiveTheme {
  if (choice === "light") return "light";
  if (choice === "dark") return "dark";
  return systemDark ? "dark" : "light";
}

/** The PWA chrome colour for a theme: each scheme's `--bg` (web/style.css). */
export function themeColorFor(theme: EffectiveTheme): string {
  return theme === "dark" ? "#14161a" : "#f6f7f9";
}

/** Each scheme's `:root` colours (web/style.css and web/styles/voice/00-frame.css), set inline
 * on the document element for a forced choice so they beat the system's media block without
 * a second declaration of them in the sheets. System clears them and the sheets decide. */
export const THEME_TOKENS: Readonly<Record<EffectiveTheme, Readonly<Record<string, string>>>> = Object.freeze({
  dark: Object.freeze({
    "--bg": "#14161a", "--panel": "#1d2026", "--edge": "#2c313a", "--ink": "#e7e9ee",
    "--muted": "#99a1b0", "--accent": "#5b9dd9", "--warn": "#d98a5b",
    "--mine": "#000000", "--mine-edge": "#33587b", "--muted-on-mine": "#b4bcc8",
    "--warn-on-mine": "#e8a27a", "--theirs": "#000000", "--third": "#000000",
    "--third-edge": "#3b3529", "--dock": "#191c22", "--ok": "#5bd98a", "--idle": "#6b7484",
    "--paused": "#9b8cf0", "--row-hover": "#262c36", "--summary-fail-ink": "#ff9aa2",
    "--pin-ink": "#f0c75e", "--pin-edge": "#d4a63a", "--pin-bg": "#3a3017",
    "--hit-bg": "#6b5410", "--hit-ink": "#fff4c7", "--reply-tint": "#181e25",
    "--read-ring": "#99a1b0", "--thread-grey": "#626975",
  }),
  light: Object.freeze({
    "--bg": "#f6f7f9", "--panel": "#ffffff", "--edge": "#d9dde4", "--ink": "#1a1d22",
    "--muted": "#5d6674", "--accent": "#24618f", "--warn": "#9a5416",
    "--mine": "#dbeaf8", "--mine-edge": "#b4d0e8", "--muted-on-mine": "#525b68",
    "--warn-on-mine": "#9a5416", "--theirs": "#ffffff", "--third": "#faf6ee",
    "--third-edge": "#e8e0cf", "--dock": "#eef1f5", "--ok": "#1f8f4c", "--idle": "#98a1b0",
    "--paused": "#5b48c4", "--row-hover": "#edf1f7", "--summary-fail-ink": "#8f1d28",
    "--pin-ink": "#7a5300", "--pin-edge": "#c08b14", "--pin-bg": "#fbf0d2",
    "--hit-bg": "#ffe066", "--hit-ink": "#1a1d22", "--reply-tint": "#e9eef3",
    "--read-ring": "#7f8896", "--thread-grey": "#9ba1ac",
  }),
});
