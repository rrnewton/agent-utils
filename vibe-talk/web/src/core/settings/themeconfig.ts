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
import type { ColourTheme } from "./device.ts";

/** Which Default the Dark side paints where its mode is Default. */
export type DarkStyle = "standard" | "extra";

/** One scheme's kept configuration. `tokens` is the Custom token map (seeded from the
 * Default, then reader-edited); it is kept even in Default mode so returning to Custom
 * restores the reader's colours. Absent tokens fall back to the Default token. */
export interface ThemeConfig {
  readonly mode: "default" | "custom";
  readonly style: DarkStyle;
  readonly tokens: Readonly<Record<string, string>>;
}

export const DEFAULT_THEME_CONFIG: ThemeConfig = Object.freeze({
  mode: "default",
  style: "standard",
  tokens: Object.freeze({}),
});

/** The colour groups the editor shows (`#2155` survey §3): 13 groups covering every
 * paintable token exactly once. Syntax (`--hl-*`) is deliberately not among them. */
export const THEME_GROUPS: readonly {
  readonly id: string;
  readonly label: string;
  readonly tokens: readonly string[];
}[] = Object.freeze([
  { id: "background", label: "Page background", tokens: ["--bg"] },
  { id: "cards", label: "Message cards", tokens: ["--panel", "--theirs"] },
  { id: "mine", label: "Your messages", tokens: ["--mine", "--mine-edge", "--muted-on-mine", "--warn-on-mine"] },
  { id: "third", label: "Others' messages", tokens: ["--third", "--third-edge"] },
  { id: "dock", label: "Bars and dock", tokens: ["--dock", "--scrim-bg", "--shadow"] },
  { id: "edge", label: "Borders and dividers", tokens: ["--edge"] },
  { id: "ink", label: "Text", tokens: ["--ink"] },
  { id: "muted", label: "Quiet text", tokens: ["--muted", "--read-ring", "--thread-grey", "--idle", "--grey-wash"] },
  { id: "accent", label: "Accent", tokens: ["--accent", "--on-accent", "--accent-bright", "--thread-sat", "--thread-light"] },
  { id: "highlight", label: "Highlight (Discuss header)", tokens: ["--discuss-head-bg", "--unread-stripe"] },
  { id: "hover", label: "Hover and reply shade", tokens: ["--row-hover", "--reply-tint"] },
  { id: "status", label: "Status and warnings", tokens: ["--ok", "--warn", "--paused", "--summary-fail-ink", "--tick-ink", "--tick-ok", "--tick-bad", "--discuss-ok-bg", "--danger", "--on-warn"] },
  { id: "marks", label: "Pins and search marks", tokens: ["--pin-ink", "--pin-edge", "--pin-bg", "--hit-bg", "--hit-ink"] },
  { id: "code", label: "Code colours", tokens: ["--hl-comment", "--hl-string", "--hl-constant", "--hl-keyword", "--hl-type", "--hl-function", "--hl-escape", "--hl-property", "--hl-macro", "--hl-variable", "--hl-inserted", "--hl-inserted-bg", "--hl-deleted", "--hl-deleted-bg", "--data-bg", "--data-ink", "--data-muted", "--data-rule", "--debug-ink"] },
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
    const tokens: Record<string, string> = {};
    if (typeof record.tokens === "object" && record.tokens !== null) {
      for (const [name, value] of Object.entries(record.tokens as Record<string, unknown>)) {
        if (!THEME_TOKEN_NAMES.includes(name) || typeof value !== "string") continue;
        if (HEX.test(value)) tokens[name] = value.toLowerCase();
        else if ((name === "--thread-sat" || name === "--thread-light") && /^\d+(\.\d+)?%$/.test(value)) tokens[name] = value;
        else if (name === "--unread-stripe" && value === "transparent") tokens[name] = value;
      }
    }
    if (mode === "default" && style === "standard" && Object.keys(tokens).length === 0) {
      return DEFAULT_THEME_CONFIG;
    }
    return Object.freeze({ mode, style, tokens: Object.freeze(tokens) });
  } catch (_error) {
    return DEFAULT_THEME_CONFIG;
  }
}

/** The kept string for a configuration; the untouched Default keeps nothing. */
export function themeConfigRaw(config: ThemeConfig): string | null {
  if (config.mode === "default" && config.style === "standard" && Object.keys(config.tokens).length === 0) {
    return null;
  }
  return JSON.stringify({ mode: config.mode, style: config.style, tokens: config.tokens });
}

/** A scheme's effective Default tokens: its scheme table, its Colour preset, and — for
 * dark in Extra style — the Extra variant over both (its Discuss highlight wins over
 * the preset's, as the survey specifies). */
export function defaultTokensFor(
  scheme: EffectiveTheme,
  preset: ColourTheme,
  style: DarkStyle,
): Readonly<Record<string, string>> {
  const merged: Record<string, string> = {
    ...THEME_TOKENS[scheme],
    "--on-accent": "#ffffff",
    "--accent-bright": scheme === "dark" ? "#5b9dd9" : "#5b9dd9",
    "--discuss-head-bg": scheme === "dark" ? "#1d2026" : "#ffffff",
    ...COLOUR_PRESETS[preset][scheme],
  };
  if (scheme === "dark" && style === "extra") Object.assign(merged, EXTRA_DARK_TOKENS);
  return Object.freeze(merged);
}

/** The tokens a configuration paints for its scheme. */
export function tokensForConfig(
  config: ThemeConfig,
  scheme: EffectiveTheme,
  preset: ColourTheme,
): Readonly<Record<string, string>> {
  const defaults = defaultTokensFor(scheme, preset, config.style);
  if (config.mode !== "custom") return defaults;
  return Object.freeze({ ...defaults, ...config.tokens });
}
