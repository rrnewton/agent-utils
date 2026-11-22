---
title: 'wrkslots-submodule-origin-refusal-wording: the submodule origin refusal names the .gitmodules URL but compares the source checkout origin'
status: open
priority: 3
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-22T08:05:00-05:00
updated_at: 2026-11-22T08:05:00-05:00
---

# Description

Removing a slot whose submodule was set up from .gitmodules can refuse with a message saying the submodule origin must match the .gitmodules URL, while the URL it quotes actually comes from the source checkout submodule origin (for example a local mirror whose upstream is a fork). .gitmodules itself names a different URL, so the message is wrong and slots created the ordinary way hit the refusal at removal.

# Design

Name what the check really compares (the source checkout submodule origin) in the refusal, and accept any of the known URLs of the same repository: the source origin, the .gitmodules URL, and the mirror upstream. Keep refusing a submodule pointing at an unrelated repository.

# Acceptance Criteria

- The message names the compared URL and its source; a submodule set up from .gitmodules is accepted when it is the same repository; an unrelated origin still refuses; tests.

# Notes

Reported by a consumer's lead (low priority); filed by the coordinator.
