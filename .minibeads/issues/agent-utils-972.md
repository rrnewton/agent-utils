---
title: 'wrkslots-salvage-residue-clean: drop Git-ignored build output from fenced-residue salvage'
status: open
priority: 2
issue_type: task
created_at: 2026-11-06T18:31:52.717421786+00:00
updated_at: 2026-11-06T20:13:15.311366412+00:00
---

# Description

[watermelon-full-preview] Follow-up to the fenced-residue recovery: the residue receipt lists ignored_build_output (target directories, .dagrun, caches). Add a supported salvage clean that drops exactly those listed paths with evidence, so a busy host does not keep multi-GB build output in wrkslots-salvage forever. Never drop a residue entry the receipt does not list as ignored build output.

# Acceptance Criteria

A fenced-residue receipt's ignored_build_output paths can be removed by a supported command with evidence; every other salvaged entry is preserved.
