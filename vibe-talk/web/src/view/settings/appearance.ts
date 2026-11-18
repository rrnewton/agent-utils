// Settings' Appearance control (`#1845 vibe-talk-appearance-setting`; `#624 voice-page-modularity`).
// The choice itself lives in core/settings; this owner paints it: the effective theme on the
// document element, the `color-scheme`, the PWA `theme-color`, and the select saying the choice.

import { THEME_TOKENS, effectiveTheme, themeColorFor } from "../../core/settings/index.ts";
import type { Appearance, EffectiveTheme } from "../../core/settings/index.ts";
import { createListening } from "../dom/index.ts";

export interface AppearanceDependencies {
  lookup(id: string): HTMLElement;
  /** The stored choice, already read through the device setting. */
  stored(): Appearance;
  /** Keep the choice on the device (and, through the sync, on the server). */
  keep(choice: Appearance): void;
  /** Whether this device holds exactly this choice: a refusing browser may keep nothing. */
  keptAs(choice: Appearance): boolean;
  /** The media query for the system's dark scheme, or null where there is none. */
  systemQuery(): MediaQueryList | null;
}

export interface AppearanceControl {
  /** The choice now in force. */
  choice(): Appearance;
  /** The theme now painted. */
  theme(): EffectiveTheme;
  /** Put `choice` in force and draw the control for it; nothing is kept. */
  apply(choice: Appearance): EffectiveTheme;
  /** Put the stored choice in force. */
  restore(): EffectiveTheme;
  unmount(): void;
}

/** Words under the control for each choice, naming what will happen next. */
export function appearanceStateWords(choice: Appearance, kept: boolean): string {
  const base = choice === "system"
    ? "Following your system. It changes when your system does."
    : choice === "light" ? "Light, whatever your system says." : "Dark, whatever your system says.";
  return kept ? base : `${base} This device did not keep it, so it lasts only until you leave.`;
}

export function mountAppearance(deps: AppearanceDependencies): AppearanceControl {
  const { listen, unmount } = createListening();
  let current: Appearance = deps.stored();
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
    }
    try {
      const picker = deps.lookup("thread-select") as HTMLElement | null;
      if (picker) picker.style.backgroundImage = choice === "dark" ? "none" : "";
    } catch (_error) { /* no picker on this surface */ }
    try {
      const meta = typeof document.querySelector === "function" ? document.querySelector('meta[name="theme-color"]') : null;
      if (meta) meta.setAttribute("content", themeColorFor(theme));
    } catch (_error) { /* no chrome to colour on this surface */ }
    const select = deps.lookup("appearance") as HTMLSelectElement | null;
    if (select && select.value !== choice) select.value = choice;
    return theme;
  }

  function apply(choice: Appearance): EffectiveTheme {
    current = choice;
    return paint(choice);
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

  const select = deps.lookup("appearance");
  if (select) listen<Event>(select, "change", () => changed());
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
    apply,
    restore: () => apply(deps.stored()),
    unmount: () => {
      if (query) {
        if (typeof query.removeEventListener === "function") query.removeEventListener("change", onSystem);
        else if (typeof query.removeListener === "function") query.removeListener(onSystem);
      }
      unmount();
    },
  };
}
