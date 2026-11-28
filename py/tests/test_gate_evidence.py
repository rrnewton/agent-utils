"""Red-step evidence (#2409 gate-red-step-logs-kept): a red gate or rerun row's
failed and NO_RESULT step logs are copied out of the gate-log pruning, capped,
bounded by age and size, named by the row's evidence_path, and never change a
verdict. No real gate runs here: the gated command is a stand-in that writes a
dagrun-shaped log directory, and the bounds are exercised in-process."""
from __future__ import annotations

import importlib.util
import json
import os
import stat
import subprocess
import sys
import textwrap
from pathlib import Path
from types import ModuleType

import pytest

REPO = Path(__file__).resolve().parents[2]
GATE = REPO / "scripts" / "gate.py"
PY = sys.executable
DAY = 86400.0


def _gate_module() -> ModuleType:
    spec = importlib.util.spec_from_file_location("gate_evidence_under_test", GATE)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module  # dataclasses look their module up there
    spec.loader.exec_module(module)
    return module


@pytest.fixture(autouse=True)
def _hermetic_roots(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Every root the evidence code reads points into tmp_path."""
    monkeypatch.setenv("GATE_LOG_ROOT", str(tmp_path / "gate-logs"))
    monkeypatch.delenv("GATE_EVIDENCE_ROOT", raising=False)
    for name in ("GATE_EVIDENCE_MAX_BYTES", "GATE_EVIDENCE_MAX_AGE_DAYS",
                 "GATE_EVIDENCE_LOG_CAP_BYTES", "GATE_LOG_KEEP"):
        monkeypatch.delenv(name, raising=False)


def _env(tmp_path: Path, **extra: str) -> dict[str, str]:
    """A gate.py subprocess environment that reads no host gate config."""
    base = {key: value for key, value in os.environ.items()
            if not key.startswith(("GATE_", "MEMORY_FLOOR_"))}
    (tmp_path / "meminfo").write_text(
        "MemTotal:       209715200 kB\nMemAvailable:   199229440 kB\n")
    return {**base, "GATE_ADMISSION_DIR": str(tmp_path / "admission"),
            "GATE_LEDGER": str(tmp_path / "ledger.jsonl"), "GATE_PRUNE_TARGETS": "off",
            "GATE_SLOTS_CONFIG": str(tmp_path / "gate-slots.toml"),
            "GATE_INTERLEAVE_CONFIG": str(tmp_path / "gate-interleave.toml"),
            "GATE_PHASE_PRIORITY_CONFIG": str(tmp_path / "gate-phase-priority.toml"),
            "GATE_BLACKOUT_CONFIG": str(tmp_path / "gate.toml"),
            "GATE_DEFER_CONFIG": str(tmp_path / "gate-defer.toml"),
            "MEMORY_FLOOR_CONFIG": str(tmp_path / "memory-floor.toml"),
            "MEMORY_FLOOR_MEMINFO": str(tmp_path / "meminfo"),
            "GATE_RERUN_PROMOTE_CONFIG": str(tmp_path / "rerun-promote.toml"),
            "GATE_LOG_ROOT": str(tmp_path / "gate-logs"),
            "GATE_KNOWN_RED": str(tmp_path / "known-red.toml"),
            "GATE_DEFER_SHARED_HOLDS_DIR": str(tmp_path / "no-shared-holds"),
            "GATE_DEFER_HOLD_REPORT_STATE": str(tmp_path / "hold-reports.json"),
            "GATE_DEFER_POLL_S": "0.05", "GATE_SLOTS": "2",
            "GATE_CPU_CONTAINMENT": "off", "GATE_PREFLIGHT_COMMAND": "true", **extra}


#: Stand-in for dagrun: fills $DAGRUN_LOG_DIR the way dagrun does
#: (0700 directory, <step>.log files, journal.jsonl) and prints dagrun's
#: step result lines. argv: exit code, optionally --selected STEPS, then
#: step=RESULT pairs.
FAKE_DAGRUN = textwrap.dedent("""\
    import json, os, sys
    def dumps(record):  # compact, as dagrun writes its journal
        return json.dumps(record, separators=(",", ":"))
    log_dir = os.environ["DAGRUN_LOG_DIR"]
    os.mkdir(log_dir, 0o700)
    args = sys.argv[2:]
    if args[:1] == ["--selected"]:  # a --rerun names its steps; the pairs below say how they end
        args = args[2:]
    code, pairs = int(sys.argv[1]), [arg.split("=", 1) for arg in args]
    with open(os.path.join(log_dir, "journal.jsonl"), "w") as journal:
        journal.write(dumps({"event": "containment", "state": "run-boxed"}) + "\\n")
        for step, result in pairs:
            with open(os.path.join(log_dir, step + ".log"), "w") as log:
                log.write(f"output of {step}\\n")
            journal.write(dumps({"event": "step_start", "step": step}) + "\\n")
            journal.write(dumps({"event": "test_start", "step": step, "test": "t1"}) + "\\n")
            journal.write(dumps({"event": "test_end", "step": step, "test": "t1",
                                 "verdict": result}) + "\\n")
            journal.write(dumps({"event": "step_end", "step": step,
                                 "ok": result == "PASS"}) + "\\n")
            glyph = {"PASS": "✓ PASS", "FAIL": "✗ FAIL", "NO_RESULT": "◌ NO_RESULT"}[result]
            print(f"[{step}] {glyph}   {step} (1s)")
    print("dagrun: " + ("OK" if code == 0 else "FAILED"))
    sys.exit(code)
""")


def _gate(tmp_path: Path, *gate_args: str, code: int, steps: dict[str, str],
          **extra: str) -> tuple[subprocess.CompletedProcess[str], dict[str, object]]:
    script = tmp_path / "fake_dagrun.py"
    script.write_text(FAKE_DAGRUN)
    selected = ["--selected", ",".join(steps)] if "--rerun" in gate_args else []
    result = subprocess.run(
        [PY, str(GATE), *gate_args, "--", PY, str(script), str(code), *selected,
         *(f"{step}={verdict}" for step, verdict in steps.items())],
        env=_env(tmp_path, **extra), capture_output=True, text=True, timeout=60)
    rows = [json.loads(line) for line in (tmp_path / "ledger.jsonl").read_text().splitlines()]
    return result, rows[-1]


def _log_dir(root: Path, run_id: str, logs: dict[str, bytes], journal: list[dict[str, object]]) -> Path:
    directory = root / run_id
    directory.mkdir(parents=True, mode=0o700)
    for step, body in logs.items():
        (directory / f"{step}.log").write_bytes(body)
    (directory / "journal.jsonl").write_text("".join(json.dumps(r) + "\n" for r in journal))
    return directory


def _root_bytes(gate: ModuleType, root: Path) -> int:
    runs = root / "runs"
    return sum(gate._entry_bytes(entry) for entry in runs.iterdir()) if runs.is_dir() else 0


def test_copy_capped_keeps_head_and_tail_and_marks_the_cut(tmp_path: Path) -> None:
    gate = _gate_module()
    cap = 2 * 1024 * 1024
    body = b"HEAD-START\n" + os.urandom(3 * cap).hex().encode()[: 4 * cap] + b"\nTAIL-END\n"
    src = tmp_path / "big.log"
    src.write_bytes(body)
    fd = os.open(src, os.O_RDONLY)
    try:
        written, cut = gate.copy_capped(fd, len(body), tmp_path / "out.log", cap)
    finally:
        os.close(fd)
    out = (tmp_path / "out.log").read_bytes()
    assert cut is True and written == len(out) <= cap
    assert len(out) > cap - 1024  # about the cap, not far under it
    assert out.startswith(b"HEAD-START\n") and out.endswith(b"\nTAIL-END\n")
    assert b"[gate] EVIDENCE CUT (#2409):" in out
    assert f"of {len(body)} bytes omitted".encode() in out
    assert stat.S_IMODE((tmp_path / "out.log").stat().st_mode) == 0o600
    # A log under the cap is copied byte for byte, with no marker.
    small = tmp_path / "small.log"
    small.write_bytes(b"short log\n")
    fd = os.open(small, os.O_RDONLY)
    try:
        assert gate.copy_capped(fd, 10, tmp_path / "small-out.log", cap) == (10, False)
    finally:
        os.close(fd)
    assert (tmp_path / "small-out.log").read_bytes() == b"short log\n"


@pytest.mark.parametrize("compact", [True, False], ids=["dagrun-compact", "spaced"])
def test_journal_copy_keeps_red_steps_and_run_events_as_valid_jsonl(tmp_path: Path, compact: bool) -> None:
    """dagrun writes compact JSON, which the copy skims without decoding;
    any other spelling is decoded, with the same selection."""
    gate = _gate_module()
    records: list[dict[str, object]] = [
        {"event": "containment", "state": "run-boxed"},
        {"event": "step_start", "step": "red.a", "cmd": 'echo \'"step":"green.b"\''},
        {"event": "test_start", "step": "green.b", "test": "x"},
        {"event": "test_start", "step": "red.a", "test": "t1"},
        {"event": "run_timeout", "cut_steps": ["red.a"]},
    ]
    records += [{"event": "test_end", "step": "red.a", "test": f"t{i}", "verdict": "ok"}
                for i in range(2000)]
    separators = (",", ":") if compact else (", ", ": ")
    path = tmp_path / "journal.jsonl"
    path.write_text("".join(json.dumps(r, separators=separators) + "\n" for r in records)
                    + "not json\n")
    fd = os.open(path, os.O_RDONLY)
    try:
        chunks, info = gate._journal_selection(fd, path.stat().st_size, {"red.a"}, 1 << 20)
        assert info == {"lines": 2003, "kept_lines": 2003, "truncated": False, "unparsed_lines": 1}
        kept = [json.loads(line) for line in b"".join(chunks).splitlines()]
        assert {r.get("step") for r in kept} == {"red.a", None}
        assert all(r["event"] != "containment" for r in kept)
        assert kept[0]["event"] == "step_start" and kept[2]["event"] == "run_timeout"
        # Past the cap: whole lines, head and tail, one cut record between them.
        chunks, info = gate._journal_selection(fd, path.stat().st_size, {"red.a"}, 8192)
    finally:
        os.close(fd)
    data = b"".join(chunks)
    assert len(data) <= 8192 and info["truncated"] is True
    kept = [json.loads(line) for line in data.splitlines()]
    cut = [r for r in kept if r["event"] == "gate_evidence_cut"]
    assert len(cut) == 1 and cut[0]["omitted_lines"] == 2003 - (len(kept) - 1)
    assert kept[0]["event"] == "step_start" and kept[-1]["test"] == "t1999"


def test_evidence_root_drops_entries_older_than_thirty_days(tmp_path: Path) -> None:
    gate = _gate_module()
    root = tmp_path / "gate-evidence"
    now = 1_800_000_000.0
    for name, age_days in (("old", 31), ("edge", 29), ("new", 1)):
        entry = root / "runs" / name
        entry.mkdir(parents=True)
        (entry / "x.log").write_bytes(b"x" * 100)
        os.utime(entry, (now - age_days * DAY, now - age_days * DAY))
    assert gate.prune_gate_evidence(root=root, now=now) == ["old"]
    assert sorted(p.name for p in (root / "runs").iterdir()) == ["edge", "new"]


def test_evidence_root_drops_the_oldest_until_under_its_size_bound(tmp_path: Path) -> None:
    gate = _gate_module()
    root = tmp_path / "gate-evidence"
    now = 1_800_000_000.0
    for i in range(6):
        entry = root / "runs" / f"run-{i}"
        entry.mkdir(parents=True)
        (entry / "x.log").write_bytes(b"x" * 40_000)
        os.utime(entry, (now - (10 - i) * 3600, now - (10 - i) * 3600))
    each = gate._entry_bytes(root / "runs" / "run-0")
    removed = gate.prune_gate_evidence(root=root, max_bytes=3 * each + 1, now=now)
    assert removed == ["run-0", "run-1", "run-2"]
    assert _root_bytes(gate, root) <= 3 * each + 1
    # `room` makes space for an entry about to be written.
    assert gate.prune_gate_evidence(root=root, max_bytes=3 * each + 1, room=each, now=now) == ["run-3"]


def test_repeated_red_runs_never_push_the_root_past_its_bound(
        tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Both bounds hold run after run, and an entry never exceeds its
    share of the bound: a run with many red steps keeps them all, each
    with a smaller head and tail."""
    gate = _gate_module()
    max_bytes = 600_000
    monkeypatch.setenv("GATE_EVIDENCE_MAX_BYTES", str(max_bytes))
    logs_root = tmp_path / "gate-logs"
    root = gate.gate_evidence_root()
    assert root == tmp_path / "gate-evidence"
    for i in range(12):
        steps = {f"s.{k}": bytes([65 + k % 26]) * 90_000 for k in range(3 if i % 4 else 40)}
        log_dir = _log_dir(logs_root, f"run-{i:02d}", steps,
                           [{"event": "step_end", "step": s, "ok": False} for s in steps])
        path = gate.keep_red_evidence(f"run-{i:02d}", log_dir, set(steps), set())
        assert path == str(root / "runs" / f"run-{i:02d}")
        entry = Path(path)
        assert gate._entry_bytes(entry) <= max_bytes // gate.GATE_EVIDENCE_ENTRY_SHARE + 64 * 1024
        assert sorted(p.name for p in entry.iterdir() if p.suffix == ".log") == sorted(
            f"{s}.log" for s in steps)
        assert _root_bytes(gate, root) <= max_bytes, i
    assert not (root / "runs" / "run-00").exists()  # the oldest went first


def test_a_failed_copy_is_reported_and_returns_no_path(
        tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]) -> None:
    gate = _gate_module()
    log_dir = _log_dir(tmp_path / "gate-logs", "run-x", {"a.b": b"boom\n"}, [])

    def explode(*_args: object) -> tuple[Path | None, str]:
        raise OSError(28, "No space left on device")

    with monkeypatch.context() as patched:
        patched.setattr(gate, "_copy_red_evidence", explode)
        assert gate.keep_red_evidence("run-x", log_dir, {"a.b"}, set()) is None
    assert "red-step evidence not kept: OSError" in capsys.readouterr().out
    # Nothing to copy at all is reported too, never raised.
    assert gate.keep_red_evidence("run-y", tmp_path / "absent", {"a.b"}, set()) is None
    assert "no step log or journal" in capsys.readouterr().out


def test_red_gate_and_rerun_rows_name_their_kept_evidence(tmp_path: Path) -> None:
    """End to end through gate.py with a stand-in dagrun: failed and
    NO_RESULT steps are kept with their test-boundary journal lines; a
    passing step is not; a green row's evidence_path is null."""
    result, row = _gate(tmp_path, code=3, steps={
        "red.fail": "FAIL", "red.noresult": "NO_RESULT", "green.ok": "PASS"})
    assert result.returncode == 3, result.stdout
    assert row["failed_steps"] == ["red.fail"] and row["incomplete_steps"] == ["red.noresult"]
    evidence = Path(str(row["evidence_path"]))
    assert evidence == tmp_path / "gate-evidence" / "runs" / Path(str(row["log_path"])).name
    assert "red-step evidence kept in" in result.stdout
    assert (evidence / "red.fail.log").read_text() == "output of red.fail\n"
    assert (evidence / "red.noresult.log").read_text() == "output of red.noresult\n"
    assert not (evidence / "green.ok.log").exists()
    journal = [json.loads(line) for line in (evidence / "journal.jsonl").read_text().splitlines()]
    assert {r["step"] for r in journal} == {"red.fail", "red.noresult"}
    assert {r["event"] for r in journal} == {"step_start", "test_start", "test_end", "step_end"}
    manifest = json.loads((evidence / "manifest.json").read_text())
    assert [(s["step"], s["result"], s["log"]) for s in manifest["steps"]] == [
        ("red.fail", "failed", "red.fail.log"), ("red.noresult", "incomplete", "red.noresult.log")]
    assert stat.S_IMODE(evidence.stat().st_mode) == 0o700

    result, row = _gate(tmp_path, "--rerun", code=3, steps={"red.fail": "FAIL", "red.nr": "NO_RESULT"})
    assert result.returncode == 3, result.stdout
    assert row["kind"] == "exclusive-rerun" and row["rerun"] is True and row["evidence_path"]
    assert row["incomplete_steps"] == ["red.nr"]
    assert sorted(p.name for p in Path(str(row["evidence_path"])).iterdir()) == [
        "journal.jsonl", "manifest.json", "red.fail.log", "red.nr.log"]

    result, row = _gate(tmp_path, code=0, steps={"green.ok": "PASS"})
    assert result.returncode == 0 and row["verdict"] == "pass"
    assert "evidence_path" in row and row["evidence_path"] is None


def test_a_copy_failure_leaves_the_verdict_and_exit_status_unchanged(tmp_path: Path) -> None:
    blocker = tmp_path / "not-a-directory"
    blocker.write_text("a file where the evidence root's parent should be\n")
    result, row = _gate(tmp_path, code=3, steps={"red.fail": "FAIL"},
                        GATE_EVIDENCE_ROOT=str(blocker / "gate-evidence"))
    assert result.returncode == 3, result.stdout
    assert row["verdict"] == "fail" and row["exit_code"] == 3
    assert row["failed_steps"] == ["red.fail"] and row["evidence_path"] is None
    assert "red-step evidence not kept" in result.stdout
    # The same failure on a passing run changes nothing either.
    result, row = _gate(tmp_path, code=0, steps={"green.ok": "PASS"},
                        GATE_EVIDENCE_ROOT=str(blocker / "gate-evidence"))
    assert result.returncode == 0 and row["verdict"] == "pass" and row["evidence_path"] is None


def test_kept_evidence_outlives_twenty_five_later_gates(
        tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """The acceptance case: after 25 later gates the red run's own log
    directory is pruned (#2078 keeps 20), its evidence is not."""
    gate = _gate_module()
    logs_root = tmp_path / "gate-logs"
    red_dir = _log_dir(logs_root, "run-000", {"red.step": b"the failure\n"},
                       [{"event": "step_end", "step": "red.step", "ok": False}])
    os.utime(red_dir, (1000, 1000))
    kept = gate.keep_red_evidence("run-000", red_dir, {"red.step"}, set())
    assert kept
    for i in range(1, 26):
        later = _log_dir(logs_root, f"run-{i:03d}", {"ok.step": b"fine\n"}, [])
        os.utime(later, (1000 + i, 1000 + i))
        assert gate.keep_red_evidence(f"run-{i:03d}", later, set(), set()) is None
        gate.prune_gate_logs()
    assert not red_dir.exists()
    assert (Path(kept) / "red.step.log").read_bytes() == b"the failure\n"
    # One end-to-end gate on top: its prune leaves the copy alone too.
    _gate(tmp_path, code=0, steps={"ok.step": "PASS"})
    assert (Path(kept) / "red.step.log").is_file()


def test_rows_without_evidence_path_still_read(tmp_path: Path) -> None:
    """Older rows have no evidence_path; `gate.py stats` reads both kinds."""
    ledger = tmp_path / "ledger.jsonl"
    old = {"kind": "gate", "verdict": "fail", "failed_steps": ["a.b"],
           "finished_utc": "2026-11-27T10:00:00Z", "exit_code": 3}
    new = {**old, "evidence_path": str(tmp_path / "gate-evidence" / "runs" / "r1")}
    ledger.write_text(json.dumps(old) + "\n" + json.dumps(new) + "\n")
    result = subprocess.run([PY, str(GATE), "stats", "--ledger", str(ledger)],
                            env=_env(tmp_path), capture_output=True, text=True, timeout=60)
    assert result.returncode == 0, result.stdout + result.stderr
    assert "a.b" in result.stdout


def test_log_names_match_dagruns_encoding() -> None:
    """The copy finds dagrun's step logs only if both encode a tag alike."""
    sys.path.insert(0, str(REPO / "py"))
    try:
        from dagrun.attribution import sanitize
    finally:
        sys.path.remove(str(REPO / "py"))
    gate = _gate_module()
    for tag in ("vibe-talk.check.one-page", "a b/c", "naïve:step", "x~y", "._-"):
        assert gate.dagrun_log_name(tag) == sanitize(tag) + ".log"
