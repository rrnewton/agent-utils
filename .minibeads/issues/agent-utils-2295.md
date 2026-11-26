---
title: 'vibe-talk-export-prompts: an incremental, idempotent, cache-only export of the owner''s prompts and agent responses to per-project, per-month markdown files'
status: open
priority: 2
issue_type: task
created_at: 2026-11-26T18:24:25.000000000+00:00
updated_at: 2026-11-26T18:24:25.000000000+00:00
---

# Description

Owner 2026-11-26: vibe-talk should also have an incremental and idempotent export
mode that keeps a full disk archive of the owner's prompts and the agent
responses to them, in a folder the owner can sync back to a laptop. The export
writes per project and per month, in chronological order. One project's
channels are exported together under that project's folder (a project may span
several channels, including its sibling channels), and a second project gets
its own folder; which channels belong to which project is configuration, never
hard-coded. The layout is
`<project>/prompts/YYYY/YYYY-MM-<project>-prompts.md` for the owner's prompts
and `<project>/ai_responses/YYYY/YYYY-MM-<project>-responses.md` for the agent
responses. Each file holds an H1 per day (`2026-11-24 prompts`, underlined with
`===`) and an H2 per prompt: `(HH:MM) user prompt` underlined with `---`, or,
when the prompt already carries an H2 (often `TODO: ...`), that heading adopted
and prefixed with `(HH:MM) `. Deeper heading structure inside a prompt is
demoted (an H1 or H2 inside a prompt becomes H3 or H4) so the day and prompt
headings stay the document structure. Where the cache records a reply edge,
a prompt links to the agent message it replies to (`Reply to [agent
2026-11-23 HH:MM](../../ai_responses/2026/2026-11-...md#anchor)`) and to the
next agent reply at its tail; anchors must resolve when clicked in Obsidian.
Responses may later move to per-week or per-day files (each message an H1 when
per-day) once the volume is known. Fast follow: an option to put a one-line
summary in place of `user prompt`, short enough that the whole heading fits a
72-character line after the 8-character `(HH:MM) ` prefix (at most 64
characters), styled after the owner's `TODO: ...` headers and produced by the
existing summary infrastructure (no slug; one message).

# Design

DESIGN FIRST: a design note to the coordinator before any code. The note
settles the CLI shape (`vibe-talk export-prompts ...`), how owner messages are
identified (the author ids already named in configuration, as the existing
cache-only readers do), the project-mapping configuration, the exact markdown
format with a worked example, the heading and demotion rules with the markdown
parser used, the anchor convention that Obsidian resolves (responses written
first, or a solid anchor convention), how reply edges are found in the cache,
the idempotence method (per-month content hash, atomic write), how much data
there is (message counts per month per channel, read from a copy of the cache
database), the summary fast follow, the tests (golden files, idempotence,
demotion, links) and the open questions for the owner.

The export itself is cache-only on the state-sync principle: it reads only
vibe-talk's local message cache and never contacts the chat provider. It builds
on the cache access and output conventions of the existing cache-only readers
(the prompts and search commands): the same configuration discovery, the same
owner identification, the same read-only cache open. Incremental and
idempotent: rerunning rewrites only the months that changed and leaves every
other month byte-identical, so syncing the folder stays cheap. The output root
and the channel-to-project mapping are both configuration, never hard-coded.
The output is private data: never written inside any git repository, output
directories mode 0700 and files 0600.

# Acceptance Criteria

- Design note approved by the coordinator before implementation code lands.
- `vibe-talk export-prompts` reads only the local message cache; it makes no
  provider call and needs no token, following the existing cache-only readers.
- Export is incremental and idempotent: a rerun over unchanged cache data
  rewrites no file, and a change in one month rewrites only that month's
  files; untouched months stay byte-identical.
- Files follow the per-project, per-month layout above, chronological, with
  the day H1, prompt H2, heading-adoption and demotion rules above.
- Reply links resolve in Obsidian (relative paths and anchors) wherever the
  cache records a reply edge.
- Output root and channel-to-project mapping come from configuration; the
  export refuses to write inside a git repository and writes directories
  0700 and files 0600.
- Tests: golden files for the markdown format, an idempotence test (second
  run changes no bytes), demotion tests, and link tests; gates stay green.
- Fast follow (separate change once the base lands): optional one-line
  summary headings as described, from the existing summary infrastructure.

# Notes

Owner request 385 (2026-11-26). The output root and the channel-to-project
mapping wait on the owner's answer; both are configuration so the wait blocks
no code. Response granularity (per-month, later perhaps per-week or per-day)
is revisited once the exported volume is known.
