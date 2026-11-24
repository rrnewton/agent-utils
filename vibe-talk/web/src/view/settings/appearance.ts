// Settings' Appearance control (`#1845 vibe-talk-appearance-setting`; `#624 voice-page-modularity`).
// The choice itself lives in core/settings; this owner paints it: the effective theme on the
// document element, the `color-scheme`, the PWA `theme-color`, and the select saying the choice.

import { COLOUR_PRESETS, DEFAULT_THEME_CONFIG, THEME_GROUPS, THEME_TOKEN_NAMES, TOKEN_LABELS, THEME_TOKENS, colourThemeOf, defaultTokensFor, effectiveTheme, themeColorFor, tokensForConfig } from "../../core/settings/index.ts";
import type { Appearance, ColourTheme, EffectiveTheme, ThemeConfig } from "../../core/settings/index.ts";
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
  /** The stored theme configuration for a scheme (`#2155`). Optional: absent means Default. */
  storedTheme?(scheme: EffectiveTheme): ThemeConfig;
  keepTheme?(scheme: EffectiveTheme, config: ThemeConfig): void;
  keptAsTheme?(scheme: EffectiveTheme, config: ThemeConfig): boolean;
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
  /** The theme configuration in force for a scheme (`#2155`). */
  config(scheme: EffectiveTheme): ThemeConfig;
  /** The tokens painted (or to paint) for a scheme, Custom applied. */
  tokens(scheme: EffectiveTheme): Readonly<Record<string, string>>;
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

/** Words under the theme editor, naming the scheme being edited and its mode. */
export function themeStateWords(scheme: EffectiveTheme, config: ThemeConfig, kept: boolean): string {
  const base = config.mode === "custom"
    ? `Editing the ${scheme} theme: your own colours, started from the Default.`
    : scheme === "dark" && config.style === "extra"
      ? "Editing the dark theme: the Extra dark Default (true black)."
      : `Editing the ${scheme} theme: the Default colours. Pick Custom to make them your own.`;
  return kept ? base : `${base} This device did not keep it, so it lasts only until you leave.`;
}

export function mountAppearance(deps: AppearanceDependencies): AppearanceControl {
  const { listen, unmount } = createListening();
  let current: Appearance = deps.stored();
  let currentColour: ColourTheme = deps.storedColour();
  let painted: EffectiveTheme = "dark";
  let paintedTokens: Readonly<Record<string, string>> = THEME_TOKENS.dark;
  const configs: Record<EffectiveTheme, ThemeConfig> = {
    light: deps.storedTheme ? deps.storedTheme("light") : DEFAULT_THEME_CONFIG,
    dark: deps.storedTheme ? deps.storedTheme("dark") : DEFAULT_THEME_CONFIG,
  };
  let editScheme: EffectiveTheme = "dark";

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
      for (const name of THEME_TOKEN_NAMES) root.style.removeProperty(name);
      if (choice !== "system") {
        for (const [name, value] of Object.entries(THEME_TOKENS[theme])) root.style.setProperty(name, value);
      }
      // The preset paints after the scheme, on both paths: its family overrides the scheme's.
      for (const [name, value] of Object.entries(COLOUR_PRESETS[currentColour][theme])) root.style.setProperty(name, value);
      // The scheme's own configuration paints last (`#2155`): Extra over the Default,
      // or the reader's Custom tokens over both.
      const effective = tokensForConfig(configs[theme], theme, currentColour);
      if (configs[theme].mode === "custom" || (theme === "dark" && configs[theme].style === "extra")) {
        for (const [name, value] of Object.entries(effective)) root.style.setProperty(name, value);
      }
      paintedTokens = effective;
    }
    try {
      const meta = typeof document.querySelector === "function" ? document.querySelector('meta[name="theme-color"]') : null;
      if (meta) meta.setAttribute("content", paintedTokens["--bg"] ?? themeColorFor(theme));
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


  // ---- Theme editor (`#2155 vibe-talk-theme-settings-pane`) ----
  function keepConfig(scheme: EffectiveTheme): boolean {
    if (!deps.keepTheme || !deps.keptAsTheme) return true;
    try {
      deps.keepTheme(scheme, configs[scheme]);
      return deps.keptAsTheme(scheme, configs[scheme]);
    } catch (_error) {
      return false;
    }
  }

  function renderEditor(): void {
    const cfg = configs[editScheme];
    const lightBtn = deps.lookup("theme-edit-light") as HTMLButtonElement | null;
    const darkBtn = deps.lookup("theme-edit-dark") as HTMLButtonElement | null;
    if (lightBtn) lightBtn.setAttribute("aria-pressed", String(editScheme === "light"));
    if (darkBtn) darkBtn.setAttribute("aria-pressed", String(editScheme === "dark"));
    const modeSelect = deps.lookup("theme-mode") as HTMLSelectElement | null;
    if (modeSelect) {
      const picker = cfg.mode === "custom" ? "custom" : editScheme === "dark" && cfg.style === "extra" ? "extra" : "default";
      if (modeSelect.value !== picker) modeSelect.value = picker;
      const extraOption = modeSelect.options ? modeSelect.options[1] as HTMLOptionElement | undefined : undefined;
      if (extraOption) extraOption.disabled = editScheme !== "dark";
    }
    const resetBtn = deps.lookup("theme-reset") as HTMLButtonElement | null;
    if (resetBtn) { resetBtn.disabled = cfg.mode !== "custom"; resetBtn.textContent = "Reset custom to Default"; }
    const host = deps.lookup("theme-groups");
    if (host && typeof document !== "undefined" && typeof document.createElement === "function") {
      const effective = tokensForConfig(cfg, editScheme, currentColour);
      host.textContent = "";
      for (const group of THEME_GROUPS) {
        // Code colours are set by the Default/preset, not edited in Custom v1 (owner 352).
        if (group.id === "code") continue;
        const box = document.createElement("fieldset");
        box.className = "theme-group";
        const legend = document.createElement("legend");
        legend.textContent = group.title;
        box.appendChild(legend);
        for (const name of group.tokens) {
          const row = document.createElement("label");
          row.className = "theme-token";
          const word = document.createElement("span");
          word.textContent = TOKEN_LABELS[name] ?? name;
          word.title = name;
          const value = effective[name] ?? "";
          const input = document.createElement("input");
          input.setAttribute("data-token", name);
          input.setAttribute("aria-label", `${group.title}: ${name}`);
          if (/^#[0-9a-f]{6}$/i.test(value)) {
            input.type = "color";
            input.value = value;
          } else {
            input.type = "text";
            input.value = value;
          }
          input.disabled = cfg.mode !== "custom";
          row.append(word, input);
          box.appendChild(row);
        }
        host.appendChild(box);
      }
    }
    const state = deps.lookup("theme-state");
    if (state) state.textContent = themeStateWords(editScheme, cfg, true);
  }

  function editorChanged(kept: boolean): void {
    if (editScheme === painted) paint(current);
    renderEditor();
    const state = deps.lookup("theme-state");
    if (state) state.textContent = themeStateWords(editScheme, configs[editScheme], kept);
  }

  function seedCustom(scheme: EffectiveTheme): void {
    const cfg = configs[scheme];
    configs[scheme] = { mode: "custom", style: cfg.style, tokens: { ...defaultTokensFor(scheme, currentColour, cfg.style) } };
  }

  const editLight = deps.lookup("theme-edit-light");
  if (editLight) listen<Event>(editLight, "click", () => { editScheme = "light"; renderEditor(); });
  const editDark = deps.lookup("theme-edit-dark");
  if (editDark) listen<Event>(editDark, "click", () => { editScheme = "dark"; renderEditor(); });
  const modeSelectEl = deps.lookup("theme-mode");
  if (modeSelectEl) listen<Event>(modeSelectEl, "change", () => {
    const picked = (modeSelectEl as HTMLSelectElement).value;
    if (picked === "custom") {
      if (Object.keys(configs[editScheme].tokens).length === 0) seedCustom(editScheme);
      else configs[editScheme] = { ...configs[editScheme], mode: "custom" };
    } else {
      configs[editScheme] = { ...configs[editScheme], mode: "default", style: picked === "extra" ? "extra" : "standard" };
    }
    editorChanged(keepConfig(editScheme));
  });
  let resetArmed = false;
  const resetEl = deps.lookup("theme-reset") as HTMLButtonElement | null;
  if (resetEl) listen<Event>(resetEl, "click", () => {
    if (!resetArmed) {
      resetArmed = true;
      resetEl.textContent = "Tap again to reset to Default";
      return;
    }
    resetArmed = false;
    resetEl.textContent = "Reset custom to Default";
    seedCustom(editScheme);
    editorChanged(keepConfig(editScheme));
  });
  const groupsHost = deps.lookup("theme-groups");
  if (groupsHost) listen<Event>(groupsHost, "input", (event: Event) => {
    const input = event.target as HTMLInputElement | null;
    const name = input && typeof input.getAttribute === "function" ? input.getAttribute("data-token") : undefined;
    if (!input || !name || configs[editScheme].mode !== "custom") return;
    configs[editScheme] = { ...configs[editScheme], tokens: { ...configs[editScheme].tokens, [name]: input.value } };
    editorChanged(keepConfig(editScheme));
  });

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
      if (deps.storedTheme) {
        configs.light = deps.storedTheme("light");
        configs.dark = deps.storedTheme("dark");
      }
      editScheme = painted;
      const theme = apply(deps.stored());
      editScheme = theme;
      renderEditor();
      return theme;
    },
    config: (scheme: EffectiveTheme) => configs[scheme],
    tokens: (scheme: EffectiveTheme) => tokensForConfig(configs[scheme], scheme, currentColour),
    unmount: () => {
      if (query) {
        if (typeof query.removeEventListener === "function") query.removeEventListener("change", onSystem);
        else if (typeof query.removeListener === "function") query.removeListener(onSystem);
      }
      unmount();
    },
  };
}
