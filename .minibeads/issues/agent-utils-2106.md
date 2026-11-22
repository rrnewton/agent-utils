---
title: 'vibe-talk-unified-cache-read-cli: one cache-only CLI for message search, owner prompts and thread listing'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
- cli
created_at: 2026-11-22T09:55:00-05:00
updated_at: 2026-11-22T09:55:00-05:00
---

# Description

The owner wants the read CLIs unified. Today the prompts command lists the owner messages with no keyword filter, and the search command goes through the running server. The owner asked for message-granularity keyword search, real time ranges, and easy thread enumeration.

# Design

A cache-only reader (no upstream chat calls, never the live server): search with AND of terms, quoted phrases, an any-of mode, case-insensitive, printing whole messages or a brief line with channel, thread (long id, slug, short id) and Eastern time, plus JSON; a mine filter so prompts becomes search with that filter and no terms; time by since (end defaults to now), until, and a from..to range, accepting a date or date and time in Eastern; a threads command listing threads active in a window (ids, slug, short id, counts, last activity) and a thread command listing or searching one thread; keep the channel filter. Existing commands keep working as aliases.

# Acceptance Criteria

- Tests against a fixture cache for AND/phrase/any matching at message granularity, the mine filter, since/until/range boundaries in Eastern time, threads listing and thread search; no network or server access in any path (asserted).

# Notes

Filed by the coordinator from an owner request; the lane refines this file.
