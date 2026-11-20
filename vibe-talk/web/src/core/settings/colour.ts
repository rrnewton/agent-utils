// The Colour theme choice (`#1961 vibe-talk-colour-themes`): which preset paints the page's
// accent family, resolved for the theme in force. Pure, as `appearance.ts` is: what a stored
// value means is handed in, so the pre-paint script in the shell and this module cannot drift
// on the choice itself. The preset tables are the single home of each family's colours; the
// sheets name only the variables, and the default `blue` family is token-for-token the look
// the page has always had (its `--accent`, `--mine` and neighbours are the values already in
// `THEME_TOKENS` and the sheets), so a device that never picks a preset paints as before.

import type { ColourTheme } from "./device.ts";
import type { EffectiveTheme } from "./appearance.ts";

/** What a kept value means: a known preset as kept, anything else (or nothing) the default blue. */
export function colourThemeOf(raw: string | null): ColourTheme {
  return raw === "teal" || raw === "plum" || raw === "amber" || raw === "blue-hc" ? raw : "blue";
}

/** The accent family a preset paints, per effective theme. `--on-accent` is the text colour on
 * an `--accent` ground; `--discuss-head-bg` is the Discuss bar header's ground (`#1960`), which
 * the default preset keeps at the panel colour until the owner picks a family that tints it. */
export const COLOUR_PRESETS: Readonly<Record<ColourTheme, Readonly<Record<EffectiveTheme, Readonly<Record<string, string>>>>>> = Object.freeze({
  blue: Object.freeze({
    dark: Object.freeze({
      "--accent": "#5b9dd9", "--on-accent": "#ffffff", "--accent-bright": "#5b9dd9",
      "--mine": "#24405c", "--mine-edge": "#33587b", "--muted-on-mine": "#b4bcc8",
      "--row-hover": "#262c36", "--reply-tint": "#181e25", "--discuss-head-bg": "#1d2026",
    }),
    light: Object.freeze({
      "--accent": "#24618f", "--on-accent": "#ffffff", "--accent-bright": "#5b9dd9",
      "--mine": "#dbeaf8", "--mine-edge": "#b4d0e8", "--muted-on-mine": "#525b68",
      "--row-hover": "#edf1f7", "--reply-tint": "#e9eef3", "--discuss-head-bg": "#ffffff",
    }),
  }),
  // Blue with the one AA repair the default keeps as a documented exception: dark ink on the
  // dark scheme's bright accent (6.3:1) instead of white on it (2.9:1). Every other token is
  // blue's, so the owner can compare exactly that one choice in the screenshots.
  "blue-hc": Object.freeze({
    dark: Object.freeze({
      "--accent": "#5b9dd9", "--on-accent": "#14161a", "--accent-bright": "#5b9dd9",
      "--mine": "#24405c", "--mine-edge": "#33587b", "--muted-on-mine": "#b4bcc8",
      "--row-hover": "#262c36", "--reply-tint": "#181e25", "--discuss-head-bg": "#1d2026",
    }),
    light: Object.freeze({
      "--accent": "#24618f", "--on-accent": "#ffffff", "--accent-bright": "#5b9dd9",
      "--mine": "#dbeaf8", "--mine-edge": "#b4d0e8", "--muted-on-mine": "#525b68",
      "--row-hover": "#edf1f7", "--reply-tint": "#e9eef3", "--discuss-head-bg": "#ffffff",
    }),
  }),
  teal: Object.freeze({
    dark: Object.freeze({
      "--accent": "#4cc2b0", "--on-accent": "#14161a", "--accent-bright": "#7ad8c8",
      "--mine": "#173f3a", "--mine-edge": "#24594f", "--muted-on-mine": "#b4d8d2",
      "--row-hover": "#253833", "--reply-tint": "#152220", "--discuss-head-bg": "#1d4a42",
    }),
    light: Object.freeze({
      "--accent": "#0f766e", "--on-accent": "#ffffff", "--accent-bright": "#3aa79b",
      "--mine": "#d7f0ea", "--mine-edge": "#a9d8ce", "--muted-on-mine": "#3f5a55",
      "--row-hover": "#e6f4f0", "--reply-tint": "#e2efec", "--discuss-head-bg": "#d9f0ea",
    }),
  }),
  plum: Object.freeze({
    dark: Object.freeze({
      "--accent": "#cf9fe0", "--on-accent": "#14161a", "--accent-bright": "#e2bff0",
      "--mine": "#3a244d", "--mine-edge": "#53356b", "--muted-on-mine": "#d3bce0",
      "--row-hover": "#332a3d", "--reply-tint": "#211a28", "--discuss-head-bg": "#4a2d63",
    }),
    light: Object.freeze({
      "--accent": "#7e3a9e", "--on-accent": "#ffffff", "--accent-bright": "#b07cc6",
      "--mine": "#f0e2f7", "--mine-edge": "#d3b3e6", "--muted-on-mine": "#5d4a68",
      "--row-hover": "#f4ecf9", "--reply-tint": "#efe6f5", "--discuss-head-bg": "#ecd9f7",
    }),
  }),
  amber: Object.freeze({
    dark: Object.freeze({
      "--accent": "#e8b34b", "--on-accent": "#14161a", "--accent-bright": "#f2cc7e",
      "--mine": "#4a3517", "--mine-edge": "#6b4d22", "--muted-on-mine": "#e6cf9e",
      "--row-hover": "#38312a", "--reply-tint": "#28211a", "--discuss-head-bg": "#5c421c",
    }),
    light: Object.freeze({
      "--accent": "#8a5a00", "--on-accent": "#ffffff", "--accent-bright": "#c08b14",
      "--mine": "#f7ead0", "--mine-edge": "#e8cf9e", "--muted-on-mine": "#66552f",
      "--row-hover": "#faf3e0", "--reply-tint": "#f5ecd7", "--discuss-head-bg": "#f7ead0",
    }),
  }),
});

/** Words naming a preset in the picker's own voice, for the state line and the options. */
export const COLOUR_THEME_WORDS: Readonly<Record<ColourTheme, string>> = Object.freeze({
  blue: "Blue",
  "blue-hc": "Blue, high contrast",
  teal: "Teal",
  plum: "Plum",
  amber: "Amber",
});
