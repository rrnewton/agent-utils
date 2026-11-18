---
title: 'vibe-talk-settings-density-and-channel-header: denser settings, channel selector in the header'
status: closed
priority: 0
issue_type: task
labels:
- vibe-talk
created_at: 2026-11-17T00:00:00.000000000+00:00
updated_at: 2026-11-17T00:00:00.000000000+00:00
---

# Description

The settings menu has too many collapsables: a section with a single
control still spends a heading and a fold on it. The Channels view does
not make visually obvious that all its controls are specific to the
channel chosen there; the header should read
`Channel Settings: [All Channels]` with the selector in the header,
defaulting to All Channels.

# Design

- A settings group with a single control shows the control inline with
  no heading and no collapsible; collapsible subheadings remain only
  where there are two or more related items (Debug is the model).
- Tighten settings spacing without changing any control, storage key,
  or setting semantics: this is a presentation change only.
- The Channels section header reads "Channel Settings:" and holds the
  channel selector (`#settings-channel`) in the header row. The
  selector defaults to All Channels when that choice is offered; the
  controls below apply to the selected channel as before.

# Acceptance Criteria

- Single-control groups are visible inline once their section is open,
  with no fold toggle or heading for the group.
- Multi-control groups (for example Debug) keep a collapsible heading.
- The Channels header shows "Channel Settings:" with the channel
  selector in the header, defaulting to All Channels.
- Existing behaviour and storage keys are unchanged.
- Before/after screenshots cover the whole settings menu at phone and
  desktop in light and dark themes.

# Notes

Owner request, thread 154. Presentation only; no setting moves.

## Resolution

Single-control settings groups are shown inline with no heading or
collapsible (Debug and other multi-control groups keep theirs), spacing
is tightened, and the Channels header reads "Channel Settings:" with
the channel selector in the header defaulting to All Channels. Existing
behaviour and storage keys are unchanged.
