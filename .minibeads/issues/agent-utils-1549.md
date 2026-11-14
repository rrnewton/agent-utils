---
title: 'documents-read-browser-voice-premise-flake: gating browser check fails under load on the server-voice premise'
status: open
priority: 1
issue_type: task
labels:
- vibe-talk
- documents
- flake
created_at: 2026-11-14T19:03:00.000000000+00:00
updated_at: 2026-11-14T19:03:00.000000000+00:00
---

# Description

[opus 5] The gating browser check `vibe-talk.check.documents-read-browser`
fails under load at `vibe-talk/tests/documents_read_browser.py` in `walk()`:
"the premise: the server voice is not the picked source" (the
`#audio-source` toggle's `aria-checked` is not `"true"` when read). It failed
in gates 7 times since 2026-11-11 and in a lane's local validate, while 10
sequential unloaded runs (5 base + 5 head) all passed: it is load-dependent.
It blocked two other lanes' work. Owner rule 346: gating checks must be
robust on a hot host (>75% CPU), with no wall-clock budgets; wait on
events/conditions with generous timeouts.

# Design

Root cause (reproduced under capped CPU load, load average above 75% of the
host's threads: 5 failures in 24 runs before the fix, every one in the
second, desktop walk): the picked voice source is decided asynchronously,
and the check read it before the decision settled, on state an earlier walk
had left behind.

- The toggle's shipped default is the device voice; the server voice is
  picked only when the page applies the server's client-config answer
  (`setReadAudioSource` in the page's read-aloud owner, called from the
  client-config applier), which also records the choice in this device's
  storage.
- The check's walks share one server, and the first walk ends with the
  device voice picked. The settings sync (`#457 settings-server-side`)
  carries that choice to the server's settings document; a sync pass that
  lands after the first walk's final toggle stores `device` there.
- The second walk's fresh browser context then inherits `device` through
  the same sync — a legitimate synced preference, not a page defect — and
  whether it lands before or after the premise reads the toggle is a race
  that load decides. Probing the page at the failure showed the server
  capability already known (`agentReadAloud` set) with the picked and
  stored source both `device`.

Fix, in the test only (no page change; the page's behaviour is the designed
sync semantics, and users picking a source on one device expect it on
another):

- `walk()` waits on the decision condition itself — the toggle drawn and
  `aria-checked === 'true'`, via `wait_for_function` with the file's usual
  generous 20s timeout, no sleep — before asserting the premise.
- Each walk (and the list walk) first forgets any synced
  `vibe-talk.voice.read-audio-source` choice the shared server holds
  (`PUT /api/v1/client-settings` with a null value), so every walk starts
  from the shipped default its premise describes.
- `walk_list()` likewise waits for the voice-source decision (capability
  known, toggle drawn) before reading the toggle it conditionally flips.

# Acceptance Criteria

- Red before: the check fails at the premise under capped CPU load on a
  hot host (observed 5/24 runs, second walk only).
- Green after: at least 10/10 runs pass under the same load (observed
  16/16 at a load average above 75% of host threads).
- No wall-clock sleeps added; waits are on page conditions with generous
  timeouts, per owner rule 346.
- `vibe-talk/web/voice.js` and `web/src/app/main.js` untouched.

# Notes

- Hypotheses checked: a server startup probe falling back to the device
  voice (the server answer is static config; no), a capability race inside
  the page (the capability was already known at the failure; no), the test
  reading the toggle before the picked source settled, on a synced choice
  left by the previous walk (yes).
