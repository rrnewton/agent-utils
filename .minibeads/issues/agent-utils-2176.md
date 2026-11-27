---
title: 'vibe-talk-discuss-browser-family-root-cause'
status: open
priority: 2
issue_type: task
labels:
- vibe-talk
- flaky-test
created_at: 2026-11-24T06:00:00-05:00
updated_at: 2026-11-27T13:50:00-05:00
---

# Description

The Discuss browser family (discuss-browser, discuss-dock-browser, discuss-strike-live-browser, discuss-readjump-browser) is red on 80+ distinct gate heads in 7 days, with wait_for_function timeouts as the sampled signature and clean passes in recent kept step logs. One shared fixture/panel path serves the family, so one investigation should find whether this is fixture timing, a panel behaviour under load, or a real product defect.

Sampled kept logs show timeouts or passes, never a failing product assertion — so today there is no proven product bug here; this investigation is how one would be found if it exists.

# Acceptance Criteria

- A named root cause per family member (fixture, harness, or product), with either a fix or a recommendation to keep or drop its #2175 known-red entry.

# Notes

Filed by the coordinator from the browser-red plan (mu-clientperf, 2026-11-24); the owning lane refines this file as it works.

## discuss-browser: a cloud-read failure with no prepare (2026-11-27)

[opus 5.5, cl-signpost3] Gate row 1795802432841883439-ba76d157 (head d0a13d89, #2385 vibe-talk-thread-open-layout-first) went red on discuss-browser, desk 1280x800 dark, with this message (verbatim, for matching the next occurrence):

"the cloud read prepared [], not the Discuss key"

It comes from the walk's cloud-read block (`#1530 discuss-summary-cloud-read-404`; discuss_browser.py near line 821). `wait_read_count("1 of 4")` passed while the routed prepare list stayed empty. The route handler appends before it fulfils, so the 4 blocks did not come from the routed prepare: the read took another path.

The likely path, from the page code:
- A document read goes through `readWithBrowserSpeech` (the device voice, which counts the summary's own blocks and sends no prepare) whenever `readAloudPlayback` is not "audio" (view/voice/read-aloud.ts, the document read).
- `readAloudPlayback` is "audio" only while the source is a server voice (`setReadAudioSource`).
- `vibe-talk.voice.read-audio-source` is a synced setting (core/settings/keys.ts `SYNCED_SETTING_KEYS`), so a client-settings answer landing after the walk's toggle can put the source back to the device voice.
- The walk checks `#audio-source` aria-checked once, before Read, and never again.
The run kept no trace or screen, so the run itself cannot confirm whether the device voice spoke.

Not reproduced: 18 of 18 focused runs passed. That was 8 on d0a13d89, 5 on base 4b981762 and 5 on d0a13d89 with that change's kill switch off, at load1 134-183. The message differs from the two recorded under #2175 (wait_for_function timeouts; "close/reopen prepared a different key").

Proposed walk fix (not landed):
- At Read, re-check that the source is still the server voice: aria-checked "true", and the page's playback "audio".
- After the read starts, assert that the device voice did not speak (`window.deviceSpeech` unchanged since the toggle).
- On a mismatch, fail with a message naming the source change, instead of this one.

