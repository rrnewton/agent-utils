"""Gate admission: the counting cap, the exclusive class, FIFO, and reaping."""
from __future__ import annotations

import fcntl
import importlib.util
import json
import os
import shutil
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
            "GATE_LEDGER": str(tmp_path / "ledger.jsonl"), "GATE_PRUNE_TARGETS": "off"}


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
    out = subprocess.run(
        ["cargo", *args, "--offline", "--message-format=json", "--manifest-path", str(ws / "Cargo.toml"),
         "--target-dir", str(target)],
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
