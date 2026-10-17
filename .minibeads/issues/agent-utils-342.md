---
title: 'settings-server-hostname: show the server host prominently at the top of Settings'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-17T11:21:32.735541303+00:00
updated_at: 2026-10-17T11:21:32.735541303+00:00
---

# Description

[muse watermelon-preview] Owner 2026-10-17: running vibe-talk on multiple servers, I forget which one this is. Show the server hostname prominently at the top of Settings. Display window.location.host (hostname plus port, so two instances on one host, e.g. ports 8421/8422, are distinguishable) as the first thing on the Settings screen, before the Voice agent section. Set via textContent in web/voice.js, styled prominently in web/voice.css, element in web/voice.html. Focused test in vibe-talk/tests/js/voice_page.test.mjs. No server/contract change.
