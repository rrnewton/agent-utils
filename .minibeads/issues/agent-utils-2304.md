---
title: 'vibe-talk-documents-read-sidecar-wallclock: the Documents read-aloud check waits on the TTS sidecar by a 20 s wall clock and goes red under gate load'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- tests
created_at: 2026-11-26T11:55:00-05:00
updated_at: 2026-11-26T11:55:00-05:00
---

# Description

The browser check `vibe-talk.check.documents-read-browser` (`vibe-talk/tests/documents_read_browser.py`) went red in an owner gate on 2026-11-26 with "the server voice was never asked to say the document", on a change that touched nothing it uses (`#2294`). Run alone, it passed 3 of 3 on that tree and 3 of 3 on its base (load1 about 150-160, 7-8 s each).

The failing step waits for the fake local-TTS sidecar to be asked for the document's words with a wall-clock budget: after the walk flips to the server voice and presses Read aloud, it polls `sidecar_texts()` every 0.1 s until `time.time() + 20` (lines 533-539). On a loaded host the request can arrive after that budget, and the check reports a missing request rather than a slow one. Owner rule 346 forbids wall-clock budgets in gating checks: they wait on events or conditions with generous timeouts, and pass with the host above 75% CPU.

The same file also starts its server and waits for the documents fold with `time.time() + 30` loops (lines 1035-1054); those are setup waits, but they follow the same pattern.

# Design

- The fake sidecar signals each `/synthesize` request (for example a `threading.Condition` or `Event` next to `SidecarHandler.texts`), and the walk waits on that condition for the count to pass `asked`, with a generous timeout that only exists to fail a hung run, never to judge speed.
- Where the wait is on the page instead (the strip lighting the first block), keep the existing condition waits.
- Consider the same treatment for the startup and documents-fold loops.

# Acceptance Criteria

- No `time.time()` budget decides a pass or fail in `documents_read_browser.py`; waits are on the sidecar request, page conditions or server answers, with generous timeouts.
- The check passes repeatedly with the host at or above the hot-load line (load1 >= 237 for the whole run), recorded in the evidence.

# Notes

Found by cl-composerblack on the `#2294` gate (230b4e32), where the step was rerun alone on the same tree. Related: owner rule 346 (no real-time-sensitive gating checks).
