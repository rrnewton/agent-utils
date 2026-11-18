---
title: 'vibe-talk-status-pill-avoids-text: the status pill and freshness line must not cover readable text or controls'
status: closed
priority: 1
issue_type: bug
assignee: mu-pilltext
labels:
- vibe-talk
created_at: 2026-11-18T07:46:00-05:00
updated_at: 2026-11-18T07:46:00-05:00
---

# Description

[watermelon-full-preview] The status pill / toast ("Ready.", "fetching the channel…", "1 message marked as dealt with…") must not cover readable text or controls. #1779 already keeps it off the docked Discuss pane; the remaining cases, all reported by the owner-facing liaison from screenshots:

1. On the first-run Welcome page (and Welcome reopened from Settings) the "Ready." pill sits over Welcome text; on desk it covers "You can re-open this screen any time from Settings". Proposal to evaluate: don't show the pill at all while the Welcome is up.
2. At the top of the channel the pill and the "Live · updated" line can cover a message card's text, its top-row buttons (Reply/…/Done) and its label chips.
3. On desk the pill can cover the "No more hits on the server" line in a transcript-at-hit view.

Also take the two shots the liaison found missing from the #1781 matrix: a phone welcome-from-settings shot and a desk no-channel guide shot.

# Design

Design note sent to coordb before coding (this lane codes only on approval). Summary of the recommendation in the note:

- Welcome (first-run and reopened): hide the status pill entirely while the Welcome is up, in the same "wait" idiom the pill already uses while the server-search section stands at the foot of the list; the error panel is a separate surface and still shows. No pill text is queued or replayed on dismiss.
- Channel head: the "Live · updated" freshness line is a separate floating pill from the status pill. Idle-fresh wording auto-takes-itself-away like the status pill; stale/offline/failed wording persists because it is a warning. Landings already measure the floating line; scroll-to-top gains the same clearance through scroll padding, which moves no layout and reserves no band.
- Channel foot: the status pill keeps floating (a reserved lane and docking it to the bottom chrome are both rejected, with reasons in the note); idle confirmations such as "Ready." take themselves away sooner than action confirmations, which keep the full reading time. The pill stays `pointer-events: none` except its dismiss control.
- Desk transcript-at-hit: extend the existing server-search wait so the pill also waits while the "No more hits on the server" foot line stands in its footprint; verify on this base whether the #1813 wait already covers the exact at-hit state and close any gap the screenshot proves.
- Phone and desk behaviour, the rejected alternatives (reserve a lane, dock to the bottom chrome, shrink to a dot), and the full screenshot matrix are in the design note.

# Acceptance Criteria

- While the Welcome is up (first-run, reopened from Settings, reopened from Help), the status pill is not drawn over Welcome text at phone 412x915 and desktop 1280x800, in light and dark; dismissing the Welcome lands as before and no stale "Ready." reappears.
- At the top of the channel, neither the status pill nor the "Live · updated" line covers the first card's text, top-row buttons (Reply/…/Done) or label chips at phone and desktop sizes, in light and dark, keyboard open and closed; a stale/offline freshness warning remains visible while it is true.
- On desk, the status pill does not cover the "No more hits on the server" line in the transcript-at-hit state.
- The #1779 docked-Discuss clearance and the server-search wait do not regress.
- Page tests at phone and desktop sizes for the exact defects, in light and dark where theme-related and keyboard-open where layout-related, with condition-based waits only.
- Screenshot matrix on a test instance (never the live instance): light and dark, keyboard really open (shrunk visualViewport verified) and closed, phone 412x915 (scale 2.6) and desktop 1280x800, Discuss half-pane open in at least the phone shots, plus the targeted states before and after, plus a phone welcome-from-settings shot and a desk no-channel guide shot; every image looked at by the lane and listed with a one-line description in the report and handoff for liaison review.

# Notes

Design note first, then implement on coordb approval (owner-priority gate, deployed on landing). Nested worktree on branch mu-pilltext/1837; screenshots and validation follow the standing owner-UI matrix and the lane's validation steps.

Known limit (coordb, 2026-11-18): the freshness line's stale/offline/failed wording is a warning and persists while it is true, so while it stands it can still cover the first card at the head of the list. The current ("Live · updated") wording is idle information and now takes itself away, which covers the common case; app landings already measure the pill while it is up. A scroll-padding clearance was tried and dropped: it fought the list's own place-keeping (the collapsed-lines keeper moved a row by the clearance), and auto-away covers the common case without it.

## Resolution

[watermelon-full-preview] Landed in two commits: (a) 0f7982ba — while the Welcome is up (first run or re-opened) the status pill waits (visibility hidden under data-screen="welcome"), so "Ready." never covers its text and expires behind it; the firstrun walk asserts the wait and gained the two missing #1781 shots (phone welcome-from-settings, desk no-channel guide). (b)+(c)+(d) b9a0889f (verified-tree landing over an overlap on fixture.mjs/main.js; combined tree checked in a detached worktree) — a current freshness pill takes itself away after 7s while stale/offline/failed wording persists; idle status confirmations ("Ready.") dismiss after 2.5s against 6s for action/progress words; and the desk at-hit state ("No more hits on the server") is proven already clear by the #1813 wait, now asserted in the server-hits walk at phone and desk sizes. Timing tests use fake timers and state transitions only; the full module glob passed 3245/3245 and the page suite 2557/2557; gate passed on rerun of a transient port-collision step. Change-specific screenshots are in /tmp/mu-pilltext-dynamic (change-* live-gone/stale/action pairs at phone and desk, light and dark; *-no-more-hits-pill-clear), before/after Welcome shots in /tmp/mu-pilltext-a-shots-before and /tmp/mu-pilltext-a-shots-after. Known limit stands as noted above: a persisting stale/offline warning can still cover the first card while it is true.
