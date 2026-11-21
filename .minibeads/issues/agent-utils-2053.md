---
title: 'vibe-talk-pace-has-no-effect: Pace reaches no voice that cannot take a rate, and says so'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- read-aloud
- ui
created_at: 2026-11-21T10:23:00-05:00
updated_at: 2026-11-21T10:23:00-05:00
---

# Description

[watermelon-full-preview] The owner, 2026-11-21 10:23 AM ET: "Also I noticed that our pacing never took a fact and neither did we gray out the pacing button to indicate that the back end couldn't actually do it."

Traced before any change (report to coordb, 2026-11-21): the page sends the pace on every path — the device voice's `utterance.rate`, the prepare body's `speed`, the speak route's `?speed=` — and the server forwards it into the speech ticket and the provider. ElevenLabs, Wit.ai and the device voice honour it. Two backends drop it at the last hop:

- the local sidecar's piper engine called `synthesize(text)` with no pace at all (kokoro, beside it, passes `speed=` through);
- the conversational cascade (`read_aloud.backend = "conversation"`, the owner's deployment) names the parameter `_speed` and discards it: its wire is a verbatim-read prompt with no rate to carry.

And nothing could grey the button: client-config's `read_aloud` description carried no pace capability, so the page had no word to read.

# Design

Approved by coordb as (a)+(b), 2026-11-21:

- (a) Wire the pace where the backend takes one: the sidecar's piper branch passes `SynthesisConfig(length_scale = 1/speed)` with the pace clamped to the server's 0.5–2.0 bounds, and kokoro's pass-through is clamped to the same bounds. No prompt-text pacing for the cascade: its read prompt is pinned verbatim (a change silently brings back the long stalls it was pinned against), and a cascade cannot honour a numeric rate reliably.
- (b) Say where it cannot: `Description.pace_supported`, served in client-config's `read_aloud`, false only for the conversation backend and sent only when false (an older server's silence reads as a pace that works). The page greys the Pace stepper, box included, with the reason "Pace isn't available with this voice" in the popover's help, the Pace button's title and its aria, whenever the voice that will read cannot be paced; the device voice always can.

# Acceptance Criteria

- A recording fake piper is handed the inverted pace as a length scale (and the posted `{"speed": 1.5}` body becomes it end to end); a recording fake kokoro is handed the pace itself; both clamped. No models, no TTS installation: `scripts/local_tts_sidecar.py --self-test`.
- A pinned Rust test that `{"speed": 1.5}` reaches the sidecar body, and an end-to-end one: `?speed=1.5` on the speak route is the fake sidecar's recorded `speed`.
- The conversation backend's description serializes `"pace_supported": false`; capable backends omit the field.
- Page shards: with `pace_supported: false` the stepper and box are disabled and the reason shows in help, title and aria; with a capable voice, and on the device voice, Pace stays live.
- A Chromium walk (fixture server only) at 412 by touch and 1280, light and dark: Pace greyed with its reason for the incapable voice, live for the capable one, with screenshots of all eight popovers.
