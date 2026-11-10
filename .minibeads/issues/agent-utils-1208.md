---
title: 'scroll-trace-layout-shift: layout shifts, row replacements and selection loss in the trace, sent by itself'
status: open
priority: 0
issue_type: feature
assignee: mu-scroll
created_at: 2026-11-10T14:16:59.330053764+00:00
updated_at: 2026-11-10T14:17:16.373865706+00:00
---

# Description

Owner reports (replies 296 and 298, 2026-11-10): reading on Android, "pretty frequently I see something cause what I'm looking at to move", and it has gotten worse. No scroll trace has reached the server yet (#1144 part 2b), so every scroll fix so far is confirmed only in emulation. The requirement now is UI stability: nothing the user is looking at moves unless the user acted.

Extend the scroll trace (same Scroll trace setting, page extended):
- a PerformanceObserver on layout-shift entries in the viewport: each shift's value, whether it had recent input, and its sources (a node descriptor with previous and current rects);
- DOM row replacements: a MutationObserver on the row containers naming removed and added row nodes for the same message id, with the code path where it can be tagged;
- selection-loss events: a selectionchange from a non-collapsed to a collapsed selection with no user input in the last ~500 ms, with what was replaced;
- AUTO-SEND: a layout shift of 0.01 or more, or a selection loss, in the viewport with no input in the last 500 ms posts the trace by itself (rate-limited, once per 60 s), so nothing needs tapping.

Selection loss itself is the companion issue; this one only makes it (and every other unmoved-by-hand shift) visible from the device.

# Design

Page-only extension of the part-2b scroll trace (web/src/view/channel/scrolltrace.ts and its wiring), behind the existing Scroll trace device setting:

- PerformanceObserver('layout-shift', buffered) where the browser has it; each entry with value >= 0.01 or any selection loss becomes a trace entry {kind shift, value, hadRecentInput, sources: node descriptor (tag/id/data-id), previous and current rects}.
- MutationObserver on the row containers (the channel lists, pinned/outgoing, transcript): childList removals+additions paired by message id within the same batch become {kind mutate, id, removed/added descriptors}; attribute/characterData churn is not traced.
- selectionchange listener: non-collapsed -> collapsed with no pointer/key/touch input in the last 500 ms (input stamps the trace already keeps) becomes {kind selection, anchor descriptor, text length - never the text}.
- Auto-send: on a qualifying shift or selection loss, post the current ring through the same send path as the Send button, at most once per 60 s (a clock through the existing port), and note it on the status line as the manual send does.
- The server's closed report shape grows only optional fields for the new kinds (validated, capped as before).

Tests: module tests for shift/mutation/selection capture, the 500 ms input gate, and the 60 s auto-send limit; a page test that a qualifying shift posts without a tap.

# Acceptance Criteria

- With Scroll trace on, a viewport layout shift >= 0.01 with no input in the last 500 ms is in the posted report with value, hadRecentInput, and source rects; one with recent input is not auto-sent.
- A selection lost with no input in the last 500 ms is in the report (descriptor and length, never text), and auto-sends.
- Auto-send fires at most once per 60 s however many shifts qualify; the manual Send still works.
- Row replacements for the same message id appear as mutation entries.
- Module and page tests pin each; validation path-selected (page-only) green.

# Notes

Follow-up to #1144 scrollback-typing-jump; companion selection-survives-updates filed next. Owner replies 296/298 (2026-11-10).
