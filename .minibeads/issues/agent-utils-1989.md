---
title: 'vibe-talk-documents-failure-screenshots: photographs of the private-repo failure state, and the composer scene failure named'
status: open
priority: 0
issue_type: task
labels:
- vibe-talk
- documents
- ui
created_at: 2026-11-20T14:54:00-05:00
updated_at: 2026-11-20T14:54:00-05:00
---

# Description

[watermelon-full-preview] Follow-up to #1980 vibe-talk-docs-private-github-load (landed 63a58bb4 + 8b00a4d6): the reader now shows the specific reason a private GitHub document failed (for example "vibe-talk has no access to <repo>") plus an "Open on GitHub" link, and a failed list row does the same, but `vibe-talk/tests/documents_browser.py` asserts that state without photographing it, so there is nothing to show the owner. Separately, screenshot scene `77-composer-keyboard-open` fails on laptop-1280 with "state was NOT reached: the composer holds the focus"; nobody has named why.

# Design

- Add `--screenshots DIR` to `vibe-talk/tests/documents_browser.py`: with the flag, the existing real-server walk saves PNGs of (a) the private-repo failure in the reader and (b) the broken document row with its reason and link, at phone and desktop widths, in light and dark theme, keyboard closed (and open on phone where a composer is involved). Without the flag the walk is unchanged. Keep every existing assertion as it is.
- Screenshots serve the real routes through the real server the test already starts (fake chat upstream and local raw stub only); no page route is stubbed and the live instance is never touched.
- Name the cause of the scene-77 laptop failure: fix the scene if it is a harness issue, or report it as a page bug if the page itself drops the composer focus.

# Acceptance Criteria

- `tests/documents_browser.py --screenshots DIR` writes the private-reader and broken-row PNGs at phone (393/412) and desktop 1280 widths, in light and dark, and the walk still passes with all prior assertions intact; without `--screenshots` it writes nothing and behaves as before.
- Every screenshot is looked at: the private failure shows the specific no-access reason and the "Open on GitHub" link, the broken row shows its reason and link, and no frame shows the bare "Could not load the document."
- The scene-77 laptop-1280 cause is named; if it is a harness issue the scene passes on laptop-1280, and if it is a page bug that is reported with evidence instead of a scene change.
- A landed SHA plus the exact command to reproduce the screenshots on the review host is reported, so the shots can be regenerated from main.

# Notes

[watermelon-full-preview] Owner-priority, deployed on landing. Dynamic testing and the screenshot matrix are standing owner rules: proof is through the real server, every image is looked at, and keyboard-open frames really shrink the viewport. The shared screenshot harness (`scripts/run.sh --screenshots`) remains the matrix harness; this issue only adds photographing to the documents walk that already reaches the failure state, and diagnoses the one matrix scene that fails on the laptop profile.
