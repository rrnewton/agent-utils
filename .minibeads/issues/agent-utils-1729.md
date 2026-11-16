---
title: 'readerbar-overlap-browser-setup-dep: the reader-bar browser check declares no contract-tools dependency'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- validation
created_at: 2026-11-16T13:32:54-05:00
updated_at: 2026-11-16T14:00:52-05:00
closed_at: 2026-11-16T14:00:52-05:00
---

# Description

[watermelon-full-preview] The `readerbar-overlap-browser` entry in `vibe-talk/tests.dag.yaml` (added by #1710) declares no `deps: [setup.contract-tools]` and none of the `jobs_flag`/`timeout`/`cpu_timeout` its sibling browser checks declare. Every browser walk serves a page build that needs the pinned esbuild/typescript in `vibe-talk/contract/node_modules`; without the dependency the step can start before that install runs whenever `node_modules` was deleted (which the lane cleanup rule requires after each validate/gate), and fails in about a second with "the page build needs the pinned esbuild". It reddened the #1722 vibe-talk-mention-link-not-clickable local validate and gate while every other browser step passed. An audit of every `*-browser` entry in `vibe-talk/tests.dag.yaml` found this the only one missing the dependency.

# Acceptance Criteria

- `readerbar-overlap-browser` declares `deps: [setup.contract-tools]` plus the same `jobs_flag`/`timeout`/`cpu_timeout` shape as `quote-dismiss-browser`, with a hint block like its siblings.
- Any other `*-browser` entry missing the same dependency is fixed in the same change (the audit above found none).

## Resolution

[watermelon-full-preview] Landed on main as `daaf2f7f`: `readerbar-overlap-browser` now declares `deps: [setup.contract-tools]` with `jobs_flag: ""`, `timeout: 360`, `cpu_timeout: 45` and a sibling-shaped hint block. The audit of every `*-browser` entry in `vibe-talk/tests.dag.yaml` found no other entry missing the dependency. In the #1722 owner gate the step passed with the fix in place.
