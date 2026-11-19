---
title: 'vibe-talk-discuss-summary-timeline-layout: Discuss summary window interleaves collapsible Q/A pairs and earlier summaries under a headed current summary'
status: open
priority: 0
issue_type: feature
labels:
- vibe-talk
- discuss
- ui
created_at: 2026-11-19T14:20:38-05:00
updated_at: 2026-11-19T14:20:38-05:00
---

# Description

[watermelon-full-preview] Owner request, thread 223, 2026-11-19 2:16 PM ET, verbatim: "Ok let's also change how prior message/responses appear in the summary window. * Current summary * Terms .. * Timeline including... * Earlier Summary.. * Messages/responses. Right now it shows a BIG formatted response to my little question and TBH I thought it was a prior summary that was erroneously showing. I want it to be collapsable questions interleaved with collapsable earlier summaries in timestamp order. I think the Question/Answer should be one pare that collapses together. Instead of "Earlier summary DATETIME" It can just say "Follow up question DATETIME". I asked a long time ago for the Expand all collapse all to be smaller and not colored so prominently. The save draft one is low key. By the way Save Draft should be STACKED vertically with send so it doesn'tuse so much horizontal space in our thin column. I want the low key expand/collapse chits to be centered and live above the summary AND above the timeline for separate expand/collapse controls. I would like some horizontal separators.. I ant it tobe more clear that the top section is the current summary (Covers ___ messages) and below is the timeline. When I expand a given summary I think it should have a bit of a horizontal rule at the top and bottom. Our CURRENT summary looks worse than the earlier summaries. The earlier summaries have actual headers. Our current ones just have smaller text that is indendented with TINY triangles. I think we should make it actual header. Actually I would like one good collapsable style here and I would like it to be shared by the current and older summaries. In fact, when we expand an earlier summary I would like it to expand only headings first. Then I would like a SMALL "expand all" to appear to the right of the "Earlier summary" header line. Incremental expanding."

Coordination: mu-discusslink (#1911 vibe-talk-discuss-message-link-opens-new-window) is fixing the Discuss message-link click in parallel; this lane touches the summary layout, not link handling, and rebases on it if it lands first. This extends #1787 vibe-talk-discuss-resummarize-stale-terms-and-dated-labels (dated "Earlier summary" labels, now renamed for follow-ups) and the owner's earlier ask, recorded as the follow-up in #1525, that the Expand all / Collapse all bar be slimmed to a low-key style like Save draft.

Today, in `vibe-talk/web/src/view/catchup/panel.ts`: the current summary's sections fold under a prominent Expand all / Collapse all bar at the top of `#discuss-answer`; the timeline (`drawTimeline`) renders each follow-up turn as a bare paragraph (`discuss-turn`), so a long formatted answer reads as a prior summary, and each superseded summary as a plain `details.discuss-old` labelled "Earlier summary · <datetime> · N unread" with its whole body shown at once. The composer row puts Save draft and Send side by side in the thin column.

# Design

- Pane structure, top to bottom: a CURRENT SUMMARY section headed "Current summary" with a "Covers N messages" line, then Terms, then a TIMELINE section headed "Timeline", with clear horizontal separators between the sections. The current summary's covered count comes from the answer already drawn (`window.count`, else its covered ids), never a new request.
- One shared collapsible style for the current summary's sections and for earlier summaries: a real section header (heading-weight text on the summary line, the disclosure marker sharing that line), and an expanded item carries a thin horizontal rule at its top and bottom. Earlier summaries reuse the same folded-section rendering as the current summary instead of a raw body dump.
- The timeline interleaves, in the timestamp order the record already returns: (a) collapsible follow-up Q/A PAIRS — one `you` turn plus the `agent` turn that answered it collapse together under "Follow-up question <datetime>" (the #1787 `summaryTimeLabel` rule), an unpaired turn standing alone under the same label — so a follow-up answer never looks like a summary; and (b) collapsible earlier summaries labelled "Earlier summary <datetime>", sharing the current summary's collapsible style.
- Expand all / Collapse all are small, low-key, uncoloured chips in the Save draft style, centred, as two separate pairs: one pair above the current summary (acting on its folded sections, keeping the #1497 fold persistence) and one pair above the timeline (acting on the timeline's pairs and earlier summaries).
- Incremental expand: opening an earlier summary shows its headings only (its sections folded); a small "expand all" then appears to the right of that summary's header line (with a matching small "collapse all" once expanded) to expand its sections in one tap.
- Save draft is stacked vertically with Send in the follow-up composer, so the thin column spends no horizontal space on two side-by-side buttons.
- No change to link handling, scope binding, requests, or the record format: layout and rendering only, in the panel, its stylesheet, and the panel markup. Condition-based waits only (owner rule 346).

# Acceptance Criteria

- The Discuss pane shows, in order: a "Current summary" header with "Covers N messages", the summary, Terms, a horizontal separator, a "Timeline" header, and the timeline; separators also bound an expanded summary item (thin rule at its top and bottom).
- A long follow-up Q/A pair stands collapsed as one "Follow-up question <datetime>" item and expands to show question and answer together; it never renders as a bare formatted answer. Earlier summaries interleave with the pairs in timestamp order.
- Two centred, small, low-key (uncoloured, Save-draft-like) Expand all / Collapse all chip pairs stand above the current summary and above the timeline, each acting only on its own section.
- Current and earlier summaries share one collapsible style with real section headers; expanding an earlier summary shows headings only, then a small expand-all to the right of its header line expands its sections.
- Save draft stands stacked vertically with Send in the composer.
- Focused panel tests at the JS level cover the pairing, the labels, both chip pairs, the headings-first incremental expand, and the covers line; existing catchup tests stay green (updated only where they asserted the old timeline shape or label).
- Dynamic testing through the real server router on a test instance (owner thread 185) and the screenshot matrix via the shared harness (scenes 73-76 plus a targeted timeline scene): phone and desktop, light and dark, Discuss docked and as a sheet, a long Q/A pair collapsed and expanded, an earlier summary headings-first then fully expanded, the current summary with real headers, both chip pairs, and Save draft stacked with Send; before from main, after from the landed main, BUILD.txt in every evidence folder.

# Notes

[watermelon-full-preview] mu-discusslayout, owner thread 223, 2026-11-19. Number pre-assigned by coordb; filed before implementation. Related: #1911 vibe-talk-discuss-message-link-opens-new-window (parallel lane, link handling only), #1787 vibe-talk-discuss-resummarize-stale-terms-and-dated-labels (dated labels this extends), #1387 discuss-collapsible-sections and #1525 fold-marker-on-heading-line (the shared fold this restyles), #1497 discuss-fold-state-persist.
