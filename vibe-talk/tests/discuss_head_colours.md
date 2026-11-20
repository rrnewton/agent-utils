# Discuss bar header highlight candidates (#1960, Task B)

Mock-ups only — no landing change beyond the variable. The Discuss bar
header (`.discuss-head`) is painted from the single CSS variable
`--discuss-head-bg` in `web/styles/voice/22-discuss.css`, whose default
is `var(--panel)`, so the shipped page looks exactly as before until
the owner picks a candidate below. Each candidate is one value of
that variable; applying the pick is a one-line change there (and
`mu-themes` / #1961 can carry the same variable in its presets).

All candidates are mixes of the existing blue accent with theme
surfaces, so each fits the blue palette in both themes and the header
text (`--ink` on the candidate) stays readable. Effective colours and
WCAG contrast ratios were computed from the theme tokens in
`web/style.css` (dark: accent #5b9dd9, panel #1d2026, page #14161a,
ink #e7e9ee; light: accent #24618f, panel #ffffff, page #f6f7f9, ink
#1a1d22; edge dark #2c313a, light #d9dde4).

| Candidate | Value of `--discuss-head-bg` | Dark effective | Light effective | vs panel dark / light | vs page dark / light | Ink text dark / light |
|---|---|---|---|---|---|---|
| C1 accent wash 18 | `color-mix(in srgb, var(--accent) 18%, var(--panel))` | #283746 | #d8e3eb | 1.33 / 1.31 | 1.48 / 1.22 | 10.07 / 12.91 |
| C2 accent wash 32 | `color-mix(in srgb, var(--accent) 32%, var(--panel))` | #31485f | #b9ccdb | 1.73 / 1.65 | 1.92 / 1.53 | 7.78 / 10.27 |
| C3 accent wash 48 | `color-mix(in srgb, var(--accent) 48%, var(--panel))` | #3b5c7c | #96b3c9 | 2.34 / 2.19 | 2.59 / 2.04 | 5.75 / 7.73 |
| C4 accent over edge 35 | `color-mix(in srgb, var(--accent) 35%, var(--edge))` | #3c5772 | #9ab2c6 | 2.17 / 2.21 | 2.41 / 2.06 | 6.19 / 7.66 |

"vs panel" is the separation that matters at half height: the Discuss
sheet's own body below the bar is `--panel`, and the message list
above it sits on the page background. C2 and C3 give the clearest
split while keeping header text well above 4.5:1 in both themes; C1
is the subtlest. A solid accent bar was rejected: ink text on it is
only 2.38 (dark) / 2.56 (light).

## Contact sheets

`discuss_head_colours_mock.html` (beside this file) renders, per theme,
four phone frames (412x915) with the Discuss sheet at half height so
its bar stands in the middle of the screen, one frame per candidate,
labelled with the candidate name, its variable value, and its ratios.
It also shows the Task A header in both forms the owner asked to
compare: the one toggle (landed) and the two buttons stacked
vertically (mock-up only).

Shoot it with:

```bash
python3 vibe-talk/tests/discuss_head_colours_shots.py --out /tmp/mu-discusshead-colours
```

That writes `light-contact.png` and `dark-contact.png`. It needs a
browser (Playwright/Chromium); on a host without the browser libraries
it exits saying so, and the sheets are shot on another host or after
the reinstall. This mock page is a colour/layout mock-up only — the
real feature is evidenced through the real server by the shared
screenshot harness (scenes 73–76, 84, 85), never by this page.
