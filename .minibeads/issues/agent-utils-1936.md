---
title: 'hooks-quote-scrolled-marker: a quoted hooks dialog whose quoting turn scrolled off still gets one Down'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-11-20T00:00:00+00:00
updated_at: 2026-11-20T00:00:00+00:00
---

# Description

[watermelon-full-preview] From the third review of #1932 (agentctl-muse-hooks-trust-auto-decline, landed 62f03e1d): the hooks-dialog parse vetoes a verbatim quote when a turn marker (❯/◆/…) sits above the quoted title. When the quoting turn has scrolled off the visible screen, no marker remains above, the quote parses as the dialog, and the answer sends exactly one Down before the cursor check stops it. No Enter is ever sent in this scenario; the harm is one stray Down in a working lane's pane (for example cycling a composer history or scrolling a view).

# Design

- Strengthen the quote veto so a dialog block with transcript-like content above it never takes a key, even with no marker visible: candidates are requiring the count line (see #1937 hooks-dialog-require-count-line) plus a startup-banner shape above the title, or refusing when anything at all sits above the title except Muse's known banner lines.

# Acceptance Criteria

- A verbatim quote at the bottom of a working lane's screen with the quoting turn scrolled off takes no key in either edition.
- The real Muse 1.4.25 capture (banner above the title) is still answered.

# Notes

Follow-up to #1932 agentctl-muse-hooks-trust-auto-decline; not a landing blocker for it (review verdict SAFE).
