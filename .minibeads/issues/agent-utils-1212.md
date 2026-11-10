---
title: 'settings-debug-section: a Debug section in Settings holding the diagnostic switches'
status: open
priority: 1
issue_type: feature
assignee: mu-scroll
created_at: 2026-11-10T14:57:50.271824093+00:00
updated_at: 2026-11-10T14:57:50.271824093+00:00
---

# Description

Owner request (reply 306, 2026-11-10): 'In our next release, let's group things under a debug section under settings. We can have various debug options that we would turn on only for debugging the app.'

A Debug section in Settings holds the diagnostic switches - today Scroll trace (with its Send button) and the keyboard debug readout - all off by default, as they already are. Future debug options go in this section instead of spreading through the settings screens. Nothing changes behaviour: same stored settings, same switches, only their home moves.

# Design

Move the Scroll trace switch + Send button and the keyboard-debug switch (+ its readout element stays where the page draws it) into a new Debug section in Settings, following the existing settings section pattern (ids unchanged so wiring, ids manifest and stored settings are untouched). Fold into the #1209 selection fix's owner-UI gate, not the running #1208 gate.

# Acceptance Criteria

- Settings shows a Debug section holding Scroll trace and the keyboard debug switch, both off by default and working as before (same device settings).
- No other settings move; ids and stored values unchanged.
- Page tests for settings layout stay green; the move lands with the #1209 gate.
