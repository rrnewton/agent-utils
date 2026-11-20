---
title: 'vibe-talk-colour-themes: configurable colour presets for the vibe-talk page'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
- ui
- settings
created_at: 2026-11-20T08:37:00-05:00
updated_at: 2026-11-20T08:37:00-05:00
---

# Description

[watermelon-full-preview] Owner request (228, 2026-11-20 08:33 ET), third priority: "Maybe we should go to reskettable color schemes where we can configure these things and and select between different presets like all these blue highlight colors we have with the blue buttons. We could make selecting the discuss bar header highlight part of that overall configurable plan and themes. I would like you to build that out but it's less urgent priority than the other things I'm mentioning that are more immediate."

The page already paints from CSS custom properties (`--accent`, `--mine`, `--row-hover`, `--reply-tint` and the rest of the token tables in `web/src/core/settings/appearance.ts`, `web/style.css` and `web/styles/voice/00-frame.css`), and the accent family already drives buttons, links, chips and pressed toggles (176 `var(--accent)` uses). What is missing is a choice: one hard-wired blue family per light/dark scheme, a Discuss header that still paints `var(--panel)`, and a handful of accent-blue literals left in the sheets (for example in `21-documents.css`).

# Design

Short design note (sent to the coordinator before building, per the lane brief):

- **Themed variables.** A preset overrides only the accent family, for the effective light or dark theme: `--accent` (buttons, links, chips, pressed toggles), `--mine` / `--mine-edge` (the owner's rows), `--muted-on-mine`, `--row-hover`, `--reply-tint`, and the new `--discuss-head-bg`. Everything else (surfaces, ink, warn/ok status, pin/hit colours, syntax highlighting in `11-message-text.css`) stays scheme-fixed; the syntax palette is a code-reading palette, not page chrome, and is inventoried as deliberately not themed.
- **Discuss header.** `.discuss-head` in `22-discuss.css` paints `background: var(--discuss-head-bg)`. The name `--discuss-head-bg` is proposed here for the Discuss-header lane (#1960 vibe-talk-discuss-header-toggle-colours) to reuse; in the default preset it equals the panel colour in each scheme, so nothing changes visually until the owner picks another preset or that lane's chosen highlight lands as a preset value.
- **Presets.** Five, each defined for light and dark: `blue` (the current look, token-for-token), `blue-hc` (blue with the dark scheme's text-on-accent pair repaired to AA), `teal`, `plum`, and `amber`. The default is `blue`, stored as nothing kept (absent means blue), so a device that never opens the setting paints exactly as today.
- **Where the setting lives.** A `Colour theme` select directly under the Appearance select in the Settings Appearance fold (`#colour-theme`, state line `#colour-theme-state`), beside Light/Dark/System because the preset resolves against the same effective theme. It is a synced device setting, key `vibe-talk.voice.colour-theme`, added to both synced-key lists (`web/src/core/settings/keys.ts` and `src/client_settings.rs`, which a server test keeps equal). An unknown stored value reads as `blue`.
- **No flash, no drift.** The core preset tables live in `web/src/core/settings/` next to `THEME_TOKENS`; the Appearance control applies the preset for the effective theme after it applies a forced Light/Dark, and the pre-paint script in the shell applies the stored preset before first paint, mirroring how forced Appearance already works. Switching Appearance (including a live system change under System) re-applies the preset for the new effective theme.
- **Hard-coded inventory.** Accent-blue literals outside the token tables (documents view and any other accent duplicates found by a comment-stripped hex scan of `web/style.css` and `web/styles/voice/*.css`) move to the themed variables with the blue preset keeping the exact current values. A page test asserts no accent literal remains outside the token tables and the syntax palette.

# Acceptance Criteria

- The default (`blue`, nothing stored) renders token-for-token identically to the current page in light and dark: the preset table for blue equals today's token values, checked by a unit test against the existing tables.
- Choosing Teal, Plum or Amber in Settings repaints buttons, links, chips, the owner's rows and the Discuss header in that family, in both light and dark, and the choice survives a reload and follows the owner through the existing settings sync (the synced-key server test passes with the new key on both lists).
- The Discuss bar header paints `var(--discuss-head-bg)`; each preset gives it a distinct value per scheme.
- An unknown or damaged stored preset value falls back to blue; a device that refuses storage still applies the choice until the page is left, and the state line says so, as Appearance does.
- Focused non-browser suites pass (core settings, settings view, page shell/settings slices, the client-settings server test). A browser test for the control and a screenshot scene are written ready to run; they run when a browser is available on the host, and before/after screenshots of the default preset are captured then (no visual change is the claim they prove).
- `cargo fmt --all -- --check` is clean on the landing tree.

## Contrast (coordinator addition, 2026-11-20)

WCAG AA is 4.5:1 for text. Measured on the preset tables (asserted by the core settings unit test):

| Preset | Scheme | Text on accent | Accent on panel | Ink on Discuss head | Quiet text on owner's row |
|---|---|---|---|---|---|
| blue | dark | 2.89:1 (exception) | 5.65:1 | 13.43:1 | 5.59:1 |
| blue | light | 6.60:1 | 6.60:1 | 16.90:1 | 5.61:1 |
| blue-hc | dark | 6.27:1 | 5.65:1 | 13.43:1 | 5.59:1 |
| blue-hc | light | 6.60:1 | 6.60:1 | 16.90:1 | 5.61:1 |
| teal | dark | 8.32:1 | 7.50:1 | 8.19:1 | 7.57:1 |
| teal | light | 5.47:1 | 5.47:1 | 14.16:1 | 6.25:1 |
| plum | dark | 8.36:1 | 7.53:1 | 9.36:1 | 7.82:1 |
| plum | light | 7.03:1 | 7.03:1 | 12.74:1 | 6.40:1 |
| amber | dark | 9.47:1 | 8.53:1 | 7.68:1 | 7.60:1 |
| amber | light | 5.93:1 | 5.93:1 | 14.19:1 | 6.07:1 |

The one exception is the default itself: blue in dark keeps white on `#5b9dd9` (2.89:1), the look the owner already has, kept unchanged until they pick otherwise. `blue-hc` is the same family with that one pair repaired (dark ink on the accent, 6.27:1), so the owner can compare both in screenshots. The unit test asserts 4.5:1 or better for every preset and pair except that one documented default. One accent-background rule is not unified on `--on-accent`: the search bucket chip keeps its own per-scheme AA pair from `#1899` (near-black on the dark accent, white on the light one), which a single `--on-accent` value for blue-dark would have broken.

# Notes

Coordination: #1960 vibe-talk-discuss-header-toggle-colours adds a CSS variable for the Discuss header highlight and lands first if it lands first; this change proposes the name `--discuss-head-bg` and will rebase onto that lane's name if it chooses a different one. Browser execution is currently unavailable on the lane host (missing system libraries since the 2026-11-19 reprovision), so browser evidence is deferred, not skipped: the test and scene are committed ready, and the coordinator is told when the lane is blocked only on that.
