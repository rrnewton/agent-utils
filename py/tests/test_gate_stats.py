"""Daily gate success-rate report (#1246): fixture-record unit tests."""
from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from zoneinfo import ZoneInfo

ET = ZoneInfo("America/New_York")
SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "gate_stats.py"
_spec = importlib.util.spec_from_file_location("gate_stats_under_test", SCRIPT)
assert _spec and _spec.loader
gate_stats = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(gate_stats)


def _ts(day: str, hour: int) -> float:
    return datetime.fromisoformat(f"{day}T{hour:02d}:00:00").replace(tzinfo=ET).timestamp()


def _utc(day: str, hour: int) -> str:
    dt = datetime.fromisoformat(f"{day}T{hour:02d}:00:00").replace(tzinfo=ET)
    return dt.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def _hist(day="2026-11-09"):
    return [
        {"class": "shared", "priority": "owner", "queued_at": _ts(day, 9), "wait_s": 100.0, "run_s": 600.0, "exit_code": 0},
        {"class": "shared", "queued_at": _ts(day, 10), "wait_s": 300.0, "run_s": 900.0, "exit_code": 1},
        {"class": "exclusive", "queued_at": _ts(day, 11), "wait_s": 1800.0, "run_s": 3600.0, "exit_code": 0},
        {"class": "exclusive", "priority": "owner", "queued_at": _ts(day, 12), "wait_s": 60.0, "run_s": 1800.0, "exit_code": 0},
        # Previous ET day: excluded.
        {"class": "shared", "queued_at": _ts("2026-11-08", 23), "wait_s": 5.0, "run_s": 5.0, "exit_code": 0},
    ]


def _ledger(day="2026-11-09"):
    return [
        # Head A: first gate fails on two steps, solo rerun later passes -> flaky.
        {"kind": "gate", "head_sha": "aaa", "verdict": "fail", "failed_steps": ["s.one", "s.two"],
         "queued_utc": _utc(day, 9), "started_utc": _utc(day, 9), "finished_utc": _utc(day, 10)},
        {"kind": "exclusive-rerun", "head_sha": "aaa", "verdict": "pass", "rerun_passed": True, "failed_steps": [],
         "queued_utc": _utc(day, 11), "started_utc": _utc(day, 11), "finished_utc": _utc(day, 12)},
        # Head B: owner, green first try.
        {"kind": "gate", "head_sha": "bbb", "verdict": "pass", "priority": "owner", "failed_steps": [],
         "queued_utc": _utc(day, 10), "started_utc": _utc(day, 10), "finished_utc": _utc(day, 11)},
        # Head C: fails first, passes on a second full gate (not first-try, not flake).
        {"kind": "gate", "head_sha": "ccc", "verdict": "fail", "failed_steps": ["s.one"],
         "queued_utc": _utc(day, 12), "started_utc": _utc(day, 12), "finished_utc": _utc(day, 13)},
        {"kind": "gate", "head_sha": "ccc", "verdict": "pass", "failed_steps": [],
         "queued_utc": _utc(day, 14), "started_utc": _utc(day, 14), "finished_utc": _utc(day, 15)},
        # Aborted gate: excluded everywhere.
        {"kind": "gate", "head_sha": "ddd", "verdict": "aborted", "failed_steps": [],
         "queued_utc": _utc(day, 15), "started_utc": _utc(day, 15), "finished_utc": _utc(day, 16)},
    ]


def test_first_try_flake_and_red_rates():
    r = gate_stats.compute(_hist(), _ledger(), "2026-11-09")
    assert r["first_try"]["heads"] == 3
    assert r["first_try"]["first_try_pass"] == 1
    assert r["first_try"]["first_try_pass_rate"] == round(1 / 3, 4)
    by_tier = {row["tier"]: row for row in r["first_try"]["by_tier"]}
    assert by_tier["owner"]["first_try_pass_rate"] == 1.0
    assert by_tier["normal"]["heads"] == 2
    assert r["gates"]["full_gates"] == 4 and r["gates"]["fail"] == 2
    assert r["gates"]["red_gate_rate"] == 0.5
    # s.one failed twice, flaky once (head A only); s.two failed once, flaky once.
    assert r["flakes"]["failed_step_instances"] == 3
    assert r["flakes"]["flaky_step_instances"] == 2
    top = {row["step"]: row for row in r["flakes"]["top_flaky"]}
    assert top["s.two"]["flake_rate"] == 1.0 and top["s.one"]["flaky"] == 1


def test_splits_percentiles_and_host_hours():
    r = gate_stats.compute(_hist(), _ledger(), "2026-11-09")
    assert r["overall"]["tickets"] == 4
    assert r["overall"]["gates_per_hour"] == round(4 / 24, 3)
    assert r["overall"]["exclusive_host_hours"] == 1.5  # (3600+1800)/3600
    splits = {(s["tier"], s["class"]): s for s in r["splits"]}
    assert splits[("owner", "shared")]["tickets"] == 1
    assert splits[("normal", "shared")]["red_rate"] == 1.0
    assert splits[("normal", "exclusive")]["wait"]["p50_s"] == 1800.0
    assert splits[("owner", "exclusive")]["run"]["p90_s"] == 1800.0


def test_batch_column_ready_and_empty_day():
    r = gate_stats.compute(_hist(), _ledger(), "2026-11-09")
    assert r["gates"]["red_batch_rate"] is None and r["gates"]["batch_gates"] == 0
    empty = gate_stats.compute([], [], "2026-11-09")
    assert empty["overall"]["tickets"] == 0
    assert empty["first_try"]["first_try_pass_rate"] is None
    assert empty["flakes"]["top_flaky"] == []
    assert "—" in gate_stats.render_markdown(empty)


def test_cli_json_and_markdown(tmp_path: Path):
    hist, led = tmp_path / "h.jsonl", tmp_path / "l.jsonl"
    hist.write_text("\n".join(json.dumps(r) for r in _hist()) + "\n")
    led.write_text("\n".join(json.dumps(r) for r in _ledger()) + "\n")
    out = subprocess.run([sys.executable, str(SCRIPT), "--day", "2026-11-09", "--json",
                          "--history", str(hist), "--ledger", str(led)],
                         capture_output=True, text=True)
    assert out.returncode == 0, out.stderr
    assert json.loads(out.stdout)["day"] == "2026-11-09"
    md = subprocess.run([sys.executable, str(SCRIPT), "--day", "2026-11-09", "--markdown",
                         "--history", str(hist), "--ledger", str(led)],
                        capture_output=True, text=True)
    assert md.returncode == 0, md.stderr
    assert "Top flaky steps" in md.stdout and "Red-batch rate" in md.stdout
