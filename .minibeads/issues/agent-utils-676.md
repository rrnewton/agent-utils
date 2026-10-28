---
title: 'settings-head-page-build: Settings'' head is three rows on a phone once the page knows its build'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
depends_on:
  agent-utils-624: blocks
created_at: 2026-10-28T23:51:43.932594656+00:00
updated_at: 2026-10-28T23:51:43.935996768+00:00
---

# Description

[opus 5.5] Found by #624 voice-page-modularity, step 4b, when the browser walks began serving the page as the server does.

On a phone (393x852), with the long host name desktop_layout_browser uses, the head of Settings takes 113 px: more than the two rows (52 px each, 104 px) that #372 settings-header-layout allows. The extra row is #529 page-build-stale-notice's "This page is build ..." line (`#settings-page-build`, a hint right after `#settings-server-host` in the Server section): on a phone Expand all and Open help sit under the host line, and the build line lands between them whenever the page knows its build.

This is the production page today, before the refactor. The page knows its build whenever its script was loaded by a hashed address, which production's shell has always given it (`/voice.js?v=<hash>`, now `/assets/voice.<hash>.js`). The walks could not see it: their fake server served `/voice.js` with no hash, so the page never knew its build and drew no build line. Proof: the unbuilt sources served with `src="/voice.js?v=<sha256 of voice.js, 128 bits>"` fail desktop_layout_browser exactly as the page build does ("phone-393: the head takes 113px, more than 2 row(s)"), and pass without the hash.

Until this is fixed, desktop_layout_browser keeps testing the no-known-build state it always covered (its shell names the script with no hash), with a comment pointing here. The fix is page layout work, so it waits for #624 (the page freeze). When fixing: decide where the build line belongs in the phone head (or that it belongs elsewhere in the Server section), then make desktop_layout serve the page build as every other walk does, so it measures the known-build state.
