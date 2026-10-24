---
title: 'rich-headers-settings-fold-flake: the rich-headers browser check can time out clicking the Rich headers fold'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- flake
- validation
created_at: 2026-10-24T11:54:39.227916886+00:00
updated_at: 2026-10-24T11:54:39.227916886+00:00
---

# Description

[opus 5.5] Found by test-time-vibe-talk on 2026-10-24, in a full-graph run (validate.py --all --keep-going) on the loaded validation host: vibe-talk.check.rich-headers-browser failed after 35 s at the 1280x800 desk in set_rich_headers (tests/rich_headers_browser.py:422). page.click('#fold-rich-headers') timed out at 30 s: Playwright resolved the button, found it visible, enabled and stable, scrolled it into view, and then reported 'element is not visible', over and over. So the Settings screen moved or folded under the click. The 412x915 phone pass before it had passed. The same check passed in the two selected gates of that day and in the last full-graph gate of 2026-10-23, and four standalone runs side by side all passed, so it is rare and seems to need load. Nothing in the test-time-vibe-talk commits touches this check or the page. Suspects: the Messages fold opening (fold-settings-messages, clicked just before) is still animating or laying out, so the inner fold moves out from under the scroll, or the Settings screen scrolls as it lays out. A fix should wait for the Messages fold body to be open and settled before clicking the inner fold, or click it the way the page's keyboard path does.

# Acceptance Criteria

The check waits on a stated condition, not a fixed delay, before it clicks the inner fold. It passes repeatedly under load, for example eight runs side by side.
