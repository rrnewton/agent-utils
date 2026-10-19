---
title: 'wrkviz-transcript-container: store the extracted transcript projections as seekable multi-member gzip'
status: closed
priority: 3
issue_type: feature
labels:
- wrkviz
created_at: 2026-10-15T15:30:49.679967984+00:00
updated_at: 2026-10-19T08:39:34.807388535+00:00
closed_at: 2026-10-19T08:39:34.807388375+00:00
---

# Description

[opus 5.5] Stranded work found while retiring an old local checkout on 2026-10-15, now kept on archived/wrkviz-transcript-container (commit 6b863cd3, 2026-08-26) and adopted into the dev harness slot wrkviz-transcript-container (rebased onto main as 7431ceef; it merges textually clean). It moves extracted/transcripts/prompts.jsonl, messages.jsonl and system-inputs.jsonl into the multi-member gzip container with a .index.jsonl sidecar that schema-3 shards already use (seekable_jsonl.write_seekable_jsonl). Measured on a real 12-team archive: those projections drop from 139.2 MB to 29.5 MB, and a slice read such as messages --range 3978-3980 reads 5.4 MB instead of 23.0 MB. query.py drops _SeekableJsonlText for member-addressed readers with linear fallbacks when the sidecar is lost; the manifest keeps the uncompressed sha256/bytes and adds container fields; the exporter accepts old and new manifests. Main has none of this: query.py, transcript_export.py and seekable_jsonl.py are unchanged since the branch point. To land: fix the readers added since then in py/tests/test_timeline_claude_history.py (lines 238 and 347 read prompts.jsonl as plain text); re-check the merged README, USER_GUIDE, ARCHITECTURE and cli.py text; and decide deliberately that an existing archive must re-run extract-transcripts before prompts/messages queries read it, since queries refuse a plain-text manifest by name.

# Acceptance Criteria

Transcript projections are written and read through the seekable gzip container with sidecar index; queries on an old plain manifest refuse with the re-extract instruction; tests and docs updated; make validate passes.
