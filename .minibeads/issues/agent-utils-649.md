---
title: 'voice-agent-tools: answer the voice agent''s read tools from the cache, keep every spoken turn''s text, cumulative recap, pace and channel controls'
status: open
priority: 0
issue_type: bug
created_at: 2026-10-28T14:03:59.096441886+00:00
updated_at: 2026-10-28T14:03:59.096441886+00:00
---

# Description

[opus 5.5] Owner report 2026-10-28: recent_activity for all channels timed out on Slack (8 s deadline, reported ok with zero messages) while read_page worked; agent audio arrives without text (voice_text_missing audio_without_text), the final turn before hang-up never shows; the recap was 19 words; pace may not work on the internal stack; wants a channel selector with All on the voice view and a design audit of the voice agent tool surface.

Plan (each lands separately):
1. Every MCP read tool answers from the message cache immediately with a freshness note; refresh behind, single-flight; a failed/partial refresh is never 'no messages'. Target: recent_activity all channels 12 h < 300 ms.
2. Agent speech text reaches page and transcript for every spoken turn (including the last at hang-up), or is labelled precisely.
3. Recap: cumulative across recent sessions within a budget; policy stated on the page.
4. Pace: detect support per backend, grey where unsupported; slider back above the controls, synced with the stepper (#567).
5. Channel selector with All on the voice view scoping the agent's default channels.
6. Design audit ai_docs/transient/2026-10-28-voice-agent-tools/README.md.
