"""local-ci-hub subcommand tests: ledger, receipt, run, health, remote, land."""
from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

BIN = Path(__file__).resolve().parents[2] / "py" / "bin" / "local-ci-hub"


def run_cli(cwd: Path, *args: str) -> subprocess.CompletedProcess:
    return subprocess.run([sys.executable, str(BIN), *args], cwd=cwd, capture_output=True, text=True, timeout=60)


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
    subprocess.run(["git", "add", "-A"], cwd=repo, timeout=15)
    subprocess.run(["git", "commit", "-qm", "toy: init"], cwd=repo, timeout=15)
    return repo


def test_help_and_quickstart(tmp_path):
    r = run_cli(tmp_path, "--help")
    assert r.returncode == 0 and "receipt" in r.stdout
    r = run_cli(tmp_path, "quickstart")
    assert r.returncode == 0 and "receipt show" in r.stdout


def test_ledger_append_publish_query(tmp_path):
    repo = toy_repo(tmp_path)
    sha = subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo, capture_output=True, text=True).stdout.strip()
    assert run_cli(repo, "--json", "ledger", "append", "--commit", sha, "--result", "PASS").returncode == 0
    assert run_cli(repo, "ledger", "publish").returncode == 0
    out = run_cli(repo, "--json", "ledger", "query", "--commit", sha)
    assert json.loads(out.stdout)[0]["result"] == "PASS"
    out = run_cli(repo, "--json", "ledger", "newest-green")
    assert json.loads(out.stdout)["commit"] == sha


def test_receipt_qualified_and_no_result(tmp_path):
    repo = toy_repo(tmp_path)
    assert run_cli(repo, "receipt", "show", "deadbeef").returncode == 1  # no-result, not qualified
    assert run_cli(repo, "run", "start").returncode == 0
    out = run_cli(repo, "--json", "receipt", "show", "HEAD")
    assert out.returncode == 0 and json.loads(out.stdout)["verdict"] == "qualified"


def test_run_failure_records_fail(tmp_path):
    repo = toy_repo(tmp_path, validate="false")
    assert run_cli(repo, "run", "start").returncode == 1  # a FAIL run exits nonzero
    out = run_cli(repo, "--json", "receipt", "show", "HEAD")
    assert json.loads(out.stdout)["verdict"] == "not-qualified"
    bad = run_cli(repo, "--json", "ledger", "first-bad")
    assert json.loads(bad.stdout).get("result") in ("FAIL", "none")


def test_health_remote_land_doctor(tmp_path):
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


def test_goalpost_prefix_and_enrich_cannot_qualify(tmp_path):
    repo = toy_repo(tmp_path)
    sha = subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo, capture_output=True, text=True).stdout.strip()
    # a 1-char prefix must never bind evidence to a commit
    assert run_cli(repo, "--json", "ledger", "append", "--commit", sha, "--result", "PASS").returncode == 0
    run_cli(repo, "ledger", "publish")
    out = run_cli(repo, "--json", "receipt", "show", sha[:1])
    assert json.loads(out.stdout)["verdict"] != "qualified"


def test_row_ref_missing_evidence_refuses(tmp_path):
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


def test_canonical_bytes_deterministic():
    from local_ci_hub.ledger import canonical_bytes, digest
    a = {"b": 1, "a": {"y": [1, 2], "x": "é"}}
    assert canonical_bytes(a) == canonical_bytes({"a": {"x": "é", "y": [1, 2]}, "b": 1})
    assert digest(a) != digest({"b": 2, "a": {"y": [1, 2], "x": "é"}})


def test_land_run_executes_under_lock(tmp_path):
    repo = toy_repo(tmp_path)
    r = run_cli(repo, "land", "run", "--holder", "a", "--", "/bin/sh", "-c", "echo landed > landed.txt")
    assert r.returncode == 0 and (repo / "landed.txt").exists()
    # lock was released afterwards
    assert run_cli(repo, "land", "acquire", "--holder", "b").returncode == 0
