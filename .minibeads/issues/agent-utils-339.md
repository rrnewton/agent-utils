---
title: 'search-multi-word: unquoted words are ANDed terms and quotes make a phrase'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-15T19:03:00.290130491+00:00
updated_at: 2026-10-15T19:03:00.290130491+00:00
---

# Description

[opus 5.5] Owner, 2026-10-15: searching for agent-utils website matches only the literal string with a space. Use the familiar web-search idiom: unquoted words are separate terms that must all appear (A B means contains A and contains B, in any order, anywhere in the message); a double-quoted part is a literal phrase ("A B"). Mixed queries combine both (A "B C"). Keep case-insensitivity and the existing highlighting (highlight each matched term and phrase). Apply it to the page's local search and to the server-side search route (#262 search-load-older) so both agree. Tests: two words in either order, a phrase, a phrase plus a word, an unbalanced quote (treat as a literal), punctuation inside words like agent-utils.
