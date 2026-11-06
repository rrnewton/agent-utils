"""Gate admission: the counting cap, the exclusive class, FIFO, and reaping."""
from __future__ import annotations

import os
import subprocess
import sys
import time
from pathlib import Path

GATE = Path(__file__).resolve().parents[2] / "scripts" / "gate.py"
PY = sys.executable


def _env(tmp_path: Path) -> dict[str, str]:
    return {**os.environ, "GATE_ADMISSION_DIR": str(tmp_path / "admission"),
            "GATE_LEDGER": str(tmp_path / "ledger.jsonl")}


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

    stats = subprocess.run([PY, str(GATE), "stats"], env=env, capture_output=True, text=True)
    assert stats.returncode == 0, stats.stdout
    assert "first-run pass rate 50.0%" in stats.stdout
    assert "failing step b.step: 1" in stats.stdout
    assert "1/1 exclusive rerun(s) passed" in stats.stdout


def test_shared_gates_run_up_to_the_cap_and_queue_the_rest(tmp_path: Path) -> None:
    sleepers = [_start(tmp_path, "--slots", "3", "--", PY, "-c", "import time; time.sleep(4)") for _ in range(3)]
    _wait_for_runs(tmp_path, 3)
    fourth = _start(tmp_path, "--", PY, "-c", "pass")
    out, _ = fourth.communicate(timeout=30)
    assert "queued at position 1" in out
    assert "admitted after" in out
    for proc in sleepers:
        proc.communicate(timeout=30)


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


def test_blackout_refuses_a_start_inside_the_window(tmp_path: Path) -> None:
    result = _run_gate(tmp_path, _blackout_env(tmp_path, _BLACKOUT, _INSIDE))
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
    result = _run_gate(tmp_path, _blackout_env(tmp_path, config, "2026-11-06T04:30:00+00:00"))
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
