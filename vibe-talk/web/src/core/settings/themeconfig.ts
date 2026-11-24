// Per-scheme theme configuration (`#2155 vibe-talk-theme-settings-pane`). Pure, as
// `appearance.ts`/`colour.ts` are: the stored string is handed in and the tokens to paint
// are handed back, so the pre-paint script, the Settings editor and this module cannot
// drift on what a kept value means. Each scheme (light/dark) is independently Default or
// Custom; Custom is seeded from that scheme's effective Default (scheme tokens, the
// Colour preset in force, and — dark only — the Extra dark variant where chosen) and
// then owns every token in `THEME_GROUPS`. The Dark side also carries its Default style:
// `standard` (the legacy dark) or `extra` (true-black, `EXTRA_DARK_TOKENS`).

import { COLOUR_PRESETS } from "./colour.ts";
import { THEME_TOKENS } from "./appearance.ts";
import type { EffectiveTheme } from "./appearance.ts";
import type { ColourTheme } from "./colour.ts";

/** Which Default the Dark side paints where its mode is Default. */
export type DarkStyle = "standard" | "extra";

/** One scheme's kept configuration. `tokens` is the Custom token map (seeded from the
 * Default, then reader-edited); it is kept even in Default mode so returning to Custom
 * restores the reader's colours. Absent tokens fall back to the Default token. */
export interface ThemeConfig {
  readonly mode: "default" | "custom";
  readonly style: DarkStyle;
  /** The premade Default in force (`THEME_PRESETS`); `legacy` is the scheme's own look. */
  readonly preset: string;
  readonly tokens: Readonly<Record<string, string>>;
}

export const DEFAULT_THEME_CONFIG: ThemeConfig = Object.freeze({
  mode: "default",
  style: "standard",
  preset: "legacy",
  tokens: Object.freeze({}),
});

/** Premade themes (`#2155` owner 352): the scheme's own legacy look, Extra dark, and
 * palettes modelled on the survey's compared apps (colours only, descriptive names).
 * Each paints the full token set for its scheme, code colours included. A preset with
 * no entry for a scheme is not offered there. */
export interface ThemePreset {
  readonly id: string;
  readonly title: string;
  readonly schemes: Partial<Record<EffectiveTheme, Record<string, string>>>;
}

export const THEME_PRESETS: readonly ThemePreset[] = [
  { id: "legacy", title: "Default", schemes: {} },
  { id: "extra", title: "Extra dark (true black)", schemes: { dark: {} } },
  { id: "discord-dark", title: "Discord-like dark", schemes: { dark: {
    "--bg": "#313338", "--panel": "#2b2d31", "--edge": "#1e1f22", "--ink": "#dbdee1", "--muted": "#949ba4",
    "--accent": "#5865f2", "--accent-bright": "#7289da", "--on-accent": "#ffffff", "--discuss-head-bg": "#2b2d31",
    "--theirs": "#2b2d31", "--mine": "#2e3350", "--mine-edge": "#5865f2", "--third": "#2f3136", "--third-edge": "#404249",
    "--dock": "#1e1f22", "--row-hover": "#35373c", "--reply-tint": "#2a2d31",
  } } },
  { id: "slack-dark", title: "Slack-like dark", schemes: { dark: {
    "--bg": "#1a1d21", "--panel": "#222529", "--edge": "#383d44", "--ink": "#d1d2d3", "--muted": "#9a9da3",
    "--accent": "#1264a3", "--accent-bright": "#4a9ede", "--on-accent": "#ffffff", "--discuss-head-bg": "#350d36",
    "--theirs": "#222529", "--mine": "#12314d", "--mine-edge": "#1264a3", "--third": "#24262b", "--third-edge": "#383d44",
    "--dock": "#171a1e", "--row-hover": "#27292d", "--reply-tint": "#15181c",
  } } },
  { id: "vscode-dark", title: "VS Code-like dark", schemes: { dark: {
    "--bg": "#1f1f1f", "--panel": "#181818", "--edge": "#2b2b2b", "--ink": "#cccccc", "--muted": "#8b949e",
    "--accent": "#0078d4", "--accent-bright": "#4da3e8", "--on-accent": "#ffffff", "--discuss-head-bg": "#181818",
    "--theirs": "#181818", "--mine": "#0e2a44", "--mine-edge": "#0078d4", "--third": "#1c1c1c", "--third-edge": "#2b2b2b",
    "--dock": "#181818", "--row-hover": "#252526", "--reply-tint": "#171717",
    "--hl-keyword": "#c678dd", "--hl-string": "#98c379", "--hl-constant": "#d19a66", "--hl-comment": "#7f848e",
    "--hl-function": "#61afef", "--hl-type": "#e5c07b", "--hl-property": "#56b6c2", "--hl-variable": "#e06c75",
  } } },
  { id: "obsidian-dark", title: "Obsidian-like dark", schemes: { dark: {
    "--bg": "#202020", "--panel": "#262626", "--edge": "#333333", "--ink": "#dcddde", "--muted": "#999999",
    "--accent": "#8a5cf5", "--accent-bright": "#a68af7", "--on-accent": "#ffffff", "--discuss-head-bg": "#262626",
    "--theirs": "#262626", "--mine": "#2e2445", "--mine-edge": "#8a5cf5", "--third": "#242422", "--third-edge": "#333333",
    "--dock": "#1a1a1a", "--row-hover": "#2c2c2c", "--reply-tint": "#1c1c1c",
  } } },
  { id: "github-dark", title: "GitHub-like dark", schemes: { dark: {
    "--bg": "#0d1117", "--panel": "#161b22", "--edge": "#30363d", "--ink": "#e6edf3", "--muted": "#8b949e",
    "--accent": "#58a6ff", "--accent-bright": "#79b8ff", "--on-accent": "#0d1117", "--discuss-head-bg": "#161b22",
    "--theirs": "#161b22", "--mine": "#0d2242", "--mine-edge": "#58a6ff", "--third": "#12161c", "--third-edge": "#30363d",
    "--dock": "#090c10", "--row-hover": "#1c2128", "--reply-tint": "#0a0d12",
  } } },
  { id: "notion-dark", title: "Notion-like dark", schemes: { dark: {
    "--bg": "#191919", "--panel": "#252525", "--edge": "#2f2f2f", "--ink": "#d4d4d4", "--muted": "#9b9b9b",
    "--accent": "#529cca", "--accent-bright": "#6fb3dd", "--on-accent": "#ffffff", "--discuss-head-bg": "#252525",
    "--theirs": "#252525", "--mine": "#1d2c38", "--mine-edge": "#529cca", "--third": "#22221f", "--third-edge": "#2f2f2f",
    "--dock": "#141414", "--row-hover": "#2a2a2a", "--reply-tint": "#171717",
  } } },
  { id: "telegram-dark", title: "Telegram-like dark", schemes: { dark: {
    "--bg": "#0e1621", "--panel": "#17212b", "--edge": "#232e3c", "--ink": "#e8e8e8", "--muted": "#7f91a4",
    "--accent": "#3390ec", "--accent-bright": "#5aabf0", "--on-accent": "#ffffff", "--discuss-head-bg": "#17212b",
    "--theirs": "#17212b", "--mine": "#0d2f4d", "--mine-edge": "#3390ec", "--third": "#131c26", "--third-edge": "#232e3c",
    "--dock": "#0a121c", "--row-hover": "#1c2936", "--reply-tint": "#0b141f",
  } } },
  { id: "github-light", title: "GitHub-like light", schemes: { light: {
    "--bg": "#ffffff", "--panel": "#f6f8fa", "--edge": "#d0d7de", "--ink": "#1f2328", "--muted": "#59636e",
    "--accent": "#0969da", "--accent-bright": "#0969da", "--on-accent": "#ffffff", "--discuss-head-bg": "#f6f8fa",
    "--theirs": "#f6f8fa", "--mine": "#dbeafe", "--mine-edge": "#0969da", "--third": "#faf8f2", "--third-edge": "#d0d7de",
    "--dock": "#eef1f4", "--row-hover": "#eef1f6", "--reply-tint": "#eef2f7",
  } } },
  { id: "vscode-light", title: "VS Code-like light", schemes: { light: {
    "--bg": "#ffffff", "--panel": "#f3f3f3", "--edge": "#e0e0e0", "--ink": "#1f1f1f", "--muted": "#6e6e6e",
    "--accent": "#0078d4", "--accent-bright": "#0078d4", "--on-accent": "#ffffff", "--discuss-head-bg": "#f3f3f3",
    "--theirs": "#f3f3f3", "--mine": "#d6e9fa", "--mine-edge": "#0078d4", "--third": "#f7f3ea", "--third-edge": "#e0e0e0",
    "--dock": "#e8e8e8", "--row-hover": "#e8e8e8", "--reply-tint": "#ececec",
    "--hl-keyword": "#af00db", "--hl-string": "#a31515", "--hl-constant": "#098658", "--hl-comment": "#008000",
    "--hl-function": "#795e26", "--hl-type": "#267f99", "--hl-property": "#001080", "--hl-variable": "#001080",
  } } },
  { id: "obsidian-light", title: "Obsidian-like light", schemes: { light: {
    "--bg": "#ffffff", "--panel": "#f2f2f2", "--edge": "#e0e0e0", "--ink": "#222222", "--muted": "#6f6f6f",
    "--accent": "#7c3aed", "--accent-bright": "#7c3aed", "--on-accent": "#ffffff", "--discuss-head-bg": "#f2f2f2",
    "--theirs": "#f2f2f2", "--mine": "#eadffc", "--mine-edge": "#7c3aed", "--third": "#f7f2e8", "--third-edge": "#e0e0e0",
    "--dock": "#e8e8e8", "--row-hover": "#e8e8e8", "--reply-tint": "#ececec",
  } } },
];

/** The premade themes offered for a scheme, legacy first. */
export function presetsFor(scheme: EffectiveTheme) {
  return THEME_PRESETS.filter((preset) => preset.id === "legacy" || preset.schemes[scheme]);
}

/** The colour groups the editor shows (`#2155` survey §3): 13 groups covering every
 * paintable token exactly once. Syntax (`--hl-*`) is deliberately not among them. */
export const THEME_GROUPS: readonly {
  readonly id: string;
  readonly title: string;
  readonly tokens: readonly string[];
}[] = Object.freeze([
  { id: "background", title: "Page", tokens: ["--bg"] },
  { id: "cards", title: "Message cards", tokens: ["--panel", "--theirs"] },
  { id: "mine", title: "Your messages", tokens: ["--mine", "--mine-edge", "--muted-on-mine", "--warn-on-mine"] },
  { id: "third", title: "Others' messages", tokens: ["--third", "--third-edge"] },
  { id: "dock", title: "Bars and dock", tokens: ["--dock", "--scrim-bg", "--shadow"] },
  { id: "edge", title: "Borders and dividers", tokens: ["--edge"] },
  { id: "ink", title: "Text", tokens: ["--ink"] },
  { id: "muted", title: "Quiet text", tokens: ["--muted", "--read-ring", "--thread-grey", "--idle", "--grey-wash"] },
  { id: "accent", title: "Accent", tokens: ["--accent", "--on-accent", "--accent-bright", "--thread-sat", "--thread-light"] },
  { id: "highlight", title: "Highlight (Discuss header)", tokens: ["--discuss-head-bg", "--unread-stripe"] },
  { id: "hover", title: "Hover and reply shade", tokens: ["--row-hover", "--reply-tint"] },
  { id: "status", title: "Status and warnings", tokens: ["--ok", "--warn", "--paused", "--summary-fail-ink", "--tick-ink", "--tick-ok", "--tick-bad", "--discuss-ok-bg", "--danger", "--on-warn"] },
  { id: "marks", title: "Pins and search marks", tokens: ["--pin-ink", "--pin-edge", "--pin-bg", "--hit-bg", "--hit-ink"] },
  { id: "code", title: "Code colours", tokens: ["--hl-comment", "--hl-string", "--hl-constant", "--hl-keyword", "--hl-type", "--hl-function", "--hl-escape", "--hl-property", "--hl-macro", "--hl-variable", "--hl-inserted", "--hl-inserted-bg", "--hl-deleted", "--hl-deleted-bg", "--data-bg", "--data-ink", "--data-muted", "--data-rule", "--debug-ink"] },
]);

/** Plain words for each token in the editor (`#2155` liaison review): the reader sees
 * these, never the raw `--token` (that stays in the row's tooltip). */
export const TOKEN_LABELS: Readonly<Record<string, string>> = Object.freeze({
  "--bg": "Page background", "--panel": "Panel background", "--theirs": "Message card",
  "--mine": "Your message card", "--mine-edge": "Your message edge",
  "--muted-on-mine": "Quiet text on your messages", "--warn-on-mine": "Warning on your messages",
  "--third": "Other people's messages", "--third-edge": "Others' message edge",
  "--dock": "Bars and dock", "--scrim-bg": "Dim behind pop-ups", "--shadow": "Shadow",
  "--edge": "Borders and dividers", "--ink": "Text", "--muted": "Quiet text",
  "--read-ring": "Read selection ring", "--thread-grey": "Quiet thread lines",
  "--idle": "Idle dot", "--grey-wash": "Read wash",
  "--accent": "Accent", "--on-accent": "Text on accent", "--accent-bright": "Bright accent",
  "--thread-sat": "Thread colour strength", "--thread-light": "Thread colour lightness",
  "--discuss-head-bg": "Discuss header", "--unread-stripe": "Unread stripe",
  "--row-hover": "Hover background", "--reply-tint": "Reply background",
  "--ok": "Success", "--warn": "Warning", "--paused": "Paused",
  "--summary-fail-ink": "Failed summary text", "--tick-ink": "Send tick",
  "--tick-ok": "Sent tick", "--tick-bad": "Failed tick",
  "--discuss-ok-bg": "New-in-Discuss background", "--danger": "Danger",
  "--on-warn": "Text on warning",
  "--pin-ink": "Pin text", "--pin-edge": "Pin edge", "--pin-bg": "Pin background",
  "--hit-bg": "Search mark background", "--hit-ink": "Search mark text",
  "--hl-comment": "Comment", "--hl-string": "String", "--hl-constant": "Number and constants",
  "--hl-keyword": "Keyword", "--hl-type": "Type", "--hl-function": "Function",
  "--hl-escape": "Escape", "--hl-property": "Property", "--hl-macro": "Macro",
  "--hl-variable": "Variable", "--hl-inserted": "Inserted text",
  "--hl-inserted-bg": "Inserted background", "--hl-deleted": "Deleted text",
  "--hl-deleted-bg": "Deleted background",
  "--data-bg": "Data panel background", "--data-ink": "Data text",
  "--data-muted": "Data quiet text", "--data-rule": "Data divider",
  "--debug-ink": "Debug readout",
});

/** Every token a Custom configuration may own: the scheme tokens plus the accent family. */
export const THEME_TOKEN_NAMES: readonly string[] = Object.freeze([
  ...Object.keys(THEME_TOKENS.dark),
  "--on-accent",
  "--accent-bright",
  "--discuss-head-bg",
]);

/** Extra dark (`#2155` survey §4): the Dark Default variant over true black. Cards are
 * `#000` in both read and unread states (with `#2154`); unread is an edge/stripe, never
 * a fill. Only tokens that differ from the standard dark Default are listed. */
export const EXTRA_DARK_TOKENS: Readonly<Record<string, string>> = Object.freeze({
  "--bg": "#000000", "--panel": "#000000", "--edge": "#2f2f36", "--ink": "#edeff3",
  "--muted": "#a3abb9", "--warn": "#e0956b",
  "--mine-edge": "#4a7db0", "--muted-on-mine": "#c2cad6", "--warn-on-mine": "#f0b28c",
  "--third-edge": "#7a6540", "--dock": "#0a0a0c",
  "--paused": "#a99bf5", "--row-hover": "#17171b",
  "--pin-bg": "#2e2510", "--hit-bg": "#5c4a0e", "--reply-tint": "#0d0f14",
  "--read-ring": "#a3abb9", "--thread-grey": "#6e7684",
  "--accent-bright": "#7ab5e8", "--discuss-head-bg": "#2e2340", "--unread-stripe": "#5b9dd9",
});

const HEX = /^#[0-9a-fA-F]{6}$/;

/** What a kept value means: a parsed configuration, anything unusable the Default. */
export function themeConfigOf(raw: string | null): ThemeConfig {
  if (!raw) return DEFAULT_THEME_CONFIG;
  try {
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) return DEFAULT_THEME_CONFIG;
    const record = parsed as Record<string, unknown>;
    const mode = record.mode === "custom" ? "custom" : "default";
    const style: DarkStyle = record.style === "extra" ? "extra" : "standard";
    const preset = typeof record.preset === "string" && THEME_PRESETS.some((p) => p.id === record.preset)
      ? record.preset
      : style === "extra" ? "extra" : "legacy";
    const tokens: Record<string, string> = {};
    if (typeof record.tokens === "object" && record.tokens !== null) {
      for (const [name, value] of Object.entries(record.tokens as Record<string, unknown>)) {
        if (!THEME_TOKEN_NAMES.includes(name) || typeof value !== "string") continue;
        if (HEX.test(value)) tokens[name] = value.toLowerCase();
        else if ((name === "--thread-sat" || name === "--thread-light") && /^\d+(\.\d+)?%$/.test(value)) tokens[name] = value;
        else if (name === "--unread-stripe" && value === "transparent") tokens[name] = value;
      }
    }
    if (mode === "default" && preset === "legacy" && Object.keys(tokens).length === 0) {
      return DEFAULT_THEME_CONFIG;
    }
    return Object.freeze({ mode, style: preset === "extra" ? "extra" : "standard", preset, tokens: Object.freeze(tokens) });
  } catch (_error) {
    return DEFAULT_THEME_CONFIG;
  }
}

/** The kept string for a configuration; the untouched Default keeps nothing. */
export function themeConfigRaw(config: ThemeConfig): string | null {
  if (config.mode === "default" && config.preset === "legacy" && Object.keys(config.tokens).length === 0) {
    return null;
  }
  return JSON.stringify({ mode: config.mode, style: config.preset === "extra" ? "extra" : "standard", preset: config.preset, tokens: config.tokens });
}

/** A scheme's effective Default tokens: its scheme table, its Colour preset, and — for
 * dark in Extra style — the Extra variant over both (its Discuss highlight wins over
 * the preset's, as the survey specifies). */
export function defaultTokensFor(
  scheme: EffectiveTheme,
  preset: ColourTheme,
  themePreset: string,
): Readonly<Record<string, string>> {
  const merged: Record<string, string> = {
    ...THEME_TOKENS[scheme],
    "--on-accent": "#ffffff",
    "--accent-bright": scheme === "dark" ? "#5b9dd9" : "#5b9dd9",
    "--discuss-head-bg": scheme === "dark" ? "#1d2026" : "#ffffff",
    ...COLOUR_PRESETS[preset][scheme],
  };
  if (scheme === "dark" && themePreset === "extra") Object.assign(merged, EXTRA_DARK_TOKENS);
  const appPreset = THEME_PRESETS.find((p) => p.id === themePreset);
  if (appPreset && appPreset.schemes[scheme]) Object.assign(merged, appPreset.schemes[scheme]);
  return Object.freeze(merged);
}

/** The tokens a configuration paints for its scheme. */
export function tokensForConfig(
  config: ThemeConfig,
  scheme: EffectiveTheme,
  preset: ColourTheme,
): Readonly<Record<string, string>> {
  const defaults = defaultTokensFor(scheme, preset, config.preset);
  if (config.mode !== "custom") return defaults;
  return Object.freeze({ ...defaults, ...config.tokens });
}
