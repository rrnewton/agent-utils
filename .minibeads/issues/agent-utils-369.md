---
title: 'pairing-link: a one-time, short-lived link that signs a phone in'
status: open
priority: 0
issue_type: feature
labels:
- vibe-talk
- pairing-link
created_at: 2026-10-19T11:56:36.697212188+00:00
updated_at: 2026-10-19T11:56:36.697212188+00:00
---

# Description

Owner request (2026-10-19): a link an agent can send in chat that is good only for a short time and, opened on a phone, signs the app in.

- POST /api/v1/pair (write token) mints a single-use code (128+ random bits, URL-safe). The server keeps only its SHA-256, in memory, with an expiry (default 300 s, max 600 s, settable per request). Several codes may be live at once.
- POST /api/v1/pair/redeem {code} (no token) returns the write token once and burns the code. Constant-time comparison; failed redeems are rate-limited per client address (429).
- No code or token is ever logged; mint, redeem and expiry log a short hash prefix only.
- The page reads #pair=<code> from the fragment, strips it with history.replaceState before any request, redeems it, stores the token under vibe-talk.token as sign-in does, and says plainly when the link expired or was used.
- vibe-talk pair-link: reads the server's config, POSTs to the local server on its bind address and prints only the link (from server.public_base_url / public_url or --base-url) and its expiry. --ttl SECONDS.
- README: 'Pairing a phone', for agents setting a server up for a human.
