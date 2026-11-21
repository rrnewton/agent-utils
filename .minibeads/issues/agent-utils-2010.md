---
title: 'vibe-talk-discuss-browser-red-on-main: three discuss browser checks fail on the collapse bar'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
- discuss
- ui
- browser
created_at: 2026-11-20T18:00:00-05:00
updated_at: 2026-11-20T22:00:00-05:00
---

# Description

[watermelon-full-preview] On untouched main `d4891b9a`, the gating browser checks `vibe-talk.check.discuss-browser`, `vibe-talk.check.discuss-dock-browser` and `vibe-talk.check.discuss-strike-live-browser` all fail with the same `Locator.click Timeout 30000ms` on the discuss collapse bar (`#discuss-answer .discuss-collapse-bar button`). The failures were invisible until 2026-11-20 5:38 PM ET, when the browser libraries returned on the validation host; before that every browser step failed at launch, so any commit since the last real green (on the prior host or before the 2026-11-19 reimage) may have introduced the break. Brief is to reproduce, decide page bug versus stale test selector, bisect to the breaking commit, report to the coordinator before fixing, then fix with the checks green.

# Design

- Reproduce the three checks locally headless on the pristine base, capturing the exact Playwright timeout output.
- Decide page versus test with direct page inspection at the click point: `document.elementFromPoint` at the bar button centre, the button's bounding box and visibility, and what element covers that point; also record whether `#discuss-answer .discuss-collapse-bar button` exists at all and where the live Expand/Collapse control actually stands.
- Bisect over recent main commits with `git bisect run` driving the focused browser check in a throwaway worktree, covering today's page landings first (`#1985` windowed, `#1986`, `#1984`, `#1961` colour themes, `#1983`-era Discuss changes and the `#1960` header-toggle work).
- Report the verdict, the breaking commit and, if it is a page bug, exactly what the owner would see on the live page to the coordinator before any fix is attempted.
- Fix the root cause in its own commit: if it is a test bug, update the stale selectors/expectations to the shipped control; if it is a page bug, restore the control's clickability. Keep the fix separate from unrelated work, run the focused suites, then one owner-priority gate, and hand the coordinator the command to reproduce on the review host.

# Acceptance Criteria

- The breaking commit is identified by bisection and named with its bisection evidence.
- Verdict is evidence-backed: element-from-point, bounding-box and visibility readings at the failing click point distinguish a covered/zero-size/hidden page control from a stale test selector.
- All three checks (`discuss-browser`, `discuss-dock-browser`, `discuss-strike-live-browser`) pass headless on the fixed tree via the real server router (fixtures only, never the live instance).
- Fix lands as its own commit with the checks green and the coordinator given the exact reproduction command for the review host.

# Notes

[watermelon-full-preview] mu-discussfix, owner-priority gate. Number 2010 pre-assigned by coordb. Related: `#1960 vibe-talk-discuss-header-toggle-colours` (the toggle that replaced the chip pair), `#1961 vibe-talk-colour-themes`, `#1985 vibe-talk-windowed-all-view-fixes`, `#1986`, `#1984`.

## Resolution

[watermelon-full-preview] Landed on github/main as 28cc9860: discuss-browser, discuss-dock-browser and discuss-strike-live-browser green in the gate on b908f455 (replayed as 28cc9860).
