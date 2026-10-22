---
title: 'search-unmatched-separators: show unmatched runs as expandable separators in every view'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
- ui
created_at: 2026-10-22T17:31:52.391824357+00:00
updated_at: 2026-10-22T23:45:05.225339087+00:00
closed_at: 2026-10-22T23:45:05.225338976+00:00
---

# Description

[muse watermelon] The owner: search filters instantly, but drilling into a thread it is easy to forget the filter is on and only a subset shows. Rather than hiding non-matching messages without trace, show separator lines like folded read messages: e.g. '33 unmatched messages' between hits. Be consistent in thread view and all views. In the thread list view treat threads as containers: three non-matching threads of five messages each say '15 unmatched messages in 3 threads'.

Message lists (Main, All, thread, Pinned, either column) already draw unmatched gap lines (#249 search-highlight seams). This issue completes the idiom: gaps in the Threads list and the two-column cards counting messages across whole threads, and tapping any unmatched separator expands that run as a fold-read separator does.

# Design

Reuse the seams pass (drawSeams/gapWords) and read-run idiom. Rule for both filters on: while a search filter chooses rows, read runs stay suspended as today (Fold read opens, Hide read steps aside on threaded providers), so a read-and-unmatched run is ONE separator labelled unmatched; expanding it shows those messages (read ones greyed as usual); closing the search restores the read mode. Revealed runs are remembered by message id under the current search key and forgotten when the search changes. Card gaps count root+replies per whole thread from what the page knows, saying 'at least' when a reply count is not exact.

# Acceptance Criteria

Page tests: gap counts across runs incl. combined rows; expanding a gap (middle and trailing) and forgetting on search change; Fold/Hide read + search together; search stays visible and gaps draw inside a thread; Threads list and two-column cards draw 'N unmatched messages in M threads' and expand. Chromium walk with screenshots at 393x852 and desktop clicking separators. make validate passes.
