---
title: 'live-reply-in-main: a threaded reply arriving live is drawn in Main and missing from its thread until a reload'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-14T20:38:33.599591765+00:00
updated_at: 2026-10-15T04:37:22.766900771+00:00
closed_at: 2026-10-15T04:37:22.766900651+00:00
---

# Description

Owner report, 2026-10-14: in a chat space where an automated bridge answers the owner's messages in threads, the bridge's threaded reply to a question appeared live in the MAIN channel; opening the thread did not show it; after a reload the reply showed correctly inside its thread (the read path placed it right, the live path did not). Suspects: the live event or delta the provider (here the Google Chat bridge provider) delivers for the reply carries no thread, or the page places a live message without a thread in Main and never corrects it when a later read or delta names its thread. Fix wherever the placement is lost, and make the page move a message from Main into its thread when a later copy names the thread.
