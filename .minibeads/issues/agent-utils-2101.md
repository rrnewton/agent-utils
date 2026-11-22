---
title: 'vibe-talk-offline-cache-phone-check-unreliable: offline-cache-phone-360-browser fails in most gates across unrelated trees'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- testing
created_at: 2026-11-22T07:40:00-05:00
updated_at: 2026-11-22T07:40:00-05:00
---

# Description

vibe-talk.check.offline-cache-phone-360-browser failed in about 20 of the last 40 gate and rerun rows on one host, on trees that do not touch the offline cache, including solo exclusive reruns where it was the only selected failure. Either main is broken for this check or the check is not robust on a loaded host.

# Design

Reproduce on an untouched main tree (solo, several runs, with the host loaded as usual) and read the failing assertion. If main is broken, fix the cause. If the check depends on wall-clock timing or frame races, make it condition-based; if that cannot be done quickly, disable it in the gate (with this issue as the tracker) under the testing rule that nothing timing-sensitive gates, and re-enable when it is robust.

# Acceptance Criteria

- The cause is named with evidence; the check passes repeatedly on a loaded host, or is disabled from gating with a tracked path back.

# Notes

Filed by the coordinator from the gate ledger.

## Diagnosis (2026-11-22, mu-offlinecache)

[watermelon-full-preview] Not a broken page and not CPU starvation: a race the check walks into at one press, converted to a failure by a wall-clock default.

- cb07ed261 (2026-11-07, the three-way view selector) turned one `#view-switch` click into two in the sign-in, links and kinds walks of `vibe-talk/tests/offline_cache_browser.py` (Channel -> Documents -> Voice). The first ledger failure of this step is the next day; there are 109 since, on unrelated trees, and e65d145c failed a solo rerun and passed the same tree minutes later, so the page code is not deterministically broken.
- Solo on an untouched main tree under load, the step failed 5 of 5 at the kinds walk's second press (`Page.click: Timeout 30000ms exceeded`), the switch resolved, visible, enabled and stable for the whole budget while `#scroll-area` and `#action-menu-open` intercepted the point. In the failing state (Documents view, 360px at a 150% root font) `#control-bar` lays out 518px wide in a 360px visual viewport with `overflow-x: visible` and no sideways scroll, so the switch sits at 368-518px, past the right edge; scrolling holds no state (`scrollLeft` stays 0), and a tap at the drawn centre is outside the screen. Whether a run's earlier timing leaves the bar settled narrow enough for the click to land is the coin flip.
- The file set no default timeout, so every bare click verdicted at Playwright's 30s; sibling checks (`documents_browser.py`, `github_helper_browser.py`) already carry a `HANG_BACKSTOP_MS = 120_000` for exactly this.

## Fix

- Landed on main from the sibling lane's #2077 work, converging on the same diagnosis: 3f005f59 classifies the fixture's channel reads by route shape, and 6cd9cf64 adds the shared `vibe-talk/tests/browser_waits.py` (`HANG_BACKSTOP_MS = 120_000`, `hot_host_timeouts` at every context in this file, `click_when_clear` at the sign-in, links and dock presses) and hand-converts the kinds walk's call-view press to the keyboard — focus, Enter, a wait for the switch to name Documents, again, then `#pane-voice` at the backstop — naming the same 518px Documents layout as the reason no coordinate press can be aimed there. This lane's independent drawn-state press (measure the centre, tap where it hits the switch, tab to it where it cannot) reached the same conclusion on the pre-landing tree and was dropped whole on the rebase as superseded; what lands from this lane is this record.
- Proof (loaded host, landed tree): five consecutive solo passes of `vibe-talk.check.offline-cache-phone-360-browser` plus a desktop-profile pass, then an owner-tier gate; the rows are the lane's report. This issue stays open until that proof lands.
