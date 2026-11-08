"""Gate admission: the counting cap, the exclusive class, FIFO, and reaping."""
from __future__ import annotations

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


def _env(tmp_path: Path) -> dict[str, str]:
    return {**os.environ, "GATE_ADMISSION_DIR": str(tmp_path / "admission"),
            "GATE_LEDGER": str(tmp_path / "ledger.jsonl"), "GATE_PRUNE_TARGETS": "off",
            "GATE_SLOTS_CONFIG": str(tmp_path / "gate-slots.toml"),
            # A lane may run the suite under GATE_SLOTS=1; the tests assume the default.
            "GATE_SLOTS": "2",
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


def test_a_rerun_ticket_jumps_ordinary_queued_gates(tmp_path: Path) -> None:
    """A --rerun ticket sorts ahead of every ordinary queued ticket and is
    granted as soon as the running gate finishes."""
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


def test_a_rerun_must_name_a_few_selected_steps(tmp_path: Path) -> None:
    none = subprocess.run([PY, str(GATE), "--rerun", "--", PY, "-c", "pass"],
                          env=_env(tmp_path), capture_output=True, text=True, timeout=30)
    assert none.returncode == 2 and "--selected" in none.stdout
    many = subprocess.run([PY, str(GATE), "--rerun", "--", PY, "-c", "pass",
                           "--selected", "a,b,c,d,e"],
                          env=_env(tmp_path), capture_output=True, text=True, timeout=30)
    assert many.returncode == 2 and "at most 4" in many.stdout


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
    listed = subprocess.run([PY, str(root / "scripts" / "validate.py"), "--list"], cwd=root, env=env,
                            capture_output=True, text=True, timeout=120)
    assert listed.returncode == 0, listed.stdout + listed.stderr
    steps = {line.split()[1] for line in listed.stdout.splitlines() if line.strip().startswith("STEP ")}
    assert set(gate.PREFLIGHT_STEPS) <= steps, sorted(set(gate.PREFLIGHT_STEPS) - steps)


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
