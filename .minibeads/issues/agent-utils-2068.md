---
title: 'public-tree-confidential-name: remove a confidential model codename from the public tree and refuse it in future'
status: closed
priority: 0
issue_type: bug
labels:
- repository
- security
created_at: 2026-11-21T13:55:00-05:00
updated_at: 2026-11-21T14:38:02-05:00
---

# Description

A confidential model codename appears in a few files of the public tree (a prompt glossary, two code comments and two issue files). In this file and everywhere else it is called only "the model codename".

# Design

Remove every occurrence without rewriting history, and add a repository check that refuses it (and other confidential names) in the public tree without the name itself appearing in the repository.

- Removal first: drop the glossary entry, refer to the earlier summarizer issue by number in the two code comments, and give the two issue files a neutral slug (`muse-model-summarizer`), keeping their numbers.
- Guard second, as its own change: salted SHA-256 digests of lowercased tokens, scanning tokens of tracked files (split on non-alphanumerics, so a hyphen-joined use is caught), with an optional private extra-names file outside the repository for local runs. Its test uses a synthetic name, never the real one.

# Acceptance Criteria

- No occurrence in the tracked tree (whole-word, case-insensitive, and per token).
- A check refuses it, tested with a synthetic name.

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.

[watermelon-full-preview] Refined by mu-codename: removal covers 13 token occurrences in 5 files; history is not rewritten (that decision stays with the owner).

## Resolution

[watermelon-full-preview] Done. Removal landed on main as 04f9832d: whole-word and per-token counts over the tracked tree are 0. This commit adds the guard: `scripts/check_confidential_names.py` (salted SHA-256 digests of lowercased tokens; files and landing-range commit messages; optional private extra-names file; synthetic-name self-test), wired beside the client-names hygiene steps in the Makefile, the validation DAG, the selector and the component manifest. A short digest is brute-forceable, so this is a tripwire against accidents, not secrecy against a determined reader. Proposed, not done here: mirror the same digest check over the landing range's commit messages in the landing tool, which lives outside this repository.
