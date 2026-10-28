---
title: 'voice-bluetooth-output: route call audio to the system/Bluetooth device'
status: open
priority: 2
issue_type: task
created_at: 2026-10-28T11:57:53.766158139+00:00
updated_at: 2026-10-28T11:57:53.766158139+00:00
---

# Description

Owner (2026-10-28): on Android, Spotify plays to connected AirPods but a vibe-talk voice call uses the phone speaker. Diagnose why Chrome on Android routes call audio to the speaker while a Bluetooth headset is connected. Fix: prefer Bluetooth/headset capture at call start; Settings input/output choice (System default + enumerated, setSinkId where supported, picker hidden where unsupported), remembered per device; read-aloud keeps A2DP/music path; chosen devices + reported route in voice diagnostics plus opt-in on-page readout. Page tests with fake enumerateDevices/getUserMedia/setSinkId; only a physical phone verifies the actual Bluetooth route.
