---
title: 'scroll-soak-load-flake: scroll-soak-browser anchor moves 28.1px under load; out of gating until robust'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
created_at: 2026-11-15T20:15:00.000000000+00:00
updated_at: 2026-11-15T20:15:00.000000000+00:00
---

# Description

`vibe-talk.check.scroll-soak-browser` (--strict) fails under host load
on unchanged code: on the phone (393px) walk, the `reaction-folded-demote`
act moves the parked anchor 28.1px (`::after y267->295`;
`li[data-id=248] y132->160`), and the anchor stays 28.1px off for the
rest of the phone walk (`read-state-by-reread`, `edit-taller-below`,
`poll-window-idle` all report MOVED). The desk walk stays within 0.2px.

Paired A/B, 2026-11-15, solo strict runs interleaved under heavy-slot:
`github/main` 0/3 green, the #1606 head 0/3 green, identical assertion
on both. Three unrelated owner-UI gates (#1606, #1588, #1680) went red
on this step the same way. Non-strict runs pass on both sides because
the move is then only recorded against the baseline.

# Design

Under owner rule 346 (no load-sensitive checks gate; condition-based
waits only), the job keeps its `--strict` verdict but leaves the gating
selection: `disabled-optin` in `vibe-talk/tests.dag.yaml`, the label
`scripts/validate.py` excludes from every default and `--all` run while
naming it as skipped. It stays runnable by hand
(`make -C vibe-talk scroll-soak-browser`, `--include-disabled`, or a
direct dagrun of the fragment). No test code changes; the 28.1px move
itself — whether a real one-frame demote repaint or frame sampling
under load — is the robustness work this issue tracks.

# Acceptance Criteria

- Default and `--all` validation no longer select
  `vibe-talk.check.scroll-soak-browser`; it is named as skipped
  (timing-sensitive, owner rule 346).
- By hand it still runs end to end at 393x852 and 1280x800.
- Re-enable bar: 10/10 consecutive strict passes with the host above
  75% CPU, using condition-based waits (owner rule 346); then drop the
  `disabled-optin` label in a follow-up change citing this issue.

# Notes

[watermelon-full-preview] Filed from #1606
message-rerender-loses-formatting, whose gate this step blocked three
times (162/163 with this as the only red; gate ledger head 61a102ae,
plus the gate's own solo --rerun red). Sibling timing quarantines:
#1250 vibe-talk-timing-robust (the `disabled-optin` mechanism and its
return bar), #1210 ui-stability-no-async-shift (what the soak guards).
