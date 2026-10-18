---
title: 'settings-collapsible: every Settings section and the groups inside it fold, folded by default, and find-in-page still finds them'
status: closed
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-18T20:43:26.568581984+00:00
updated_at: 2026-10-18T22:20:19.991219002+00:00
closed_at: 2026-10-18T22:20:19.991218891+00:00
---

# Description

[opus 5.5] Owner, 2026-10-18 ("settings complaints"): "every top-level section collapsible, collapsed by default; its H2 subsections collapsed too; find-in-page still finds hidden text (use <details>/hidden=until-found)." Each Settings section has a heading that opens and folds it, and each headed group inside a section folds the same way; both start folded. The browser's find-in-page must still find words inside a folded section or group and open it (<details> or hidden=until-found, never display: none). A heading that folds is a real button for the keyboard and a screen reader, and says whether it is open. Every setting keeps its id and its storage key. Page tests with negative controls, a Chromium check that a search for folded text opens its group, and screenshots at 393x852, 1280x800 and 1920x1080.
