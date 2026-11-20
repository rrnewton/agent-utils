---
title: 'hooks-dialog-shell-prompt-veto: banner-like lines above the dialog can veto a real answer'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-11-20T00:00:00+00:00
updated_at: 2026-11-20T00:00:00+00:00
---

# Description

[watermelon-full-preview] From the third review of #1932 (agentctl-muse-hooks-trust-auto-decline, landed 62f03e1d): the turn-marker veto above the dialog title also fires on lines that are not transcript: a shell prompt starting with ❯ (starship/pure-style prompts) left above Muse's banner, or old Muse transcript left on screen at a revive, vetoes the answer. This fails closed (the launch names `waiting: hooks trust` and a person can clear it), so it is a liveness gap, not a safety one. The two editions also differ in Unicode handling around the markers (Python code points vs Rust char prefix stripping), which a fix should reconcile and pin with a shared scenario.

# Design

- Narrow the veto to transcript evidence that cannot be a startup remnant (for example requiring the marker line to sit below the banner, or matching full prompt shapes), keeping the verbatim-quote scenarios (including #1948 hooks-quote-scrolled-marker) key-free.
- Pin both editions to identical acceptance on the shared scenario table, including the Unicode marker cases.

# Acceptance Criteria

- A real dialog with a ❯-led shell prompt above the banner is answered in both editions.
- Every quoted-dialog scenario still takes no key, and the editions agree on each.

# Notes

Follow-up to #1932 agentctl-muse-hooks-trust-auto-decline; not a landing blocker for it (review verdict SAFE).
