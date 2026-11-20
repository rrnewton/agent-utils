---
title: 'agentctl-muse-hooks-trust-auto-decline: answer Muse plugin-hooks trust dialog by policy instead of stalling launches'
status: closed
priority: 0
issue_type: feature
labels:
- agentctl
created_at: 2026-11-19T00:00:00+00:00
updated_at: 2026-11-19T00:00:00+00:00
---

# Description

[watermelon-full-preview] At Muse startup, a dialog "Review plugin hooks: N new or changed hooks need your trust" offers "1. Review hooks", "2. Trust all and continue", "3. Continue without trusting" (footer: "Up/Down to choose, Enter to confirm. Esc continues without trusting."). Since #1893 agentctl-launch-failed-but-live, agentctl names this state (`waiting: hooks trust`) but never answers it, so `agentctl start NAME --profile muse-...` stalls there, reports launch failure, and never delivers the brief. The owner (2026-11-19): "Dude you just clear it! That's annoying red tape. I thought agentctl was supposed to try to clear these automatically." and "It is absolutely critical to solve problems and keep autonomous development flowing." The answer is a policy setting, not hard-coded (owner, same evening): "no you can continue with trusting... we've developed our own hooks, no external ones".

# Design

- Strict recognition in both editions: the title "Review plugin hooks" AND all three option texts ("Review hooks", "Trust all and continue", "Continue without trusting") must match. Anything else (permission prompts, a different trust dialog, an "approve command" prompt) is never touched. If option 3's text is missing or reworded, refuse to act and report. The looser #1893 recognizer keeps naming the state for status; only the strict match may be answered.
- Policy key `muse_hooks_trust`: `"decline"` (option 3, Continue without trusting) or `"trust"` (option 2, Trust all and continue), in `.agentctl/profiles.json`: per-profile `profiles.<name>.muse_hooks_trust`, with a per-registry default at top level `muse_hooks_trust`. Precedence: profile > registry default > built-in `"decline"`. An invalid value refuses at load (fail closed). No silent trust: option 2 is sent only when the setting explicitly says `"trust"`. profiles.json is the home because both editions warn-and-ignore unknown keys there (Python `_unknown_key_paths`/`unknown_keys_warning`; Rust `forward_compat::KeptKeys`), while `watch.json`'s `WatchFile` is `deny_unknown_fields`, so a new key there would make an old watch binary refuse its whole config. No new persisted field: the decision is resolved from config at launch/watch time and only logged, naming the decision, the hook count if shown, and which setting it came from.
- `agentctl start` (and revive, restart and restore, which launch Muse the same way through the client readiness wait) recognises the strict dialog in a pane agentctl itself just launched, answers per policy by explicitly selecting the option and confirming (Down/Enter, never Esc as first choice), verifies the dialog is gone and Muse reached its composer, then continues the start: deliver the brief or goal and promote the record to running.
- The watcher (watch-agents and watch): when a managed Muse lane of this registry shows the strict dialog, answer per the same policy and log it. Only panes whose record this registry owns; never other registries' or unmanaged panes.

# Acceptance Criteria

- A Muse launch that hits the strict dialog is answered per policy, reaches its composer, delivers its brief, and its record promotes to running; the log names the decision, the hook count if shown, and the setting's source.
- Recorded pane texts: the exact dialog is answered with the policy's option; near-miss texts are left alone; with the default (decline) option 2 is never sent, and option 2 is sent only under an explicit `"trust"` setting.
- An end-to-end start test with a fake Muse that shows the dialog, then the composer: the brief is delivered.
- The watcher answers the strict dialog only in this registry's managed Muse panes and logs the decision and its source.
- `common/docs/agentctl/harness-support-matrix.md` (Muse row) is updated in the same commit.

# Notes

Owner-urgent (2026-11-19). Builds on #1893 agentctl-launch-failed-but-live, which recognised the dialog and deliberately left answering unbuilt pending the owner's decision; this issue is that decision. Config key and file were reported to the coordinator before implementing (reader skew: profiles.json vs watch.json readers).

## Resolution

[watermelon-full-preview] Landed 62f03e1d on github/main (gate on 9c14124c, base dc2c5fdb: only reds the 4 pidfd_open EINVAL foreign-ownership ids, identical on main, coordb waiver; combined tree with #1928 verified: mypy strict, py 713, rs 1030; third review SAFE on all 17 scenarios in both editions). Both editions answer the exact Muse plugin-hooks trust dialog (bottom-anchored parse of the real Muse 1.4.25 capture) per `muse_hooks_trust` in profiles.json (profile > registry default > built-in decline; trust only when explicit; invalid refuses at load; no record field added), in start/revive/restart/restore launches and revive-candidate verification, and the Rust watchers answer only this registry's managed Muse lanes; every answer re-reads before each key and logs decision, count and source. `common/docs/agentctl/harness-support-matrix.md` Muse row updated in the same change. Follow-ups filed: #1948 hooks-quote-scrolled-marker, #1949 hooks-dialog-require-count-line, #1950 hooks-dialog-real-capture-answer-test, #1951 hooks-dialog-shell-prompt-veto.
