---
title: 'vibe-talk-discuss-chips-triangles-terms: Make summary chips small, disclosure triangles readable, and show Terms in Discuss'
status: open
priority: 0
issue_type: feature
labels:
- vibe-talk
- discuss
- ui
created_at: 2026-11-19T17:53:26-05:00
updated_at: 2026-11-19T17:53:26-05:00
---

# Description

[watermelon-full-preview] Owner UI follow-up to #1912 vibe-talk-discuss-summary-timeline-layout (landed 7968ce34). The owner asked for "small and low-key" controls and complained about "TINY triangles". The liaison reviewed the live result and asks, fast-track: (a) the summary-level "Expand all / Collapse all" chips under "Current summary" are still large on phone and must match the small size of the timeline and per-summary chips on phone and desktop; (b) the disclosure triangles beside Progress, Review and the timeline items are still tiny and must be a readable size (about the heading's cap height or more), vertically aligned with the heading text, with a comfortable tap target on phone (at least 32 px); (c) no screenshot shows the Terms block (point 1 of #1912) — check whether the screenshot fixture lacks Terms, add a stub answer with Terms so one shot shows it, and make sure Terms renders well. Before/after shots of scenes 74 and 84 (phone and desktop, light and dark) plus the Terms shot.

Today the summary-level chip pair and the timeline/per-summary chip pairs render at different sizes, the native disclosure markers render tiny beside the section headings, and the screenshot fixture's stub answer carries no Terms so the Terms block never appears in evidence.

# Design

- One shared small chip style for every Expand all / Collapse all pair in the Discuss pane: the summary-level pair (under "Current summary"), the timeline-level pair, and the per-summary pair(s), on both phone and desktop. The fix is in the stylesheet/panel markup only; the three pairs keep acting on their own sections.
- Disclosure markers for Progress, Review and the timeline items (follow-up pairs and earlier summaries) render at a readable size — about the heading's cap height or more — vertically aligned with the heading text, with a tap target of at least 32 px on phone. Prefer a styled marker over the native tiny triangle; keep the expanded/collapsed rotation and the shared collapsible style from #1912.
- Screenshot fixture: give the stub Discuss answer a Terms block so a shot shows Terms; verify Terms renders well (heading, list, spacing) in light and dark on phone and desktop. No change to scope binding, requests, or the record format.
- (d) Phone sheet: the follow-up composer (field, Save draft, Send) must not slide under the sticky unread/hours/Summarize bottom bar; the whole composer stands inside the sheet, keyboard open and closed. Layout only, in the Discuss stylesheet.
- Condition-based waits only (owner rule 346). Dynamic testing through the real server router on a test instance (owner thread 185); screenshot matrix via the shared harness.

# Acceptance Criteria

- The summary-level Expand all / Collapse all chips match the timeline and per-summary chips in size (small, low-key, Save-draft-like) on phone 412x915 and desktop 1280x800.
- Disclosure triangles beside Progress, Review and timeline items are readable (about the heading cap height or more), vertically aligned with the heading text, and present a tap target of at least 32 px on phone.
- One screenshot shows the Terms block rendering well; the fixture stub answer carries Terms and Terms appears in the Discuss pane through the real routes.
- Focused tests cover the chip sizing, the marker sizing/alignment/tap target, and the Terms fixture; existing catchup tests stay green.
- Before/after screenshots of scenes 74 and 84 plus the Terms shot: phone and desktop, light and dark, Discuss half-pane open in phone shots, BUILD.txt in every evidence folder; after from landed main. Dynamic exercise of the feature on a test instance with evidence.
- (d, liaison addition 2026-11-19) On phone 412x915 the Discuss composer's Send button is clipped at the bottom, half hidden behind the unread/hours/Summarize row (see /tmp/mu-discusslink-numbers/after-7134da49-*.png). The whole composer (field, Save draft, Send) stands fully visible inside the sheet at phone sizes, with the keyboard open and closed; before/after shots show that state.

# Notes

[watermelon-full-preview] mu-discusschips, owner thread 223 follow-up, 2026-11-19. Number pre-assigned by coordb; filed before implementation. Related: #1912 vibe-talk-discuss-summary-timeline-layout (the redesign this follows up), #1911 vibe-talk-discuss-message-link-opens-new-window (parallel link handling), #1525 fold-marker-on-heading-line, #1497 discuss-fold-state-persist.
