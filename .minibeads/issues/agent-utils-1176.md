---
title: "doc-kind-buckets: recognise minibeads issues and GitHub gists as their own document kinds"
status: closed
priority: 1
issue_type: task
created_at: 2026-11-10T00:55:14.000000000+00:00
updated_at: 2026-11-10T12:02:34.000000000+00:00
closed_at: 2026-11-10T12:02:34.000000000+00:00
---

# Description

Owner reply 282 (2026-11-09 7:22 PM ET): minibeads issues are hyperlinked and appear as regular GitHub markdown documents inside the documents view. The owner would like a heuristic for recognising minibeads issues and showing them as a different document type of issue rather than a markdown file. They happen to be represented as markdown files, but they can be treated separately; with filters on document types, separate buckets for markdown versus pastes versus issues are expected. GitHub gists could be in the same category as pastes.

Today `vibe-talk/src/documents.rs` recognises only GitHub blob markdown/files, pastes under configured prefixes, and a reserved Google Docs kind. A `.minibeads/issues/<prefix>-<n>.md` blob link folds as ordinary markdown, and a `gist.github.com` link folds as nothing.

# Design

1. Add a new `DocKind` for issues (`github_issue` wire name, alongside the unchanged `github_markdown`, `github_file`, `paste`, `google_doc` names): a GitHub blob link (and its `raw.githubusercontent.com` twin) whose path is exactly `.minibeads/issues/<prefix>-<n>.md`, where `<prefix>` is a valid name and `<n>` is decimal digits. A path heuristic alone classifies; fetched frontmatter (title, status) only enriches the card. The card shows its own icon, `#<n>` plus the title and the status (open, in_progress, closed), and the reader renders the frontmatter as an issue header, not raw YAML.
2. Recognise `gist.github.com` links as documents in the Pastes bucket: a new kind fetched with the GitHub token like the other GitHub kinds (raw content through the API; a multi-file gist shows the first file, or all files concatenated). URL forms: `gist.github.com/<owner>/<id>`, with optional revision segment, and the `gist.githubusercontent.com` raw form.
3. Add type filter buckets in the Documents view, consistent with the existing Documents control bar: Markdown | Pastes (pastes plus GitHub gists) | Issues (plus an "other files" bucket for `GithubFile` if it needs a home). Filtering is a page-side view over the kind field; the server wire already carries it.
4. Wire compatibility: existing kind names stay. Old cached fold rows re-classify on read or refresh, with no migration that breaks rollback (an unknown older kind name must not be assumed; the fold keeps loading).

# Acceptance Criteria

- A GitHub blob link to `.minibeads/issues/agent-utils-1176.md` classifies as the issue kind, shows an issue icon, `#1176` plus its title and its status once fetched, and renders an issue header in the reader instead of raw YAML.
- Near-misses stay ordinary markdown: `.minibeads/other/x.md`, `.minibeads/issues/x.txt`, `.minibeads/issues/no-number.md`, and a missing `.md` suffix.
- A `gist.github.com` link appears in the Pastes bucket and fetches through the GitHub token; a multi-file gist shows its content without error.
- The Documents view can filter to Markdown, Pastes, and Issues separately; "other files" (if kept) holds the remaining GitHub files.
- Classifier unit tests cover issue paths, near-misses, and gist URL forms; a wire/serde round-trip covers the new kind names; a page unit test covers the buckets; a browser walk shows one item of each kind.
- Old cached rows still load, and existing `github_markdown`, `github_file`, `paste`, `google_doc` rows keep their kinds.

# Notes

Owner reply 282, 2026-11-09 7:22 PM ET. Server work lives in `vibe-talk/src/documents.rs`; the Documents view lives under `vibe-talk/web/src/view/documents/`. Page-code edits are named to the coordinator before editing (concurrent Documents control-bar and voice-page work).

## Follow-up gate verdict (2026-11-10)

[watermelon-full-preview mu-docbuckets] Landed on main as f01bc7ff after the owner-priority path-selected gate passed (153 passed, 0 failed). The fast-track follow-up `validate.py --all --keep-going` run (owner priority, gate parent 2103516 / ticket 2103521, log /tmp/docbuckets-gate-all-owner.log) finished its main phase: dagrun FAIL, 234 passed, 8 failed in 1537.9s. Every #1176 doc-kind-buckets touched step passed, including vibe-talk.check.documents-buckets-browser, vibe-talk.check.documents-browser, vibe-talk.check.documents-read-browser, vibe-talk.test.unit (3181 passed), all vibe-talk.test.page slices, and vibe-talk.check.contract. No touched step is red, so no fix-forward is required.

The 8 failures are all in untouched code and show load/census time bounds under host load:

- vibe-talk.check.keyboard-browser: CPU-TIMEOUT over 60s cpu (107s)
- vibe-talk.check.two-column-browser: search-replies-place-1280, the cards walk scrolled the left column
- packages.standalone.wrkslots: wrkslots remove refused, /proc scan after the path fence exceeded its 30-second bound
- python.wrkslots.shard-1: test_remove_write_rust.py::test_an_older_reader_refuses_only_an_outstanding_owner_journal, /proc scan before the path fence exceeded its 30-second bound
- python.wrkslots.shard-2: test_remove_write_rust.py::test_remove_matches_python, same /proc scan bound
- python.wrkslots.shard-6: test_reclaim_slot_state.py::test_remove_reclaims_state_home_and_bundles_unpushed_git and test_remove_write_rust.py::test_remove_forced_churn_refuses_like_python, same /proc scan bound
- python-lifecycle.host.shard-0: test_lifecycle.py::test_remove_agent_batch_removes_disk_image_slots
- python-lifecycle.host.shard-2: test_lifecycle.py::test_validate_complete_still_refuses_live_path_use_after_owner_dies, read-only census exceeded its 120.0s operation-wide bound (elapsed 120.1s)

Per coordinator direction, solo reruns of these 8 untouched failures were skipped, and the queued gate-exclusive phase (cross steps for agentctl/herdr-run, untouched by this change) was stopped before admission. scripts/check_client_names.py run bare before this record: ok, 4038 files name no consuming project.
