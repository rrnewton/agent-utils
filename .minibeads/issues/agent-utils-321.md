---
title: 'slack-emoji-shortcodes: show standard Slack emoji shortcodes as emoji'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-15T06:03:00.407145513+00:00
updated_at: 2026-10-15T06:03:00.407145513+00:00
---

# Description

Found by a read-only exercise of a Slack deployment on 2026-10-14 (the Slack move's parity stage). Slack returns emoji as shortcodes, so the page shows ':crab:' and ':slightly_smiling_face:' literally and speech says ':slightly smiling face:'. In the Slack reader (or rendering), map standard shortcodes (including skin-tone suffixes) to Unicode for display and speech, leave custom workspace emoji as their :name: text, and never change what is posted. Tests with standard, skin-tone and custom shortcodes.
