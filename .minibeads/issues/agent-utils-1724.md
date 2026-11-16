---
title: 'vibe-talk-discuss-panel-ignores-light-theme: Discuss panel follows the active theme'
status: closed
priority: 0
issue_type: task
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-16T17:05:00+00:00
updated_at: 2026-11-16T18:12:00+00:00
closed_at: 2026-11-16T18:12:00+00:00
---

# Description

[watermelon-full-preview] Owner, 11:57 AM ET (screenshot, Android, light theme, keyboard open, Discuss half-pane open): "As you will see in this screenshot when I switch my Android device from dark background to light background. The discuss panel does not change and it remains black background. Creating an ugly mismashed effect."

The Discuss half-pane (header "Discuss agent" with its buttons, the summary body, and the unread/hours/Summarize row) stays dark while the rest of the page is light.

# Design

Root cause: `vibe-talk/web/styles/voice/22-discuss.css` states its colours literally ("ALWAYS DARK, in either colour scheme", copied from the Documents outline pop-up in `21-documents.css`): panel ground `#1f232b`, ink `#e7e9ee`, control ground `#262c36`, edge `#3d4452`, muted `#aab2c0`, and matching literals for links, focus, struck/fresh marks and the drag grip. The page's theme is the scheme tokens in `style.css` and `00-frame.css` (`--panel`, `--ink`, `--edge`, `--muted`, `--accent`, `--warn`, `--ok`, `--row-hover`), switched by `prefers-color-scheme` only; the page has no in-app theme setting, so the system preference is the one source to follow.

Fix: the whole Discuss panel takes those tokens, so the half-pane, the full-screen (`maximized`) mode and the desktop docked column — all the one `#discuss-panel` — follow the active scheme with readable contrast in both. Dark values stay visually the same (the tokens' dark values are the literals' own or their near neighbours). The Documents outline pop-up keeps its always-dark dress; it is not this bug.

# Acceptance Criteria

- Page tests at phone (412x915, scale 2.6) and desktop (1280x800) for the exact defect, in light and dark theme, with the Discuss half-pane open in at least the phone shots: in each scheme the panel's computed ground and ink match the page's scheme tokens and read at 4.5:1 or better. Condition-based waits only.
- Screenshot matrix on a local test instance (fixture data or a fake API; never the live instance or its database): light and dark theme x keyboard open (visualViewport shrink emulated) and closed x phone 412x915 and desktop 1280x800, Discuss half-pane open in at least the phone shots, plus the targeted state before and after. Every image inspected by the author; folder path and a one-line description per image in the report and handoff.

## Resolution

[watermelon-full-preview] Delivered in 216a6829 and landed on main as 046c4430 with its gate record (gate exit 0 on tree 216a6829; local validate exit 0 under heavy-slot). Every Discuss colour in `22-discuss.css` — panel ground, sticky head, phone bottom bar, icon/Summarize/mark buttons, inputs, muted text, answer links, focus rings, struck/fresh marks and the drag grip — now takes the page scheme tokens (`--panel`, `--ink`, `--edge`, `--muted`, `--accent`, `--warn`, `--ok`, `--row-hover`), so the half-pane, the full-screen mode and the desktop docked column follow `prefers-color-scheme` like the rest of the page, with dark rendering visually unchanged. `discuss_browser.py` walks phone (393x852 dark, 390x844 light) and desk (1280x800 dark and light) and asserts the panel, head and Summarize control compute to the resolved tokens at 4.5:1 or better; all four walks passed. Acceptance screenshots from the test instance (fake catchup API, never live) were inspected at both sizes in both schemes, keyboard open and closed, plus light docked and maximized: `/tmp/discuss-shots/before` (black panel on the light page) and `/tmp/discuss-shots/after` (the same states themed). Deployment is the coordinator's midnight batch.
