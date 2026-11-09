---
title: 'revive-reused-pane-env: revive into a reused pane silently drops the profile environment values'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-09T13:53:37+00:00
updated_at: 2026-11-09T14:45:51.991004551+00:00
closed_at: 2026-11-09T14:45:51.991004411+00:00
---

# Description

[watermelon-full-preview] From the 2026-11-09 host reboot recovery. Lanes
record environment_names from their launch profile, but every revived lane
on an env-gated profile came back without the values (its harness showed
the downgraded mode), silently.

Capture and re-supply are not the loss: records store names only by design
(values are never persisted), and Rust revive already reloads the profile
values into the launch options and fails loudly when the profile or a name
is gone; --refresh-profile changes nothing here. The drop is at delivery
on the reused-pane path only. After a reboot, a lane on the herdr-pane
adapter reuses the terminal server's auto-restored idle shell;
create_presentation_in's reuse branch (rs/agentctl/src/subagents.rs)
returns without applying the launch environment, and the harness then
starts in that shell inheriting its environment. The original tab's --env
died with the original shell. New-tab delivery passes --env correctly, and
the Python edition (no reuse path) is unaffected. Restore inherits the bug
through revive; restart onto the same live shell is broken only when that
shell already lacked the values. The candidate re-records the names each
generation, so records look right while the value never lands.

# Design

- Refuse pane reuse when the launch environment is non-empty: the lane
  falls back to a fresh tab that receives --env. Typing export lines into
  the reused shell is rejected - values would sit in scrollback, unlike
  process-env delivery.
- Never launch silently without the environment: if values cannot be
  applied, revive says so instead of starting the harness downgraded.
- Lanes with no recorded environment names keep in-place reuse exactly as
  today.

# Acceptance Criteria

- A revive with profile environment into a provable idle shell launches in
  a fresh tab and the harness process carries the values (test asserts the
  delivered environment, not just the recorded names).
- A revive with no environment still reuses the pane in place.
- The existing reuse test's no-second-application expectation is updated
  deliberately, and restore shows the same behavior through revive.
- Python edition unchanged (it has no reuse path); differential coverage
  notes the edition difference if it is observable there.

# Notes

Remediation for lanes already running downgraded: restart does not help
(same env-less shell); close the stale tab, then plain revive creates a
new tab with --env. One observed lane had also lost its recorded
environment_names entirely, which needs --refresh-profile or a fresh start
regardless of this fix.

## Resolution

[watermelon-full-preview] Delivered with this closing commit. Pane reuse
in preflight_revive now requires an empty recorded environment: an
env-carrying revive takes a fresh tab, which receives --env through the
existing delivery path, instead of relaunching in the reused shell that
would silently drop the values. Confirmed for the report: plain revive
already re-supplied the values from the profile (and --refresh-profile
would not have changed the outcome) - the loss was delivery-side only.
New regression test
revive_with_profile_environment_never_reuses_the_pane_and_delivers_the_values
asserts the fresh pane and the delivered KEY=VALUE; the no-environment
reuse test still reuses in place. Rust-only: the Python edition has no
pane-reuse path. Unverified limit: restart into the same live shell is
unchanged (that shell was launched with the environment); a lane already
downgraded still needs the close-tab-then-revive remediation above.
