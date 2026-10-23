---
title: 'webapp-deep-links: the page''s URL selects the view, and a link can name one message'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-23T14:54:33.612862611+00:00
updated_at: 2026-10-23T14:54:33.612862611+00:00
---

# Description

[muse watermelon] The owner: "How linkable are our different views and messages? We started off with URLs like https://<host>/voice ... the 'voice' was dumb; we have a one-page webapp, right? So shouldn't there be some argument to the single page that selects the view and makes things linkable? Taking that further, shouldn't we have a link that jumps to a specific message in vibe-talk (separate from the upstream message), i.e. clicking that URL should open vibe-talk in a given view, highlighting a specific message? Then we'd have 'Copy vibe-talk link' as well. But how durable are those links? Specific to a vibe-talk server, or could another server, or a reboot of the same server, identify the same message with the same upstream (e.g. using the Slack id as the primary key, so it's the same in any vibe-talk server connected to the same upstream)?"

Wanted: the page's URL reflects and restores view (Main/All/Threads/thread/voice), channel, thread and an optional message to focus, in path or query (not the hash, which holds the pairing code), consistent with the #423 history entries. "/" is the app, "/voice" an alias. Message links carry durable upstream identity (provider kind + upstream channel id + upstream message id, + thread id), never server-local row ids, so any server on the same upstream resolves them and a reboot does not break them; an unloaded message is fetched around, then highlighted. "Copy vibe-talk link" in the menu beside the upstream link. Design note first (ai_docs/transient/2026-10-23-deep-links.md): scheme, durability, privacy (ids only, never text). Scheme documented in the README.

# Acceptance Criteria

URL round-trip for each view (open link -> same view/channel/thread; navigate -> URL follows; Back/Forward agree with #423). A deep link to an unloaded message loads around it and highlights it. Links hold upstream ids only: same link resolves against a fresh server instance on the same upstream data (durability test). Copy vibe-talk link copies it. / and /voice both serve the app. Chromium walks at desk and 393x852.
