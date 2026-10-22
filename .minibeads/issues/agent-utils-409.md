---
title: 'open-upstream-link: open a message in Slack, Google Chat or Discord from its ⋯ menu'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-22T02:39:01.319941060+00:00
updated_at: 2026-10-22T02:48:23.310439211+00:00
---

# Description

[muse watermelon] The owner, on #408 copy-upstream-link (landed 2026-10-22): "I like Copy in slack so much I would like an Open in slack direct link as well." Then, the same day: "Actually can't we JUST have a hyperlink that says 'Open in Slack' and then you can right-click / copy link it in the browser?"

So the ⋯ menu holds ONE item for a message's own address in its service: a real hyperlink labelled "Open in <service>" ("Open in Slack", "Open in Discord"), replacing #408's "Copy link in <service>" item. It is a genuine anchor with the server-formed upstream_url as its href, target=_blank and rel=noopener noreferrer, so it opens in a new tab or window — leaving the installed PWA for the browser or the service's own app rather than navigating the PWA away — and the browser's own right-click, long-press and copy-link-address work on it. Present exactly where Copy link was: the same upstream_url, absent where none came back, never shown disabled. In Chromium app mode (display: standalone) an anchor with target=_blank opens a separate page and leaves the app page where it was, as window.open(url, "_blank", "noopener,noreferrer") does, while a plain anchor navigates the app itself away — measured, Chromium 149.0.7827.55.

The keys: l still copies the address (a keyboard has no right-click to copy the link with), and Shift+O opens it (bare o opens or closes the message here; its shifted form opens it in its service). Both are listed in the ? help; on a row with no address both do nothing. The link, the copy key and the open key all read the address from the one upstreamUrlOf read; no second URL logic.

Scope, at the owner's word: Slack and Discord. Google Chat is going away soon and gets no work here — no fixtures or tests for it; the server's already-landed address formation is untouched. Discord messages whose address the server forms (discord.com/channels/<guild>/<channel>/<message>, #408) offer "Open in Discord" exactly as Slack messages do, tested with the fake Discord provider.

# Acceptance Criteria

A message carrying upstream_url offers a real "Open in <service>" hyperlink in its ⋯ menu where "Copy link in <service>" was, and no Copy link item; the anchor's href is the exact upstream_url with target=_blank and rel=noopener noreferrer, and choosing it opens that address in a new tab or window with window.opener null while this page stays put. A message with no upstream_url offers no such item. l on the highlighted row still copies the address and says so; Shift+O opens it; on a row without one both keys do nothing; the ? help lists both. The page test pins presence, absence, href/target/rel, the l copy and the Shift+O key (fake Discord provider); a Chromium walk at 1280x800 and 393x852 (Slack-named channel) checks the menu, opens the item and the key, and reads the new page's URL and opener. #408's tests and user-facing text say Open, not Copy link.
