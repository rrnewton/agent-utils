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
        '[remote]\nprovider="none"\n[receipt]\nqualifying=["local-ledger-pass"]\n')
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
    assert run_cli(repo, "run", "start").returncode == 0
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
