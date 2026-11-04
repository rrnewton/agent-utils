---
title: 'vibe-talk-private-docs: serve unpublished documents from the vibe-talk server, with a CLI and a skill to add them'
status: open
priority: 2
issue_type: feature
created_at: 2026-11-04T22:59:15.719666824+00:00
updated_at: 2026-11-04T22:59:15.719666824+00:00
---

# Description

The owner (2026-11-04): "Sometimes I don't WANT to publish a doc to [a paste service] or github. But I want to read it on the go." The vibe-talk server should serve private documents from its own state directory: a `documents/` dir beside its durable state, listed in the Documents view. It should also have a CLI, and a skill so an agent can "publish a private document to vibe-talk".

# Design

See the design's "Private documents" section: `ai_docs/transient/2026-10-26-documents-view/README.md`. In short:
- Storage: `documents/` beside `[storage] path` (or `[documents] private_dir`, absolute, 0700), one file per document plus a `.meta.json` sidecar. Markdown, text and HTML only.
- CLI: `vibe-talk docs add FILE [--name] [--title]` prints the private URL and document key; `vibe-talk docs path NAME` prints the path to write plus the URL; `docs list`, `docs rm`, and `--json` on each.
- A `private` document kind, keyed `private/NAME`, in the same index; it is read with the existing token (no new access tier).
- Skill `skills/vibe-talk-private-docs`, shipped together with the CLI.

# Acceptance Criteria

- Server/CLI tests: add, replace (version bump), path-then-write pickup, list, rm, type refusal, URL and key output, and a 0700 directory.
- The Documents view lists and opens private documents (page part after, and following, #624 voice-page-modularity's rule).
- The skill ships with the CLI; the userguide and help document the commands; gate passes.

# Notes

P2, owner TODO (reply 147) via the lead, 2026-11-04. The server part may start before the page part, as with #628 documents-index-server. No lane yet.
