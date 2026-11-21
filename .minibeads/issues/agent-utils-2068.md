---
title: 'public-tree-confidential-name: remove a confidential model codename from the public tree and refuse it in future'
status: open
priority: 0
issue_type: bug
labels:
- repository
- security
created_at: 2026-11-21T13:55:00-05:00
updated_at: 2026-11-21T13:55:00-05:00
---

# Description

A confidential model codename appears in a few files of the public tree (a prompt glossary, two code comments and two issue files).

# Design

Remove every occurrence without rewriting history, and add a repository check that refuses it (and other confidential names) in the public tree without the name itself appearing in the repository.

# Acceptance Criteria

- No occurrence in the tracked tree.
- A check refuses it, tested with a synthetic name.

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.
