---
title: 'vibe-talk-discuss-header-toggle-colours: One Expand/Collapse toggle in the Discuss summary header, larger, to the right; candidate highlight colours for the Discuss bar'
status: closed
priority: 0
issue_type: feature
labels:
- vibe-talk
- discuss
- ui
created_at: 2026-11-20T08:37:00-05:00
updated_at: 2026-11-20T09:36:20-05:00
---

# Description

[watermelon-full-preview] Owner request 228 (2026-11-20, second priority). The Discuss summary header on mobile is badly laid out: the summary line and the "Expand all / Collapse all" chips take separate rows. Drop "all" from the labels, make the controls bigger (comfortable touch targets), pack the two header lines densely, and put the control to their right — or, if there is no room, replace the pair with one toggle button: it says "Expand" when everything is collapsed or anywhere in between (bias to expand all) and "Collapse" only when everything is expanded; tapping applies to all. If that toggle is liked, reuse the same toggle for every section-header expand/collapse. Also experiment with highlight colours for the Discuss bar header: in half-height mode on mobile it is not visually clear where the top and bottom separate. Send screenshots of mock-ups of a few suggested highlight colours with the Discuss bar in the middle of the screen so the owner can pick one for contrast and fit with the colour scheme.

Today the summary-level chip pair (under "Current summary"), the timeline chip pair, and the per-earlier-summary pair are two-button "Expand all / Collapse all" bars (made small in #1924 vibe-talk-discuss-chips-triangles-terms), and the Discuss bar header uses the plain panel background, so the split is unclear at half height.

# Design

- One shared toggle component (`makeDiscussToggle`) for every expand/collapse-all control in the Discuss pane: the Current summary header, the Timeline header, and each earlier summary's header. One button only; its label is "Expand" unless every fold under its root is open, then "Collapse"; tapping applies to all folds under that root (nested folds included where the old pair did that) and keeps the kept-fold state. No "all" in any label.
- The Current summary header packs its two lines densely (title, then "Covers N messages") in a left column, with the single toggle to their right, vertically centred, at a comfortable touch target (at least 44 px tall on phone, at least 36 px on desktop), on phone 412x915 and desktop 1280x800, light and dark.
- A two-buttons-stacked-vertically variant is provided as a mock-up (in the Task B mock-up page) so the owner can compare; it does not land as a second live control unless the owner picks it.
- Task B (mock-ups, no landing needed beyond the variable): the Discuss bar header background becomes a single CSS variable (`--discuss-head-bg`, default the current panel background, so landing changes nothing visually). 3-5 candidate highlight colours that fit the existing blue palette are shown on contact sheets (one per theme, light and dark), each candidate labelled with its colour and its contrast ratio against the neighbouring surfaces, with the Discuss bar at half height in the middle of a phone screen. The chosen colour is then a one-line variable change (feeds #1961 theme work).
- Section-header folds themselves (the disclosure headings from `foldSectionsByHeading`, Progress / Review and friends) keep their readable triangles from #1924; the shared toggle governs the expand-all/collapse-all action for their group, not each heading's own tap.
- Condition-based waits only (owner rule 346). Dynamic testing through the real server router on a test instance (owner thread 185); screenshot matrix via the shared harness; browsers are unavailable on this host until the library reinstall, so the browser test and screenshot script are written ready and the gate is told.

# Acceptance Criteria

- No "Expand all" / "Collapse all" text remains in the Discuss pane; every expand/collapse-all control is the one shared toggle, labelled "Expand" when all folds are collapsed or any fold is closed, "Collapse" only when every fold is open, and tapping it opens or closes all folds under its root.
- The Current summary header shows its two lines densely packed on the left with the toggle to their right on phone 412x915 and desktop 1280x800, light and dark; the toggle's touch target is at least 44 px tall on phone.
- The same toggle serves the Current summary, the Timeline, and each earlier summary; a mixed (part-open) state reads "Expand" and tapping it opens everything. For the Current summary, "everything" includes the Terms block under the same header: a folded Terms alone keeps the label at "Expand", and the toggle opens and closes Terms with the answer's folds.
- Focused page tests cover the toggle label rules (all closed, mixed, all open), the tap applying to all, the header layout, and the absence of "all"; existing catchup tests stay green.
- Task B: `--discuss-head-bg` is the one place the Discuss bar header colour lives; a mock-up page plus screenshot script produce light and dark contact sheets of 3-5 labelled candidates with contrast ratios, Discuss bar at half height mid-screen. The mock-ups are delivered for the owner's pick and do not change the landed default.
- Before/after screenshots (phone and desktop, light and dark, Discuss half-pane open in phone shots, BUILD.txt per folder) once a browser is available; until then the script is ready and the block is reported. Dynamic exercise of the feature on a test instance with evidence.

# Notes

[watermelon-full-preview] mu-discusshead, owner request 228, 2026-11-20. Number pre-assigned by coordb; filed before implementation. Related: #1912 vibe-talk-discuss-summary-timeline-layout (the redesign this follows up), #1924 vibe-talk-discuss-chips-triangles-terms (small chips and readable triangles this supersedes for the chip pairs), #1961 (later theme work this feeds), #1497 discuss-fold-state-persist, #1387 discuss-collapsible-sections.

## Resolution

Landed on github/main as `cdcc8523` (mu-discusshead, 2026-11-20) and deployed by coordb the same day. One shared Expand/Collapse toggle serves the Current summary (Terms included in its scope), the Timeline, and each earlier summary; the Current header packs its two lines densely with the toggle to their right. `.discuss-head` is painted from `--discuss-head-bg` (default `var(--panel)`); the four candidate values with contrast ratios are in `vibe-talk/tests/discuss_head_colours.md`, with light/dark contact sheets delivered to the owner for the pick — the chosen colour is a one-line follow-up. Focused page/type/page-build suites passed; browser evidence ran on a host with Chromium (real-server scenes 73-76, 84, 85 green at `42bb6779`), and local gate reds were the missing-browser-library environment only, landed under coordb's waiver.
