---
title: 'gchat-mention-defuse: defuse mention syntax in posts to a Google Chat bridge instead of having them refused'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- send
created_at: 2026-10-23T01:16:57.709043305+00:00
updated_at: 2026-10-23T01:16:57.709043305+00:00
---

# Description

[opus 5.5] A Google Chat bridge refuses a post whose text contains mention syntax Google Chat could expand ('@word' not preceded by a letter, digit, '_', '@' or '.', and '<users/...>' tokens) with HTTP 400. Text quoted from a Google Chat message carries such names ('@Ada Lovelace' as the message reads), and so does a reply that addresses someone, which is more usual in threads. Every such send was refused.

Wanted: for a provider that declares this rule, vibe-talk defuses the syntax before the post is cut in parts: a WORD JOINER (U+2060) after the '@' of a notifying '@word', and after the '<' of '<users/'. The text reads the same and nobody is notified. A Discord-protocol provider whose markup is Google Chat's declares it by default (defuse_mentions, which an operator may set either way); every other provider is untouched.

# Acceptance Criteria

Unit tests for the transform (each pattern, emails and Discord <@id> untouched, idempotent); the post route defuses only for a provider that declares the rule; config parsing tests.
