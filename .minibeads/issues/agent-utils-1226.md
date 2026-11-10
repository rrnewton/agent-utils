---
title: 'watch-verdict-dedupe: watch-agents re-sends an unchanged DONE or BLOCKED verdict after a mass screen change'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
- watch-agents
created_at: 2026-11-10T16:54:43+00:00
updated_at: 2026-11-10T16:54:43+00:00
---

# Description

[watermelon-full-preview] Since about 11:00 AM ET on 2026-11-10, the coordinator receives the same "X finished its turn with a verdict (DONE: ...)" notice for 10 to 15 idle agents again and again, every few minutes, though none of them did anything new.

Evidence (read-only, the watcher's own persisted state on the affected host, `agent-watch-state.json`): every idle agent's `child-done` episode has `since` and `screen_changed_at` equal to one identical timestamp across all of them (for example 1794326402, then 1794327574), and a later sample shows the same pattern again at 1794328448 for nine agents. Something changes every pane's screen hash at the same instant; candidates are a Herdr-wide redraw, a resize when a tab is opened or retired, or the status-bar clock. The watcher then starts a fresh `child-done` episode and notifies again.

The watcher code did not change in today's turnover, so this is latent: it has been there since the episode logic took its clock from the screen, and a mass screen change is what exposes it.

Where it lives: `rs/agentctl/src/agent_watch.rs`. A `child-done` episode's `since` is derived from `memory.screen_changed_at` (the `IdleNoVerdict | ChildDone` arm in `update_episode`), and `screen_changed_at` advances whenever `screen_hash` changes. The episode does carry a verdict identity (`turn`, a hash of the verdict line's first words, `verdict_turn`), but the episode's clock and lifecycle still follow the screen: a mass change moves every idle agent's clock to the same instant, and any path that ends the episode between passes (a transient classification as working, running, or unreadable during the redraw) restarts it as a new episode that is due at once (`--child-done-seconds` defaults to 0) and notifies again.

# Design

- A verdict that has not changed must never be re-notified. Key a `child-done` episode on the verdict itself: the verdict line's text plus the harness turn (transcript progress, or the harness's own turn id where the record carries one), not on the screen hash or `screen_changed_at`.
- A screen change that leaves the verdict line and the turn unchanged keeps the existing episode, including its `notified` state and its original clock, whatever it does to `screen_hash`.
- A genuinely new verdict (different verdict text, or the same marker on a later turn) is a new episode and notifies once, as today.
- Keep the stall, idle, and approval detection that legitimately uses screen changes: only the `child-done` episode identity moves off the screen.
- Find the real mass-change trigger and name it here; fix it at the source too if that is cheap. If it is not cheap, file it as its own issue and link it.
- Rust is the primary edition; check the repository rules on Python parity before touching the other edition (the Python edition refuses `watch-agents` as Rust-only, per #509).

# Acceptance Criteria

- A test replays a mass screen-hash change (every agent's screen changes at the same instant, verdicts unchanged) and asserts zero new notices.
- The same agent ending a later turn with a new verdict notifies exactly once more.
- Stall, idle-without-verdict, and approval episodes still start, time out, and clear on screen changes as before.
- The mass-change trigger is named in this issue's notes, and either fixed at the source or filed as a linked issue.

# Notes

Related: #509 agentctl-child-done-notice (introduced the `child-done` episode, deduplicated per turn), #571 watch-agents-scale, #574 harness-adapters, #669 watch-notice-delivery, #675 watch-blocked-fast-alerts, #932 watch-agents-write-churn (which already saw a repeated `child-done` notice re-logged about every 62 s). To be updated with the trigger once it is found.
