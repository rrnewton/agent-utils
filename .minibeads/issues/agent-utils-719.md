---
title: 'gate-admission: counting admission with an exclusive class for load-sensitive steps'
status: open
priority: 1
issue_type: task
created_at: 2026-11-01T19:45:41.616494084+00:00
updated_at: 2026-11-01T19:45:41.616494084+00:00
---

# Description

[muse watermelon] Replace the one-gate-at-a-time flock with counting admission (default 3 concurrent gates) plus an exclusive class so load-sensitive steps never share the box. Drop-in scripts/gate with queue position and wait printed; history for gates/hour and wait measurement. Coordinator switches lanes over after review.
