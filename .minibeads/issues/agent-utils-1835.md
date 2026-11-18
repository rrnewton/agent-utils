---
title: 'label-dialog-placement: keep the Combine/Edit dialog over the Settings column'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- labels
- ui
created_at: 2026-11-18T06:34:26-05:00
updated_at: 2026-11-18T06:34:26-05:00
---

# Description

[watermelon-full-preview] Follow-up from #1825 vibe-talk-hashtag-labels. On desktop, Settings > Labels' Combine/Edit dialog does not sit over the Settings column: it spans from the left edge across the Settings header (about 0-666 px in `desk-light-7-combine-dialog.png`). The phone presentation is constrained by the viewport, but the desktop placement is visibly detached from the Labels group that opened it.

# Design

- Keep the shared `.data-popout` behaviour, but give the Labels group the positioned context the pop-out expects. `#dismissed-group` already supplies `position: relative`; `#labels-group` has no equivalent rule, so the dialog resolves against a broader Settings ancestor.
- Prefer the smallest scoped CSS fix (for example, `#labels-group { position: relative; }`) over changing `.data-popout` globally, so Recently dismissed and other pop-outs do not move.
- Do not start until coordb confirms the #1781 vibe-talk landing freeze has lifted. This issue lands in its own worktree, separately from #1836 label-inline-fallback.

# Acceptance Criteria

- On desktop light and dark, the Combine dialog opens over the Settings column, aligned with the Labels group and inside the Settings panel edges; it does not cross the Settings header or extend past the panel.
- The Edit-group dialog opened from `(G)` has the same placement.
- A phone check shows the dialog remains usable and inside the viewport.
- Screenshot evidence is captured, inspected, and sent to coordb before landing.
- The focused page/settings checks and the labels browser walk pass; no other `.data-popout` surface changes placement.

# Notes

Known cause from the #1825 audit: `.data-popout` is absolutely positioned with `left: -0.3rem; right: -0.3rem; top: 0`. Its existing dismissed-message use is anchored by `#dismissed-group { position: relative; }`; the Labels group lacks that positioning context.

## Resolution

[watermelon-full-preview] Landed on main as 2ac79646: `#labels-group { position: relative; }`, so the Combine/Edit dialog sits over the Settings column; desktop light/dark and phone screenshots in /tmp/mu-dialogfix-shots (gate pass on 99efe9be).
