---
title: "doc-kind-buckets: recognise minibeads issues and GitHub gists as their own document kinds"
status: open
priority: 1
issue_type: task
created_at: 2026-11-10T00:55:14.000000000+00:00
updated_at: 2026-11-10T00:55:14.000000000+00:00
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
