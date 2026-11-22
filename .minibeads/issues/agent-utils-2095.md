---
title: 'agent-transcript-archive-density: measure whether the wrkviz archive is denser than raw transcripts before deleting any'
status: open
priority: 2
issue_type: task
labels:
- wrkviz
created_at: 2026-11-22T06:05:00-05:00
updated_at: 2026-11-22T06:05:00-05:00
---

# Description

The owner's hypothesis is that the wrkviz archive format is denser than raw agent transcripts; Muse sessions grow about 3.5 GB a day and Codex and Claude transcripts are 65 GB. Before any deletion he asked for a sample measurement.

# Design

Sample one Muse session older than a week, plus one Codex and one Claude transcript: import each into an archive, verify the archive reads back, and compare the bytes deleting the original would free with the bytes the archive grows by. Report the numbers either way. Delete nothing in this issue; a bulk import and delete is a separate decision for the owner.

# Acceptance Criteria

- The three sample measurements are reported with method; nothing is deleted.

# Notes

Filed by the coordinator from owner directions relayed by a consumer's lead; the owning lane refines this file as it works.
