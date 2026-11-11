# wrkslots Rust-primary parity inventory (milestone 1)

Date: 2026-11-07. Issue: #562 rust-primary-wrkslots (P2, owner priority).
Lane: mu-rswrk, slot rswrk, branch mu-rswrk/rust-primary-wrkslots.
Measured at HEAD `5bb572f1863996ba19a166a83c3c21596db5ab8d` (github/main after rebase).

Finish line: every wrkslots command is Rust-default with differential
coverage, and the Python edition is archived, as #238 describes for
agentctl. Until then, new behaviour lands in BOTH editions.

Sizes at HEAD: Python `py/wrkslots/cli.py` 68,166 lines (plus imagecmd
1,810, sandbox 2,355, slotimage 1,859, worktreesimage 614, yamlconfig
623, imagehelper 476; 75,903 total). Rust `rs/wrkslotsd/src` 51,092
lines across 56 modules (6,933 of them `tests.rs`).

## Router truth today (verified in code)

- `validation/editions.json`: `wrkslots` primary is **python**, Rust
  mirror state is **absent**; `wrkslotsd` is a separate Rust-primary
  observer entry. **No wrkslots command is Rust-default today.**
- Default (no env): every command runs Python (`py/bin/wrkslots`).
- `WRKSLOTS_ENGINE=rust`: `common/bin/engine-resolver` execs
  `rs/bin/wrkslotsd` with the caller's arguments verbatim. The Rust CLI
  speaks wrkslotsd names (`status-text`, `parse-state`, `--root`), not
  the Python CLI, so this opt-in is **not** drop-in compatible.
- `WRKSLOTS_ENGINE=rust-soak`: `common/bin/wrkslots-soak` accepts the
  exact Python CLI, translates covered commands to wrkslotsd names,
  runs everything else in Python with a stderr note, and logs each
  invocation (cmd, edition, rc, duration). Flag mismatch also falls
  back to Python (never silently).
- Generic `DAGRUN_ENGINE=rust` / `AGENT_UTILS_ENGINE=rust` for
  wrkslots refuses with exit 127 (mirror not at parity).
- Every Rust **writer** is fixture-guarded: it refuses unless
  `WRKSLOTSD_FIXTURE_WRITER=1` (`ensure_states::require_fixture`).
  Live registries stay on Python until an approved cutover. So even
  "full" writers below are fixture-parity only, not live-default.

## Per-command parity

Status: FULL = differential parity on fixtures; PARTIAL = a named
subset ported, the rest refuses by name toward Python; NONE = no Rust
counterpart. "Router" is Python for every row today (see above).
Differential coverage lives in `py/wrkslots/tests/test_*_rust.py`
(45 files) plus `py/tests/test_wrkslots_soak.py`,
`test_engine_resolver_wrkslots_optin.py`,
`test_wrkslotsd_source_contract.py`. There is no `cross/` wrkslots
differential; the Python-test-driven harness is the coverage.

| Python command | Rust (wrkslotsd) | Status | Differential |
|---|---|---|---|
| status | status-text, parse-status | FULL | test_status_cli/state_rust |
| doctor | doctor-text, parse-doctor | FULL | test_doctor_state/audit_cli_rust |
| audit | audit-text, parse-audit | FULL* | test_audit_state_rust (54 cases) |
| unpushed | unpushed, unpushed-text | FULL | test_unpushed_state_rust |
| shell-command | shell-command | FULL | test_shell_command_cli_rust |
| config (read) | validate-config, parse-config | FULL | test_config_validate/yamlconfig_rust |
| (formats) | parse-state/archive/handoff/hold/journal/states/global/holds/journals, parse-storage(-git), rebuild/status/explain/plan | FULL | one *_rust file each |
| hold / unhold | hold, unhold | FULL (writer) | test_hold_write_rust |
| heartbeat | heartbeat | FULL (writer) | test_heartbeat_write_rust |
| write-handoff | write-handoff | FULL (writer) | test_handoff_write_rust |
| read-handoff | read-handoff | FULL (writer) | test_read_handoff_write_rust |
| release | release | FULL (writer) | test_release_write_rust |
| adopt | adopt | FULL (writer) | test_adopt_write_rust |
| transfer-owner | transfer-owner (incl. --reopen) | FULL (writer) | test_transfer_owner_write_rust |
| recover-unbound-owner | recover-unbound-owner | FULL (writer) | test_recover_unbound_write_rust |
| freeze | freeze | FULL | test_freeze_rust |
| classify-create-journals | classify-create-journals | PARTIAL (see c4c4c64a gap) | test_classify_journals_rust (6) |
| reap-stray-worktrees | reap-stray-worktrees | FULL | test_reap_stray_rust |
| reclaim-orphan-state | reclaim-orphan-state | FULL | test_reclaim_state_rust |
| create | create, create-text | PARTIAL: worktree representation only; image representation refuses | test_create_write/text_rust |
| finish | finish | PARTIAL: ordinary path; slot-image records stay Python | test_finish_write_rust |
| remove | remove | PARTIAL: ordinary path; host-context, attestation, push/bundle/drift salvage, legacy HANDOFF.md quarantine stay Python | test_remove_write_rust |
| recover-absent-agent-row(s) | same names (+ -text) | PARTIAL: non-destructive outcomes; census/salvage apply refuses | test_recover_absent_write_rust |
| recover-absent-validate-rows | same name (+ -text) | PARTIAL: input/recognition; census plan/apply refuses | test_recover_validate_write_rust |
| recover-ownerless-agent-worktree | same name | PARTIAL: plan outcomes; destructive apply refuses | test_recover_ownerless_write_rust |
| recover-ownerless-agent-clone | same name | PARTIAL: plan outcomes; inventory/apply refuses | test_recover_clone_write_rust |
| recover-ownerless-agent-cache | same name | PARTIAL: plan outcomes; relocation refuses | test_recover_cache_write_rust |
| recover-ownerless-validate-batch / classify-… | same names (+ -text) | PARTIAL: argument contract only; census/recovery refuses | test_recover_cache_write_rust |
| recover | recover | PARTIAL: dispatch outcomes only; resume/roll-forward/cleanup refuse (see #1007) | test_recover_main_write_rust (6) |
| quickstart | — | NONE (5-line Python handler; text only) | — |
| init | ensure-states (bootstrap only) | NONE as a command | test_ensure_states_rust (partial) |
| register | — | NONE (52-line handler) | — |
| import-existing | — | NONE (169-line handler) | — |
| supersede-handoff | — | NONE (107-line handler) | — |
| relocate-repository | — | NONE (206-line handler) | — |
| retirement-queue | — | NONE (8-line handler; read model exists) | — |
| retire-pending | — | NONE (378-line handler) | — |
| clean-caches | — (audit census is separate) | NONE (266-line handler) | — |
| remove-validate-batch | — | NONE (62-line handler + seal machinery) | — |
| remove-agent-batch | — | NONE (44-line handler) | — |
| config convert | — (readers exist) | NONE (56-line handler) | — |
| image (7 actions) | — | NONE (slotimage 1,859 + imagecmd) | — |
| worktrees-image (5 actions) | — | NONE (worktreesimage 614) | — |
| migrate | — | NONE | — |
| run | — | NONE (sandbox 2,355) | — |
| box (incl. coordinator) | — | NONE | — |
| sandbox | — | NONE | — |
| limits | — | NONE | — |

\* audit FULL with two named spelling corners still Python-owned:
non-UTF-8 entry-name glob matching and generic JSON/OS error
spellings (#562 slices 57–58 closed the funded corners).

Python lines left by area (handler spans in `py/wrkslots/cli.py`):
audit 1,005 (reader done; command shell remains), remove-in-scope
1,049, recover 541 + recover-create 395 + recover-finish 432 +
partial-updates 344, retire-pending 378, create 343,
ownerless-validate-batch 292+247, clean-caches 266, init 222,
relocate 206, reap-stray 200, read/write-handoff 178 each,
import-existing 169, transfer-owner 159 (done), release 156.
The long tail is recovery apply paths and the image/sandbox surface.

## Recent changes folded in (verified at HEAD)

- #1007 create-journal-rollforward (`dd825eee`, Python only):
  `recover` rolls a scoped journal one step behind its event log
  forward (`_scoped_journal_roll_forward_target` 83 lines +
  `_roll_forward_lagging_scoped_journals` 54 lines in cli.py).
  Rust `recover` refuses resume outright
  (`recover_main_write.rs`: "resuming the recorded {kind} recovery
  stays with the Python edition"). **Gap — port step P1.**
- #982 transfer-owner (`41b7daa7`): landed in **both** editions,
  including `--reopen` and the nested-worktree warning
  (`transfer_write.rs` warns and records
  `unregistered_nested_worktrees`). The brief's "Rust lacks #982"
  is stale; **no gap**. (Brief correction reported to coordb.)
- #992 live-journal busy (`5e1d1c99` + `5ba9c3ec`): landed in both
  editions; Rust prints the busy JSON and exits 75
  (`cli.rs` busy branch, `classify_journals.rs` owner/scope proof).
  **No gap.**
- `c4c4c64a` classify-create-journals structured blocking reasons
  (Python only): `--format json` now prints
  `{blocking:true, blocking_reasons:[{kind, operation, actor_state,
  targets, path}], schema:2}` for seal refusals. Rust classify emits
  only the success `{blocking:false}` and the #992 busy form.
  **Gap — port step P2.**

## Ordered port plan (revised 2026-11-07, coordb direction)

Rust becomes primary EARLY, per command, not in one final flip.
Each step: Rust implementation, differential test against Python,
soak mapping extended, router default flipped for that command once
parity is proven. Python becomes the opt-out
(`WRKSLOTS_ENGINE=python`) and stays only until P12. New behaviour
lands in BOTH editions while both exist. P1, P3/P4, and the P5
families run CONCURRENTLY across internal subagents (one per
command group); integration and landing stay with mu-rswrk.

- P2 (next). classify-create-journals blocking_reasons (c4c4c64a).
  S (0.5–1 d). Proves the port loop.
- P10 + early P11 (immediately after P2). CLI compatibility shell:
  wrkslotsd accepts the exact Python CLI (names, flags, global
  options) so `WRKSLOTS_ENGINE=rust` is drop-in; soak translation
  retired. Then flip Rust-default for the commands already FULL:
  status, doctor, audit, unpushed, shell-command, config read,
  hold/unhold, heartbeat, write/read-handoff, release, adopt,
  transfer-owner, recover-unbound-owner, freeze,
  reap-stray-worktrees, reclaim-orphan-state. Staged guard lift:
  readers first, then writers (`WRKSLOTSD_FIXTURE_WRITER` lifted in
  stages), each soaked on the harness with Python as opt-out.
  M (3–4 d). **Target: those commands Rust-default by 2026-11-14.**
- P1 (parallel). recover roll-forward (#1007) + journal resume
  dispatch. M (2–3 d).
- P3/P4 (parallel). Small NONE commands (quickstart,
  retirement-queue, register, supersede-handoff, config convert),
  then import-existing, relocate-repository, init shell. M (4 d).
- P5 (parallel, per family). Recovery apply paths:
  recover-create/finish resume, absent-row census/salvage,
  ownerless apply (worktree/clone/cache), validate-batch
  census/recovery. Each family flips Rust-default as it lands.
  L (8–12 d, split per family).
- P6. retire-pending, remove-validate-batch, remove-agent-batch,
  clean-caches. M–L (5–7 d).
- P7 (mu-rswrk side). create/finish/remove completion: image
  representation hooks, host-context and salvage corners.
  (Slot-image record diagnosis already landed in slice 45.) L (4–6 d).
- P8/P9 (mu-rsimg lane, parallel). Image surface (image,
  worktrees-image, migrate) and sandbox surface (run, box, sandbox,
  limits). Self-contained modules; see boundary below. L (8–12 d).
- Flip completion. Every remaining command flips Rust-default as
  soon as its port lands; full Rust-default by 2026-12-10.
- P12. Archive Python per #238: move `py/wrkslots` to archived with
  a deprecation notice once every command is Rust-default with
  differential coverage. S (1 d). **Target: 2026-12-15.**

Total remaining: roughly 40–60 agent-days across both lanes.

## Boundary with mu-rsimg (P8/P9 lane)

- mu-rswrk owns wrkslotsd's shared core: `rs/wrkslotsd/src/cli.rs`,
  the format modules, `ensure_states`, the router
  (`common/bin/engine-resolver`, `common/bin/wrkslots-soak`), and
  `validation/editions.json`.
- mu-rsimg owns the new Rust modules for the image and sandbox
  surface (counterparts of `py/wrkslots/slotimage.py`,
  `imagecmd.py`, `worktreesimage.py`, `sandbox.py`,
  `imagehelper.py`) and their differentials.
- mu-rsimg asks mu-rswrk before touching any shared file above;
  mu-rswrk integrates shared-file changes. Until direct agentctl
  sends are set up, both lanes coordinate through coordb.
- Each lane lands from its own slot; neither edits the other's
  slot checkout.

## Landing sequence (revised clock ETAs, America/New_York)

- M1 inventory (this doc): 2026-11-07.
- P2: 2026-11-08 17:00 ET.
- P10 + early flip (FULL commands Rust-default): 2026-11-14 17:00 ET.
- P1 (parallel): 2026-11-15 17:00 ET. P3/P4 (parallel): 2026-11-18.
- P5 families: 2026-11-25 17:00 ET. P6: 2026-11-28 17:00 ET.
- P7: 2026-12-02 17:00 ET. P8/P9 (mu-rsimg): 2026-12-06 17:00 ET.
- Full Rust-default: 2026-12-10 17:00 ET.
- P12 archive: 2026-12-15 17:00 ET.

Gate per landing: `python3 scripts/validate.py --all --keep-going`
on the exact push tree, bare `scripts/check_client_names.py`, and
`cargo fmt --all --manifest-path rs/Cargo.toml -- --check`. A failure
on the exact push tree is real unless it passes alone 5 times in a
row on that same tree with a named load-dependent cause (coordb
rule, 2026-11-07). Land via `land-main`; report each 40-hex SHA to
coordb only after it exits 0.

Coordination: another team's lane lands wrkslots fixes in
`py/wrkslots/cli.py` often; rebase onto github/main frequently and
port its new changes to Rust in the same step. Never leave Rust
behind Python again.
