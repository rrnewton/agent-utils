---
title: 'own-read-stored: own messages start read in the stored read state, then toggle like any other'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- read-state
created_at: 2026-10-26T20:04:53.984929174+00:00
updated_at: 2026-10-26T20:04:53.984929174+00:00
---

# Description

[opus 5.5] Follow-up to #536 mark-own-messages-read. Owner, 2026-10-26: "I thought that we have a single bit of state for whether a given message is read or unread and that is stored in our state server side. But are you telling me that we don't track that state for my sent messages? The Mark is read doesn't actually change their state. It just sort of changes, the UI and how I view them? ... I think it was simpler to just do what the setting says which is to mark them as read IE record the state that they are read and then I can toggle one individually and it's perfectly valid. But it's just a question of whether the state starts as read or starts as unread when the message is ingested"

Agreed design:
1. At first sight of a message by the server (push, relist, backfill, posted through vibe-talk), its read bit is initialised: read if it is the owner's own message and "Mark my own messages as read" is on, else unread. Written once, never re-applied: a message he marks unread stays unread across restarts, relists and re-ingest.
2. After that it is an ordinary read bit: Done and e work on own messages again; the computed-on-read rule goes, the shared who-is-mine identity logic stays.
3. The setting governs messages first seen from then on. A one-time "Apply to existing messages" action next to it marks his existing unread messages read (confirmed first, undoable).
4. Own-message initial marks must not evict real Done marks from the dismissal retention cap.
5. Counts, icons, /todo, thread cards and agent counts all read the one stored bit.
