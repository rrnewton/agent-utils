---
title: 'live-reply-in-main: a threaded reply arriving live is drawn in Main and missing from its thread until a reload'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-14T20:38:33.599591765+00:00
updated_at: 2026-10-14T20:38:33.599591765+00:00
---

# Description

Owner 2026-10-14 ~13:20 PT, Google Chat instance (devbig030), space herdr-hermit-014 (channel 910000000000000105): the owner asked 'How goes it?'; the chat bridge's reply (as Meta Bot, threaded off that message in Google Chat) appeared live in the MAIN channel; opening the thread did not show it; after a reload it showed correctly inside the thread. So the live path placed a threaded reply in Main and not in its thread, while the read path placed it right. Suspects: the private Google Chat adapter's tail-fed event or delta (W9) carrying no thread for the reply, or the page placing a live message without a thread in Main and never correcting it when a later read or delta names its thread. Fix wherever the placement is lost, and make the page move a message from Main into its thread when a later copy names the thread.
