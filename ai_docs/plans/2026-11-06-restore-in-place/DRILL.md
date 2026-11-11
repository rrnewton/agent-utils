# Verification drill (item D): everything comes back on its own

## Rehearsal without a reboot (done in tests)

The fake-Herdr restore/revive suites rehearse the reboot cycle: lanes are
killed (pane dead or at an idle shell), restore/revive runs, and the layout
and report are asserted.

    cargo test -p agentctl --lib subagents::tests::restore   # 35 passed
    cargo test -p agentctl --lib subagents::tests::revive    # 53 passed
    cargo test -p agentctl --lib session_resolution          # resolver (#947)
    cargo test -p agentctl --lib tab_order                   # canonical order

Covered: revive in the recorded pane, revive into a uniquely-matching
auto-restored tab, per-lane dispositions (resumed-in-place / parked /
failed), parked labelling, restart in place (same pane, session resumed,
slot transferred), and the resolver finding a resumed Muse by argv.

## Boot-path check (done read-only on devbig014)

    systemd-analyze --user verify agent-restore.service \
        agentctl-restore-agent-utils-014.service   # no errors in our units
    systemctl --user list-unit-files --state=enabled

Findings are in BOOT-INVENTORY.md: linger is on; both restore units are
enabled; the loaded agentctl-restore copy still has Restart=no until
hands runs daemon-reload with the retry change (agent-utils-975).

## Real reboot drill (proposal, needs the owner's explicit OK)

Disrupts every team on the host, so it is a separate step:

1. Pick a window; every lane writes a restart note (background jobs,
   monitors, subagents, crons to re-arm) and ends its turn RESTART-READY.
2. `agentctl shutdown` from an external helper (exits every agent at an
   idle boundary, records panes/sessions).
3. Reboot; confirm herdr-server, both restore units (with retry), watch,
   bridges, and vibe-talk targets come up enabled without manual enable.
4. Assert: restore report has no unexplained refused rows, dispositions
   are resumed-in-place/parked only, no duplicate bare-shell tabs remain
   after one watch pass, and each coordinator answers on its own name.
