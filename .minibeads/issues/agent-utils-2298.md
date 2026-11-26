---
title: 'vibe-talk-ui-rules-and-design-review: a UI rules file every UI lane loads, and an advisory design-review pass over each UI change'
status: open
priority: 1
issue_type: task
assignee: mu-uidesign
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-26T11:06:00-05:00
updated_at: 2026-11-26T11:06:00-05:00
---

# Description

The owner, 2026-11-26, of agent-built user interfaces: "these AI generated uis often seem very bad or unintuitive on first try. They litter verbose prose in places that a human would never put it in a UI. And simply as a layout thing. If you look at this screenshot it's got three buttons. I don't think it carefully designed UI would ever have three buttons like that right centered and not filling the column. They would probably either be centered or would fill the column. Is that expressed by some rule of mobile design?? I want to see if we load the right context and we do so intentionally if we can do some of this UI ux design review agentically."

The screenshot is the top of Settings on a phone (the same screenshot as #2297 vibe-talk-settings-header-layout, which owns the fix to that header). This issue owns the general lesson, in three parts. First, a short sourced survey of what practitioners report about language models doing intentional UI and UX design: which loaded context, skills, prompts and review loops have worked, including a verification of whether Anthropic publishes a "frontend-design" skill and what it says. Second, a short UI rules file in this repository that every vibe-talk UI lane loads before touching the page, distilling Material 3 and the Apple Human Interface Guidelines (layout and alignment, button groups, text economy, touch targets, spacing, truncation, status chips) together with this page's standing local rules. Third, an advisory design-review pilot: a fresh reviewer with the rules file as context reads a UI change's screenshot matrix and lists violations with crops, tried on the Settings screenshot and on the after-landing screenshot sets of #2277 vibe-talk-key-scroll-freezes-11-26, #2279 vibe-talk-post-facto-labels and #2285 vibe-talk-labels-browser-tapped-message-jumps.

# Design

No product code changes in this issue. The survey is read-only web research with a source for every claim, delivered to the coordinator as a report; only its distilled, checkable rules enter this repository. The rules file lives at vibe-talk/docs/ui-rules.md (vibe-talk keeps its operator documents as Markdown beside the app; a docs directory keeps the app root to its current four documents). It stays short enough to load into every UI lane's context: each rule is one imperative line, a one-line reason and a reference. Platform rules cite the Material 3 or Human Interface Guidelines section they distil; local rules are labelled local and include the row being read never moving while growth extends below, one Send per composer, no verbose prose in the UI, whole-pixel placement, dark-mode composers matching the cards, and the required screenshot matrix (light and dark, phone and desktop, keyboard open and closed, Discuss open). The button-group rules state explicitly when end alignment applies (dialog actions) and what page-level controls do instead on a narrow screen (fill the column at equal width, or centre as a group), which is the convention the Settings header broke.

The review pilot is advisory only: it produces a violations list (rule, where, crop, suggested fix) and a reusable reviewer brief the coordinator can add to a UI lane's flow before human-facing review. It also states what a reviewer can and cannot judge from screenshots alone (for example, motion, timing and focus order need traces or interaction, not stills). Findings against already-landed work are reported, not fixed here; any fix is a follow-up issue.

# Acceptance Criteria

- The survey report names a source for every factual claim, marks opinion as opinion, and states plainly whether Anthropic's "frontend-design" skill exists, what it says if it does, and how it applies or does not apply to this page.
- vibe-talk/docs/ui-rules.md exists, is short enough to load whole into a lane brief, and every rule carries a reason and a reference; the end-aligned page-buttons case, verbose chrome prose and silent truncation are each forbidden by a named rule.
- The pilot review covers the Settings screenshot and sampled matrices from #2277 vibe-talk-key-scroll-freezes-11-26, #2279 vibe-talk-post-facto-labels and #2285 vibe-talk-labels-browser-tapped-message-jumps, lists the files actually viewed, and gives each violation a rule, a location, a crop and a suggested fix, plus an explicit cannot-judge-from-screenshots list.
- A reusable reviewer brief is delivered with the report so the coordinator can run the same pass on the next UI change without rewriting it.
- scripts/check_issue_parse.py and scripts/check_client_names.py pass on the change; no page, server or contract code is touched.

# Notes

[watermelon-full-preview] Owner request 2026-11-26, relayed by the coordinator. #2297 vibe-talk-settings-header-layout fixes the Settings header itself; this issue must not touch page code or duplicate that fix. The rules file lands only on the coordinator's go-ahead after the usual gate. If the pilot finds real defects in landed work, file them as separate issues rather than widening this one.
