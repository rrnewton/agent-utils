---
title: 'agentctl-launch-failed-but-live: Muse hooks-trust dialog fails launches of live panes and promote refuses them'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-18T00:00:00.000000000+00:00
updated_at: 2026-11-18T00:00:00.000000000+00:00
---

# Description

Since the Muse update on 2026-11-18, every new Muse session in a workspace with plugin hooks first shows a "Review plugin hooks" dialog ("N new or changed hooks need your trust"; options "Review hooks", "Trust all and continue", "Continue without trusting"; Esc continues without trusting). agentctl's launch readiness wait only looks for a verified idle composer, so the launch times out and the record is filed `launch_failed` ("custom harness muse did not reach a verified idle composer"; with caller-supplied environment the status detail is omitted entirely). The panes are in fact live: a person presses Esc later and the agents work, but the record stays `launch_failed`. The repair command that exists for exactly this state, `agentctl promote`, refuses these records outright ("promote applies to native and root-slot-relay interactive Herdr launches and native tmux launches") because the standard Muse record adapter (`herdr-pane`) is not in its list. Sends to such lanes therefore refuse or hang while their panes work.

# Design

- Recognize the hooks-trust dialog as a known Muse startup state in both editions: a screen holding "Review plugin hooks" together with "Continue without trusting" is a pane waiting on a person, never a failed launch and never a composer.
- Launch readiness keeps waiting while the dialog shows instead of treating it as composer progress that never comes; when the launch bound expires with the dialog still up, the failure names the state (waiting: hooks trust), not a missing idle composer. Nothing ever answers the dialog: "Trust all" is never pressed, and no key is sent by default. An opt-in to continue without trusting (Esc) is a profile/flag decision for the owner, left off and unbuilt here.
- `agentctl status` for such a pane reports `waiting: hooks trust` while the dialog shows, so a coordinator sees the real hold instead of a working/staged guess.
- `agentctl promote` accepts the standard interactive Herdr record adapters (`herdr-pane` included, backend Herdr) with the same identity proofs as today, so a `launch_failed` record whose pane proves live (verified idle composer or working) can be marked running and take sends again.

# Acceptance Criteria

- A launch whose pane shows the hooks-trust dialog does not end as a generic idle-composer timeout; while the dialog shows, status reports `waiting: hooks trust`, and no input is ever sent to the dialog.
- Fixture tests pin the dialog text (from the installed Muse) in both editions, and a dialog frame never reads as an idle composer or a draft.
- `agentctl promote` on a `herdr-pane` Muse record whose pane is live flips it to `running`; a record whose pane fails the identity checks is still refused.
- A real Muse launch that hits the dialog is exercised end to end (launch, human-equivalent Esc by hand on a throwaway agent, promote, send) before landing.
- `common/docs/agentctl/harness-support-matrix.md` trust-dialog row is updated in the same change.

# Notes

Reported 2026-11-18 after the Muse update; five lanes across two teams were recorded `launch_failed` while live. No lane, host or person is named here.
