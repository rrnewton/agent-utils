# restore-in-place: reuse Herdr auto-restored tabs (design note, draft)

Problem (owner, 2026-11-06): after a host reboot Herdr auto-restores old
tabs as bare shells in their original cwd, but agentctl restore/revive
launches each agent in a NEW tab, leaving the restored shells as cruft;
lanes with pane=null/session=null are neither revived nor closed nor
labelled; revived lanes were not bound to their native sessions.

Design:
- Census live panes/tabs once at restore start (Herdr restore metadata:
  pane/tab ids, tab labels, cwd).
- For each registered lane, resolve its session: recorded native_session
  (#916), else Muse session-index.db / Claude project files fallback.
- If the recorded pane survives as an idle shell -> revive in place
  (existing #744 reuse). Else if exactly one orphan idle pane matches the
  recorded cwd and (tab label == lane name or recorded tab) -> revive
  reusing that pane, then pin native_session.
- Lanes that should not come back (excluded, or no session and no pane):
  close their duplicate restored tab when it is a bare shell duplicate of
  a revived lane; otherwise rename its tab "parked: NAME". Report per
  lane: resumed-in-place / parked / failed(reason).
- Idempotent: a second restore finds lanes live in place and skips;
  no tab is created when an in-place target exists.
- Never type into unregistered panes; stranded input is refused by
  identity checks (existing).

Implementation slices:
1. restore.rs: orphan-pane census + parked/failed report vocabulary +
   tab labelling/closing for parked/duplicate shells (fake Herdr tests).
2. revive reuse of a matched orphan pane (if slice 1 shows recorded-pane
   reuse insufficient on real Herdr IDs).

Evidence: ~/agent_scratch/restore-inplace-evidence-20261106/
