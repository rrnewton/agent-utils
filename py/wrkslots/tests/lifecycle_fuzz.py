#!/usr/bin/env python3
"""Seeded random lifecycle fuzz with the transition model as oracle (#2043).

Generates reproducible random command sequences (create, hold,
unhold, remove, recover) over two slots in a scratch registry,
injects crashes at the CLI's test seams between journal and event
writes, and after every step decodes the real state (event chain,
ACTIVE snapshot, journals including containing-refs sidecars, seal)
in a separate snippet process.  The decoded state is judged
against the invariants the TLA+ transition spec checks: the event chain
always loads or its refusal names an outstanding journal/seal,
the reloaded journal equals the in-memory journal after expansion,
every interrupted state is completed by one recover (or the
refusal is classified), no refusal on a quiescent consistent
store, and snapshot divergence heals on the next writer.

Live-owner vocabulary (adopt, heartbeat by a live owner,
transfer-owner via the mock-agent tree) is exercised by
e2e_stress.py and joins this harness's vocabulary only where the
scratch project can drive it without an agent tree; the seeded
core here is the journal/crash/recover surface the model covers.

Run by hand (opt-in soak):
    python3 py/wrkslots/tests/lifecycle_fuzz.py --seed 1 --steps 24
The gated subset is test_lifecycle_fuzz.py (seeds 1..4).
"""

from __future__ import annotations

import argparse
import atexit
import dataclasses
import json
import os
import random
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from wrkslots.tests import e2e_stress as e2e  # noqa: E402

SLOTS = ("fz-a", "fz-b")
CRASH_POINTS = (
    "after-create-worktree",
    "after-active-write",
    "after-finish-journal",
    "after-remove-worktree",
    "after-operation-completed",
)

_DECODE_PROGRAM = """
import json, sys
from pathlib import Path
from wrkslots import cli

root, machine = sys.argv[1], sys.argv[2]
config = cli._load_config(root, machine)
out = {"chain_loads": False, "error": "", "slots": [], "archived": [],
       "journals": [], "sidecars": [], "seal": False, "snap_rev": -1,
       "tip": -1}
try:
    states = cli._load_all_active(config, require_repository=False)
    out["chain_loads"] = True
    state = next(s for s in states if s.machine == machine)
    out["tip"] = state.revision
    out["slots"] = sorted(r.slot for r in state.slots)
    archives = cli._load_all_archives(config, require_repository=False)
    out["archived"] = sorted(
        r["slot"] for a in archives if a.machine == machine
        for r in a.records)
except Exception as exc:
    out["error"] = f"{type(exc).__name__}: {exc}"
control = config.control
for pat in ("FINISH.*.journal", "CREATE.*.journal", "ACTIVE.*.journal"):
    for path in sorted(control.glob(pat)):
        entry = {"name": path.name, "reload_equal": None}
        try:
            loaded, raw = cli._load_journal(config, selected_path=path)
            entry["kind"] = raw.get("kind")
            entry["blocker"] = cli._journal_blocker(raw, path)[0]
            disk = json.loads(path.read_text(encoding="utf-8")) \\
                if path.exists() else None
            if isinstance(disk, dict):
                cli._expand_finish_sidecars(config, disk)
                entry["reload_equal"] = cli._json_equal(disk, raw) \\
                    if raw.get("kind") == "finish" else None
        except Exception as exc:
            entry["error"] = f"{type(exc).__name__}: {exc}"
        out["journals"].append(entry)
side = control / "containing-refs"
if side.is_dir():
    out["sidecars"] = sorted(p.name for p in side.glob("*.json"))
out["seal"] = bool(list(control.glob("VALIDATE-BATCH-SEAL.*.journal")))
snap = Path(root) / "worktrees" / f"ACTIVE.{machine}.json"
if snap.exists():
    try:
        out["snap_rev"] = json.loads(snap.read_text(encoding="utf-8"))["revision"]
    except Exception:
        out["snap_rev"] = -2
print(json.dumps(out, sort_keys=True))
"""


class FuzzFailure(RuntimeError):
    """One decoded state diverged from the model oracle."""


_CLI_TIMEOUT = 600.0


def _cli(
    project: e2e.TestProject,
    *args: str,
    expected: int | tuple[int, ...] = 0,
    trace: e2e.Trace | None = None,
) -> subprocess.CompletedProcess[str]:
    """wrkslots CLI with a hot-host bound (owner testing rule).

    Every wait in this harness is condition-based (decode
    polling); the only fixed bounds are process ceilings, and
    those must survive a saturated gate host where one census
    can take minutes — 600s, not e2e's interactive 120s.
    """
    return e2e._run(
        [sys.executable, "-m", "wrkslots", "--project-root",
         str(project.root), "--machine", e2e.MACHINE,
         "--wait-lock", "30", *args],
        cwd=project.root, env=project.environment,
        expected=expected, timeout=_CLI_TIMEOUT, trace=trace,
    )


def _remove(
    project: e2e.TestProject, slot: str, trace: e2e.Trace,
    *, expected: tuple[int, ...] = (0, 3, 86),
) -> subprocess.CompletedProcess[str]:
    return _cli(
        project, "remove", slot, "--coordinator-pid", str(os.getpid()),
        "--expected-generation", "1", expected=expected, trace=trace,
    )


def _create_unbound(
    project: e2e.TestProject, slot: str, agent: str, trace: e2e.Trace
) -> None:
    _cli(
        project, "create", slot, "--slot-type", "agent",
        "--coordinator-authorized", "--agent", agent,
        "--task", f"task-{slot}", "--purpose", f"e2e scenario {slot}",
        "--coordinator-pid", str(os.getpid()),
        "--repo", "product=product", "--branch", f"product=e2e/{slot}",
        trace=trace,
    )


_FAULT_PROGRAM = """
import os, runpy, stat, sys
real_fsync = os.fsync
target = int(os.environ.get("WRKSLOTS_FUZZ_FAULT_AFTER", "0"))
seen = {"n": 0}
def flaky(fd):
    try:
        is_dir = stat.S_ISDIR(os.fstat(fd).st_mode)
    except OSError:
        is_dir = False
    if is_dir:
        if seen["n"] == target:
            raise OSError(5, "injected directory fsync failure")
        seen["n"] += 1
    return real_fsync(fd)
os.fsync = flaky
sys.argv = ["wrkslots"] + sys.argv[1:]
runpy.run_module("wrkslots", run_name="__main__")
"""


def _cli_fault(
    project: e2e.TestProject, *args: str, after: int = 0
) -> subprocess.CompletedProcess[str]:
    """One CLI call whose (after+1)-th directory fsync fails (EIO).

    The rename has already landed when the fault fires, so the
    durable truth is whatever decode finds afterwards; the
    reported outcome is compared against it by the caller.
    Sweeping ``after`` walks the fault across create's commit
    boundaries (journal, event, ACTIVE snapshot).
    """
    env = dict(project.environment)
    env["WRKSLOTS_FUZZ_FAULT"] = "fsync-dir"
    env["WRKSLOTS_FUZZ_FAULT_AFTER"] = str(after)
    return subprocess.run(
        [sys.executable, "-c", _FAULT_PROGRAM, "--project-root",
         str(project.root), "--machine", e2e.MACHINE, "--wait-lock", "10",
         *args],
        cwd=project.root, env=env, text=True, capture_output=True,
        timeout=_CLI_TIMEOUT, check=False,
    )


def _classify_fault_create(
    reported_ok: bool, state: dict[str, object], slot: str
) -> str:
    slots = state.get("slots")
    durable = isinstance(slots, list) and slot in slots
    journals = state.get("journals")
    journaled = isinstance(journals, list) and any(
        isinstance(j, dict) and slot in str(j.get("name", ""))
        for j in journals
    )
    committed = durable or journaled
    if reported_ok and committed:
        return "consistent-commit"
    if not reported_ok and not committed:
        return "consistent-refusal"
    if not reported_ok and committed:
        return "FINDING reports-failure-but-state-committed"
    return "FINDING reports-success-but-state-absent"


def _decode(project: e2e.TestProject) -> dict[str, object]:
    done = subprocess.run(
        [sys.executable, "-c", _DECODE_PROGRAM, str(project.root), e2e.MACHINE],
        cwd=project.root, env=project.environment,
        text=True, capture_output=True, timeout=_CLI_TIMEOUT, check=False,
    )
    if done.returncode != 0:
        raise FuzzFailure(f"decode crashed: {done.stderr.strip()}")
    value = json.loads(done.stdout)
    if not isinstance(value, dict):
        raise FuzzFailure(f"decode is not an object: {value!r}")
    return value


def _classify_state(state: dict[str, object]) -> str:
    """Operator reading of one decoded state (never itself a failure)."""
    if state.get("chain_loads"):
        return "loadable"
    journals = state.get("journals")
    entries = (
        [j for j in journals if isinstance(j, dict)]
        if isinstance(journals, list) else []
    )
    # A refusal to recover a journal whose owner is still LIVE is
    # correct behaviour (#1367), classified, never an invariant
    # failure.
    if entries and all(j.get("blocker") == "live" for j in entries):
        return "expected-live-refusal"
    if entries or state.get("seal"):
        return "interrupted-explained"
    return "unexplained-refusal"


def _oracle(state: dict[str, object], *, step: str) -> list[str]:
    """Model invariants over one decoded state; returns violations."""
    problems: list[str] = []
    journals = state.get("journals")
    if not isinstance(journals, list):
        return [f"{step}: decode has no journal list"]
    outstanding = [j for j in journals if isinstance(j, dict)]
    if not state.get("chain_loads"):
        # The event chain always loads, or the refusal is explained
        # by an outstanding journal or seal (C1 names the rest).
        if not outstanding and not state.get("seal"):
            problems.append(
                f"{step}: chain refuses with no journal/seal outstanding: "
                f"{state.get('error')}"
            )
    for entry in outstanding:
        if entry.get("reload_equal") is False:
            problems.append(
                f"{step}: reloaded journal differs from its expanded "
                f"on-disk form: {entry.get('name')}"
            )
        if entry.get("reload_equal") is None and entry.get("error"):
            problems.append(
                f"{step}: journal {entry.get('name')} does not load: "
                f"{entry.get('error')}"
            )
    return problems


def _wait_chain_loads(
    project: e2e.TestProject, *, timeout: float = 30.0
) -> dict[str, object]:
    """Condition-based wait (never a sleep-as-sync): poll decode."""
    deadline = time.monotonic() + timeout
    state = _decode(project)
    while not state.get("chain_loads") and time.monotonic() < deadline:
        time.sleep(0.1)
        state = _decode(project)
    return state


_ACTIVE: dict[str, object] = {}


def _attached_loop_devices(root: Path) -> set[str]:
    """Loop devices whose backing file lives under ``root``.

    Recorded by device name at attach time (after project
    creation and after every step). Teardown touches only
    devices in this recorded set — never a pattern, never a
    device another run attached.
    """
    devices: set[str] = set()
    try:
        done = subprocess.run(
            ["losetup", "-l", "-O", "NAME,BACK-FILE"],
            text=True, capture_output=True, timeout=30, check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return devices
    prefix = str(root) + "/"
    for line in done.stdout.splitlines()[1:]:
        fields = line.split()
        if len(fields) >= 2 and fields[1].startswith(prefix):
            devices.add(fields[0])
    return devices


def _record_attached(project: e2e.TestProject) -> None:
    recorded = _ACTIVE.get("devices")
    current = _attached_loop_devices(project.root)
    if isinstance(recorded, set):
        recorded |= current
    else:
        _ACTIVE["devices"] = set(current)


def _residual_loop_devices(project: e2e.TestProject) -> set[str]:
    recorded = _ACTIVE.get("devices")
    if not isinstance(recorded, set):
        return set()
    return recorded & _attached_loop_devices(project.root)


def _exit_sweep() -> None:
    """Detach this run's slot images through wrkslots (#2048).

    Every exit path funnels here (finally, atexit, signal), so a
    run never leaves a mounted image for its rmtree to delete
    under: that is what stranded loop devices in /tmp.
    """
    project = _ACTIVE.get("project")
    trace = _ACTIVE.get("trace")
    if isinstance(project, e2e.TestProject) and isinstance(trace, e2e.Trace):
        try:
            e2e._detach_project_images(project.root, trace)
        except Exception:
            pass
        residual = _residual_loop_devices(project)
        if residual:
            # Unmount went through wrkslots; a recorded device
            # still attached is reported by name, never detached
            # by pattern or by hand here.
            _ACTIVE["residual_loops"] = residual
            print(f"residual loop devices for this run: "
                  f"{sorted(str(d) for d in residual)}", file=sys.stderr)


def _on_signal(signum: int, _frame: object) -> None:
    trees = _ACTIVE.get("trees")
    if isinstance(trees, dict):
        _stop_trees(trees)
    # Stop the in-flight CLI child first: it may hold the slot
    # whose images the sweep must unmount. Children are read
    # from /proc, never matched by name or pattern.
    try:
        for task in Path(f"/proc/{os.getpid()}/task").iterdir():
            try:
                children = (task / "children").read_text(
                    encoding="ascii").split()
            except OSError:
                continue
            for raw in children:
                try:
                    os.kill(int(raw), signal.SIGTERM)
                except (OSError, ValueError):
                    pass
    except OSError:
        pass
    _exit_sweep()
    os._exit(128 + signum)


def _assert_mounts_allowed() -> None:
    """Fail loudly before any sudo -n mount helper can run.

    Image slots mount through wrkslots' sudo -n helper. Root
    mounts are opt-in (owner rule): a run whose projects would
    use the image representation refuses here, by name, unless
    WRKSLOTS_ALLOW_ROOT_TESTS=1. Plain-worktree runs
    never reach the helper.
    """
    representation = os.environ.get("WRKSLOTS_INIT_REPRESENTATION", "image")
    if representation != "worktree" and \
            os.environ.get("WRKSLOTS_ALLOW_ROOT_TESTS") != "1":
        raise FuzzFailure(
            "image slots would mount through the sudo -n helper; "
            "set WRKSLOTS_INIT_REPRESENTATION=worktree for a "
            "root-free run or WRKSLOTS_ALLOW_ROOT_TESTS=1 "
            "to allow root mounts"
        )


def _stop_trees(trees: dict[str, e2e.AgentTree]) -> None:
    for tree in trees.values():
        try:
            tree.stop()
        except Exception:
            tree.terminate_tree()
    trees.clear()


def _heal_if_journaled(project: e2e.TestProject, trace: e2e.Trace) -> None:
    """One recover before a create when a journal is outstanding.

    A crashed create that reached ACTIVE publication leaves a
    completed-create journal; classification then refuses new
    creates registry-wide until recover clears it — exactly the
    model's one-recover completion, so the generator heals first
    instead of treating the refusal as a divergence.
    """
    state = _decode(project)
    if state.get("journals"):
        _cli(project, "recover", "--coordinator-pid", str(os.getpid()),
                 expected=(0, 3), trace=trace)


def _land_and_finish(project: e2e.TestProject, tree: e2e.AgentTree) -> None:
    """Publish the slot's work and finish it (the fence lands inside)."""
    e2e._do_and_land_work(project, tree, tree.trace)
    e2e._finish(tree)


def _run_seed(
    seed: int, steps: int, base: Path, *, keep: bool, faults: bool = False
) -> None:
    _assert_mounts_allowed()
    trace = e2e.Trace(base / "trace.jsonl", seed=seed)
    project = e2e._make_project(base, trace, ttl_seconds=2)
    rng = random.Random(seed)
    live: set[str] = set()
    archived: set[str] = set()
    trees: dict[str, e2e.AgentTree] = {}
    runner_owned: set[str] = set()
    landed: set[str] = set()
    # A failure mid-sequence must not orphan mock-agent trees.
    atexit.register(_stop_trees, trees)
    _ACTIVE.update(project=project, trace=trace, trees=trees)
    atexit.register(_exit_sweep)
    _record_attached(project)
    _ACTIVE.update(project=project, trace=trace, trees=trees)
    atexit.register(_exit_sweep)
    _record_attached(project)
    for index in range(steps):
        slot = rng.choice(SLOTS)
        choices = ["create", "hold", "unhold", "remove", "recover", "status",
                   "adopt", "heartbeat", "finish", "fence", "transfer"]
        if faults:
            choices.append("fault-create")
        op = rng.choice(choices)
        crash = rng.choice(CRASH_POINTS) if rng.random() < 0.25 else None
        label = f"seed={seed} step={index} op={op} slot={slot} crash={crash}"
        env = dict(project.environment)
        if crash:
            env["WRKSLOTS_TEST_INTERRUPT"] = crash
        project_crash = (
            dataclasses.replace(project, environment=env) if crash else project
        )
        try:
            if op == "fault-create" and slot not in live and slot not in archived:
                fault_after = rng.randrange(5)
                done = _cli_fault(
                    project, "create", slot, "--slot-type", "agent",
                    "--coordinator-authorized", "--agent", f"fz-{slot}",
                    "--task", f"task-{slot}", "--purpose", "fuzz fault",
                    "--coordinator-pid", str(os.getpid()),
                    "--repo", "product=product",
                    "--branch", f"product=e2e/{slot}",
                    after=fault_after,
                )
                after = _decode(project)
                verdict = _classify_fault_create(
                    done.returncode == 0, after, slot
                )
                trace.add("fault", label=label, verdict=verdict,
                          rc=done.returncode, after=fault_after)
                if verdict.startswith("FINDING"):
                    raise FuzzFailure(f"{label}: {verdict}")
            elif op == "adopt" and slot not in live and slot not in archived:
                _heal_if_journaled(project, trace)
                _create_unbound(project_crash, slot, f"fz-{slot}", trace)
                tree = e2e.AgentTree(project, slot, f"fz-{slot}", trace)
                trees[slot] = tree
                e2e._adopt(tree)
                live.add(slot)
            elif op == "heartbeat" and slot in trees:
                trees[slot].wrkslots(
                    "heartbeat", slot, "--agent", f"fz-{slot}",
                    "--owner-pid", str(trees[slot].info["engine_pid"]),
                    "--expected-generation", "1", expected=(0, 3))
            elif op == "heartbeat" and slot in runner_owned:
                _cli(project_crash, "heartbeat", slot, "--agent",
                         f"fz-new-{slot}", "--owner-pid", str(os.getpid()),
                         "--expected-generation", "1",
                         expected=(0, 3), trace=trace)
            elif op == "finish" and slot in trees:
                tree = trees.pop(slot)
                if slot in landed:
                    e2e._finish(tree)
                else:
                    _land_and_finish(project, tree)
                landed.discard(slot)
                tree.stop()
                live.discard(slot)
            elif op == "fence" and slot in trees:
                # Publish, then interrupt finish at its fence seam:
                # the fenced finish journal is what recover meets.
                if slot not in landed:
                    e2e._do_and_land_work(project, trees[slot], trace)
                    landed.add(slot)
                fence_env = dict(project.environment)
                fence_env["WRKSLOTS_TEST_INTERRUPT"] = rng.choice(
                    ("after-path-fence", "after-finish-journal"))
                fenced = dataclasses.replace(project, environment=fence_env)
                _cli(fenced, "finish", slot, "--agent", f"fz-{slot}",
                         "--owner-pid", str(trees[slot].info["engine_pid"]),
                         "--expected-generation", "1",
                         "--validation", "wrkslots fuzz: pass",
                         expected=(0, 3, 86), trace=trace)
            elif op == "transfer" and slot in trees:
                tree = trees.pop(slot)
                tree.kill_engine()
                tree.terminate_tree()
                deadline = time.monotonic() + 20
                moved = False
                while time.monotonic() < deadline and not moved:
                    done = _cli(
                        project, "transfer-owner", slot, "--agent",
                        f"fz-new-{slot}", "--owner-pid", str(os.getpid()),
                        "--expected-generation", "1",
                        expected=(0, 3), trace=trace)
                    moved = done.returncode == 0
                    if not moved:
                        time.sleep(0.2)
                if moved:
                    runner_owned.add(slot)
            elif op == "create" and slot not in live and slot not in archived:
                _heal_if_journaled(project, trace)
                _create_unbound(project_crash, slot, f"fz-{slot}", trace)
                live.add(slot)
            elif op == "hold" and slot in live:
                _cli(project_crash, "hold", slot, "--reason", "fuzz",
                         expected=(0, 3), trace=trace)
            elif op == "unhold" and slot in live:
                _cli(project_crash, "unhold", slot,
                         expected=(0, 3), trace=trace)
            elif op == "remove" and slot in live:
                _wait_chain_loads(project)
                done = _remove(
                    project_crash, slot, trace, expected=(0, 3, 86)
                )
                if done.returncode == 0:
                    live.discard(slot)
            elif op == "recover":
                _cli(project_crash, "recover", "--coordinator-pid",
                         str(os.getpid()),
                         expected=(0, 3, 86), trace=trace)
            else:
                _cli(project_crash, "status", expected=(0, 3), trace=trace)
        except Exception:
            if not crash:
                raise
        state = _decode(project)
        # The decoded registry, not the intended sequence, is the
        # truth about which slots exist (a crash can land either
        # side of the durable write).
        decoded_slots = state.get("slots")
        if isinstance(decoded_slots, list):
            live = {s for s in decoded_slots if s in SLOTS}
        decoded_arch = state.get("archived")
        if isinstance(decoded_arch, list):
            archived = {s for s in decoded_arch if s in SLOTS}
        problems = _oracle(state, step=label)
        if _classify_state(state) == "expected-live-refusal":
            # Correct refusal; nothing to heal while the owner lives.
            pass
        elif crash or state.get("journals"):
            healed = _wait_chain_loads(project)
            problems += [
                p for p in _oracle(healed, step=label + " post-recover-wait")
                if p not in problems
            ]
        _record_attached(project)
        _record_attached(project)
        trace.add("step", label=label, chain_loads=state.get("chain_loads"),
                  classification=_classify_state(state),
                  journals=state.get("journals"))
        if problems:
            raise FuzzFailure("; ".join(problems))
    _stop_trees(trees)
    final = _wait_chain_loads(project)
    problems = _oracle(final, step=f"seed={seed} final")
    if problems:
        raise FuzzFailure("; ".join(problems))
    print(f"lifecycle fuzz passed; seed={seed} steps={steps}")


def _run_ops_file(ops_path: Path, root: Path) -> None:
    """Child mode: execute a fixed op list against a shared project."""
    spec = json.loads(ops_path.read_text(encoding="utf-8"))
    project = e2e.TestProject(
        root=root, repository=root / "product",
        remote=root.parent / "product.git", environment=e2e._environment(),
    )
    trace = e2e.Trace(ops_path.with_suffix(".trace.jsonl"), seed=int(spec["seed"]))
    trees: dict[str, e2e.AgentTree] = {}
    atexit.register(_stop_trees, trees)
    owned: set[str] = set()
    for item in spec["ops"]:
        op, slot = item["op"], item["slot"]
        label = f"worker step op={op} slot={slot}"
        before = _decode(project)
        raw_slots = before.get("slots")
        known = (
            {s for s in raw_slots if isinstance(s, str)}
            if isinstance(raw_slots, list) else set()
        )
        raw_arch = before.get("archived")
        gone = (
            {s for s in raw_arch if isinstance(s, str)}
            if isinstance(raw_arch, list) else set()
        )
        if op == "create" and (slot in known or slot in gone):
            continue
        if op in ("hold", "unhold", "remove") and slot not in known:
            continue
        if op == "adopt" and slot not in known and slot not in gone:
            _heal_if_journaled(project, trace)
            _create_unbound(project, slot, f"fz-{slot}", trace)
            tree = e2e.AgentTree(project, slot, f"fz-{slot}", trace)
            trees[slot] = tree
            e2e._adopt(tree)
        elif op == "heartbeat" and slot in trees:
            trees[slot].wrkslots(
                "heartbeat", slot, "--agent", f"fz-{slot}",
                "--owner-pid", str(trees[slot].info["engine_pid"]),
                "--expected-generation", "1", expected=(0, 3))
        elif op == "heartbeat" and slot in owned:
            _cli(project, "heartbeat", slot, "--agent", f"fz-new-{slot}",
                     "--owner-pid", str(os.getpid()),
                     "--expected-generation", "1",
                     expected=(0, 3), trace=trace)
        elif op == "transfer" and slot in trees:
            tree = trees.pop(slot)
            tree.kill_engine()
            tree.terminate_tree()
            done = _cli(project, "transfer-owner", slot, "--agent",
                            f"fz-new-{slot}", "--owner-pid", str(os.getpid()),
                            "--expected-generation", "1",
                            expected=(0, 3), trace=trace)
            if done.returncode == 0:
                owned.add(slot)
        elif op == "create":
            _heal_if_journaled(project, trace)
            _create_unbound(project, slot, f"fz-{slot}", trace)
        elif op == "hold":
            _cli(project, "hold", slot, "--reason", "fuzz",
                     expected=(0, 3), trace=trace)
        elif op == "unhold":
            _cli(project, "unhold", slot, expected=(0, 3), trace=trace)
        elif op == "remove":
            done = _remove(project, slot, trace, expected=(0, 3))
        elif op == "recover":
            _cli(project, "recover", "--coordinator-pid",
                     str(os.getpid()), expected=(0, 3), trace=trace)
        state = _decode(project)
        problems = _oracle(state, step=label)
        if problems:
            raise FuzzFailure("; ".join(problems))
    for tree in trees.values():
        try:
            tree.stop()
        except Exception:
            tree.terminate_tree()
    print(f"worker done seed={spec['seed']}")


def _wait_no_holders(root: Path, *, timeout: float = 300.0) -> None:
    """Condition wait: no live process cwd under ``root``.

    A joined worker's mock command tree can drain for a few
    seconds after the worker exits; unmounting into that window
    refuses on the census. Poll the condition, never a duration.
    """
    prefix = str(root) + "/"
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        holders = []
        for cwd in Path("/proc").glob("[0-9]*/cwd"):
            try:
                target = os.readlink(cwd)
            except OSError:
                continue
            if target == str(root) or target.startswith(prefix):
                holders.append(cwd.parent.name)
        if not holders:
            return
        time.sleep(1)


def _run_concurrent(seed: int, base: Path) -> None:
    """Two processes, disjoint slots, one shared registry."""
    _assert_mounts_allowed()
    trace = e2e.Trace(base / "trace.jsonl", seed=seed)
    project = e2e._make_project(base, trace, ttl_seconds=2)
    _ACTIVE.update(project=project, trace=trace, trees={})
    atexit.register(_exit_sweep)
    _record_attached(project)
    rng = random.Random(seed)
    children: list[subprocess.Popen[str]] = []
    specs: list[Path] = []
    for worker, slot in enumerate(SLOTS):
        ops = []
        for _ in range(10):
            op = rng.choice(("create", "hold", "unhold", "remove", "recover",
                             "adopt", "heartbeat", "transfer"))
            ops.append({"op": op, "slot": slot})
        spec_path = base / f"ops-{worker}.json"
        spec_path.write_text(
            json.dumps({"seed": seed + worker, "ops": ops}), encoding="utf-8"
        )
        specs.append(spec_path)
        children.append(subprocess.Popen(
            [sys.executable, str(Path(__file__).resolve()),
             "--ops-file", str(spec_path), "--project-root", str(project.root)],
            text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        ))
    failures = []
    for child in children:
        out, err = child.communicate(timeout=600)
        if child.returncode != 0:
            failures.append(f"worker pid={child.pid} rc={child.returncode}: {err[-400:]}")
    if failures:
        raise FuzzFailure(" | ".join(failures))
    _wait_no_holders(project.root)
    healed = _wait_chain_loads(project)
    problems = _oracle(healed, step=f"seed={seed} concurrent final")
    e2e._cli(project, "recover", "--coordinator-pid", str(os.getpid()),
             expected=(0, 3), trace=trace)
    final = _wait_chain_loads(project)
    problems += _oracle(final, step=f"seed={seed} concurrent post-recover")
    if problems:
        raise FuzzFailure("; ".join(problems))
    print(f"lifecycle fuzz concurrent passed; seed={seed}")



def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--steps", type=int, default=24)
    parser.add_argument("--keep", action="store_true")
    parser.add_argument("--faults", action="store_true",
                        help="include injected directory-fsync failures")
    parser.add_argument("--concurrent", action="store_true",
                        help="two worker processes on disjoint slots")
    parser.add_argument("--ops-file", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--project-root", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.ops_file is not None:
        assert args.project_root is not None
        _run_ops_file(args.ops_file, args.project_root)
        return 0
    signal.signal(signal.SIGTERM, _on_signal)
    signal.signal(signal.SIGINT, _on_signal)
    base = Path(tempfile.mkdtemp(prefix="wrkslots-fuzz-"))
    try:
        if args.concurrent:
            _run_concurrent(args.seed, base)
        else:
            _run_seed(args.seed, args.steps, base, keep=args.keep,
                      faults=args.faults)
    finally:
        _exit_sweep()
        if not args.keep:
            import shutil

            remaining = e2e._mount_points_under(base)
            if remaining:
                # Never delete files under a live mount (#2048):
                # retain and say so instead of stranding loops.
                print(f"retained {base}: mounts remain: {remaining}",
                      file=sys.stderr)
            else:
                shutil.rmtree(base, ignore_errors=True)
    # Residual devices are reported by name above and their
    # directory retained; copies held by other namespaces are
    # the #2050 class, not a seed failure. The strict
    # zero-in-this-namespace property is asserted by
    # test_failing_run_leaves_no_loop_devices.
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
