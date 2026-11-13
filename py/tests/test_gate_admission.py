"""Gate admission: the counting cap, the exclusive class, FIFO, and reaping."""
from __future__ import annotations

import datetime as dt
import fcntl
import importlib.util
import json
import os
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path
from types import ModuleType

import pytest

GATE = Path(__file__).resolve().parents[2] / "scripts" / "gate.py"
PY = sys.executable


@pytest.fixture(autouse=True)
def _hermetic_gate_config(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """No host gate config may change a test result: every per-host file
    gate.py reads (interleave, slots, phase priority, blackout) points at
    a test-owned path that does not exist, and the interleave env hook is
    cleared. Tests that exercise a config set their own path or value
    after this fixture. The 2026-11-11 failure: the host's
    gate-interleave.toml held a temporary after_owners = 99, and the
    in-process interleave test read it and saw no interleave."""
    monkeypatch.setenv("GATE_INTERLEAVE_CONFIG", str(tmp_path / "hostless-gate-interleave.toml"))
    monkeypatch.setenv("GATE_SLOTS_CONFIG", str(tmp_path / "hostless-gate-slots.toml"))
    monkeypatch.setenv("GATE_SLOTS", "2")
    monkeypatch.setenv("GATE_PHASE_PRIORITY_CONFIG", str(tmp_path / "hostless-gate-phase-priority.toml"))
    monkeypatch.setenv("GATE_BLACKOUT_CONFIG", str(tmp_path / "hostless-gate.toml"))
    monkeypatch.setenv("GATE_DEFER_CONFIG", str(tmp_path / "hostless-gate-defer.toml"))
    monkeypatch.setenv("GATE_DEFER_POLL_S", "0.05")
    monkeypatch.delenv("GATE_INTERLEAVE_AFTER", raising=False)
    monkeypatch.delenv("GATE_DEFER_UNIT_PATTERN", raising=False)
    monkeypatch.delenv("GATE_DEFER_MODE", raising=False)
    monkeypatch.delenv("GATE_DEFER_OBSERVE_LOG", raising=False)
    monkeypatch.delenv("GATE_DEFER_HOLDS_DIR", raising=False)
    monkeypatch.delenv("GATE_DEFER_QUIET_COMMAND", raising=False)
    monkeypatch.delenv("GATE_DEFER_NOW", raising=False)
    # Tests run real gate.py subprocesses whose commands wait on files in
    # tmp_path; with CPU containment on, each admitted command becomes a
    # real systemd user scope, and a test that times out leaves its scope
    # (and its wait loop) running on the host. Tests never create real
    # scopes: the containment wrapper itself is covered in-process by
    # test_contained_command_wraps_in_a_quota_scope.
    monkeypatch.setenv("GATE_CPU_CONTAINMENT", "off")


def _env(tmp_path: Path) -> dict[str, str]:
    base = {key: value for key, value in os.environ.items()
            if key not in ("GATE_PHASE_PRIORITY", "GATE_PRIORITY", "GATE_PRIORITY_REASON",
                           "GATE_INTERLEAVE_AFTER", "GATE_INTERLEAVE_CONFIG",
                           "GATE_DEFER_UNIT_PATTERN", "GATE_DEFER_MODE",
                           "GATE_DEFER_OBSERVE_LOG", "GATE_DEFER_HOLDS_DIR",
                           "GATE_DEFER_QUIET_COMMAND", "GATE_DEFER_NOW")}
    return {**base, "GATE_ADMISSION_DIR": str(tmp_path / "admission"),
            "GATE_LEDGER": str(tmp_path / "ledger.jsonl"), "GATE_PRUNE_TARGETS": "off",
            "GATE_SLOTS_CONFIG": str(tmp_path / "gate-slots.toml"),
            "GATE_INTERLEAVE_CONFIG": str(tmp_path / "gate-interleave.toml"),
            "GATE_PHASE_PRIORITY_CONFIG": str(tmp_path / "gate-phase-priority.toml"),
            "GATE_BLACKOUT_CONFIG": str(tmp_path / "gate.toml"),
            "GATE_DEFER_CONFIG": str(tmp_path / "gate-defer.toml"),
            "GATE_DEFER_POLL_S": "0.05",
            # A lane may run the suite under GATE_SLOTS=1; the tests assume the default.
            "GATE_SLOTS": "2",
            # Admitted test commands never run in real systemd scopes (see
            # the fixture): a timed-out test would leak the scope.
            "GATE_CPU_CONTAINMENT": "off",
            # The real preflight runs validation steps; these tests give it a stand-in.
            "GATE_PREFLIGHT_COMMAND": "true"}


def _start(tmp_path: Path, *args: str) -> subprocess.Popen[str]:
    return subprocess.Popen(
        [PY, str(GATE), *args], env=_env(tmp_path),
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)


def _wait_for_runs(tmp_path: Path, count: int) -> None:
    runs = tmp_path / "admission" / "runs"
    deadline = time.time() + 10
    while time.time() < deadline:
        if runs.exists() and len(list(runs.glob("*.json"))) >= count:
            return
        time.sleep(0.05)
    raise AssertionError("gate runners did not start")


def test_finished_runs_append_to_the_durable_ledger(tmp_path: Path) -> None:
    """#827: every finished run lands in the ledger outside /tmp, with its
    verdict and failed steps, and `stats` reports the first-run pass rate."""
    ledger = tmp_path / "ledger.jsonl"
    env = {**_env(tmp_path), "GATE_LEDGER": str(ledger)}
    ok = subprocess.run(
        [PY, str(GATE), "--", PY, "-c",
         "print('[a.step] ✓ PASS x'); print('validate: OK')"],
        env=env, capture_output=True, text=True)
    assert ok.returncode == 0, ok.stdout
    bad = subprocess.run(
        [PY, str(GATE), "--", PY, "-c",
         "print('[b.step] ✗ FAIL y'); print('validate: FAILED'); raise SystemExit(3)"],
        env=env, capture_output=True, text=True)
    assert bad.returncode == 3, bad.stdout
    rerun = subprocess.run(
        [PY, str(GATE), "--exclusive", "--", PY, "-c", "print('[b.step] ✓ PASS y'); print('validate: OK')"],
        env=env, capture_output=True, text=True)
    assert rerun.returncode == 0, rerun.stdout

    import json as _json
    rows = [_json.loads(line) for line in ledger.read_text().splitlines()]
    assert [row["verdict"] for row in rows] == ["pass", "fail", "pass"]
    assert rows[1]["failed_steps"] == ["b.step"] and rows[1]["kind"] == "gate"
    assert rows[2]["kind"] == "exclusive-rerun" and rows[2]["rerun_passed"] is True
    assert rows[0]["head_sha"] and rows[0]["finished_utc"].endswith("Z")
    assert all(row["queued_utc"].endswith("Z") for row in rows)
    assert all(row["queued_utc"] <= row["started_utc"] for row in rows)
    assert all(isinstance(row["wait_s"], float) and row["wait_s"] >= 0 for row in rows)

    stats = subprocess.run([PY, str(GATE), "stats"], env=env, capture_output=True, text=True)
    assert stats.returncode == 0, stats.stdout
    assert "first-run pass rate 50.0%" in stats.stdout
    assert "failing step b.step: 1" in stats.stdout
    assert "1/1 exclusive rerun(s) passed" in stats.stdout
    assert "queue wait over 3 run(s): median" in stats.stdout


def test_gate_children_do_not_inherit_the_host_readers_optin(tmp_path: Path) -> None:
    """#1170: gate children force the wrkslots readers opt-out.

    A parent may have opted the whole host into the readers soak by env and
    file; neither the preflight nor the admitted command may receive it.
    """

    preflight_env = tmp_path / "preflight.env"
    readers_file = tmp_path / "wrkslots-rust-readers"
    readers_file.write_text("enabled = true\n")
    env = {
        **_env(tmp_path),
        "WRKSLOTS_RUST_READERS": "1",
        "WRKSLOTS_RUST_READERS_FILE": str(readers_file),
        "GATE_PREFLIGHT_COMMAND": f"env > {preflight_env}",
    }
    child = (
        "import os; "
        "print('readers=' + os.environ.get('WRKSLOTS_RUST_READERS', '')); "
        "print('readers_file=' + os.environ.get('WRKSLOTS_RUST_READERS_FILE', '')); "
        "print('run_id=' + ('yes' if os.environ.get('GATE_ADMISSION_RUN_ID') else 'no'))"
    )
    result = subprocess.run(
        [PY, str(GATE), "--", PY, "-c", child],
        env=env, capture_output=True, text=True, timeout=30,
    )
    expected_file = str(tmp_path / "admission" / "no-wrkslots-rust-readers")
    assert result.returncode == 0, result.stdout
    assert "readers=0" in result.stdout.splitlines()
    assert f"readers_file={expected_file}" in result.stdout.splitlines()
    assert "run_id=yes" in result.stdout.splitlines()
    preflight_lines = preflight_env.read_text().splitlines()
    assert "WRKSLOTS_RUST_READERS=0" in preflight_lines
    assert f"WRKSLOTS_RUST_READERS_FILE={expected_file}" in preflight_lines


def test_shared_gates_run_up_to_the_cap_and_queue_the_rest(tmp_path: Path) -> None:
    sleepers = [_start(tmp_path, "--slots", "3", "--", PY, "-c", "import time; time.sleep(4)") for _ in range(3)]
    _wait_for_runs(tmp_path, 3)
    fourth = _start(tmp_path, "--", PY, "-c", "pass")
    out, _ = fourth.communicate(timeout=30)
    assert "queued at position 1" in out
    assert "admitted after" in out
    for proc in sleepers:
        proc.communicate(timeout=30)


def test_the_host_slots_file_caps_every_waiter_and_is_reread_while_queued(tmp_path: Path) -> None:
    """A host cap lowers a waiter's own --slots, and lifting it admits the
    queued waiter at once, without re-queueing, while the first still runs."""
    cap = tmp_path / "gate-slots.toml"
    cap.write_text("slots = 1\n")
    holder = _start(tmp_path, "--slots", "3", "--", PY, "-c", "import time; time.sleep(8)")
    _wait_for_runs(tmp_path, 1)
    second = _start(tmp_path, "--slots", "3", "--", PY, "-c", "pass")
    _wait_for_queue(tmp_path, 1)
    time.sleep(1.5)
    assert second.poll() is None  # held by the cap, not by its own --slots 3
    cap.unlink()
    out, _ = second.communicate(timeout=30)
    assert holder.poll() is None, "the second gate should start beside the first once the cap lifts"
    assert "queued at position 1" in out
    assert "admitted after" in out
    holder.communicate(timeout=30)


def test_a_malformed_host_slots_file_caps_at_one_and_says_why(tmp_path: Path) -> None:
    cap = tmp_path / "gate-slots.toml"
    for text in ("slots = 0\n", "slots = 'two'\n", "slots = 2\nextra = 1\n", "not toml = =\n"):
        cap.write_text(text)
        result = subprocess.run([PY, str(GATE), "status"], env={**_env(tmp_path), "GATE_SLOTS": "3"},
                                capture_output=True, text=True, timeout=30)
        assert result.returncode == 0, result.stdout
        assert "shared slots 1" in result.stdout, (text, result.stdout)
        assert "capping at 1" in result.stdout, (text, result.stdout)
    cap.write_text("slots = 2\n")
    result = subprocess.run([PY, str(GATE), "status"], env=_env(tmp_path),
                            capture_output=True, text=True, timeout=30)
    assert f"shared slots 2 (host cap 2 from {cap})" in result.stdout
    cap.unlink()
    result = subprocess.run([PY, str(GATE), "status"], env=_env(tmp_path),
                            capture_output=True, text=True, timeout=30)
    assert "shared slots 2 (no host cap;" in result.stdout


def _wait_for_queue(tmp_path: Path, count: int) -> None:
    queue = tmp_path / "admission" / "queue"
    deadline = time.time() + 10
    while time.time() < deadline:
        if queue.exists() and len(list(queue.glob("*.json"))) >= count:
            return
        time.sleep(0.05)
    raise AssertionError("gate ticket did not queue")


def test_an_exclusive_run_waits_for_an_empty_box_and_blocks_new_gates(tmp_path: Path) -> None:
    holder = _start(tmp_path, "--slots", "3", "--", PY, "-c", "import time; time.sleep(4)")
    _wait_for_runs(tmp_path, 1)
    exclusive = _start(tmp_path, "--exclusive", "--", PY, "-c", "pass")
    # Start the shared gate only once the exclusive ticket is durably queued
    # ahead of it; starting both at once races their enqueue order.
    _wait_for_queue(tmp_path, 1)
    shared = _start(tmp_path, "--", PY, "-c", "pass")
    shared_out, _ = shared.communicate(timeout=30)
    exclusive_out, _ = exclusive.communicate(timeout=30)
    holder.communicate(timeout=30)
    # The shared gate queued behind the exclusive ticket ahead of it.
    assert "queued at position" in shared_out
    assert "class=exclusive" in exclusive_out


def test_a_nested_exclusive_gate_releases_its_parent_and_waits_for_other_gates(
    tmp_path: Path,
) -> None:
    order = tmp_path / "order.txt"
    holder = _start(
        tmp_path, "--", PY, "-c",
        f"import time; time.sleep(3); open({str(order)!r}, 'a').write('holder\\n')",
    )
    _wait_for_runs(tmp_path, 1)
    nested_script = (
        "import subprocess, sys\n"
        f"done = subprocess.run([sys.executable, {str(GATE)!r}, '--exclusive', '--', "
        f"sys.executable, '-c', \"open({str(order)!r}, 'a').write('nested\\\\n')\"])\n"
        "sys.exit(done.returncode)\n"
    )
    outer = _start(tmp_path, "--", PY, "-c", nested_script)
    outer_out, _ = outer.communicate(timeout=60)
    holder.communicate(timeout=30)
    assert outer.returncode == 0
    # The nested exclusive phase ran alone: after the unrelated holder gate
    # finished, never beside it, and without deadlocking on its own parent.
    assert order.read_text().splitlines() == ["holder", "nested"]
    assert "class=exclusive" in outer_out


def test_status_reports_finished_runs(tmp_path: Path) -> None:
    done = _start(tmp_path, "--", PY, "-c", "pass")
    done.communicate(timeout=30)
    status = subprocess.run([PY, str(GATE), "status"], env=_env(tmp_path),
                            capture_output=True, text=True, timeout=30)
    assert "1 finished in 24h" in status.stdout


_BLACKOUT = """\
[[blackout]]
start = "00:15"
end = "04:00"
timezone = "America/New_York"
reason = "neighbour nightly job"
"""

_INSIDE = "2026-11-06T06:30:00+00:00"  # 01:30 EST, inside 00:15-04:00
_OUTSIDE = "2026-11-06T12:00:00+00:00"  # 07:00 EST, outside


def _blackout_env(tmp_path: Path, config_text: str | None, now: str) -> dict[str, str]:
    env = _env(tmp_path)
    env.pop("GATE_ADMISSION_RUN_ID", None)  # a fresh start, not a nested gate
    config = tmp_path / "gate.toml"
    if config_text is not None:
        config.write_text(config_text, encoding="utf-8")
    env["GATE_BLACKOUT_CONFIG"] = str(config)
    env["GATE_BLACKOUT_NOW"] = now
    return env


def _run_gate(tmp_path: Path, env: dict[str, str], *args: str) -> subprocess.CompletedProcess[str]:
    del tmp_path
    return subprocess.run(
        [PY, str(GATE), *args, "--", PY, "-c", "pass"],
        env=env, capture_output=True, text=True, timeout=30,
    )


def test_blackout_queues_a_start_inside_the_window_until_it_ends(tmp_path: Path) -> None:
    """A start requested inside a window waits it out in the queue rather than
    being refused, so the lane need not resubmit; nothing runs meanwhile."""
    marker = tmp_path / "ran"
    result = subprocess.run(
        [PY, str(GATE), "--wait-timeout", "2", "--", PY, "-c",
         f"open({str(marker)!r}, 'w').close()"],
        env=_blackout_env(tmp_path, _BLACKOUT, _INSIDE), capture_output=True, text=True, timeout=30,
    )
    assert result.returncode == 75, result.stdout  # gave up waiting: never admitted
    assert "waits out blackout 00:15-04:00 America/New_York" in result.stdout
    assert "starts resume 2026-11-06 04:00 EST" in result.stdout
    assert not marker.exists()


def test_no_wait_blackout_refuses_a_start_inside_the_window(tmp_path: Path) -> None:
    result = _run_gate(tmp_path, _blackout_env(tmp_path, _BLACKOUT, _INSIDE), "--no-wait-blackout")
    assert result.returncode == 78, result.stdout
    assert "00:15-04:00 America/New_York" in result.stdout
    assert "neighbour nightly job" in result.stdout
    assert "next allowed start 2026-11-06 04:00 EST" in result.stdout


def test_blackout_allows_a_start_outside_the_window(tmp_path: Path) -> None:
    result = _run_gate(tmp_path, _blackout_env(tmp_path, _BLACKOUT, _OUTSIDE))
    assert result.returncode == 0, result.stdout


def test_blackout_window_crossing_midnight(tmp_path: Path) -> None:
    config = _BLACKOUT.replace('start = "00:15"', 'start = "23:00"').replace(
        'end = "04:00"', 'end = "02:00"'
    )
    # 2026-11-06T04:30Z is 23:30 EST on Nov 5: inside a 23:00-02:00 window.
    result = _run_gate(tmp_path, _blackout_env(tmp_path, config, "2026-11-06T04:30:00+00:00"),
                       "--no-wait-blackout")
    assert result.returncode == 78, result.stdout
    assert "next allowed start 2026-11-06 02:00 EST" in result.stdout
    # 2026-11-06T08:00Z is 03:00 EST: outside it.
    result = _run_gate(tmp_path, _blackout_env(tmp_path, config, "2026-11-06T08:00:00+00:00"))
    assert result.returncode == 0, result.stdout


def test_ignore_blackout_starts_and_is_recorded_in_the_ledger(tmp_path: Path) -> None:
    env = _blackout_env(tmp_path, _BLACKOUT, _INSIDE)
    result = _run_gate(tmp_path, env, "--ignore-blackout")
    assert result.returncode == 0, result.stdout
    import json as _json

    rows = [_json.loads(line) for line in (tmp_path / "ledger.jsonl").read_text().splitlines()]
    assert rows[-1]["blackout_ignored"] is True
    plain = _run_gate(tmp_path, _blackout_env(tmp_path, _BLACKOUT, _OUTSIDE))
    assert plain.returncode == 0, plain.stdout
    rows = [_json.loads(line) for line in (tmp_path / "ledger.jsonl").read_text().splitlines()]
    assert rows[-1]["blackout_ignored"] is False


def test_no_blackout_config_means_no_blackout(tmp_path: Path) -> None:
    result = _run_gate(tmp_path, _blackout_env(tmp_path, None, _INSIDE))
    assert result.returncode == 0, result.stdout


def test_malformed_blackout_config_refuses_with_a_clear_error(tmp_path: Path) -> None:
    for text in (
        "this is not toml = =",
        '[[blackout]]\nstart = "25:00"\nend = "04:00"\ntimezone = "America/New_York"\n',
        '[[blackout]]\nstart = "00:15"\nend = "04:00"\ntimezone = "Nowhere/Special"\n',
        '[[blackout]]\nstart = "00:15"\nstart = "00:16"\nend = "04:00"\ntimezone = "America/New_York"\n',
    ):
        result = _run_gate(tmp_path, _blackout_env(tmp_path, text, _OUTSIDE))
        assert result.returncode == 2, (text, result.stdout)
        assert "gate blackout config" in result.stdout


def test_a_marked_phase_keeps_nested_blackout_behaviour(tmp_path: Path) -> None:
    """#1146 audit: the phase mark is not a rerun. A marked nested phase
    inside a blackout follows its parent admission's rule (it queues and
    waits the window out, even with --no-wait-blackout), while a top-level
    --rerun with --no-wait-blackout is refused. Its five selected steps go
    through the nested path, never the rerun step check."""
    env = _blackout_env(tmp_path, _BLACKOUT, _INSIDE)
    env["GATE_PHASE_PRIORITY"] = "1"
    runs = tmp_path / "admission" / "runs"
    runs.mkdir(parents=True)
    (runs / "parent-live.json").write_text(json.dumps(
        {"id": "parent-live", "pid": os.getpid(), "class": "shared", "queued_at": 0}))
    nested = subprocess.run(
        [PY, str(GATE), "--exclusive", "--no-wait-blackout", "--wait-timeout", "2",
         "--", PY, "-c", "pass", "--selected", "a.step,b.step,c.step,d.step,e.step"],
        env={**env, "GATE_ADMISSION_RUN_ID": "parent-live"},
        capture_output=True, text=True, timeout=30)
    assert nested.returncode == 75, nested.stdout  # waited out, not refused
    assert "waits out blackout" in nested.stdout
    rerun = subprocess.run(
        [PY, str(GATE), "--rerun", "--no-wait-blackout", "--", PY, "-c", "pass",
         "--selected", "a.step"],
        env=env, capture_output=True, text=True, timeout=30)
    assert rerun.returncode == 78, rerun.stdout


def test_blackout_help_documents_config_and_flag(tmp_path: Path) -> None:
    result = subprocess.run(
        [PY, str(GATE), "--help"], env=_env(tmp_path), capture_output=True, text=True, timeout=30
    )
    assert result.returncode == 0
    assert "~/.local/etc/agent-utils/gate.toml" in result.stdout
    assert "[[blackout]]" in result.stdout
    assert "--ignore-blackout" in result.stdout
    assert "--no-wait-blackout" in result.stdout
    assert "--rerun" in result.stdout


def _gate_scope_units() -> set[str]:
    """Live agent-utils-gate-*.scope user units, or empty where systemctl
    cannot list them (no user systemd on this host)."""
    try:
        done = subprocess.run(
            ["systemctl", "--user", "list-units", "agent-utils-gate-*.scope",
             "--all", "--no-legend"],
            capture_output=True, text=True, timeout=30)
    except OSError:
        return set()
    if done.returncode != 0:
        return set()
    return {line.split()[0] for line in done.stdout.splitlines() if line.split()}


def test_a_rerun_ticket_jumps_ordinary_queued_gates(tmp_path: Path) -> None:
    """A --rerun ticket sorts ahead of every ordinary queued ticket and is
    granted as soon as the running gate finishes."""
    scopes_before = _gate_scope_units()
    order = tmp_path / "order.txt"

    def note(tag: str) -> str:
        return f"open({str(order)!r}, 'a').write({tag!r} + '\\n')"

    gate = tmp_path / "release"
    running = _start(tmp_path, "--exclusive", "--", PY, "-c",
                     f"import os, time\nwhile not os.path.exists({str(gate)!r}): time.sleep(0.05)")
    _wait_for_runs(tmp_path, 1)
    queue = tmp_path / "admission" / "queue"

    def wait_queued(count: int) -> None:
        deadline = time.time() + 10
        while len(list(queue.glob("*.json"))) < count:
            assert time.time() < deadline, "tickets did not queue"
            time.sleep(0.05)

    ordinary = _start(tmp_path, "--", PY, "-c", note("ordinary"))
    wait_queued(1)
    rerun = _start(tmp_path, "--rerun", "--", PY, "-c", note("rerun"), "--selected", "a.step")
    wait_queued(2)
    gate.write_text("")
    for proc in (running, ordinary, rerun):
        out, _ = proc.communicate(timeout=30)
        assert proc.returncode == 0, out
    assert order.read_text().split() == ["rerun", "ordinary"]
    # No systemd scope this test created may survive it (they leaked on
    # 2026-11-10/11 when the test timed out under load). Scope teardown
    # can lag process exit, so poll briefly.
    deadline = time.time() + 10
    leaked = _gate_scope_units() - scopes_before
    while leaked and time.time() < deadline:
        time.sleep(0.25)
        leaked = _gate_scope_units() - scopes_before
    assert not leaked, f"test leaked gate scope(s): {sorted(leaked)}"


def test_cpu_quota_comes_from_the_slots_file_with_a_default(tmp_path: Path) -> None:
    """#1254: cpu_quota_cpus in the gate-slots file sets the per-gate cap;
    no setting means the 48-CPU default; an env override wins."""
    gate = _gate_module()
    slots = tmp_path / "gate-slots.toml"
    slots.write_text("slots = 2\ncpu_quota_cpus = 12\n")
    assert gate.host_cpu_quota_cpus(slots) == 12
    assert gate.host_slot_cap(slots) == (2, None)
    slots.write_text("slots = 3\n")
    assert gate.host_cpu_quota_cpus(slots) == gate.DEFAULT_CPU_QUOTA_CPUS == 48
    old = os.environ.get("GATE_CPU_QUOTA_CPUS")
    os.environ["GATE_CPU_QUOTA_CPUS"] = "7"
    try:
        assert gate.host_cpu_quota_cpus() == 7
        # An explicit file stays authoritative inside a contained gate,
        # whose child environment sets $GATE_CPU_QUOTA_CPUS.
        slots.write_text("slots = 2\ncpu_quota_cpus = 12\n")
        assert gate.host_cpu_quota_cpus(slots) == 12
    finally:
        if old is None:
            del os.environ["GATE_CPU_QUOTA_CPUS"]
        else:
            os.environ["GATE_CPU_QUOTA_CPUS"] = old


def test_capped_environment_lowers_schedulers_but_keeps_a_smaller_caller_value(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """#1254: VALIDATE_MAX_CPUS (dagrun -j) and GOMAXPROCS (esbuild) are
    capped at the quota; a smaller caller value is honoured."""
    gate = _gate_module()
    monkeypatch.delenv("VALIDATE_MAX_CPUS", raising=False)
    monkeypatch.delenv("GOMAXPROCS", raising=False)
    capped = gate._capped_environment(48)
    assert capped["VALIDATE_MAX_CPUS"] == "48" and capped["GOMAXPROCS"] == "48"
    monkeypatch.setenv("VALIDATE_MAX_CPUS", "8")
    monkeypatch.setenv("GOMAXPROCS", "200")
    capped = gate._capped_environment(48)
    assert capped["VALIDATE_MAX_CPUS"] == "8" and capped["GOMAXPROCS"] == "48"


def test_contained_command_wraps_in_a_quota_scope(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """#1254: the admitted command runs in a systemd user scope with
    CPUQuota = quota x 100%; the opt-out runs it bare."""
    gate = _gate_module()
    monkeypatch.delenv("GATE_CPU_CONTAINMENT", raising=False)
    wrapped, contained = gate._contained_command(["echo", "hi"], "1234-abcd", 48)
    assert contained and "CPUQuota=4800%" in wrapped and wrapped[-2:] == ["echo", "hi"]
    assert any("agent-utils-gate-1234-abcd" in word for word in wrapped)
    monkeypatch.setenv("GATE_CPU_CONTAINMENT", "off")
    bare, contained = gate._contained_command(["echo", "hi"], "1234-abcd", 48)
    assert not contained and bare == ["echo", "hi"]
def test_timing_sensitive_steps_come_from_the_dag_declaration() -> None:
    """#1260: only a gate-exclusive-labelled (or unknown) step is sensitive."""
    gate = _gate_module()
    labels = {
        "python.tick-hub.test": {"validation-cross"},
        "cross.agentctl.differential-0": ["component-agentctl", gate.TIMING_SENSITIVE_LABEL],
    }
    labels = {k: set(v) for k, v in labels.items()}
    assert gate._timing_sensitive_steps(["python.tick-hub.test"], labels) == []
    assert gate._timing_sensitive_steps(
        ["python.tick-hub.test", "cross.agentctl.differential-0"], labels
    ) == ["cross.agentctl.differential-0"]
    assert gate._timing_sensitive_steps(["no.such.step"], labels) == ["no.such.step"]
    real = gate._dag_step_labels()
    assert gate.TIMING_SENSITIVE_LABEL in real["cross.agentctl.differential-0"]
    assert gate._timing_sensitive_steps(["cross.agentctl.differential-0"], real)
    assert gate._timing_sensitive_steps(["python.tick-hub.test"], real) == []


def test_a_non_timing_rerun_runs_shared_and_still_records_a_rerun(tmp_path: Path) -> None:
    """#1260: a rerun of known non-timing steps is admitted shared, keeps
    the rerun ledger kind, and says which class it took."""
    env = _env(tmp_path)
    done = subprocess.run(
        [PY, str(GATE), "--rerun", "--", PY, "-c", "print('validate: OK')",
         "--selected", "python.tick-hub.test"],
        env=env, capture_output=True, text=True, timeout=60)
    assert done.returncode == 0, done.stdout
    assert "runs in the shared class" in done.stdout
    rows = [json.loads(line) for line in (tmp_path / "ledger.jsonl").read_text().splitlines()]
    assert rows[-1]["kind"] == "exclusive-rerun" and rows[-1]["rerun"] is True
def _fake_validate(tmp_path: Path) -> Path:
    fake = tmp_path / "validate.py"
    fake.write_text("print('validate: OK')\n")
    return fake


def _local_env(tmp_path: Path) -> dict[str, str]:
    return {**_env(tmp_path), "GATE_LOCAL_PREFLIGHT_DIR": str(tmp_path / "local-passes")}


def test_a_full_gate_without_a_local_pass_is_refused(tmp_path: Path) -> None:
    """#1262: a full gate with no fresh local pass for its tree never queues."""
    fake = _fake_validate(tmp_path)
    done = subprocess.run([PY, str(GATE), "--", PY, str(fake)],
                          env=_local_env(tmp_path), capture_output=True, text=True, timeout=60)
    assert done.returncode == 65, done.stdout
    assert "local preflight FAILED" in done.stdout
    rows = [json.loads(line) for line in (tmp_path / "ledger.jsonl").read_text().splitlines()]
    assert rows[-1]["kind"] == "preflight-refused"
    assert rows[-1]["local_preflight"] == "missing"


def test_a_full_gate_with_a_fresh_local_pass_or_the_escape_hatch_queues(tmp_path: Path) -> None:
    """#1262: a fresh same-tree record admits; --no-local-preflight admits
    and the ledger records its reason."""
    gate = _gate_module()
    fake = _fake_validate(tmp_path)
    env = _local_env(tmp_path)
    tree = gate._working_tree_hash(gate._repo_root())
    assert tree
    passes = tmp_path / "local-passes"
    passes.mkdir()
    (passes / f"{tree}.json").write_text(json.dumps(
        {"tree": tree, "finished_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}))
    done = subprocess.run([PY, str(GATE), "--", PY, str(fake)],
                          env=env, capture_output=True, text=True, timeout=60)
    assert done.returncode == 0, done.stdout
    assert "local preflight passed" in done.stdout
    rows = [json.loads(line) for line in (tmp_path / "ledger.jsonl").read_text().splitlines()]
    assert rows[-1]["local_preflight"] == "passed"

    skipped = subprocess.run(
        [PY, str(GATE), "--no-local-preflight", "docs-only change", "--", PY, str(fake)],
        env={**env, "GATE_LOCAL_PREFLIGHT_DIR": str(tmp_path / "empty-passes")},
        capture_output=True, text=True, timeout=60)
    assert skipped.returncode == 0, skipped.stdout
    rows = [json.loads(line) for line in (tmp_path / "ledger.jsonl").read_text().splitlines()]
    assert rows[-1]["local_preflight"] == "skipped: --no-local-preflight docs-only change"


def test_a_stale_local_pass_does_not_satisfy_the_gate(tmp_path: Path) -> None:
    """#1262: a record older than the freshness window does not admit."""
    gate = _gate_module()
    fake = _fake_validate(tmp_path)
    tree = gate._working_tree_hash(gate._repo_root())
    passes = tmp_path / "local-passes"
    passes.mkdir()
    stale = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(time.time() - 48 * 3600))
    (passes / f"{tree}.json").write_text(json.dumps({"tree": tree, "finished_utc": stale}))
    done = subprocess.run([PY, str(GATE), "--", PY, str(fake)],
                          env=_local_env(tmp_path), capture_output=True, text=True, timeout=60)
    assert done.returncode == 65, done.stdout


def test_a_rerun_must_name_a_few_selected_steps(tmp_path: Path) -> None:
    none = subprocess.run([PY, str(GATE), "--rerun", "--", PY, "-c", "pass"],
                          env=_env(tmp_path), capture_output=True, text=True, timeout=30)
    assert none.returncode == 2 and "--selected" in none.stdout
    many = subprocess.run([PY, str(GATE), "--rerun", "--", PY, "-c", "pass",
                           "--selected", "a,b,c,d,e"],
                          env=_env(tmp_path), capture_output=True, text=True, timeout=30)
    assert many.returncode == 2 and "at most 4" in many.stdout


def _start_env(tmp_path: Path, env: dict[str, str], *args: str) -> subprocess.Popen[str]:
    del tmp_path
    return subprocess.Popen(
        [PY, str(GATE), *args], env=env,
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)


def _phase_scenario(tmp_path: Path, env: dict[str, str]) -> tuple[list[str], dict[str, object]]:
    """A shared holder runs; an admitted shared gate starts its nested
    exclusive phase, which queues behind the holder; then an ordinary shared
    gate and a later rerun queue behind it. Returns the run order and the
    phase ticket."""
    order = tmp_path / "order.txt"
    release = tmp_path / "release"
    holder = _start_env(
        tmp_path, env, "--", PY, "-c",
        f"import os, time\nwhile not os.path.exists({str(release)!r}): time.sleep(0.05)")
    _wait_for_runs(tmp_path, 1)
    nested_script = (
        "import subprocess, sys\n"
        f"done = subprocess.run([sys.executable, {str(GATE)!r}, '--exclusive', '--', "
        f"sys.executable, '-c', \"open({str(order)!r}, 'a').write('phase\\\\n')\", "
        "'--selected', 'cross.a,cross.b,cross.c,cross.d,cross.e'])\n"
        "sys.exit(done.returncode)\n"
    )
    outer = _start_env(tmp_path, env, "--", PY, "-c", nested_script)
    queue = tmp_path / "admission" / "queue"
    deadline = time.time() + 20
    ticket: dict[str, object] = {}
    while not ticket and time.time() < deadline:
        for path in queue.glob("*.json"):
            try:
                data = json.loads(path.read_text())
            except (OSError, ValueError):
                continue
            if data.get("class") == "exclusive":
                ticket = data
        time.sleep(0.05)
    assert ticket, "the nested phase did not queue"

    def note(tag: str) -> str:
        return f"open({str(order)!r}, 'a').write({tag!r} + '\\n')"

    ordinary = _start_env(tmp_path, env, "--", PY, "-c", note("ordinary"))
    _wait_for_queue(tmp_path, 2)
    rerun = _start_env(tmp_path, env, "--rerun", "--", PY, "-c", note("rerun"),
                       "--selected", "a.step")
    _wait_for_queue(tmp_path, 3)
    release.write_text("")
    for proc in (holder, outer, ordinary, rerun):
        out, _ = proc.communicate(timeout=60)
        assert proc.returncode == 0, out
    return order.read_text().split(), ticket


def test_a_marked_nested_phase_is_not_overtaken_by_a_later_rerun(tmp_path: Path) -> None:
    """#1146: with the phase mark on (env hook), a nested exclusive phase
    keeps the rerun tier it queued in: a later rerun cannot pass it, but
    still passes an ordinary shared gate. The phase's five `--selected`
    steps are not refused as an over-long rerun, its preflight record stays
    'nested gate', and the ledger keeps kind 'exclusive-rerun' while naming
    the phase and the switch source."""
    env = {**_env(tmp_path), "GATE_PHASE_PRIORITY": "1"}
    order, ticket = _phase_scenario(tmp_path, env)
    assert order == ["phase", "rerun", "ordinary"]
    assert ticket["rerun"] is True and ticket["nested_phase"] is True
    assert str(ticket["phase_priority"]).startswith("env")
    [row] = [row for row in _ledger_rows(tmp_path) if row.get("nested_phase") is True]
    assert row["kind"] == "exclusive-rerun" and row["rerun"] is False
    assert row["preflight"] == "skipped: nested gate"
    assert str(row["phase_priority"]).startswith("env")
    history = [json.loads(line) for line in
               (tmp_path / "admission" / "history.jsonl").read_text().splitlines()]
    [hrow] = [row for row in history if row.get("nested_phase") is True]
    assert hrow["rerun"] is False and str(hrow["phase_priority"]).startswith("env")


def test_a_nested_phase_is_unmarked_by_default(tmp_path: Path) -> None:
    """Switch off is today's behaviour: the later rerun jumps the phase."""
    order, ticket = _phase_scenario(tmp_path, _env(tmp_path))
    assert order == ["rerun", "phase", "ordinary"]
    assert ticket["rerun"] is False and ticket["nested_phase"] is True
    assert ticket["phase_priority"] is None


def test_a_marked_nested_phase_from_the_host_file(tmp_path: Path) -> None:
    (tmp_path / "gate-phase-priority.toml").write_text("phase_priority = true\n")
    order, ticket = _phase_scenario(tmp_path, _env(tmp_path))
    assert order == ["phase", "rerun", "ordinary"]
    assert str(ticket["phase_priority"]).startswith("host file")
    [row] = [row for row in _ledger_rows(tmp_path) if row.get("nested_phase") is True]
    assert str(row["phase_priority"]).startswith("host file")


def test_phase_priority_switch_sources(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    gate = _gate_module()
    config = tmp_path / "gate-phase-priority.toml"
    monkeypatch.setenv("GATE_PHASE_PRIORITY_CONFIG", str(config))
    monkeypatch.delenv("GATE_PHASE_PRIORITY", raising=False)
    enabled, source = gate.phase_priority()
    assert enabled is False and "default off" in source
    config.write_text("phase_priority = true\n")
    enabled, source = gate.phase_priority()
    assert enabled is True and source.startswith("host file")
    config.write_text("phase_priority = 'yes'\n")
    enabled, source = gate.phase_priority()
    assert enabled is False and "off" in source
    monkeypatch.setenv("GATE_PHASE_PRIORITY", "0")
    enabled, source = gate.phase_priority()
    assert enabled is False and source.startswith("env")
    config.write_text("phase_priority = true\n")
    monkeypatch.setenv("GATE_PHASE_PRIORITY", "1")
    enabled, source = gate.phase_priority()
    assert enabled is True and source.startswith("env")


def test_marked_tickets_keep_the_legacy_sort_for_old_clients(tmp_path: Path) -> None:
    """An old client reads only `rerun`, class and filename order: the new
    fields must not change the order it computes, and it makes no max-steps
    or blackout judgement of another lane's ticket."""
    gate = _gate_module()
    adm = gate.Admission(tmp_path / "admission", 2)
    queue = tmp_path / "admission" / "queue"
    rows = [
        ("050-shared", "shared", False, {}),
        ("100-phase", "exclusive", True,
         {"nested_phase": True, "phase_priority": "env GATE_PHASE_PRIORITY=1"}),
        ("150-ordinary", "exclusive", False, {}),
        ("200-rerun", "exclusive", True, {}),
    ]
    for run_id, cls, rerun, extra in rows:
        (queue / f"{run_id}.json").write_text(json.dumps(
            {"id": run_id, "pid": os.getpid(), "class": cls, "queued_at": 0,
             "command": [], "rerun": rerun, **extra}))
    assert [str(data["id"]) for _, data in adm.tickets()] == [
        "100-phase", "200-rerun", "050-shared", "150-ordinary"]


def test_the_phase_mark_requires_a_live_parent(tmp_path: Path) -> None:
    """A stale parent runs/ file left behind (dead pid) plus the leaked env
    var is not a nested phase: no mark, even with the switch on."""
    env = {**_env(tmp_path), "GATE_PHASE_PRIORITY": "1"}
    holder = _start_env(tmp_path, env, "--exclusive", "--", PY, "-c",
                        "import time; time.sleep(6)")
    _wait_for_runs(tmp_path, 1)
    runs = tmp_path / "admission" / "runs"
    (runs / "stale-parent.json").write_text(json.dumps(
        {"id": "stale-parent", "pid": 99999999, "class": "shared", "queued_at": 0}))
    stale = _start_env(tmp_path, {**env, "GATE_ADMISSION_RUN_ID": "stale-parent"},
                       "--exclusive", "--", PY, "-c", "pass")
    _wait_for_queue(tmp_path, 1)
    [ticket_path] = list((tmp_path / "admission" / "queue").glob("*.json"))
    ticket = json.loads(ticket_path.read_text())
    assert ticket["rerun"] is False and ticket["nested_phase"] is False
    assert ticket["phase_priority"] is None
    out, _ = stale.communicate(timeout=30)
    assert stale.returncode == 0, out
    holder.communicate(timeout=30)


def test_a_leaked_run_id_alone_gives_no_phase_mark(tmp_path: Path) -> None:
    """GATE_ADMISSION_RUN_ID leaked into an unrelated child, with no runs/
    file behind it at all, is not a nested phase: no mark, switch on."""
    env = {**_env(tmp_path), "GATE_PHASE_PRIORITY": "1"}
    holder = _start_env(tmp_path, env, "--exclusive", "--", PY, "-c",
                        "import time; time.sleep(6)")
    _wait_for_runs(tmp_path, 1)
    leaked = _start_env(tmp_path, {**env, "GATE_ADMISSION_RUN_ID": "no-such-run"},
                        "--exclusive", "--", PY, "-c", "pass")
    _wait_for_queue(tmp_path, 1)
    [ticket_path] = list((tmp_path / "admission" / "queue").glob("*.json"))
    ticket = json.loads(ticket_path.read_text())
    assert ticket["rerun"] is False and ticket["nested_phase"] is False
    out, _ = leaked.communicate(timeout=30)
    assert leaked.returncode == 0, out
    assert "rerun tier" not in out
    holder.communicate(timeout=30)


def test_a_malformed_switch_file_is_off_and_never_blocks(tmp_path: Path) -> None:
    """A malformed host file means off plus a warning; the phase still runs."""
    (tmp_path / "gate-phase-priority.toml").write_text("phase_priority = [\n")
    env = _env(tmp_path)
    runs = tmp_path / "admission" / "runs"
    runs.mkdir(parents=True)
    (runs / "parent-live.json").write_text(json.dumps(
        {"id": "parent-live", "pid": os.getpid(), "class": "shared", "queued_at": 0}))
    result = subprocess.run(
        [PY, str(GATE), "--exclusive", "--", PY, "-c", "pass"],
        env={**env, "GATE_ADMISSION_RUN_ID": "parent-live"},
        capture_output=True, text=True, timeout=30)
    assert result.returncode == 0, result.stdout
    assert "gate: warning: phase priority off" in result.stdout
    assert "is not TOML" in result.stdout
    [row] = _ledger_rows(tmp_path)
    assert row["nested_phase"] is True and row["phase_priority"] is None
    explained = subprocess.run([PY, str(GATE), "queue", "--explain"], env=env,
                               capture_output=True, text=True, timeout=30)
    assert explained.returncode == 0 and "phase priority off" in explained.stdout
    assert "is not TOML" in explained.stdout


def _write_ticket(root: Path, folder: str, run_id: str, data: dict[str, object]) -> None:
    folder_path = root / folder
    folder_path.mkdir(parents=True, exist_ok=True)
    (folder_path / f"{run_id}.json").write_text(json.dumps(data))


def test_queue_explain_names_runs_and_tickets_ahead(tmp_path: Path) -> None:
    """#1146: a legacy (unmarked) phase ticket's starvation is visible: the
    running run it waits for and the later rerun queued ahead of it."""
    now = time.time()
    root = tmp_path / "admission"
    _write_ticket(root, "runs", "000-run", {
        "id": "000-run", "pid": os.getpid(), "class": "shared",
        "queued_at": now - 600, "started_at": now - 540,
        "command": [PY, "scripts/validate.py", "--all"]})
    _write_ticket(root, "queue", "100-phase", {
        "id": "100-phase", "pid": os.getpid(), "class": "exclusive",
        "queued_at": now - 3600, "rerun": False,
        "command": [PY, "common/bin/dagrun", "run", "--max-steps", "1",
                    "--selected", "cross.a,cross.b"]})
    _write_ticket(root, "queue", "200-rerun", {
        "id": "200-rerun", "pid": os.getpid(), "class": "exclusive",
        "queued_at": now - 60, "rerun": True,
        "command": [PY, "-c", "pass", "--selected", "a.step"]})
    # A ticket from before the rerun field existed.
    _write_ticket(root, "queue", "300-shared", {
        "id": "300-shared", "pid": os.getpid(), "class": "shared",
        "queued_at": now - 30, "command": [PY, "scripts/validate.py", "--all"]})
    plain = subprocess.run([PY, str(GATE), "queue"], env=_env(tmp_path),
                           capture_output=True, text=True, timeout=30)
    assert plain.returncode == 0, plain.stdout
    positions = [plain.stdout.index(run_id) for run_id in ("200-rerun", "100-phase", "300-shared")]
    assert positions == sorted(positions), plain.stdout
    explained = subprocess.run([PY, str(GATE), "queue", "--explain"], env=_env(tmp_path),
                               capture_output=True, text=True, timeout=30)
    assert explained.returncode == 0, explained.stdout
    assert "phase priority off" in explained.stdout
    assert "running 000-run" in explained.stdout
    assert "inferred gate-exclusive phase" in explained.stdout
    assert "blocked by running 000-run" in explained.stdout
    assert "blocked by ticket 200-rerun ahead (rerun tier)" in explained.stdout
    assert "starvation: 1 rerun ticket(s) queued after this ticket are ahead of it" in explained.stdout
    assert "blocked by ticket 100-phase ahead (FIFO)" in explained.stdout


def test_queue_explain_says_admissible_when_nothing_blocks(tmp_path: Path) -> None:
    root = tmp_path / "admission"
    _write_ticket(root, "queue", "100-shared", {
        "id": "100-shared", "pid": os.getpid(), "class": "shared",
        "queued_at": time.time(), "rerun": False, "command": [PY, "-c", "pass"]})
    explained = subprocess.run([PY, str(GATE), "queue", "--explain"], env=_env(tmp_path),
                               capture_output=True, text=True, timeout=30)
    assert explained.returncode == 0, explained.stdout
    assert "admissible now" in explained.stdout


def test_an_owner_priority_ticket_jumps_reruns_and_ordinary_gates(tmp_path: Path) -> None:
    """#1180: an owner-priority ticket queued last is admitted ahead of a
    rerun and an ordinary ticket alike, without preempting the running
    gate; the ticket, history and ledger all carry the reason."""
    order = tmp_path / "order.txt"

    def note(tag: str) -> str:
        return f"open({str(order)!r}, 'a').write({tag!r} + '\\n')"

    gate = tmp_path / "release"
    running = _start(tmp_path, "--exclusive", "--", PY, "-c",
                     f"import os, time\nwhile not os.path.exists({str(gate)!r}): time.sleep(0.05)")
    _wait_for_runs(tmp_path, 1)
    ordinary = _start(tmp_path, "--", PY, "-c", note("ordinary"))
    _wait_for_queue(tmp_path, 1)
    rerun = _start(tmp_path, "--rerun", "--", PY, "-c", note("rerun"), "--selected", "a.step")
    _wait_for_queue(tmp_path, 2)
    env = {**_env(tmp_path), "GATE_PRIORITY": "owner",
           "GATE_PRIORITY_REASON": "#286 owner UI fast-track"}
    owner = _start_env(tmp_path, env, "--", PY, "-c", note("owner"))
    _wait_for_queue(tmp_path, 3)
    [ticket] = [json.loads(path.read_text())
                for path in (tmp_path / "admission" / "queue").glob("*.json")
                if json.loads(path.read_text()).get("priority") == "owner"]
    assert ticket["priority_reason"] == "#286 owner UI fast-track"
    assert ticket["rerun"] is False and ticket["class"] == "shared"
    gate.write_text("")
    for proc in (running, ordinary, rerun, owner):
        out, _ = proc.communicate(timeout=60)
        assert proc.returncode == 0, out
    assert order.read_text().split() == ["owner", "rerun", "ordinary"]
    [row] = [row for row in _ledger_rows(tmp_path) if row.get("priority") == "owner"]
    assert row["priority_reason"] == "#286 owner UI fast-track"
    history = [json.loads(line)
               for line in (tmp_path / "admission" / "history.jsonl").read_text().splitlines()]
    [hrow] = [row for row in history if row.get("priority") == "owner"]
    assert hrow["priority_reason"] == "#286 owner UI fast-track"


def test_owner_tickets_keep_arrival_order_and_a_normal_waiter_cannot_pass_them(
    tmp_path: Path,
) -> None:
    """FIFO within the owner tier: with one free slot the earlier owner
    ticket wins, and a normal shared waiter queued before both still
    waits behind them (no poll race past a queued owner ticket)."""
    order = tmp_path / "order.txt"

    def note(tag: str) -> str:
        return f"open({str(order)!r}, 'a').write({tag!r} + '\\n')"

    gate = tmp_path / "release"
    running = _start(tmp_path, "--exclusive", "--", PY, "-c",
                     f"import os, time\nwhile not os.path.exists({str(gate)!r}): time.sleep(0.05)")
    _wait_for_runs(tmp_path, 1)
    ordinary = _start(tmp_path, "--slots", "1", "--", PY, "-c", note("ordinary"))
    _wait_for_queue(tmp_path, 1)
    first = _start(tmp_path, "--slots", "1", "--priority", "owner",
                   "--priority-reason", "#286 first", "--",
                   PY, "-c", note("owner1") + "; import time; time.sleep(2)")
    _wait_for_queue(tmp_path, 2)
    second = _start(tmp_path, "--slots", "1", "--priority", "owner",
                    "--priority-reason", "#286 second", "--", PY, "-c", note("owner2"))
    _wait_for_queue(tmp_path, 3)
    gate.write_text("")
    for proc in (running, ordinary, first, second):
        out, _ = proc.communicate(timeout=60)
        assert proc.returncode == 0, out
    assert order.read_text().split() == ["owner1", "owner2", "ordinary"]


def test_shared_tickets_are_granted_in_queue_order_when_slots_free(tmp_path: Path) -> None:
    """#1180 anomaly regression: two shared waiters behind a barrier must
    be granted in queue order, not in 1s-poll race order (2026-11-09
    8:54 PM ET: a shared ticket queued hours later won the race)."""
    order = tmp_path / "order.txt"

    def note(tag: str) -> str:
        return f"open({str(order)!r}, 'a').write({tag!r} + '\\n')"

    gate = tmp_path / "release"
    running = _start(tmp_path, "--exclusive", "--", PY, "-c",
                     f"import os, time\nwhile not os.path.exists({str(gate)!r}): time.sleep(0.05)")
    _wait_for_runs(tmp_path, 1)
    first = _start(tmp_path, "--slots", "1", "--",
                   PY, "-c", note("first") + "; import time; time.sleep(2)")
    _wait_for_queue(tmp_path, 1)
    second = _start(tmp_path, "--slots", "1", "--", PY, "-c", note("second"))
    _wait_for_queue(tmp_path, 2)
    gate.write_text("")
    for proc in (running, first, second):
        out, _ = proc.communicate(timeout=60)
        assert proc.returncode == 0, out
    assert order.read_text().split() == ["first", "second"]


def test_owner_priority_requires_a_reason_and_a_known_value(tmp_path: Path) -> None:
    no_reason = subprocess.run([PY, str(GATE), "--priority", "owner", "--", PY, "-c", "pass"],
                               env=_env(tmp_path), capture_output=True, text=True, timeout=30)
    assert no_reason.returncode == 2 and "reason" in no_reason.stdout
    bad_env = subprocess.run([PY, str(GATE), "--", PY, "-c", "pass"],
                             env={**_env(tmp_path), "GATE_PRIORITY": "urgent"},
                             capture_output=True, text=True, timeout=30)
    assert bad_env.returncode == 2 and "GATE_PRIORITY" in bad_env.stdout
    child = ("import os; print('priority=' + os.environ.get('GATE_PRIORITY', '<unset>')); "
             "print('reason=' + os.environ.get('GATE_PRIORITY_REASON', '<unset>'))")
    stripped = subprocess.run(
        [PY, str(GATE), "--priority", "owner", "--priority-reason", "#286", "--",
         PY, "-c", child],
        env={**_env(tmp_path), "GATE_PRIORITY": "owner", "GATE_PRIORITY_REASON": "#286"},
        capture_output=True, text=True, timeout=30)
    assert stripped.returncode == 0, stripped.stdout
    assert "priority=<unset>" in stripped.stdout.splitlines()
    assert "reason=<unset>" in stripped.stdout.splitlines()
    help_text = subprocess.run([PY, str(GATE), "--help"], env=_env(tmp_path),
                               capture_output=True, text=True, timeout=30)
    assert "--priority" in help_text.stdout and "GATE_PRIORITY" in help_text.stdout


def test_legacy_clients_see_an_owner_ticket_as_an_ordinary_one(tmp_path: Path) -> None:
    """Mixed versions: a ticket written before the priority fields (and an
    owner ticket read by such a client) changes nothing for the legacy
    sort (`rerun`, then filename), while the new sort puts owner first."""
    gate = _gate_module()
    adm = gate.Admission(tmp_path / "admission", 2)
    queue = tmp_path / "admission" / "queue"
    rows = [
        ("050-ordinary", "shared", False, {}),
        ("100-owner", "shared", False,
         {"priority": "owner", "priority_reason": "#286"}),
        ("150-legacy", "shared", None, {}),  # no rerun field at all
        ("200-rerun", "exclusive", True, {}),
    ]
    for run_id, cls, rerun, extra in rows:
        data: dict[str, object] = {"id": run_id, "pid": os.getpid(), "class": cls,
                                   "queued_at": 0, "command": [], **extra}
        if rerun is not None:
            data["rerun"] = rerun
        (queue / f"{run_id}.json").write_text(json.dumps(data))
    tickets = adm.tickets()
    assert [str(data["id"]) for _, data in tickets] == [
        "100-owner", "200-rerun", "050-ordinary", "150-legacy"]
    # The deployed legacy algorithm on the same files: filename order,
    # stable by the rerun field alone.
    legacy = sorted(tickets, key=lambda item: item[0].name)
    legacy.sort(key=lambda item: 0 if item[1].get("rerun") else 1)
    assert [str(data["id"]) for _, data in legacy] == [
        "200-rerun", "050-ordinary", "100-owner", "150-legacy"]


DISABLED_PHASE_COMMAND = [
    "dagrun", "run", "--max-steps", "1", "--selected",
    "cross.agentctl.differential-0,cross.herdr-run.differential",
]


def _phase_ticket(root: Path, run_id: str, command: list[str], **extra: object) -> None:
    _write_ticket(root, "queue", run_id, {
        "id": run_id, "pid": os.getpid(), "class": "exclusive",
        "queued_at": 0, "command": command, "rerun": True,
        "nested_phase": True, "phase_priority": "env GATE_PHASE_PRIORITY=1",
        **extra})


def test_stale_disabled_cross_phases_are_reaped_not_run(tmp_path: Path) -> None:
    """Owner reply 346 / #1249: a queued, unstarted exclusive phase over
    only the five disabled cross differentials is dropped by reaping, so
    it never runs and never stands ahead of an owner ticket. A phase over
    other steps, an explicit --rerun, and a marked --cross-differentials
    opt-in phase all survive."""
    gate = _gate_module()
    root = tmp_path / "admission"
    adm = gate.Admission(root, 2)
    _phase_ticket(root, "100-stale", DISABLED_PHASE_COMMAND)
    _phase_ticket(root, "200-other", ["dagrun", "--selected", "cross.tick-hub.differential"])
    _phase_ticket(root, "300-optin", DISABLED_PHASE_COMMAND,
                  cross_differentials_opt_in=True)
    _write_ticket(root, "queue", "400-rerun", {
        "id": "400-rerun", "pid": os.getpid(), "class": "exclusive",
        "queued_at": 0, "command": DISABLED_PHASE_COMMAND, "rerun": True,
        "nested_phase": False})
    adm._reap()
    remaining = sorted(path.name for path in (root / "queue").glob("*.json"))
    assert remaining == ["200-other.json", "300-optin.json", "400-rerun.json"]
    history = [json.loads(line) for line in (root / "history.jsonl").read_text().splitlines()]
    assert any(row.get("kind") == "stale-disabled-phase" and row.get("id") == "100-stale"
               for row in history)


def test_an_owner_ticket_outranks_an_unstarted_marked_phase(tmp_path: Path) -> None:
    """Owner tier (owner reply 344) beats a phase-priority-marked exclusive
    phase that has not started, even when the phase queued first."""
    gate = _gate_module()
    root = tmp_path / "admission"
    adm = gate.Admission(root, 2)
    _phase_ticket(root, "100-phase", ["dagrun", "--selected", "cross.tick-hub.differential"])
    _write_ticket(root, "queue", "200-owner", {
        "id": "200-owner", "pid": os.getpid(), "class": "shared",
        "queued_at": 0, "command": [], "rerun": False,
        "priority": "owner", "priority_reason": "owner reply 344"})
    tickets = adm.tickets()
    assert [str(data["id"]) for _, data in tickets] == ["200-owner", "100-phase"]
    assert adm.grant_blockers("200-owner", "shared", tickets, []) == []
    blockers = adm.grant_blockers("100-phase", "exclusive", tickets, [])
    assert [str(data["id"]) for _, data, _ in blockers] == ["200-owner"]


def test_a_fresh_disabled_phase_is_not_queued_without_opt_in(tmp_path: Path) -> None:
    """A nested gate over only disabled differentials, without the
    --cross-differentials marker validate.py sets, exits 0 without
    queueing or running its command."""
    runs = tmp_path / "admission" / "runs"
    runs.mkdir(parents=True)
    (runs / "parent-live.json").write_text(json.dumps(
        {"id": "parent-live", "pid": os.getpid(), "class": "shared", "queued_at": 0}))
    marker = tmp_path / "ran.txt"
    result = subprocess.run(
        [PY, str(GATE), "--exclusive", "--", PY, "-c",
         f"open({str(marker)!r}, 'w').write('ran')",
         "--selected", "cross.agentctl.differential-1"],
        env={**_env(tmp_path), "GATE_ADMISSION_RUN_ID": "parent-live"},
        capture_output=True, text=True, timeout=30)
    assert result.returncode == 0, result.stdout
    assert "disabled" in result.stdout
    assert not marker.exists()
    assert not list((tmp_path / "admission" / "queue").glob("*.json"))


def test_concurrent_admission_mixed_versions_never_exceeds_the_cap(tmp_path: Path) -> None:
    """Mixed versions: one Admission ordering by the new tiers and one by
    the legacy rerun-only sort grant against the same state dir; the flock
    around check-and-write means runs/ never passes the cap, and exactly
    the cap's worth of grants succeed."""
    gate = _gate_module()
    root = tmp_path / "admission"
    # One Admission per thread below: real waiters are separate
    # processes, each with its own lock fd; threads sharing one fd would
    # share its flock. This first one just creates the state dirs.
    gate.Admission(root, 2)

    def legacy_tickets() -> list[tuple[Path, dict[str, object]]]:
        out = []
        for path in sorted((root / "queue").glob("*.json")):
            data = gate.Admission._read(path)
            if data is not None:
                out.append((path, data))
        out.sort(key=lambda item: 0 if item[1].get("rerun") else 1)
        return out

    def legacy_blockers_for(client: object):  # type: ignore[no-untyped-def]
        def legacy_blockers(run_id: str, cls: str,
                            tickets: list[tuple[Path, dict[str, object]]],
                            runs: list[dict[str, object]]) -> list[tuple[str, dict[str, object], str]]:
            # The deployed pre-#1180 rules, verbatim in effect: shared
            # waiters ignore shared tickets ahead and know no priority.
            ids = [str(data["id"]) for _, data in tickets]
            if run_id not in ids:
                return []
            ahead = tickets[: ids.index(run_id)]
            if cls == "exclusive":
                return ([("ticket", data, "ahead") for _, data in ahead]
                        + [("run", run, "running") for run in runs])
            blockers: list[tuple[str, dict[str, object], str]] = [
                ("ticket", data, "ahead") for _, data in ahead if data.get("class") == "exclusive"]
            blockers += [("run", run, "running") for run in runs if run.get("class") == "exclusive"]
            if not blockers and len(runs) >= client.effective_slots():  # type: ignore[attr-defined]
                blockers += [("run", run, "slots-full") for run in runs]
            return blockers

        return legacy_blockers

    def old_client() -> object:
        client = gate.Admission(root, 2)
        client.tickets = legacy_tickets
        client.grant_blockers = legacy_blockers_for(client)
        return client
    for index in range(6):
        extra: dict[str, object] = (
            {"priority": "owner", "priority_reason": "#286"} if index == 5 else {})
        _write_ticket(root, "queue", f"{index:03d}-t{index}", {
            "id": f"{index:03d}-t{index}", "pid": os.getpid(), "class": "shared",
            "queued_at": index, "command": [], "rerun": False, **extra})
    import threading
    granted: list[str] = []
    lock = threading.Lock()

    def attempt(client: object, run_id: str) -> None:
        if client.try_grant(run_id, "shared"):  # type: ignore[attr-defined]
            with lock:
                granted.append(run_id)

    clients = [gate.Admission(root, 2) if i % 2 == 0 else old_client() for i in range(6)]
    threads = [threading.Thread(target=attempt, args=(client, f"{i:03d}-t{i}"))
               for i, client in enumerate(clients)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(timeout=30)
    assert len(granted) == 2
    assert len(list((root / "runs").glob("*.json"))) == 2


def test_status_and_queue_name_the_owner_priority_and_reason(tmp_path: Path) -> None:
    now = time.time()
    root = tmp_path / "admission"
    _write_ticket(root, "queue", "100-ordinary", {
        "id": "100-ordinary", "pid": os.getpid(), "class": "shared",
        "queued_at": now - 600, "rerun": False, "command": [PY, "-c", "pass"]})
    _write_ticket(root, "queue", "200-owner", {
        "id": "200-owner", "pid": os.getpid(), "class": "shared",
        "queued_at": now - 60, "rerun": False,
        "priority": "owner", "priority_reason": "#286 owner UI fast-track",
        "command": [PY, "-c", "pass"]})
    status = subprocess.run([PY, str(GATE), "status"], env=_env(tmp_path),
                            capture_output=True, text=True, timeout=30)
    assert status.returncode == 0, status.stdout
    assert "owner" in status.stdout and "#286 owner UI fast-track" in status.stdout
    explained = subprocess.run([PY, str(GATE), "queue", "--explain"], env=_env(tmp_path),
                               capture_output=True, text=True, timeout=30)
    assert explained.returncode == 0, explained.stdout
    assert explained.stdout.index("200-owner") < explained.stdout.index("100-ordinary")
    assert "owner priority (#286 owner UI fast-track)" in explained.stdout
    assert "blocked by ticket 200-owner ahead (owner tier)" in explained.stdout


FAILING_PREFLIGHT = (
    "echo '[x.check] ✗ FAIL   content check'; "
    "echo '[x.check] E  CheckError: public docstring model.py:1 is not standalone'; "
    "echo '[y.check] ✓ PASS   other check'; exit 1"
)


def _ledger_rows(tmp_path: Path) -> list[dict[str, object]]:
    return [json.loads(line) for line in (tmp_path / "ledger.jsonl").read_text().splitlines()]


def test_a_failing_preflight_refuses_to_queue_and_shows_the_failure(tmp_path: Path) -> None:
    """A deterministic content failure is reported before the run takes a
    ticket: the command never runs, nothing is queued, and the ledger records
    the refusal apart from gates."""
    ran = tmp_path / "ran"
    env = {**_env(tmp_path), "GATE_PREFLIGHT_COMMAND": FAILING_PREFLIGHT}
    done = subprocess.run([PY, str(GATE), "--", PY, "-c", f"open({str(ran)!r}, 'w')"],
                          env=env, capture_output=True, text=True, timeout=30)
    assert done.returncode == 65, done.stdout
    assert "preflight FAILED" in done.stdout and "not queued" in done.stdout
    assert "[x.check] E  CheckError: public docstring model.py:1" in done.stdout
    assert "[y.check]" not in done.stdout, "only the failing steps' lines are shown"
    assert not ran.exists()
    assert not list((tmp_path / "admission" / "queue").glob("*.json"))
    assert not list((tmp_path / "admission" / "runs").glob("*.json"))
    [row] = _ledger_rows(tmp_path)
    assert row["kind"] == "preflight-refused" and row["preflight"] == "failed"
    assert row["failed_steps"] == ["x.check"] and row["exit_code"] == 65
    stats = subprocess.run([PY, str(GATE), "stats"], env=env, capture_output=True, text=True)
    assert "0 first-run gate(s)" in stats.stdout, stats.stdout
    assert "preflight refused 1 run(s) before they queued" in stats.stdout


def test_a_passing_preflight_prints_one_line_and_is_recorded(tmp_path: Path) -> None:
    done = subprocess.run([PY, str(GATE), "--", PY, "-c", "print('validate: OK')"],
                          env=_env(tmp_path), capture_output=True, text=True, timeout=30)
    assert done.returncode == 0, done.stdout
    [line] = [line for line in done.stdout.splitlines() if "preflight" in line]
    assert line.startswith("gate: preflight passed in "), done.stdout
    [row] = _ledger_rows(tmp_path)
    assert row["kind"] == "gate" and row["preflight"] == "passed"


def test_skip_preflight_queues_anyway_and_is_recorded(tmp_path: Path) -> None:
    env = {**_env(tmp_path), "GATE_PREFLIGHT_COMMAND": FAILING_PREFLIGHT}
    done = subprocess.run([PY, str(GATE), "--skip-preflight", "--", PY, "-c", "pass"],
                          env=env, capture_output=True, text=True, timeout=30)
    assert done.returncode == 0, done.stdout
    assert "preflight" not in done.stdout
    [row] = _ledger_rows(tmp_path)
    assert row["preflight"] == "skipped: --skip-preflight"
    stats = subprocess.run([PY, str(GATE), "stats"], env=env, capture_output=True, text=True)
    assert "1 run(s) passed --skip-preflight" in stats.stdout, stats.stdout


def test_a_rerun_and_a_nested_gate_skip_the_preflight(tmp_path: Path) -> None:
    """Both run on a tree that already passed the preflight."""
    env = {**_env(tmp_path), "GATE_PREFLIGHT_COMMAND": FAILING_PREFLIGHT}
    rerun = subprocess.run([PY, str(GATE), "--rerun", "--", PY, "-c", "pass", "--selected", "a.step"],
                           env=env, capture_output=True, text=True, timeout=30)
    assert rerun.returncode == 0, rerun.stdout
    nested_script = (
        "import subprocess, sys\n"
        f"done = subprocess.run([sys.executable, {str(GATE)!r}, '--exclusive', '--', "
        "sys.executable, '-c', 'pass'])\n"
        "sys.exit(done.returncode)\n"
    )
    outer = subprocess.run([PY, str(GATE), "--skip-preflight", "--", PY, "-c", nested_script],
                           env=env, capture_output=True, text=True, timeout=60)
    assert outer.returncode == 0, outer.stdout
    rows = _ledger_rows(tmp_path)
    assert [row["preflight"] for row in rows] == [
        "skipped: rerun", "skipped: nested gate", "skipped: --skip-preflight"]


def test_the_preflight_runs_named_validation_steps() -> None:
    """The default preflight runs real steps of the validation graph, so it can
    never drift from what the gate itself checks."""
    gate = _gate_module()
    env = {key: value for key, value in os.environ.items() if key != "GATE_PREFLIGHT_COMMAND"}
    with pytest.MonkeyPatch.context() as patch:
        patch.delenv("GATE_PREFLIGHT_COMMAND", raising=False)
        command = gate.preflight_command()
    assert command[command.index("--selected") + 1] == ",".join(gate.PREFLIGHT_STEPS)
    root = GATE.parents[1]
    # --all: check against the whole graph, not this checkout's diff-selected plan.
    listed = subprocess.run([PY, str(root / "scripts" / "validate.py"), "--all", "--list"], cwd=root, env=env,
                            capture_output=True, text=True, timeout=120)
    assert listed.returncode == 0, listed.stdout + listed.stderr
    steps = {line.split()[1] for line in listed.stdout.splitlines() if line.strip().startswith("STEP ")}
    assert set(gate.PREFLIGHT_STEPS) <= steps, sorted(set(gate.PREFLIGHT_STEPS) - steps)


def _hold(tmp_path: Path, *args: str) -> subprocess.CompletedProcess[str]:
    env = _env(tmp_path)
    env.pop("GATE_HOLD_NOW", None)
    return subprocess.run([PY, str(GATE), "hold", *args], env=env,
                          capture_output=True, text=True, timeout=30)


def _set_hold(tmp_path: Path, reason: str = "cpu measurement") -> None:
    done = _hold(tmp_path, "--until", "2999-01-01T00:00:00Z", "--reason", reason)
    assert done.returncode == 0, done.stdout


def test_a_hold_blocks_the_preflight_until_it_is_released(tmp_path: Path) -> None:
    """#1370: a hold stops a new gate before its preflight, not only before
    its ticket (2026-11-12: preflight dagrun scopes ran 7:43-7:54 inside a
    7:26-8:01 hold). Release lets the same invocation proceed in order."""
    marker = tmp_path / "preflight-ran"
    env = {**_env(tmp_path), "GATE_PREFLIGHT_COMMAND": f"touch {marker}"}
    env.pop("GATE_HOLD_NOW", None)
    _set_hold(tmp_path)
    output = tmp_path / "gate-output.txt"
    with output.open("w") as stream:
        proc = subprocess.Popen([PY, str(GATE), "--", PY, "-c", "pass"], env=env,
                                stdout=stream, stderr=subprocess.STDOUT, text=True)
        try:
            deadline = time.time() + 10
            text = ""
            while "admission hold active" not in text:
                assert time.time() < deadline, "the gate never reported the hold"
                assert proc.poll() is None, output.read_text()
                text = output.read_text() if output.exists() else ""
                time.sleep(0.05)
            assert not marker.exists(), "the preflight ran while the hold was active"
            assert not list((tmp_path / "admission" / "queue").glob("*.json"))
            released = _hold(tmp_path, "--release")
            assert released.returncode == 0, released.stdout
            proc.wait(timeout=30)
        finally:
            if proc.poll() is None:
                proc.kill()
    text = output.read_text()
    assert proc.returncode == 0, text
    assert marker.exists()
    assert text.index("admission hold active") < text.index("preflight passed")
    assert "admission hold lifted" in text


def test_no_wait_hold_refuses_before_the_preflight(tmp_path: Path) -> None:
    """#1370: --no-wait-hold chooses refusal over waiting, with the reason
    and end time in the message, and the preflight never runs."""
    marker = tmp_path / "preflight-ran"
    env = {**_env(tmp_path), "GATE_PREFLIGHT_COMMAND": f"touch {marker}"}
    env.pop("GATE_HOLD_NOW", None)
    _set_hold(tmp_path, "quiet please")
    done = subprocess.run([PY, str(GATE), "--no-wait-hold", "--", PY, "-c", "pass"],
                          env=env, capture_output=True, text=True, timeout=30)
    assert done.returncode == 79, done.stdout
    assert "admission hold" in done.stdout and "quiet please" in done.stdout
    assert "2999-01-01" in done.stdout
    assert not marker.exists()
    assert not list((tmp_path / "admission" / "queue").glob("*.json"))


def test_an_expired_hold_is_ignored(tmp_path: Path) -> None:
    """#1370: a hold whose end has passed blocks nothing and is reaped."""
    state = tmp_path / "admission"
    state.mkdir(parents=True)
    (state / "hold.json").write_text(
        json.dumps({"until": 1, "reason": "long over", "set_at": 0}))
    marker = tmp_path / "preflight-ran"
    env = {**_env(tmp_path), "GATE_PREFLIGHT_COMMAND": f"touch {marker}"}
    env.pop("GATE_HOLD_NOW", None)
    done = subprocess.run([PY, str(GATE), "--", PY, "-c", "pass"],
                          env=env, capture_output=True, text=True, timeout=30)
    assert done.returncode == 0, done.stdout
    assert marker.exists()
    assert "admission hold" not in done.stdout
    status = subprocess.run([PY, str(GATE), "status"], env=env,
                            capture_output=True, text=True, timeout=30)
    assert "no admission hold" in status.stdout, status.stdout


def test_status_shows_the_hold(tmp_path: Path) -> None:
    """#1370: `gate status` (and bare `gate hold`) name the hold's reason
    and end time; after release both say there is none."""
    _set_hold(tmp_path, "neighbour KVM window")
    status = subprocess.run([PY, str(GATE), "status"], env=_env(tmp_path),
                            capture_output=True, text=True, timeout=30)
    assert status.returncode == 0, status.stdout
    assert "admission hold active until 2999-01-01T00:00:00Z" in status.stdout
    assert "neighbour KVM window" in status.stdout
    shown = _hold(tmp_path)
    assert shown.returncode == 0 and "neighbour KVM window" in shown.stdout
    assert _hold(tmp_path, "--release").returncode == 0
    status = subprocess.run([PY, str(GATE), "status"], env=_env(tmp_path),
                            capture_output=True, text=True, timeout=30)
    assert "no admission hold" in status.stdout, status.stdout


def test_hold_until_accepts_iso_times_and_durations() -> None:
    """#1370: --until takes an ISO 8601 time or a duration from now."""
    gate = _gate_module()
    assert gate.parse_hold_until("30m", now=1000.0) == 1000.0 + 30 * 60
    assert gate.parse_hold_until("2h", now=1000.0) == 1000.0 + 2 * 3600
    assert gate.parse_hold_until("2999-01-01T00:00:00Z", now=1000.0) > 1000.0
    with pytest.raises(gate.HoldError):
        gate.parse_hold_until("whenever", now=1000.0)


def test_contained_command_applies_the_cpu_cap_and_nice_10(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """#1370: the containment wrapper carries the per-gate CPU cap and
    nice 10, for gates and for the preflight alike."""
    gate = _gate_module()
    monkeypatch.delenv("GATE_CPU_CONTAINMENT", raising=False)
    wrapped, contained = gate._contained_command(["echo", "hi"], "1370-test", 48)
    assert contained and "CPUQuota=4800%" in wrapped
    assert any(word.endswith("nice") for word in wrapped) and "10" in wrapped


def test_the_preflight_runs_contained_like_a_gate(tmp_path: Path) -> None:
    """#1370: the preflight child sees the capped scheduler widths and
    runs at nice 10 under the per-gate CPU quota."""
    env_file = tmp_path / "preflight.env"
    nice_file = tmp_path / "preflight.nice"
    env = {**_env(tmp_path), "GATE_CPU_QUOTA_CPUS": "7",
           "GATE_PREFLIGHT_COMMAND": (
               f"env > {env_file}; {PY} -c \"import os; print(os.nice(0))\" > {nice_file}")}
    env.pop("GATE_CPU_CONTAINMENT", None)
    env.pop("GATE_HOLD_NOW", None)
    env.pop("VALIDATE_MAX_CPUS", None)
    env.pop("GOMAXPROCS", None)
    done = subprocess.run([PY, str(GATE), "--", PY, "-c", "pass"],
                          env=env, capture_output=True, text=True, timeout=60)
    assert done.returncode == 0, done.stdout
    assert "preflight containment" in done.stdout and "CPUQuota=700%" in done.stdout
    recorded = dict(line.split("=", 1) for line in env_file.read_text().splitlines() if "=" in line)
    assert recorded["GATE_CPU_QUOTA_CPUS"] == "7"
    assert recorded["VALIDATE_MAX_CPUS"] == "7" and recorded["GOMAXPROCS"] == "7"
    # `nice -n 10` raises this process's niceness by 10 (capped at 19); a
    # lane starts at 0, so there the preflight runs at exactly nice 10.
    assert int(nice_file.read_text()) == min(19, os.nice(0) + 10)


def test_a_dead_queue_ticket_never_blocks_admission(tmp_path: Path) -> None:
    state = tmp_path / "admission" / "queue"
    state.mkdir(parents=True)
    (state / "0-dead.json").write_text(
        '{"id": "0-dead", "pid": 99999999, "class": "exclusive", "queued_at": 0}')
    proc = _start(tmp_path, "--", PY, "-c", "pass")
    out, _ = proc.communicate(timeout=30)
    assert proc.returncode == 0
    assert "queued at position" not in out
    assert time.time() > 0  # the dead ticket was reaped, not waited on


def test_an_owner_ticket_stays_ahead_of_a_long_waiting_normal(tmp_path: Path) -> None:
    """#1219: the owner tier is absolute — a fresh owner ticket ranks
    ahead of a normal ticket that has waited many hours, and FIFO holds
    inside each tier. No field promotes a normal ticket."""
    gate = _gate_module()
    adm = gate.Admission(tmp_path / "admission", 2)
    queue = tmp_path / "admission" / "queue"
    now = time.time()
    rows = [
        ("100-old-normal", "shared", False, {}, now - 12 * 3600),
        ("200-rerun", "exclusive", True, {}, now - 30),
        ("300-owner", "shared", False,
         {"priority": "owner", "priority_reason": "#1 owner UI"}, now - 10),
        ("400-young-normal", "shared", False, {}, now - 60),
    ]
    for run_id, cls, rerun, extra, queued in rows:
        (queue / f"{run_id}.json").write_text(json.dumps(
            {"id": run_id, "pid": os.getpid(), "class": cls, "queued_at": queued,
             "command": [], "rerun": rerun, **extra}))
    assert [str(d["id"]) for _, d in adm.tickets()] == [
        "300-owner", "200-rerun", "100-old-normal", "400-young-normal"]


def test_queue_shows_cwd_and_warns_about_starved_normals(tmp_path: Path) -> None:
    """#1219: the read-only queue view shows position, class, priority,
    age and cwd (a dash for tickets predating the field), and warns when
    a normal ticket has waited over 2h. It must not rewrite tickets."""
    now = time.time()
    root = tmp_path / "admission"
    _write_ticket(root, "queue", "100-starved", {
        "id": "100-starved", "pid": os.getpid(), "class": "shared",
        "queued_at": now - 3 * 3600, "rerun": False,
        "cwd": "/slots/alpha/repo", "command": [PY, "-c", "pass"]})
    _write_ticket(root, "queue", "200-legacy", {
        "id": "200-legacy", "pid": os.getpid(), "class": "shared",
        "queued_at": now - 30, "rerun": False, "command": [PY, "-c", "pass"]})
    before = (root / "queue" / "100-starved.json").read_text()
    shown = subprocess.run([PY, str(GATE), "queue", "--explain"], env=_env(tmp_path),
                           capture_output=True, text=True, timeout=30)
    assert shown.returncode == 0, shown.stdout
    assert "#1 100-starved" in shown.stdout and "cwd=/slots/alpha/repo" in shown.stdout
    assert "cwd=-" in shown.stdout, "a ticket predating the cwd field shows a dash"
    assert "warning: 1 normal ticket(s) have waited over 2h" in shown.stdout
    assert (root / "queue" / "100-starved.json").read_text() == before


def _history_rows(root: Path, rows: list[dict[str, object]]) -> None:
    root.mkdir(parents=True, exist_ok=True)
    (root / "history.jsonl").write_text(
        "".join(json.dumps(row) + "\n" for row in rows))


def test_interleave_puts_oldest_non_owner_next_after_two_owners(tmp_path: Path) -> None:
    """#1219 interleave: with two consecutive owner admissions in the
    record, the oldest non-owner ticket sorts ahead of waiting owners,
    in memory only — its ticket file is never rewritten."""
    gate = _gate_module()
    root = tmp_path / "admission"
    adm = gate.Admission(root, 2)
    now = time.time()
    _history_rows(root, [
        {"id": "h1", "started_at": now - 300, "priority": "owner"},
        {"id": "h2", "started_at": now - 200, "priority": "owner"},
    ])
    queue = root / "queue"
    rows = [
        ("100-normal-old", {}, now - 3600),
        ("200-normal-young", {}, now - 60),
        ("300-owner", {"priority": "owner", "priority_reason": "#1"}, now - 30),
        ("400-owner", {"priority": "owner", "priority_reason": "#1"}, now - 20),
    ]
    for run_id, extra, queued in rows:
        (queue / f"{run_id}.json").write_text(json.dumps(
            {"id": run_id, "pid": os.getpid(), "class": "shared", "queued_at": queued,
             "command": [], "rerun": False, **extra}))
    before = (queue / "100-normal-old.json").read_text()
    tickets = adm.tickets()
    assert [str(d["id"]) for _, d in tickets] == [
        "100-normal-old", "300-owner", "400-owner", "200-normal-young"]
    assert tickets[0][1].get("interleave_selected") is True
    assert (queue / "100-normal-old.json").read_text() == before


def test_interleave_marked_tickets_are_normal(tmp_path: Path) -> None:
    """#1219: a stopgap ticket marked interleave: in its reason orders
    as ordinary FIFO and breaks an owner streak, never owner tier."""
    gate = _gate_module()
    root = tmp_path / "admission"
    adm = gate.Admission(root, 2)
    now = time.time()
    _history_rows(root, [
        {"id": "h1", "started_at": now - 300, "priority": "owner"},
        {"id": "h2", "started_at": now - 200, "priority": "owner",
         "priority_reason": "interleave: oldest normal"},
    ])
    assert adm._trailing_owner_admissions() == 0
    queue = root / "queue"
    (queue / "100-marked.json").write_text(json.dumps(
        {"id": "100-marked", "pid": os.getpid(), "class": "shared",
         "queued_at": now - 3600, "command": [], "rerun": False,
         "priority": "owner", "priority_reason": "interleave: oldest normal"}))
    (queue / "200-owner.json").write_text(json.dumps(
        {"id": "200-owner", "pid": os.getpid(), "class": "shared",
         "queued_at": now - 60, "command": [], "rerun": False,
         "priority": "owner", "priority_reason": "#1 owner UI"}))
    assert [str(d["id"]) for _, d in adm.tickets()] == ["200-owner", "100-marked"]


def test_interleave_host_setting(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    gate = _gate_module()
    config = tmp_path / "gate-interleave.toml"
    monkeypatch.setenv("GATE_INTERLEAVE_CONFIG", str(config))
    monkeypatch.delenv("GATE_INTERLEAVE_AFTER", raising=False)
    assert gate.interleave_after() == (2, "default 2")
    config.write_text("after_owners = 3\n")
    value, source = gate.interleave_after()
    assert value == 3 and "host file" in source
    monkeypatch.setenv("GATE_INTERLEAVE_AFTER", "1")
    value, source = gate.interleave_after()
    assert value == 1 and source.startswith("env")
    monkeypatch.setenv("GATE_INTERLEAVE_AFTER", "bogus")
    value, _ = gate.interleave_after()
    assert value == 2


def test_interleaved_admission_order_end_to_end(tmp_path: Path) -> None:
    """#1219: 5 owner and 3 normal tickets behind a busy box, one slot,
    admit O O N O O N O N."""
    order = tmp_path / "order.txt"

    def note(tag: str) -> str:
        return (f"open({str(order)!r}, 'a').write({tag!r} + '\\n'); "
                "import time; time.sleep(1)")

    gate_file = tmp_path / "release"
    running = _start(tmp_path, "--exclusive", "--", PY, "-c",
                     f"import os, time\nwhile not os.path.exists({str(gate_file)!r}): time.sleep(0.05)")
    _wait_for_runs(tmp_path, 1)
    normals = []
    for i in (1, 2, 3):
        normals.append(_start(tmp_path, "--slots", "1", "--", PY, "-c", note(f"N{i}")))
        _wait_for_queue(tmp_path, i)
    owner_env = {**_env(tmp_path), "GATE_PRIORITY": "owner",
                 "GATE_PRIORITY_REASON": "#1 owner UI"}
    owners = []
    for i in (1, 2, 3, 4, 5):
        owners.append(_start_env(tmp_path, owner_env, "--slots", "1", "--",
                                 PY, "-c", note(f"O{i}")))
        _wait_for_queue(tmp_path, 3 + i)
    gate_file.write_text("")
    for proc in [running, *normals, *owners]:
        out, _ = proc.communicate(timeout=120)
        assert proc.returncode == 0, out
    assert order.read_text().split() == [
        "O1", "O2", "N1", "O3", "O4", "N2", "O5", "N3"]


def test_no_starvation_warning_for_young_or_owner_waits(tmp_path: Path) -> None:
    gate = _gate_module()
    now = time.time()
    tickets = [
        (Path("a.json"), {"queued_at": now - 3 * 3600, "priority": "owner"}),
        (Path("b.json"), {"queued_at": now - 60}),
    ]
    assert gate._starvation_warning(tickets, now) is None
    tickets.append((Path("c.json"), {"queued_at": now - 3 * 3600}))
    warning = gate._starvation_warning(tickets, now)
    assert warning is not None and "oldest 3.0h" in warning


# ------------------------------------------------- cargo target pruning (#940)


def _gate_module() -> ModuleType:
    spec = importlib.util.spec_from_file_location("gate_under_test", GATE)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module  # dataclasses look their module up there
    spec.loader.exec_module(module)
    return module


def _write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


def _cargo(ws: Path, target: Path, *args: str) -> list[dict[str, object]]:
    # Incremental caches are part of what the prune is tested on, so they are
    # built whatever the caller's CARGO_INCREMENTAL says (a gate run with it
    # at 0 left none to count).
    out = subprocess.run(
        ["cargo", *args, "--offline", "--message-format=json", "--manifest-path", str(ws / "Cargo.toml"),
         "--target-dir", str(target)],
        env={**os.environ, "CARGO_INCREMENTAL": "1"},
        capture_output=True, text=True, timeout=300, check=True,
    ).stdout
    return [m for m in map(json.loads, out.splitlines()) if m.get("reason") == "compiler-artifact"]


def _age(target: Path, seconds: float) -> None:
    """Make everything built so far look *seconds* old (fingerprints and
    incremental caches are what the prune reads)."""
    old = time.time() - seconds
    for path in [*target.rglob(".fingerprint/*/*"), *target.rglob("incremental/*")]:
        os.utime(path, (old, old))


@pytest.mark.skipif(shutil.which("cargo") is None, reason="needs cargo")
def test_prune_drops_superseded_builds_and_a_rerun_rebuilds_nothing(tmp_path: Path) -> None:
    """#940: a build superseded by a version change goes, with its incremental
    cache; the two variants two commands still need stay (`cargo build` and
    `cargo test` see `dep` with different features), and so does a dependency
    built days ago that the current builds still use. Rerunning both commands
    afterwards finds every unit fresh."""
    ws = tmp_path / "ws"
    _write(ws / "Cargo.toml", '[workspace]\nmembers = ["app", "dep", "base"]\nresolver = "2"\n')
    _write(ws / "base/Cargo.toml", '[package]\nname = "base"\nversion = "0.1.0"\nedition = "2021"\n')
    _write(ws / "base/src/lib.rs", "pub fn one() -> u32 { 1 }\n")
    dep_manifest = ('[package]\nname = "dep"\nversion = "{v}"\nedition = "2021"\n'
                    '[features]\nextra = []\n[dependencies]\nbase = {{ path = "../base" }}\n')
    _write(ws / "dep/Cargo.toml", dep_manifest.format(v="0.1.0"))
    _write(ws / "dep/src/lib.rs", "pub fn f() -> u32 { base::one() }\n")
    _write(ws / "app/Cargo.toml", '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n'
           '[dependencies]\ndep = { path = "../dep" }\n'
           '[dev-dependencies]\ndep = { path = "../dep", features = ["extra"] }\n')
    _write(ws / "app/src/main.rs", "fn main() { println!(\"{}\", dep::f()); }\n")
    _write(ws / "app/tests/t.rs", "#[test]\nfn t() { assert_eq!(dep::f(), 1); }\n")
    target = tmp_path / "target"
    _cargo(ws, target, "build")
    _cargo(ws, target, "test", "--no-run")
    old_dep = {p.name for p in (target / "debug/.fingerprint").glob("dep-*")}
    _age(target, 3 * 86400)
    _write(ws / "dep/Cargo.toml", dep_manifest.format(v="0.2.0"))
    _cargo(ws, target, "build")
    _cargo(ws, target, "test", "--no-run")
    incremental_before = len(list((target / "debug/incremental").glob("dep-*")))

    gate = _gate_module()
    (root, profiles), = gate.target_dirs(target).items()
    result = gate.prune_target_root(root, profiles, now=time.time() + 2 * 3600)
    assert result.skipped is None and result.units > 0 and result.freed > 0

    left = {p.name for p in (target / "debug/.fingerprint").glob("dep-*")}
    assert left and not (left & old_dep)
    assert not [p for p in (target / "debug/deps").iterdir() if any(h.rsplit("-", 1)[1] in p.name for h in old_dep)]
    assert list((target / "debug/.fingerprint").glob("base-*"))
    assert len(list((target / "debug/incremental").glob("dep-*"))) < incremental_before
    for command in (["build"], ["test", "--no-run"]):
        stale = [m["target"] for m in _cargo(ws, target, *command) if not m.get("fresh")]
        assert stale == [], (command, stale)


def test_prune_skips_a_target_dir_cargo_is_using(tmp_path: Path) -> None:
    """#940: with cargo's build lock held, nothing in that target dir is touched."""
    profile = tmp_path / "target/debug"
    (profile / ".fingerprint/gone-0123456789abcdef").mkdir(parents=True)
    (tmp_path / "target/CACHEDIR.TAG").write_text("Signature: 8a477f597d28d172789f06886806bc55\n")
    lock = profile / ".cargo-build-lock"
    lock.touch()
    gate = _gate_module()
    with lock.open() as held:
        fcntl.flock(held, fcntl.LOCK_EX)
        result = gate.prune_target_root(tmp_path / "target", [profile], now=time.time() + 7200)
    assert result.skipped == "cargo is using it"
    assert (profile / ".fingerprint/gone-0123456789abcdef").is_dir()
    unreadable = profile / ".fingerprint/odd-0123456789abcdee/lib-odd.json"
    _write(unreadable, "{not json")
    result = gate.prune_target_root(tmp_path / "target", [profile], now=time.time() + 7200)
    assert result.skipped and result.skipped.startswith("unreadable fingerprint")
    assert (profile / ".fingerprint/gone-0123456789abcdef").is_dir()


def test_a_finished_gate_prunes_its_own_targets_and_records_the_bytes(tmp_path: Path) -> None:
    """#940: a gate prunes its own checkout's target dir and nothing else, and
    reports target bytes before and after in its output and its ledger line,
    with the verdict still last; a failed build's leftovers go once older than
    an hour."""
    checkout = tmp_path / "checkout"
    (checkout / "scripts").mkdir(parents=True)
    shutil.copy(GATE, checkout / "scripts/gate.py")
    old = time.time() - 7200
    stale: list[Path] = []
    for top in (checkout / "rs/target", tmp_path / "other/rs/target"):
        unit = top / "debug/.fingerprint/gone-0123456789abcdef"
        unit.mkdir(parents=True)
        _write(top / "debug/deps/libgone-0123456789abcdef.rlib", "x" * 9000)
        os.utime(unit, (old, old))
        stale.append(unit)
    env = {**_env(tmp_path), "GATE_TRIM_COMMAND": f"{PY} -c \"print('blocks returned')\""}
    env.pop("GATE_PRUNE_TARGETS")
    # A run ID leaked from an outer gate (this test runs inside one) is not a nested gate.
    env["GATE_ADMISSION_RUN_ID"] = "1-leaked-from-an-outer-gate"
    failing = {**env, "GATE_TRIM_COMMAND": f"{PY} -c \"raise SystemExit(5)\""}
    result = subprocess.run([PY, str(checkout / "scripts/gate.py"), "--", PY, "-c", "print('validate: OK')"],
                            env=env, capture_output=True, text=True, timeout=60)
    assert result.returncode == 0, result.stdout
    assert f"cargo target {checkout / 'rs/target'}:" in result.stdout, result.stdout
    assert "pruned 1 stale unit(s)" in result.stdout
    assert "gate: trim: blocks returned" in result.stdout, "a prune that freed space runs the trim"
    assert result.stdout.splitlines()[-1] == "validate: OK"
    assert not stale[0].exists() and not (checkout / "rs/target/debug/deps/libgone-0123456789abcdef.rlib").exists()
    assert stale[1].is_dir(), "another checkout's target dir is never pruned by a gate"
    row = json.loads((tmp_path / "ledger.jsonl").read_text().splitlines()[-1])
    assert row["target_bytes_before"] > row["target_bytes_after"]
    assert row["host_bytes_before"] is None  # a temp dir is not on a slot image
    assert row["trim_exit"] == 0

    unit = checkout / "rs/target/debug/.fingerprint/gone-0123456789abcdee"
    unit.mkdir(parents=True)
    os.utime(unit, (old, old))
    failed_trim = subprocess.run([PY, str(checkout / "scripts/gate.py"), "--", PY, "-c", "print('validate: OK')"],
                                 env=failing, capture_output=True, text=True, timeout=60)
    assert failed_trim.returncode == 0, "a failing trim never fails the gate"
    assert "gate: trim command exited 5" in failed_trim.stdout, failed_trim.stdout

    report = subprocess.run([PY, str(checkout / "scripts/gate.py"), "prune", str(tmp_path / "other/rs/target")],
                            capture_output=True, text=True, timeout=60)
    assert report.returncode == 2 and "--other-checkouts" in report.stderr
    report = subprocess.run([PY, str(checkout / "scripts/gate.py"), "prune", "--other-checkouts",
                             str(tmp_path / "other/rs/target")], capture_output=True, text=True, timeout=60)
    assert report.returncode == 0 and "would prune 1 stale unit(s)" in report.stdout, report.stdout
    assert stale[1].is_dir(), "a report removes nothing"


_CHILD_TREE = """
import os, signal, subprocess, sys, time
if sys.argv[2] == "ignore":
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
grandchild = subprocess.Popen([sys.executable, "-c",
    "import signal, sys, time\\n"
    "if sys.argv[1] == 'ignore': signal.signal(signal.SIGTERM, signal.SIG_IGN)\\n"
    "time.sleep(300)", sys.argv[2]])
with open(sys.argv[1], "w") as fh:
    fh.write(f"{os.getpid()} {grandchild.pid}")
time.sleep(300)
"""


def _alive_pid(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    # A zombie still answers kill(0); it no longer runs.
    try:
        return Path(f"/proc/{pid}/stat").read_text().split(") ")[1][0] != "Z"
    except (OSError, IndexError):
        return False


def _start_tree(tmp_path: Path, mode: str) -> tuple[subprocess.Popen[str], list[int]]:
    pids = tmp_path / "pids"
    script = tmp_path / "tree.py"
    script.write_text(_CHILD_TREE)
    env = {**_env(tmp_path), "GATE_ABORT_GRACE_S": "1"}
    gate = subprocess.Popen([PY, str(GATE), "--", PY, str(script), str(pids), mode], env=env,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    deadline = time.time() + 15
    while time.time() < deadline and not (pids.exists() and pids.read_text().strip()):
        time.sleep(0.05)
    assert pids.exists(), "the gated command did not start"
    return gate, [int(p) for p in pids.read_text().split()]


@pytest.mark.parametrize("mode", ["obey", "ignore"])
def test_a_stopped_gate_stops_its_whole_command_tree_and_records_the_abort(
        tmp_path: Path, mode: str) -> None:
    """SIGTERM to gate.py reaches the command's grandchildren too (they used to
    be orphaned and keep running outside admission); a tree that ignores
    SIGTERM is SIGKILLed after the grace period. The slot is released and the
    ledger says aborted."""
    gate, pids = _start_tree(tmp_path, mode)
    assert all(_alive_pid(pid) for pid in pids)
    gate.send_signal(signal.SIGTERM)
    out, _ = gate.communicate(timeout=30)
    assert gate.returncode == 128 + signal.SIGTERM, out
    deadline = time.time() + 5
    while time.time() < deadline and any(_alive_pid(pid) for pid in pids):
        time.sleep(0.05)
    assert not any(_alive_pid(pid) for pid in pids), (mode, out)
    expected = "stopped by SIGTERM" if mode == "obey" else "SIGKILLed after 2s"
    assert f"gate: aborted by SIGTERM; run's process group {expected}" in out
    assert not list((tmp_path / "admission" / "runs").glob("*.json"))
    row = json.loads((tmp_path / "ledger.jsonl").read_text().splitlines()[-1])
    assert row["verdict"] == "aborted"
    assert row["aborted"].startswith("SIGTERM")
    stats = subprocess.run([PY, str(GATE), "stats"], env=_env(tmp_path),
                           capture_output=True, text=True, timeout=30)
    assert "1 run(s) aborted by a signal" in stats.stdout
    assert "0 first-run gate(s)" in stats.stdout


# ------------------------------------------------- merge-queue deferral (#1406)

_STUB_UNIT = "protected-queue-7.service"


def _stub_systemctl(tmp_path: Path) -> tuple[Path, Path, Path]:
    """A stubbed systemctl on PATH (never the real one): it reports the
    protected unit while the flag file exists, and counts its calls so a
    test can prove the gate's poll is paced, never an unbounded spawn loop."""
    bindir = tmp_path / "stubbin"
    bindir.mkdir(exist_ok=True)
    flag = tmp_path / "unit-active"
    calls = tmp_path / "systemctl-calls"
    stub = bindir / "systemctl"
    stub.write_text(
        "#!/usr/bin/env python3\n"
        "import pathlib, sys\n"
        f"flag = pathlib.Path({str(flag)!r})\n"
        f"calls = pathlib.Path({str(calls)!r})\n"
        "calls.write_text(str(int(calls.read_text()) + 1) if calls.exists() else '1')\n"
        "if flag.exists():\n"
        f"    print({_STUB_UNIT!r} + ' loaded active running stub')\n"
    )
    stub.chmod(0o755)
    return bindir, flag, calls


def _defer_env(tmp_path: Path, bindir: Path, **overrides: str) -> dict[str, str]:
    env = _env(tmp_path)
    env["PATH"] = str(bindir) + os.pathsep + env.get("PATH", "")
    env["GATE_DEFER_UNIT_PATTERN"] = "protected-*"
    env.update(overrides)
    return env


def _wait_for_deferred(tmp_path: Path) -> None:
    deferred = tmp_path / "admission" / "deferred"
    deadline = time.time() + 10
    while time.time() < deadline:
        if deferred.exists() and list(deferred.glob("*.json")):
            return
        time.sleep(0.05)
    raise AssertionError("gate did not record a deferred wait")


def test_a_gate_defers_while_a_protected_unit_is_active_then_starts(tmp_path: Path) -> None:
    """#1406: the command does not run while the stubbed unit is active,
    `status` shows the deferred gate and its unit, and the gate starts when
    the unit finishes — with no deadline that could fail it."""
    bindir, flag, calls = _stub_systemctl(tmp_path)
    flag.touch()
    marker = tmp_path / "ran"
    gate = subprocess.Popen(
        [PY, str(GATE), "--", PY, "-c", f"open({str(marker)!r}, 'w').close()"],
        env=_defer_env(tmp_path, bindir),
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    try:
        _wait_for_deferred(tmp_path)
        assert not marker.exists()
        assert gate.poll() is None
        status = subprocess.run([PY, str(GATE), "status"], env=_defer_env(tmp_path, bindir),
                                capture_output=True, text=True, timeout=30)
        assert status.returncode == 0, status.stdout
        assert "deferred" in status.stdout and _STUB_UNIT in status.stdout
    finally:
        flag.unlink(missing_ok=True)
    out, _ = gate.communicate(timeout=30)
    assert gate.returncode == 0, out
    assert marker.exists()
    assert f"deferred: waiting on {_STUB_UNIT}" in out
    assert "deferral cleared" in out
    # Paced poll (0.05 s in this env over a short wait), never a spawn loop.
    assert 0 < int(calls.read_text()) < 400


def test_an_empty_or_unset_pattern_means_no_deferral(tmp_path: Path) -> None:
    bindir, flag, _calls = _stub_systemctl(tmp_path)
    flag.touch()
    for pattern in ("", None):
        env = _defer_env(tmp_path, bindir)
        if pattern is None:
            del env["GATE_DEFER_UNIT_PATTERN"]
        else:
            env["GATE_DEFER_UNIT_PATTERN"] = pattern
        result = subprocess.run([PY, str(GATE), "--", "/bin/true"], env=env,
                                capture_output=True, text=True, timeout=30)
        assert result.returncode == 0, result.stdout
        assert "deferred" not in result.stdout


def test_owner_priority_defers_unless_the_host_file_exempts_it(tmp_path: Path) -> None:
    """#1406 choice: owner gates defer too by default; the host file alone
    can exempt them with owner_defers = false."""
    bindir, flag, _calls = _stub_systemctl(tmp_path)
    flag.touch()
    config = tmp_path / "gate-defer.toml"
    config.write_text('defer_unit_pattern = "protected-*"\n')
    env = _defer_env(tmp_path, bindir)
    del env["GATE_DEFER_UNIT_PATTERN"]  # the host file decides
    gate = subprocess.Popen(
        [PY, str(GATE), "--priority", "owner", "--priority-reason", "#1406 test",
         "--", "/bin/true"],
        env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    try:
        _wait_for_deferred(tmp_path)
        assert gate.poll() is None
    finally:
        flag.unlink(missing_ok=True)
    out, _ = gate.communicate(timeout=30)
    assert gate.returncode == 0, out
    assert f"deferred: waiting on {_STUB_UNIT}" in out
    config.write_text('defer_unit_pattern = "protected-*"\nowner_defers = false\n')
    flag.touch()
    result = subprocess.run(
        [PY, str(GATE), "--priority", "owner", "--priority-reason", "#1406 test",
         "--", "/bin/true"],
        env=env, capture_output=True, text=True, timeout=30)
    assert result.returncode == 0, result.stdout
    assert "deferred" not in result.stdout


def test_a_running_gate_is_never_paused_by_a_new_deferral(tmp_path: Path) -> None:
    """#1406: deferral is start-only. A gate already running when the
    protected unit becomes active runs to completion untouched."""
    bindir, flag, _calls = _stub_systemctl(tmp_path)
    marker = tmp_path / "ran"
    script = (
        "import pathlib, time\n"
        f"mark = pathlib.Path({str(flag)!r})\n"
        "mark.touch()\n"
        "time.sleep(2)\n"
        f"open({str(marker)!r}, 'w').close()\n"
    )
    result = subprocess.run([PY, str(GATE), "--", PY, "-c", script],
                            env=_defer_env(tmp_path, bindir),
                            capture_output=True, text=True, timeout=30)
    assert result.returncode == 0, result.stdout
    assert marker.exists()
    assert "deferred: waiting" not in result.stdout


def test_a_malformed_defer_file_refuses_the_start(tmp_path: Path) -> None:
    (tmp_path / "gate-defer.toml").write_text('defer_unit_pattern = 3\n')
    result = subprocess.run([PY, str(GATE), "--", "/bin/true"], env=_env(tmp_path),
                            capture_output=True, text=True, timeout=30)
    assert result.returncode == 2, result.stdout
    assert "gate defer config" in result.stdout


def _marker_hold(tmp_path: Path, name: str, status_text: str, cap: object = 3600,
          declaration: str | None = None) -> Path:
    holds = tmp_path / "holds"
    holds.mkdir(parents=True, exist_ok=True)
    status = tmp_path / f"{name}-status.txt"
    status.write_text(status_text)
    if declaration is None:
        declaration = json.dumps({"marker_file": str(status), "max_hold_seconds": cap})
    (holds / f"{name}.json").write_text(declaration)
    return holds


_NOW = dt.datetime(2026, 11, 12, 12, 0, tzinfo=dt.timezone.utc)


def test_measurement_hold_fixtures(tmp_path: Path) -> None:
    """#1406 hold contract: active, ended, expired, unparseable (fails
    closed) and empty-directory cases."""
    gate = _gate_module()
    active = _marker_hold(tmp_path / "a", "w", "WINDOW-START 2026-11-12T11:30:00Z\n")
    blockers = gate.measurement_hold_blockers(str(active), _NOW)
    assert len(blockers) == 1 and blockers[0].startswith("hold w ("), blockers
    ended = _marker_hold(tmp_path / "b", "w",
                  "WINDOW-START 2026-11-12T11:00:00Z\nWINDOW-END 2026-11-12T11:30:00Z\n")
    assert gate.measurement_hold_blockers(str(ended), _NOW) == []
    expired = _marker_hold(tmp_path / "c", "w", "WINDOW-START 2026-11-12T08:00:00Z\n", cap=3600)
    assert gate.measurement_hold_blockers(str(expired), _NOW) == []
    bad = _marker_hold(tmp_path / "d", "w", "irrelevant\n", declaration="{not json")
    blockers = gate.measurement_hold_blockers(str(bad), _NOW)
    assert len(blockers) == 1 and "hold declaration" in blockers[0], blockers
    assert str(bad / "w.json") in blockers[0]
    empty = tmp_path / "empty-holds"
    empty.mkdir()
    assert gate.measurement_hold_blockers(str(empty), _NOW) == []
    assert gate.measurement_hold_blockers(str(tmp_path / "absent-holds"), _NOW) == []
    # A relative marker path resolves against the host-configured root, and
    # the status_file spelling names the same file.
    rel = tmp_path / "rel"
    (rel / "holds").mkdir(parents=True)
    (rel / "marker.txt").write_text("WINDOW-START 2026-11-12T11:30:00Z\n")
    (rel / "holds" / "r.json").write_text(json.dumps(
        {"marker_file": "marker.txt", "max_hold_seconds": 3600}))
    assert len(gate.measurement_hold_blockers(str(rel / "holds"), _NOW, str(rel))) == 1
    (rel / "holds" / "r.json").write_text(json.dumps(
        {"status_file": "marker.txt", "max_hold_seconds": 3600}))
    assert len(gate.measurement_hold_blockers(str(rel / "holds"), _NOW, str(rel))) == 1


def test_a_measurement_hold_defers_a_gate_until_it_ends(tmp_path: Path) -> None:
    holds = _marker_hold(tmp_path, "w", "WINDOW-START 2026-11-12T11:30:00Z\n")
    status = tmp_path / "w-status.txt"
    env = {**_env(tmp_path), "GATE_DEFER_HOLDS_DIR": str(holds),
           "GATE_DEFER_NOW": "2026-11-12T12:00:00+00:00"}
    gate = subprocess.Popen([PY, str(GATE), "--", "/bin/true"], env=env,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    try:
        _wait_for_deferred(tmp_path)
        shown = subprocess.run([PY, str(GATE), "status"], env=env,
                               capture_output=True, text=True, timeout=30)
        assert "deferred" in shown.stdout and "hold w" in shown.stdout
        with status.open("a") as fh:
            fh.write("WINDOW-END 2026-11-12T12:00:01Z\n")
        out, _ = gate.communicate(timeout=30)
    finally:
        if gate.poll() is None:
            gate.kill()
            gate.communicate(timeout=30)
    assert gate.returncode == 0, out
    assert "deferred: waiting on hold w" in out


def test_the_quiet_command_fails_closed(tmp_path: Path) -> None:
    gate = _gate_module()
    assert gate.quiet_command_blocker("") is None
    assert gate.quiet_command_blocker("/bin/true") is None
    blocker = gate.quiet_command_blocker("/bin/false")
    assert blocker is not None and "exited 1" in blocker


def test_defer_help_documents_the_host_file(tmp_path: Path) -> None:
    result = subprocess.run([PY, str(GATE), "--help"], env=_env(tmp_path),
                            capture_output=True, text=True, timeout=30)
    assert result.returncode == 0
    assert "~/.local/etc/agent-utils/gate-defer.toml" in result.stdout
    assert "defer_unit_pattern" in result.stdout
    assert "owner_defers" in result.stdout
    assert "never paused" in result.stdout


# --------------------------------- defer patterns and observe (#1489)


def _multi_unit_systemctl(tmp_path: Path) -> Path:
    """A stubbed systemctl reporting a fixed active set, filtered by the
    glob argument the way the real command filters, so include and
    exclude matching in gate.py is what the test exercises."""
    bindir = tmp_path / "multibin"
    bindir.mkdir(exist_ok=True)
    stub = bindir / "systemctl"
    stub.write_text(
        "#!/usr/bin/env python3\n"
        "import fnmatch, sys\n"
        "units = ['protected-alpha.service', 'protected-worker.service',\n"
        "         'other-queue.service']\n"
        "pattern = sys.argv[4] if len(sys.argv) > 4 else '*'\n"
        "for unit in units:\n"
        "    if fnmatch.fnmatchcase(unit, pattern):\n"
        "        print(unit + ' loaded active running stub')\n"
    )
    stub.chmod(0o755)
    return bindir


def test_defer_list_pattern_and_exclude_matching(
        tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """#1489: a list of include globs defers on any of them, and an
    excluded name never defers even when an include glob catches it."""
    gate = _gate_module()
    bindir = _multi_unit_systemctl(tmp_path)
    monkeypatch.setenv("PATH", str(bindir) + os.pathsep + os.environ.get("PATH", ""))
    units, problem = gate.active_defer_units(("protected-*",), ("*-worker.service",))
    assert problem is None
    assert units == ["protected-alpha.service"], units
    units, problem = gate.active_defer_units(
        ("protected-*", "other-*"), ("*-worker.service",))
    assert problem is None
    assert units == ["other-queue.service", "protected-alpha.service"], units
    units, _problem = gate.active_defer_units(("protected-*",), ())
    assert units == ["protected-alpha.service", "protected-worker.service"], units


def test_defer_config_accepts_a_list_and_validates_the_new_keys() -> None:
    """#1489: defer_unit_pattern takes a string or a list of strings;
    the new keys refuse malformed values instead of ignoring them."""
    gate = _gate_module()
    single = gate._defer_from_mapping({"defer_unit_pattern": "protected-*"}, "t")
    assert single.patterns == ("protected-*",)
    listed = gate._defer_from_mapping(
        {"defer_unit_pattern": ["a-*", "b-*"], "defer_unit_exclude": ["*-worker.service"],
         "defer_mode": "observe", "defer_observe_log": "/tmp/observe.jsonl"}, "t")
    assert listed.patterns == ("a-*", "b-*")
    assert listed.excludes == ("*-worker.service",)
    assert listed.mode == "observe"
    assert listed.observe_log == "/tmp/observe.jsonl"
    assert gate._defer_from_mapping({}, "t").mode == "enforce"
    for raw in ({"defer_unit_pattern": ["a-*", 3]},
                {"defer_unit_pattern": [3]},
                {"defer_unit_exclude": "*-worker.service"},
                {"defer_unit_exclude": [3]},
                {"defer_mode": "sometimes"},
                {"defer_mode": 3},
                {"defer_observe_log": 3},
                {"defer_observe_log": ["/tmp/x"]}):
        with pytest.raises(gate.DeferConfigError):
            gate._defer_from_mapping(raw, "t")


def test_observe_mode_logs_and_never_waits(tmp_path: Path) -> None:
    """#1489 observe: with a protected unit permanently active the gate
    still runs at once, one JSON line per would-have-deferred check names
    the units (excluded ones absent), and status shows the mode."""
    bindir = _multi_unit_systemctl(tmp_path)
    observe_log = tmp_path / "observe.jsonl"
    (tmp_path / "gate-defer.toml").write_text(
        'defer_unit_pattern = ["protected-*", "other-*"]\n'
        'defer_unit_exclude = ["*-worker.service"]\n'
        'defer_mode = "observe"\n'
        f'defer_observe_log = "{observe_log}"\n')
    env = _env(tmp_path)
    env["PATH"] = str(bindir) + os.pathsep + env.get("PATH", "")
    marker = tmp_path / "ran"
    result = subprocess.run(
        [PY, str(GATE), "--", PY, "-c", f"open({str(marker)!r}, 'w').close()"],
        env=env, capture_output=True, text=True, timeout=30)
    assert result.returncode == 0, result.stdout
    assert marker.exists()
    assert "deferred: waiting" not in result.stdout
    lines = [json.loads(line) for line in observe_log.read_text().splitlines() if line.strip()]
    assert lines, "observe mode recorded nothing"
    for line in lines:
        assert line["mode"] == "observe"
        assert line["units"] == ["other-queue.service", "protected-alpha.service"], line
        assert line["gate_id"]
        assert line["time"].endswith("Z")
    status = subprocess.run([PY, str(GATE), "status"], env=env,
                            capture_output=True, text=True, timeout=30)
    assert status.returncode == 0, status.stdout
    assert "observe" in status.stdout


def test_enforce_with_a_list_pattern_still_defers(tmp_path: Path) -> None:
    """#1489: enforce is unchanged apart from the list form — the gate
    waits while a listed pattern matches and starts when it clears."""
    bindir, flag, _calls = _stub_systemctl(tmp_path)
    flag.touch()
    (tmp_path / "gate-defer.toml").write_text('defer_unit_pattern = ["protected-*"]\n')
    env = _env(tmp_path)
    env["PATH"] = str(bindir) + os.pathsep + env.get("PATH", "")
    gate = subprocess.Popen([PY, str(GATE), "--", "/bin/true"], env=env,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    try:
        _wait_for_deferred(tmp_path)
        assert gate.poll() is None
    finally:
        flag.unlink(missing_ok=True)
    out, _ = gate.communicate(timeout=30)
    assert gate.returncode == 0, out
    assert f"deferred: waiting on {_STUB_UNIT}" in out
