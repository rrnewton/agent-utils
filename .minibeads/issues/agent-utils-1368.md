---
title: 'ttl-rescue-ref-namespace: default rescue refs to refs/rescue, overridable, collision-safe'
status: open
priority: 1
issue_type: task
labels:
- wrkslots
created_at: 2026-11-12T08:00:00+00:00
updated_at: 2026-11-12T08:00:00+00:00
---

# Description

[watermelon-full-preview] Follow-up to #1293 wrkslots-ttl-escalation-teardown, at a consuming project's request: TTL rescue currently pushes to refs/heads/rescue/<slot>/<date>/<name>. Those are GitHub branches, so they show in branch lists and pull-request UIs, and the consumer's stale-branch sweeps protect only refs/rescue/*, main, integration/* and archive/*, so a sweep could delete a rescue. Ordinary remove salvage is not involved: it pushes to refs/salvage/<machine>/<slot>/... and the absent-agent rescue already lives under refs/rescue/wrkslots (refs/heads/rescue/wrkslots is only a legacy read prefix). The TTL rescue refs are the only refs/heads/rescue/<slot>/... writer.

# Design

- Default the rescue namespace to refs/rescue/<slot>/<date>/<name>, outside refs/heads. The consumer already keeps many refs in that convention, so never collide silently: if the exact ref exists with the same target, reuse it; with a different target, suffix (-2, -3, ...) and name the actual ref in the receipt.
- Overridable with a --rescue-ref-root flag on ttl-report and ttl-teardown and a ttl_rescue_ref_root wrkslots config key in the existing optional-key style (mirrored in the Rust config, whose schema is shared), so a project can keep refs/heads/rescue.
- ttl-report's planned refs show the effective root. The push refspec (HEAD:refs/rescue/...) is the same shape removal salvage already pushes to refs/salvage/..., so non-branch refs need no new mechanism; the fixture tests prove it against a local remote.
- The bundle fallback is unchanged. No ordinary-remove ref changes.

# Acceptance Criteria

- Default rescue and report refs are under refs/rescue/<slot>/<date>/; the flag and the config key each move them (flag wins), including back to refs/heads/rescue.
- An existing rescue ref with the same target is reused; with a different target the push lands on a suffixed ref, the original is untouched, and the receipt names the suffixed ref.
- A failed push still falls back to the verified bundle exactly as before.
- Python and Rust stay in differential parity for the config key; focused tests assert refs and bundles at fixed clocks, never wall-clock budgets.

# Notes

Number reserved by coordb from the devbig014 block. Implementation by mu-ttlteardown; lands slice by slice under the path-selected gate.
