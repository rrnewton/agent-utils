---
title: 'relay-mirror-autolink-escape: keep Slack link markup out of signed relay envelopes'
status: open
priority: 2
issue_type: bug
created_at: 2026-11-06T02:00:00.000000000+00:00
updated_at: 2026-11-06T02:00:00.000000000+00:00
---

# Description

The agent-teams relay mirror posts each envelope as JSON inside a code block. Slack still auto-links text inside it, and that broke two posts on 2026-11-05:

1. **Signature failure.** An envelope whose `key` was `917-918-20261105` was stored by Slack as `<tel:917-918-20261105|917-918-20261105>`. `key` is part of the v2 signed material, so the receiver quarantined the post with "envelope signature does not verify". The sender's stored copy verifies, and `relay` (pubkey, sig, sig2) was byte-identical. The mirror already escapes `:` as `:`, but nothing stops a phone-, date-, email- or URL-shaped run from being linked.
2. **False hash mismatch.** Two message bodies literally contained the text `<tel:...|...>` (they were explaining case 1). The reader's markup undo rewrote that literal text, so the body no longer matched `body_sha256`, and the receiver quarantined the post and raised an owner alarm. The reader cannot tell Slack's markup from literal text that looks like markup.

Both messages also arrived directly over serve, so nothing was lost. But each case quarantines a legitimate post and can raise a false tampering alarm.

# Design

- **Mirror (sender):** in every JSON string value it posts, body included, escape the characters Slack acts on:
  - `<`, `>` and `|` as `<`, `>` and `|`;
  - digits, or `-`, as `\u00XX` wherever needed so no run looks like a phone number, date, email or URL to Slack's linker (escaping every digit and `-` in string values is acceptable).
  
  This is still valid JSON that decodes to the same values, so signatures and hashes are unchanged. Once `<`, `>` and `|` are always escaped, any `<...|...>` seen on read is provably Slack's.
- **Reader (receiver), as defence in depth:** before parsing, undo Slack's `<tel:X|X>`, `<mailto:X|X>` and `<http...|X>` (and bare `<http...>`) wrapping inside the code block. Only do this where the result reproduces the exact signed bytes; never guess.
- Keep compatibility with posts from older mirrors that lack the new escaping.

# Acceptance Criteria

- Regression tests round-trip through a simulated Slack rewrite:
  - key `917-918-20261105`;
  - an email-shaped key;
  - a URL-shaped key;
  - a body that literally contains `<tel:917-918-20261105|917-918-20261105>`.
  
  Each must verify and deliver, with no quarantine and no alarm.
- The first three still verify after the reader's undo even when posted by a mirror without the new escaping.
- No change to the envelope schema or signed material.

# Notes

Reported 2026-11-05 8:35 PM ET. Found by diffing the stored relay post against the sender's outbox copy. Both quarantined copies were duplicates of messages already delivered directly.
