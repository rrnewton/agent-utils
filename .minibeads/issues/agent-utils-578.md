---
title: 'search-status-clarity: the search loading view says what was searched, where, and how far back with no gaps'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- search
created_at: 2026-10-27T15:18:31.921931905+00:00
updated_at: 2026-10-27T15:18:31.921931905+00:00
---

# Description

[opus 5.5] Owner, 2026-10-27: "nothing in the search loading view makes sense... Right now it shows me no hits and a button 'Search before October 25'. I press it and it does nothing for a while and then shows THE SAME button." The readout ("Nothing in the 82 messages loaded so far, back to yesterday", "the most recent messages; older ones are not loaded", "Searching… 1 of 5 · Stop", "Flat view includes replies for at most 10 threads per page") does not say whether the 82 messages are a dense history or a sparse set, what "1 of 5" counts, or where the search ran.

Want: (1) every press of "Search before …" visibly advances the boundary, or says clearly why it could not; (2) the status says "Searched N messages, a complete history back to <date time> (no gaps), plus M scattered older ones", names where the search ran (this browser; the server and upstream search are designed in #521 server-side-search but not built), gives every progress number its unit, and has no page jargon; reply coverage in All said in thread terms; (3) a no-gaps claim only where the reads prove it.
