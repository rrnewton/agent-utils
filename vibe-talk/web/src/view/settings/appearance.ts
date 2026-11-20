// Settings' Appearance control (`#1845 vibe-talk-appearance-setting`; `#624 voice-page-modularity`).
// The choice itself lives in core/settings; this owner paints it: the effective theme on the
// document element, the `color-scheme`, the PWA `theme-color`, and the select saying the choice.

import { COLOUR_PRESETS, THEME_TOKENS, colourThemeOf, effectiveTheme, themeColorFor } from "../../core/settings/index.ts";
import type { Appearance, ColourTheme, EffectiveTheme } from "../../core/settings/index.ts";
import { createListening } from "../dom/index.ts";

export interface AppearanceDependencies {
  lookup(id: string): HTMLElement;
  /** The stored choice, already read through the device setting. */
  stored(): Appearance;
  /** Keep the choice on the device (and, through the sync, on the server). */
  keep(choice: Appearance): void;
  /** Whether this device holds exactly this choice: a refusing browser may keep nothing. */
  keptAs(choice: Appearance): boolean;
  /** The stored Colour theme preset (`#1961 vibe-talk-colour-themes`). */
  storedColour(): ColourTheme;
  /** Keep the preset on the device (and, through the sync, on the server). */
  keepColour(choice: ColourTheme): void;
  /** Whether this device holds exactly this preset. */
  keptAsColour(choice: ColourTheme): boolean;
  /** The media query for the system's dark scheme, or null where there is none. */
  systemQuery(): MediaQueryList | null;
}

export interface AppearanceControl {
  /** The choice now in force. */
  choice(): Appearance;
  /** The theme now painted. */
  theme(): EffectiveTheme;
  /** The Colour theme preset now in force. */
  colour(): ColourTheme;
  /** Put `choice` in force and draw the control for it; nothing is kept. */
  apply(choice: Appearance): EffectiveTheme;
  /** Put `preset` in force for the theme now painted and draw its control; nothing is kept. */
  applyColour(preset: ColourTheme): EffectiveTheme;
  /** Put the stored choice in force. */
  restore(): EffectiveTheme;
  unmount(): void;
}

/** Words under the control for each choice, naming what will happen next. */
export function appearanceStateWords(choice: Appearance, kept: boolean): string {
  const base = choice === "system"
    ? "Follows your device's light or dark setting."
    : choice === "light"
      ? "Always light, even when your device is set to dark."
      : "Always dark, even when your device is set to light.";
  return kept ? base : `${base} This device did not keep it, so it lasts only until you leave.`;
}

/** Words under the Colour theme control for each preset, naming what it paints. */
export function colourStateWords(choice: ColourTheme, kept: boolean): string {
  const base = choice === "blue"
    ? "The blue this page has always had."
    : choice === "blue-hc"
      ? "Blue with dark words on the dark scheme's blue, for contrast."
      : "Buttons, links, chips, your rows and the Discuss header in this colour.";
  return kept ? base : `${base} This device did not keep it, so it lasts only until you leave.`;
}

export function mountAppearance(deps: AppearanceDependencies): AppearanceControl {
  const { listen, unmount } = createListening();
  let current: Appearance = deps.stored();
  let currentColour: ColourTheme = deps.storedColour();
  let painted: EffectiveTheme = "dark";

  function systemDark(): boolean {
    try {
      const query = deps.systemQuery();
      return query ? query.matches : true;
    } catch (_error) {
      return true;
    }
  }

  function paint(choice: Appearance): EffectiveTheme {
    const theme = effectiveTheme(choice, systemDark());
    painted = theme;
    const root = document.documentElement;
    root.setAttribute("data-appearance", choice);
    root.setAttribute("data-colour-theme", currentColour);
    root.setAttribute("data-theme", theme);
    if (root.classList && typeof root.classList.toggle === "function") {
      root.classList.toggle("theme-dark", theme === "dark");
      root.classList.toggle("theme-light", theme === "light");
    }
    root.style.colorScheme = theme;
    // A forced choice beats the system's media block: its colours inline, System's cleared.
    if (root.style && typeof root.style.setProperty === "function" && typeof root.style.removeProperty === "function") {
      for (const tokens of Object.values(THEME_TOKENS)) {
        for (const name of Object.keys(tokens)) root.style.removeProperty(name);
      }
      if (choice !== "system") {
        for (const [name, value] of Object.entries(THEME_TOKENS[theme])) root.style.setProperty(name, value);
      }
      // The preset paints after the scheme, on both paths: its family overrides the scheme's.
      for (const [name, value] of Object.entries(COLOUR_PRESETS[currentColour][theme])) root.style.setProperty(name, value);
    }
    try {
      const meta = typeof document.querySelector === "function" ? document.querySelector('meta[name="theme-color"]') : null;
      if (meta) meta.setAttribute("content", themeColorFor(theme));
    } catch (_error) { /* no chrome to colour on this surface */ }
    const select = deps.lookup("appearance") as HTMLSelectElement | null;
    if (select && select.value !== choice) select.value = choice;
    const colourSelect = deps.lookup("colour-theme") as HTMLSelectElement | null;
    if (colourSelect && colourSelect.value !== currentColour) colourSelect.value = currentColour;
    return theme;
  }

  function apply(choice: Appearance): EffectiveTheme {
    current = choice;
    return paint(choice);
  }

  function applyColour(preset: ColourTheme): EffectiveTheme {
    currentColour = preset;
    return paint(current);
  }

  function changed(): void {
    const select = deps.lookup("appearance") as HTMLSelectElement;
    const next: Appearance = select.value === "light" || select.value === "dark" ? select.value : "system";
    apply(next);
    let kept = false;
    try {
      deps.keep(next);
      kept = deps.keptAs(next);
    } catch (_error) {
      kept = false;
    }
    deps.lookup("appearance-state").textContent = appearanceStateWords(next, kept);
  }

  function colourChanged(): void {
    const select = deps.lookup("colour-theme") as HTMLSelectElement;
    const next = colourThemeOf(select.value);
    applyColour(next);
    let kept = false;
    try {
      deps.keepColour(next);
      kept = deps.keptAsColour(next);
    } catch (_error) {
      kept = false;
    }
    deps.lookup("colour-theme-state").textContent = colourStateWords(next, kept);
  }

  const select = deps.lookup("appearance");
  if (select) listen<Event>(select, "change", () => changed());
  const colourSelect = deps.lookup("colour-theme");
  if (colourSelect) listen<Event>(colourSelect, "change", () => colourChanged());
  let query: MediaQueryList | null = null;
  try {
    query = deps.systemQuery();
  } catch (_error) {
    query = null;
  }
  const onSystem = (): void => {
    if (current === "system") paint("system");
  };
  if (query) {
    if (typeof query.addEventListener === "function") query.addEventListener("change", onSystem);
    else if (typeof query.addListener === "function") query.addListener(onSystem);
  }

  return {
    choice: () => current,
    theme: () => painted,
    colour: () => currentColour,
    apply,
    applyColour,
    restore: () => {
      currentColour = deps.storedColour();
      return apply(deps.stored());
    },
    unmount: () => {
      if (query) {
        if (typeof query.removeEventListener === "function") query.removeEventListener("change", onSystem);
        else if (typeof query.removeListener === "function") query.removeListener(onSystem);
      }
      unmount();
    },
  };
}
