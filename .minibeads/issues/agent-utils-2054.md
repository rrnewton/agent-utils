---
title: 'vibe-talk-reader-kepler-flaky: kepler read-aloud mispronounces and sometimes voices LLM asides'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-11-21T11:45:00-05:00
updated_at: 2026-11-21T11:45:00-05:00
---

# Description

The owner reports the kepler-based read-aloud mispronounces words and sometimes says things like 'maybe in Spanish'. The kepler path normally speaks the sent text by a verbatim fast path, but falls back through its language model when that path misses.

# Design

First measure: capture the spoken transcript the voice stack returns per read and diff it against the text sent, counting fallthrough. Options for an LLM-free read-aloud path are listed for the owner's choice; the speech-to-speech prod-tier ask is drafted in the harness repo for the owner's review.

# Acceptance Criteria

- The behaviour described under Design, covered by a deterministic regression test (condition-based, no wall-clock budgets).

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.
