---
title: "wrkviz-scoped-summary-site: the site build renders scoped summaries (summary_scopes/<scope>), not only summary_data"
status: open
priority: 3
issue_type: feature
created_at: 2026-11-10T02:25:00.000000000+00:00
updated_at: 2026-11-10T02:25:00.000000000+00:00
---

# Description

`wrkviz summarize` can write summaries for a narrower scope (for example coordinator sessions only) under
`summary_scopes/<scope>/`, but the site build reads only `summary_data`. On 2026-11-09 a one-day run summarized six
coordinators into `summary_scopes/coordinator`, and those summaries could not be shown on the site without
re-running summarize at full scope for the same teams and day.

# Design

- Let the site build take one or more scoped summary sources (`--summary-scope NAME`, or a config key) and render
  them next to, or instead of, the full-scope summaries. The scope is labelled in the UI so a reader can tell a
  coordinator-only summary from a full one.
- Where both exist for the same team and day, the full-scope summary wins unless the scope is selected
  explicitly. Never mix the two silently.
- Deep links (view, time window, project, team) carry the scope when it is not the default.

# Acceptance Criteria

- A fixture archive with a scoped summary for one team-day builds a site that shows it, labelled with its scope.
- A team-day with both scoped and full summaries shows the full one by default and the scoped one when selected.
- A deep link that names a scope restores it.
- The user guide documents the option.

# Notes

Filed for the lead coordinator (2026-11-09) as a follow-up to an owner request to browse a one-day coordinator
run on the site. For tonight that run is being re-done at full scope as an operation (no code change).
