---
title: 'fts-marker-residue: deletions by marker leave the detail=none index bloated past compaction and optimize'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- cache
depends_on:
  agent-utils-604: related
  agent-utils-602: related
created_at: 2026-10-28T05:15:00.880789252+00:00
updated_at: 2026-10-28T05:15:00.880789252+00:00
---

# Description

[opus 5.5] Found while measuring #604 server-side-search. Under cache schema 5 (detail=none, #602 history-settings-and-limits), FTS5 deletes made with secure-delete off (the writer's bulk paths: eviction, large erases and age passes, and until #604 the search_text rebuild) leave the index far larger than its content: a probe of 20,000 rows each rewritten once went 11.7 MB -> 41.8 MB, 20.5 MB after a full 'optimize', against 7.2 MB built afresh or with secure-delete on. In the cache, 20,000 rows rebuilt by marker left message_fts at 27.1 MB through the writer's compaction, the weekly merge, optimize and VACUUM, where a fresh 'rebuild' gives 10.1 MB. #604 works around it for the search_text rebuild (one FTS 'rebuild' at its end). Open: (1) whether erased rows' trigram entries survive (a privacy question: the erase promises the text leaves the file); (2) the eviction and erase paths, which still delete by marker; (3) whether compaction's 'merge into the oldest segment drops markers' assumption holds for detail=none. Probe code and numbers: tests/cache_bench.rs measure_folded_search_text_rebuild, and the handoff of #604.
