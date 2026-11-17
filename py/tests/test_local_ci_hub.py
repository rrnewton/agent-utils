"""local-ci-hub subcommand tests: ledger, receipt, run, health, remote, land."""
from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

BIN = Path(__file__).resolve().parents[2] / "py" / "bin" / "local-ci-hub"


def run_cli(cwd: Path, *args: str) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    # Isolate the generic defer surface: no host hold or quiet signal
    # may leak into these tests from the machine running them.
    env["GATE_DEFER_CONFIG"] = str(cwd / "no-defer.toml")
    env["GATE_DEFER_HOLDS_DIR"] = ""
    env["GATE_DEFER_SHARED_HOLDS_DIR"] = ""
    env["GATE_DEFER_QUIET_COMMAND"] = ""
    host_file = cwd / "host-limits.toml"
    if host_file.exists():
        env["AGENT_UTILS_HOST_LIMITS_CONFIG"] = str(host_file)
        env["AGENT_UTILS_HOST_LIMITS_STAT_A"] = str(cwd / "stat-a.txt")
        env["AGENT_UTILS_HOST_LIMITS_STAT_B"] = str(cwd / "stat-b.txt")
    return subprocess.run([sys.executable, str(BIN), *args], cwd=cwd, capture_output=True, text=True, timeout=60, env=env)


def toy_repo(tmp_path: Path, validate: str = "true") -> Path:
    repo = tmp_path / "toy"
    repo.mkdir()
    subprocess.run(["git", "init", "-q", "-b", "main", "."], cwd=repo, timeout=15)
    subprocess.run(["git", "config", "user.email", "t@example.com"], cwd=repo, timeout=15)
    subprocess.run(["git", "config", "user.name", "t"], cwd=repo, timeout=15)
    (repo / "README.md").write_text("# toy\n")
    (repo / "local-ci-hub.toml").write_text(
        '[project]\nname="toy"\n[validate]\ncommand=["/bin/sh","-c","' + validate + '"]\n'
        'admission_slots=2\n[ledger]\nteam="toy"\npublish="none"\nlocation={path="ci-ledger"}\n'
        '[remote]\nprovider="none"\n[receipt]\nqualifying=["local-ledger-pass"]\nrequire_full_evidence=false\n[land]\nstate_dir="ignored/local-ci-hub"\n')
    state_root = repo / "host-state"
    (repo / "host-limits.toml").write_text(
        'version = 1\n[admission]\nmax_concurrent_validation_runs = 4\n'
        'cleanup_pending_max = 6\nslot_fs_min_free_bytes = 1\n'
        '[run_scope]\ncpu_quota_cpus = 2\nmemory_max_bytes = 1048576\npids_max = 1024\n'
        f'[paths]\nstate_root = "{state_root}"\n')
    zero = "\n".join(["cpu  0 0 0 0 0 0 0 0 0 0"] + [f"cpu{i} 0 0 0 0 0 0 0 0 0 0" for i in range(4)]) + "\n"
    idle = "\n".join(["cpu  0 0 0 100 0 0 0 0 0 0"] + [f"cpu{i} 0 0 0 100 0 0 0 0 0 0" for i in range(4)]) + "\n"
    (repo / "stat-a.txt").write_text(zero)
    (repo / "stat-b.txt").write_text(idle)
    subprocess.run(["git", "add", "-A"], cwd=repo, timeout=15)
    subprocess.run(["git", "commit", "-qm", "toy: init"], cwd=repo, timeout=15)
    return repo


def test_help_and_quickstart(tmp_path: Path) -> None:
    r = run_cli(tmp_path, "--help")
    assert r.returncode == 0 and "receipt" in r.stdout
    r = run_cli(tmp_path, "quickstart")
    assert r.returncode == 0 and "receipt show" in r.stdout


def test_ledger_append_publish_query(tmp_path: Path) -> None:
    repo = toy_repo(tmp_path)
    sha = subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo, capture_output=True, text=True).stdout.strip()
    assert run_cli(repo, "--json", "ledger", "append", "--commit", sha, "--result", "PASS").returncode == 0
    assert run_cli(repo, "ledger", "publish").returncode == 0
    out = run_cli(repo, "--json", "ledger", "query", "--commit", sha)
    assert json.loads(out.stdout)[0]["result"] == "PASS"
    out = run_cli(repo, "--json", "ledger", "newest-green")
    assert json.loads(out.stdout)["commit"] == sha


def test_receipt_qualified_and_no_result(tmp_path: Path) -> None:
    repo = toy_repo(tmp_path)
    assert run_cli(repo, "receipt", "show", "deadbeef").returncode == 1  # no-result, not qualified
    assert run_cli(repo, "run", "start").returncode == 0
    out = run_cli(repo, "--json", "receipt", "show", "HEAD")
    assert out.returncode == 0 and json.loads(out.stdout)["verdict"] == "qualified"


def test_run_start_honours_commit_and_cell(tmp_path: Path) -> None:
    repo = toy_repo(tmp_path)
    fake = "a" * 40
    r = run_cli(repo, "run", "start", "--commit", fake, "--cell", "cell-a")
    assert r.returncode == 0, r.stdout + r.stderr
    out = run_cli(repo, "--json", "ledger", "query", "--commit", fake)
    rows = json.loads(out.stdout)
    assert rows and rows[0]["commit"] == fake and rows[0].get("cell") == "cell-a"


def test_run_failure_records_fail(tmp_path: Path) -> None:
    repo = toy_repo(tmp_path, validate="false")
    assert run_cli(repo, "run", "start").returncode == 1  # a FAIL run exits nonzero
    out = run_cli(repo, "--json", "receipt", "show", "HEAD")
    assert json.loads(out.stdout)["verdict"] == "not-qualified"
    bad = run_cli(repo, "--json", "ledger", "first-bad")
    assert json.loads(bad.stdout).get("result") in ("FAIL", "none")


def test_health_remote_land_doctor(tmp_path: Path) -> None:
    repo = toy_repo(tmp_path)
    assert json.loads(run_cli(repo, "--json", "health").stdout)["status"] in ("pass", "fail")
    remote = run_cli(repo, "--json", "remote", "status")
    assert json.loads(remote.stdout)["status"] == "disabled"  # local-only needs no network
    assert run_cli(repo, "doctor").returncode == 0
    assert run_cli(repo, "land", "acquire", "--holder", "a").returncode == 0
    assert run_cli(repo, "land", "acquire", "--holder", "b").returncode == 1
    assert run_cli(repo, "land", "release", "--holder", "a").returncode == 0
    sha = subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo, capture_output=True, text=True).stdout.strip()
    assert run_cli(repo, "land", "verify", "--commit", sha).returncode == 0


def test_goalpost_prefix_and_enrich_cannot_qualify(tmp_path: Path) -> None:
    repo = toy_repo(tmp_path)
    sha = subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo, capture_output=True, text=True).stdout.strip()
    # a 1-char prefix must never bind evidence to a commit
    assert run_cli(repo, "--json", "ledger", "append", "--commit", sha, "--result", "PASS").returncode == 0
    run_cli(repo, "ledger", "publish")
    out = run_cli(repo, "--json", "receipt", "show", sha[:1])
    assert json.loads(out.stdout)["verdict"] != "qualified"


def _row_ref_repo(tmp_path: Path) -> tuple[Path, Path]:
    repo = tmp_path / "toy"
    repo.mkdir()
    subprocess.run(["git", "init", "-q", "-b", "main", "."], cwd=repo, timeout=15)
    subprocess.run(["git", "config", "user.email", "t@example.com"], cwd=repo, timeout=15)
    subprocess.run(["git", "config", "user.name", "t"], cwd=repo, timeout=15)
    (repo / "README.md").write_text("# toy\n")
    evidence = tmp_path / "evidence"
    evidence.mkdir()
    (repo / "local-ci-hub.toml").write_text(
        '[project]\nname="toy"\n[validate]\ncommand=["/bin/true"]\n'
        'admission_slots=2\n[ledger]\nteam="toy"\npublish="none"\ncounted_repos=["primary","owner/primary"]\n'
        f'evidence_root="{evidence}"\nlocation={{path="ci-ledger"}}\n'
        '[remote]\nprovider="none"\n[receipt]\nqualifying=["local-ledger-pass"]\nrequire_full_evidence=true\n'
        '[receipt.release_payload]\npath="target/validate/primary"\nprofile="validate"\ndebug_assertions=true\noverflow_checks=true\n'
        '[land]\nstate_dir="ignored/local-ci-hub"\n')
    subprocess.run(["git", "add", "-A"], cwd=repo, timeout=15)
    subprocess.run(["git", "commit", "-qm", "toy: init"], cwd=repo, timeout=15)
    return repo, evidence


def _full_row(sha: str, run_id: str, *, result: str = "pass", profile: str = "full",
              finished_at: str = "2026-10-30T17:37:20Z") -> dict[str, object]:
    return {"commit": sha, "run_id": run_id, "result": result, "commit_anchored": True,
            "executed_tests": 10, "passed_tests": 10 if result == "pass" else 9,
            "profile": profile, "selection_mode": "full" if profile == "full" else "only",
            "tree_dirty": False, "failures": 0 if result == "pass" else 1,
            "gates_run": 1, "gates_expected": 1, "finished_at": finished_at,
            "coverage": {"planned_test_nodes": 1, "zero_executed_nodes": [], "absent_nodes": []}}


def _row_ref_event(evidence: Path, row: dict[str, object], *, event_id: str,
                   outcome: str | None = None, host: str = "testhost") -> dict[str, object]:
    import hashlib
    raw = json.dumps(row, sort_keys=True).encode()
    sha256 = hashlib.sha256(raw).hexdigest()
    locator = f"run-evidence/{host}/{sha256}.json.zst"
    dest = evidence / locator
    dest.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(["zstd", "-q", "-f", "-o", str(dest)], input=raw, check=True, timeout=30)
    blob = dest.read_bytes()
    codec = {"codec": "row-ref/v1", "locator": locator, "sha256": sha256, "bytes": len(raw),
             "compressed_sha256": hashlib.sha256(blob).hexdigest(), "compressed_bytes": len(blob)}
    return {"schema": "validate-ledger/v1", "event_type": "run.result", "event_id": event_id,
            "run_id": str(row["run_id"]), "commit": str(row["commit"]), "host": host,
            "outcome": outcome or str(row["result"]), "legacy_row_codec": codec}


def _write_ledger(repo: Path, events: list[dict[str, object]]) -> None:
    led = repo / "ci-ledger"
    led.mkdir(exist_ok=True)
    (led / "ledger.jsonl").write_text("".join(json.dumps(e) + "\n" for e in events))


def test_row_ref_valid_evidence_qualifies(tmp_path: Path) -> None:
    repo, evidence = _row_ref_repo(tmp_path)
    sha = subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo, capture_output=True, text=True).stdout.strip()
    _write_ledger(repo, [_row_ref_event(evidence, _full_row(sha, "r1"), event_id="e1")])
    out = run_cli(repo, "--json", "receipt", "show", "HEAD")
    assert json.loads(out.stdout)["verdict"] == "qualified"


def test_row_ref_mismatched_evidence_refuses_sticky(tmp_path: Path) -> None:
    repo, evidence = _row_ref_repo(tmp_path)
    sha = subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo, capture_output=True, text=True).stdout.strip()
    good = _row_ref_event(evidence, _full_row(sha, "r1"), event_id="e1")
    # Tamper: locator no longer names the row hash.
    bad = json.loads(json.dumps(good))
    bad["legacy_row_codec"] = dict(bad["legacy_row_codec"], locator="run-evidence/testhost/" + "0" * 64 + ".json.zst")
    _write_ledger(repo, [bad])
    out = run_cli(repo, "--json", "receipt", "show", "HEAD")
    assert json.loads(out.stdout)["verdict"] != "qualified"
    # Commit mismatch in the referenced row also refuses.
    other = _full_row("0" * 40, "r1")
    _write_ledger(repo, [_row_ref_event(evidence, other, event_id="e2")])
    out = run_cli(repo, "--json", "receipt", "show", "HEAD")
    assert json.loads(out.stdout)["verdict"] != "qualified"
    # Sticky: a refused row-ref for a run is not revived by a later inline PASS for the same run.
    refused = _row_ref_event(evidence, _full_row(sha, "r1"), event_id="e3")
    codec = refused["legacy_row_codec"]
    assert isinstance(codec, dict)
    refused["legacy_row_codec"] = {**codec, "compressed_sha256": "0" * 64}
    inline_pass: dict[str, object] = {"schema": "validate-ledger/v1", "event_type": "run.result", "event_id": "e4",
                   "run_id": "r1", "commit": sha, "outcome": "pass", "legacy_row": _full_row(sha, "r1")}
    _write_ledger(repo, [refused, inline_pass])
    out = run_cli(repo, "--json", "receipt", "show", "HEAD")
    assert json.loads(out.stdout)["verdict"] != "qualified"


def test_row_ref_partial_fail_does_not_veto_full_pass(tmp_path: Path) -> None:
    # Fixture of the real e3120d83 shape: a full PASS plus a later
    # only-portable FAIL (and only-portable PASS) at the same commit.
    repo, evidence = _row_ref_repo(tmp_path)
    sha = subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo, capture_output=True, text=True).stdout.strip()
    events = [
        _row_ref_event(evidence, _full_row(sha, "r-full", finished_at="2026-10-30T17:37:20Z"), event_id="e1"),
        _row_ref_event(evidence, _full_row(sha, "r-partial-fail", result="fail", profile="only-portable",
                                           finished_at="2026-11-01T13:01:45Z"), event_id="e2", outcome="fail"),
        _row_ref_event(evidence, _full_row(sha, "r-partial-pass", profile="only-portable",
                                           finished_at="2026-11-01T14:02:16Z"), event_id="e3"),
    ]
    _write_ledger(repo, events)
    out = run_cli(repo, "--json", "receipt", "show", "HEAD")
    assert json.loads(out.stdout)["verdict"] == "qualified"
    # A genuine clean-full FAIL at least as new as the PASS still vetoes.
    events.append(_row_ref_event(evidence, _full_row(sha, "r-full-fail", result="fail",
                                                     finished_at="2026-11-02T00:00:00Z"), event_id="e4", outcome="fail"))
    _write_ledger(repo, events)
    out = run_cli(repo, "--json", "receipt", "show", "HEAD")
    assert json.loads(out.stdout)["verdict"] != "qualified"


def test_row_ref_missing_evidence_refuses(tmp_path: Path) -> None:
    repo = toy_repo(tmp_path)
    sha = subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo, capture_output=True, text=True).stdout.strip()
    led = repo / "ci-ledger"; led.mkdir(exist_ok=True)
    ev = {"schema": "validate-ledger/v1", "event_type": "run.result", "event_id": "e1",
          "run_id": "r1", "commit": sha, "outcome": "pass", "legacy_row": {},
          "legacy_row_codec": {"codec": "row-ref/v1", "locator": "run-evidence/x/y.json.zst",
                               "sha256": "0" * 64, "bytes": 2, "compressed_sha256": "0" * 64}}
    (led / "ledger.jsonl").write_text(json.dumps(ev) + "\n")
    out = run_cli(repo, "--json", "receipt", "show", "HEAD")
    assert json.loads(out.stdout)["verdict"] != "qualified"  # missing evidence refuses, never qualifies


def test_superseded_release_builder_does_not_qualify(tmp_path: Path) -> None:
    # A PASS carrying a superseded payload must not qualify: only the
    # configured current payload names a builder.
    repo, evidence = _row_ref_repo(tmp_path)
    sha = subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo, capture_output=True, text=True).stdout.strip()
    superseded = dict(_full_row(sha, "r1", finished_at="2026-09-30T00:00:00Z"),
                      release_builder="builder-a",
                      e2e_payload={"path": "target/debug/primary", "profile": "debug",
                                   "debug_assertions": True, "overflow_checks": True})
    _write_ledger(repo, [_row_ref_event(evidence, superseded, event_id="e1")])
    out = run_cli(repo, "--json", "receipt", "show", "HEAD")
    assert json.loads(out.stdout)["verdict"] != "qualified"
    # The configured current payload still qualifies (the refusal is the superseded payload, not the fields).
    current = dict(superseded, run_id="r2",
                   e2e_payload={"path": "target/validate/primary", "profile": "validate",
                                "debug_assertions": True, "overflow_checks": True})
    _write_ledger(repo, [_row_ref_event(evidence, current, event_id="e2")])
    out = run_cli(repo, "--json", "receipt", "show", "HEAD")
    assert json.loads(out.stdout)["verdict"] == "qualified"


def test_identity_claim_without_a_configured_payload_does_not_qualify(tmp_path: Path) -> None:
    # With no current payload configured, a builder claim cannot be checked, so it is not
    # honoured; a row that makes no identity claim still qualifies.
    repo, evidence = _row_ref_repo(tmp_path)
    config = repo / "local-ci-hub.toml"
    text = config.read_text()
    config.write_text(text[:text.index("[receipt.release_payload]")] + text[text.index("[land]"):])
    sha = subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo, capture_output=True, text=True).stdout.strip()
    claim = dict(_full_row(sha, "r1"), release_builder="builder-a",
                 e2e_payload={"path": "target/validate/primary", "profile": "validate",
                              "debug_assertions": True, "overflow_checks": True})
    _write_ledger(repo, [_row_ref_event(evidence, claim, event_id="e1")])
    out = run_cli(repo, "--json", "receipt", "show", "HEAD")
    assert json.loads(out.stdout)["verdict"] != "qualified"
    _write_ledger(repo, [_row_ref_event(evidence, _full_row(sha, "r2"), event_id="e2")])
    out = run_cli(repo, "--json", "receipt", "show", "HEAD")
    assert json.loads(out.stdout)["verdict"] == "qualified"


def test_unlisted_repo_row_does_not_count(tmp_path: Path) -> None:
    # A row whose repo is not one of the counted repos is not counted even
    # when it sits in the same ledger; a counted repo row is.
    repo, evidence = _row_ref_repo(tmp_path)
    sha = subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo, capture_output=True, text=True).stdout.strip()
    other = dict(_full_row(sha, "r1"), repo="other", schema_version=3)
    _write_ledger(repo, [_row_ref_event(evidence, other, event_id="e1")])
    out = run_cli(repo, "--json", "receipt", "show", "HEAD")
    assert json.loads(out.stdout)["verdict"] != "qualified"
    primary = dict(other, run_id="r2", repo="primary")
    _write_ledger(repo, [_row_ref_event(evidence, primary, event_id="e2")])
    out = run_cli(repo, "--json", "receipt", "show", "HEAD")
    assert json.loads(out.stdout)["verdict"] == "qualified"


def test_canonical_bytes_deterministic() -> None:
    from local_ci_hub.ledger import canonical_bytes, digest
    a = {"b": 1, "a": {"y": [1, 2], "x": "é"}}
    assert canonical_bytes(a) == canonical_bytes({"a": {"x": "é", "y": [1, 2]}, "b": 1})
    assert digest(a) != digest({"b": 2, "a": {"y": [1, 2], "x": "é"}})


def test_doctor_reports_host_limits(tmp_path: Path) -> None:
    repo = toy_repo(tmp_path)
    out = run_cli(repo, "--json", "doctor")
    assert out.returncode == 0, out.stdout + out.stderr
    section = json.loads(out.stdout)["checks"]["host_limits"]
    assert section["limits"]["file_present"] is True
    assert section["gate"]["g"] == "unknown"  # unset admission dir: informational only
    assert section["live"]["cleanup_pending_max"] == 6


def test_run_refused_below_disk_floor_is_no_result(tmp_path: Path) -> None:
    repo = toy_repo(tmp_path)
    host_file = repo / "host-limits.toml"
    host_file.write_text(host_file.read_text().replace(
        "slot_fs_min_free_bytes = 1", "slot_fs_min_free_bytes = 1000000000000000000"))
    r = run_cli(repo, "--json", "run", "start")
    assert r.returncode == 75, r.stdout + r.stderr  # refused: no result, never a failure
    out = run_cli(repo, "--json", "ledger", "query")
    assert json.loads(out.stdout) == []  # no checkout, no FAIL row


def test_run_sweep_clears_cleanup_pending(tmp_path: Path) -> None:
    repo = toy_repo(tmp_path)
    runs = repo / "host-state" / "runs"
    runs.mkdir(parents=True)
    (repo / "host-state").chmod(0o700)
    (runs / "run-old.json").write_text(json.dumps(
        {"run_id": "run-old", "state": "cleanup-pending"}))
    out = run_cli(repo, "--json", "run", "sweep")
    assert out.returncode == 0 and json.loads(out.stdout)["deleted"] == 1
    assert not (runs / "run-old.json").exists()


def test_run_handles_match_registered_schema() -> None:
    schema_path = Path(__file__).resolve().parents[2] / "validation" / "schemas" / "local-ci-hub-run-v1.schema.json"
    schema = json.loads(schema_path.read_text())
    states = {v["const"] for v in schema["$defs"]["RunState"]["oneOf"]}
    cleanups = {v["const"] for v in schema["$defs"]["CleanupState"]["oneOf"]}
    assert schema["title"] == "RunHandle"
    for required in ("schema", "run_id", "commit", "state"):
        assert required in schema["required"]
    samples = [
        {"schema": "local-ci-hub/run/v1", "run_id": "run-1", "commit": "a" * 40,
         "cell": "", "state": "running"},
        {"schema": "local-ci-hub/run/v1", "run_id": "run-2", "commit": "b" * 40,
         "cell": "full", "state": "passed", "cleanup": "done", "outcome": "pass", "exit_code": 0},
        {"schema": "local-ci-hub/run/v1", "run_id": "run-3", "commit": "c" * 40,
         "cell": "", "state": "could-not-run", "cleanup": "pending",
         "outcome": "no_result", "exit_code": 75},
        {"schema": "local-ci-hub/run/v1", "run_id": "run-4", "commit": "d" * 40,
         "cell": "", "state": "finished"},  # legacy spelling stays valid
    ]
    for handle in samples:
        assert handle["state"] in states and handle.get("cleanup", "none") in cleanups
        assert handle["schema"] == "local-ci-hub/run/v1"


def test_land_run_executes_under_lock(tmp_path: Path) -> None:
    repo = toy_repo(tmp_path)
    r = run_cli(repo, "land", "run", "--holder", "a", "--", "/bin/sh", "-c", "echo landed > landed.txt")
    assert r.returncode == 0 and (repo / "landed.txt").exists()
    # lock was released afterwards
    assert run_cli(repo, "land", "acquire", "--holder", "b").returncode == 0
