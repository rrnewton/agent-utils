---
title: 'existing-tab-adoption: make discovery and post-adoption guarantees unambiguous'
status: open
priority: 2
issue_type: task
external_ref: gh-123
created_at: 2026-11-15T17:47:04.000000000+00:00
updated_at: 2026-11-15T17:47:04.000000000+00:00
---

# Description

Adopting an existing terminal tab works once exact identity is known, but quickstart and adopt help do not show how to discover the four required assertions, adopt NAME help calls NAME a registered session name, the guide's status/goal native-availability claim does not match which command inspects native goal state, and a duplicate native session reported by a second pane after adoption blocks observation (status/read, global resolver) but not submission (send, pending drain, goal), whose wrapper enumerates only the saved workspace in both editions.

# Design

Put the read-only discovery sequence (pane list/get, workspace get; workspace ID for the terminal backend vs workspace label for adopt --workspace; tab vs exact agent pane) into quickstart and adopt --help. State that NAME is a new local registry name and adoption renames nothing in the backend. Distinguish goal submission, confirmed delivery, native goal state, and completion, naming which command reports native availability. Make the submission path apply the same provider-local native-session uniqueness as observation (global enumeration for the session-uniqueness question, exact-pane assertion unchanged), refusing delivery while retaining safely pending work.

# Acceptance Criteria

- Quickstart and adopt help are self-contained for the existing-tab path, including consistent --registry use.
- A fixture adopting one pane with a session, then exposing a second pane with the same session in the same harness, makes send, drain, and goal submission refuse and keep work pending (both editions), while an unrelated session and the same ID string under a different harness remain deliverable.
- Foreign stop still only unregisters/archives and leaves pane and process running, refusing when live identity cannot be verified.

# Notes

[watermelon-full-preview] Filed from GitHub #123 existing-tab-adoption (https://github.com/rrnewton/agent-utils/issues/123) during Request 098 triage. Non-goals from that audit remain: no auto-adoption by cwd/title, no rename/takeover, no relaxed identity assertions. Adjacent closed work (#928 native-session-collision, #916/#948 session capture) does not cover the post-adoption submission gap.
