---
title: 'unpack-scroll-anchor: preserve the reader when no thread card is visible'
status: closed
priority: 1
issue_type: bug
created_at: 2026-10-25T21:58:38.659820020+00:00
updated_at: 2026-10-25T21:59:15.786870433+00:00
closed_at: 2026-10-25T21:59:15.786870289+00:00
---

# Description

[GPT-6 Codex codex-vibe-discord] Fixed the baseline threads-unpack-browser failure. cardAnchor had no anchor when the reader was deep inside unpacked replies and all cards were above the viewport; redraw lost the scroll position. Reused existing cardAnchor/placeCardAnchor with a scroll-offset fallback only when no card is visible. Original origin/main and candidate without fallback both fail identically (Done moves y=336.375 to y=484.375); candidate with fallback passes the unchanged Chromium suite in dark/light desktop and phone layouts, keyboard/Undo parity, and the 40-thread performance case. This before/after is the mutation control. No assertion was weakened.
