---
title: 'vibe-talk-appearance-copy: fix the Appearance help text and the repeated label'
status: closed
priority: 0
issue_type: task
labels:
- vibe-talk
- ui
- settings
created_at: 2026-11-18T13:57:05-05:00
updated_at: 2026-11-18T13:57:05-05:00
---

# Description

[watermelon-full-preview] Follow-up to #1845 vibe-talk-appearance-setting (landed cfbfe817, live). Coordb's review of the screenshot matrix found the state line under Settings > Messages > Appearance reads as if a forced choice followed the system ("Light, whatever your system says."), and "Appearance" is said three times on the one control (section, subsection, label).

# Design

- Per-choice state words: Light "Always light, even when your device is set to dark."; Dark "Always dark, even when your device is set to light."; System "Follows your device's light or dark setting." The not-kept suffix stays for a refusing device.
- Drop the duplicate `<label for="appearance">Appearance</label>`; the subsection heading names the control and the select keeps an `aria-label`.
- Keep the group under Messages: the choice is synced, and Screen and keys is the one section marked "(client side)"; moving a synced choice under that heading would say the opposite of where it is kept.

# Acceptance Criteria

- Each choice shows its new sentence in Settings, at phone and desktop sizes in light and dark, captured on a test instance and inspected.
- "Appearance" appears as the subsection heading only, with no second label over the select.
- The page and module tests that hold the wording pass.

# Notes

Owner-priority, path-selected gate.

## Resolution

State words now read per choice (Always light / Always dark / Follows your device), the
duplicate label is dropped (the select keeps an aria-label), and the group stays under
Messages because the choice is synced while Screen and keys is the "(client side)" section.
