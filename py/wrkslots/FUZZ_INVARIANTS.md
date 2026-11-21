# Shared fuzz invariants for wrkslots (#2043)

Both fuzz harnesses — the seeded lifecycle harness in this
repository and any external fault-injection harness — check
the SAME invariants against the SAME model. This note is the
contract: entry points, decode helpers, exact predicates,
and the oracle interface. Scratch registries only; never
point any of this at a live registry.

## Entry points (this repository)

| What | Where |
|---|---|
| Seeded harness | `py/wrkslots/tests/lifecycle_fuzz.py` (run as a script) |
| Gated subset | `py/wrkslots/tests/test_lifecycle_fuzz.py` (seeds 1..4 + one concurrent seed) |
| Fsync fault regression | `py/wrkslots/tests/test_fault_fsync.py` |
| Model (oracle source) | `py/wrkslots/transitionmodel.py`, `py/wrkslots/TRANSITIONMODEL.md` |
| TLA+ mirror | `py/wrkslots/spec/WrkslotsTransition.tla` (+ `run_tlc.sh`, opt-in) |
| Prior-art stress suite | `py/wrkslots/tests/e2e_stress.py` (scenarios, mock-agent tree) |

CLI (from the repository root, `PYTHONPATH=py`):

```bash
python3 py/wrkslots/tests/lifecycle_fuzz.py --seed 1 --steps 24
python3 py/wrkslots/tests/lifecycle_fuzz.py --seed 21 --concurrent
python3 py/wrkslots/tests/lifecycle_fuzz.py --seed 3 --steps 32 --faults
python3 -m pytest py/wrkslots/tests/test_lifecycle_fuzz.py -q
```

Seeds are `random.Random(seed)` draws; a seed plus its step
count fully determines the intended sequence. The trace
(`trace.jsonl` beside the scratch project) records every
step's label, decoded classification, and journal list, so a
failure replays by re-running its seed. Crash points are the
CLI test seams (`WRKSLOTS_TEST_INTERRUPT=<point>`, the child
exits 86; points include `after-create-worktree`,
`after-active-write`, `after-finish-journal`,
`after-path-fence`, `after-remove-worktree`,
`after-operation-completed`), plus SIGKILL of harness-started
owners by literal PID. Directory-fsync faults use a wrapper
that patches `os.fsync` to raise EIO on the (after+1)-th
directory fsync of one CLI call (`--faults`, offsets 0..4).

Scratch registry: each run builds an isolated project under
the system temp dir (`_make_project`: bare remote, product
clone, `wrkslots init --heartbeat-ttl-seconds 2`), machine
name `e2ehost`, two slots (`fz-a`, `fz-b`). Containment:
each seed is a subprocess with a bounded wall clock; the
gated runner adds CPU and virtual-memory rlimits
(`test_lifecycle_fuzz.py::_limits`).

## Representation without sudo and without FUSE

Under a ptrace harness, sudo's setuid does not elevate, so
the kernel image backend cannot run; this team does not
create FUSE mounts, so the `fuse` backend (fuse2fs) is out
too. Use plain directories from project creation:

```bash
wrkslots init <root> --slot-representation worktree
# config key: slot_representation: worktree
# existing project: wrkslots image set-default worktree
```

Only slots created afterwards are affected. In this mode
the image-representation invariants do not apply: slot-image
mount state and detach, image ceilings/trim, the state-image
private-HOME layer, and mount-leak teardown of the #2048
class. Every invariant below still applies unchanged —
state files are plain files, so the fsync family applies
directly.

## Decode helpers

After every step the real state is decoded in a separate
snippet process (`lifecycle_fuzz._decode`, program in
`_DECODE_PROGRAM`) using only `wrkslots.cli` readers:
`_load_all_active` (event-chain fold; `chain_loads`,
`tip`, active `slots`), `_load_all_archives` (`archived`),
`_load_journal` per outstanding journal (`kind`, plus
`_journal_blocker(raw, path)[0]` actor-liveness class:
`live|dead|legacy|indeterminate`), `_expand_finish_sidecars`
+ `_json_equal` for journal form equality, the
`containing-refs` sidecar listing, the validate-batch seal
glob, and the `ACTIVE.<machine>.json` snapshot revision.
External harnesses should decode through the same cli
readers (or the JSON `status` output) rather than parsing
files ad hoc, so both sides judge identical bytes.

## Invariants (name — exact predicate over decoded state)

1. **chain-always-loads** — `_load_all_active` succeeds, OR
   its refusal coincides with an outstanding journal or
   seal. A refusal with neither is a divergence.
2. **reloaded-equals-memory** — for every finish journal,
   the on-disk bytes expanded by `_expand_finish_sidecars`
   are `_json_equal` to the journal returned by
   `_load_journal`. (The #2041 invariant, on real bytes.)
3. **one-recover-completes** — after any crash step, a
   single `wrkslots recover` returns the registry to
   loadable with journals cleared, OR the refusal is
   classified (next item). The harness heals before creates:
   a stranded completed-create journal blocks new creates
   until that one recover clears it.
4. **live-owner-refusal-is-correct** — if every outstanding
   journal's blocker class is `live`, the state classifies
   `expected-live-refusal` (see `_classify_state`); it is
   never counted as an invariant failure, and no heal is
   attempted while the owner lives.
5. **no-refusal-on-quiescent-consistent** — in a resting,
   loadable, snapshot-in-sync state, the completion identity
   compares do not refuse (model ghost `formStuck` stays
   false).
6. **snapshot-heals** — ACTIVE snapshot divergence after a
   crash is lag only; the next successful writer or recover
   resynchronises it (no wedge sits on a loadable chain).
7. **fsync-report-matches-durable** — for an injected
   directory-fsync failure, reported success requires the
   create's row or journal durable, and a reported refusal
   with the journal durable classifies
   `FINDING reports-failure-but-state-committed`
   (`_classify_fault_create`); the reverse mismatch never
   appeared in the offset sweep 0..5.

State classification (`_classify_state`): `loadable`,
`expected-live-refusal`, `interrupted-explained`,
`unexplained-refusal`. Only the last, plus violations of
1–3 and 5–7, are divergences. Every divergence is reported
as a minimized seed+step reproduction plus a regression
test; product fixes stay with the owning lane.

## Model-oracle interface

```python
import sys; sys.path.insert(0, "py")
from wrkslots import transitionmodel as tm
ex = tm.explore(tm.Scenario())        # current-main variants
ex.violations("forked-chain")         # sticky safety ghosts
ex.wedges(); ex.manual_terminals()    # no-exit / hand-exit states
ex.trace(state)                       # shortest action-name trace
tm.is_clean(state); tm.is_resting(state)
```

`Scenario(fixes=...)` selects code variants
(`pre-delete-classify`, `rebase-on-advance`,
`stale-previous-refusal`, `assert-compares-revision`); the
TLA+ spec mirrors the same actions and constants for TLC.
The fuzz oracle above is the decoded-state projection of
these checks; the gated Python explorer remains the
in-validation cross-check.
