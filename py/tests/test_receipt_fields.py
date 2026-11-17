"""Receipt-field contract tests: versioned predicate and adapter."""
from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from local_ci_hub import result_adapter  # noqa: E402
from local_ci_hub.config import Config  # noqa: E402
from local_ci_hub.receipt import show  # noqa: E402

BIN = Path(__file__).resolve().parents[2] / "py" / "bin" / "local-ci-hub"
SHA = "a" * 40


def _cfg(tmp_path: Path) -> Config:
    return Config(path=tmp_path / "local-ci-hub.toml", root=tmp_path,
                  ledger_dir=tmp_path / "ledger", state_dir=tmp_path / "state",
                  require_full_evidence=True, repo="repo-a",
                  counted_repos=["repo-a", "repo-b"], known_gaps=["gap-1", "gap-2"])


def _verdict(tmp_path: Path, row: dict[str, object]) -> str:
    cfg = _cfg(tmp_path)
    cfg.ledger_dir.mkdir(parents=True, exist_ok=True)
    event = {"schema": "validate-ledger/v1", "event_type": "run.result",
             "event_id": "e1", "run_id": str(row.get("run_id", "r1")),
             "commit": SHA, "outcome": str(row.get("result", "pass")).lower(),
             "legacy_row": row}
    (cfg.ledger_dir / "ledger.jsonl").write_text(json.dumps(event) + "\n")
    return str(show(cfg, SHA)["verdict"])


def _good_v6() -> dict[str, object]:
    return {"commit": SHA, "run_id": "r1", "result": "pass", "schema_version": 6,
            "repo": "repo-a", "producer": "local-ci-hub", "record_id": "rec-r1",
            "tree": "b" * 40, "commit_anchored": True, "tree_dirty": False,
            "executed_tests": 10, "passed_tests": 10, "failures": 0,
            "profile": "full", "selection_mode": "full",
            "admission": "host-limits-live",
            "admission_floor_evidence": {"floor_kind": "current-main",
                                         "floor_sha": SHA, "observed_main_sha": SHA},
            "tooling_sha": "c" * 40, "log_identity": "log-r1",
            "finished_at": "2026-11-17T00:00:00Z",
            "cell_results": {"run_id": "r1", "commit": SHA},
            "gates_run": 1, "gates_expected": 1,
            "coverage": {"planned_test_nodes": 1, "zero_executed_nodes": [], "absent_nodes": []}}


def test_schema6_requires_repo_and_matching_repo(tmp_path: Path) -> None:
    assert _verdict(tmp_path / "a", _good_v6()) == "qualified"
    missing = {k: v for k, v in _good_v6().items() if k != "repo"}
    assert _verdict(tmp_path / "b", missing) != "qualified"
    other = dict(_good_v6(), repo="repo-b")
    assert _verdict(tmp_path / "c", other) != "qualified"  # same SHA, another repo


def test_schema6_requires_full_profile_and_result_set(tmp_path: Path) -> None:
    assert _verdict(tmp_path / "a", dict(_good_v6(), profile="super")) != "qualified"
    assert _verdict(tmp_path / "b", dict(_good_v6(), result="bogus")) != "qualified"
    assert _verdict(tmp_path / "c", dict(_good_v6(), result="failed")) != "qualified"


def test_schema_versions_gate_and_coverage_thresholds(tmp_path: Path) -> None:
    legacy = {k: v for k, v in _good_v6().items()
              if k not in ("schema_version", "repo", "gates_run", "gates_expected", "coverage")}
    assert _verdict(tmp_path / "a", legacy) == "qualified"  # era rules, not vacuous clauses
    v5 = dict(legacy, schema_version=5, repo="repo-a")
    assert _verdict(tmp_path / "b", v5) != "qualified"  # schema 5 requires gates+coverage
    v5_full = dict(v5, gates_run=1, gates_expected=1,
                   coverage={"planned_test_nodes": 1, "zero_executed_nodes": [], "absent_nodes": []})
    assert _verdict(tmp_path / "c", v5_full) == "qualified"
    over = dict(v5_full, gates_run=2)
    assert _verdict(tmp_path / "d", over) != "qualified"  # accounting must close


def test_known_gap_exception_must_be_ledgered(tmp_path: Path) -> None:
    short = dict(_good_v6(), passed_tests=9, known_gaps=["gap-1"])
    assert _verdict(tmp_path / "a", short) == "qualified"
    unledgered = dict(_good_v6(), passed_tests=9)
    assert _verdict(tmp_path / "b", unledgered) != "qualified"
    nested = dict(_good_v6())
    nested.pop("executed_tests"); nested.pop("passed_tests")
    nested["test_results"] = {"totals": {"executed_tests": 10, "passed_tests": 10}}
    assert _verdict(tmp_path / "c", nested) == "qualified"  # totals read nested


def _without(key: str) -> dict[str, object]:
    return {k: v for k, v in _good_v6().items() if k != key}


def test_per_clause_refusals(tmp_path: Path) -> None:
    cases = [
        dict(_good_v6(), producer="nobody"), dict(_good_v6(), tooling_sha="unknown"),
        dict(_good_v6(), admission="made-up-kind"), _without("record_id"),
        _without("log_identity"), _without("tree"), dict(_good_v6(), tree_dirty=True),
        _without("passed_tests"), dict(_good_v6(), executed_tests=0.5),
        dict(_good_v6(), finished_at="zzz"), dict(_good_v6(), selection_mode="only"),
        dict(_good_v6(), cell_results={"run_id": "zzz", "commit": SHA}),
        dict(_good_v6(), admission_floor_evidence={"floor_kind": "current-main",
                                                   "floor_sha": "b" * 40, "observed_main_sha": SHA}),
        dict(_good_v6(), schema_version="6"), dict(_good_v6(), schema_version=6.0),
        dict(_good_v6(), schema_version=99), dict(_good_v6(), passed_tests=20),
        dict(_good_v6(), passed_tests=8, known_gaps=["junk"]), dict(_good_v6(), result=" Pass "),
    ]
    for index, row in enumerate(cases):
        assert _verdict(tmp_path / f"c{index}", row) != "qualified", row


def test_schema4_era_and_gap_names(tmp_path: Path) -> None:
    v4 = {k: v for k, v in _good_v6().items()
          if k not in ("schema_version", "gates_run", "gates_expected", "coverage",
                       "admission", "admission_floor_evidence", "tooling_sha",
                       "producer", "record_id", "log_identity", "cell_results")}
    v4["schema_version"] = 4
    assert _verdict(tmp_path / "a", v4) == "qualified"  # era rules only
    gap = dict(_good_v6(), passed_tests=8, known_gaps=["gap-1", "gap-2"])
    assert _verdict(tmp_path / "b", gap) == "qualified"  # named, ledgered gaps
    skip = dict(_good_v6(), gates_run=1, gates_expected=2, intentional_skipped_nodes=["n"])
    assert _verdict(tmp_path / "c", skip) == "qualified"  # legitimate intentional skip


def test_adapter_maps_and_drops(tmp_path: Path) -> None:
    out = result_adapter.adapt({
        "test_results": {"totals": {"executed_tests": 7, "passed_tests": 7,
                                    "filtered_tests": 1, "failures": 0}},
        "profile": "full", "gates_run": [1, 2], "coverage": {"planned_test_nodes": 2},
        "executed_tests": -3, "release_builder": 42})
    assert out["executed_tests"] == 7 and out["filtered_tests"] == 1 and "known_gaps" not in out
    assert out["gates_run"] == 2 and "release_builder" not in out
    assert result_adapter.from_result_file(tmp_path / "absent.json") == {}


def test_runner_record_shape(tmp_path: Path) -> None:
    repo = tmp_path / "toy"
    repo.mkdir()
    for cmd in (["git", "init", "-q", "-b", "main", "."],
                ["git", "config", "user.email", "t@example.com"],
                ["git", "config", "user.name", "t"]):
        subprocess.run(cmd, cwd=repo, timeout=15)
    (repo / "README.md").write_text("# toy\n")
    state_root = repo / "host-state"
    (repo / "local-ci-hub.toml").write_text(
        '[project]\nname="toy"\nrepo="repo-a"\n[validate]\ncommand=["/bin/true"]\n'
        'admission_slots=2\n[ledger]\nteam="toy"\npublish="none"\ncounted_repos=["repo-a"]\nlocation={path="ci-ledger"}\n'
        '[remote]\nprovider="none"\n[receipt]\nqualifying=["local-ledger-pass"]\nrequire_full_evidence=false\n[land]\nstate_dir="ignored/local-ci-hub"\n')
    (repo / "host-limits.toml").write_text(
        'version = 1\n[admission]\nmax_concurrent_validation_runs = 4\n'
        'cleanup_pending_max = 6\nslot_fs_min_free_bytes = 1\n'
        '[run_scope]\ncpu_quota_cpus = 2\nmemory_max_bytes = 1048576\npids_max = 1024\n'
        f'[paths]\nstate_root = "{state_root}"\n')
    import os
    zero = "\n".join(["cpu  0 0 0 0 0 0 0 0 0 0"] + [f"cpu{i} 0 0 0 0 0 0 0 0 0 0" for i in range(4)]) + "\n"
    idle = "\n".join(["cpu  0 0 0 100 0 0 0 0 0 0"] + [f"cpu{i} 0 0 0 100 0 0 0 0 0 0" for i in range(4)]) + "\n"
    (repo / "stat-a.txt").write_text(zero)
    (repo / "stat-b.txt").write_text(idle)
    subprocess.run(["git", "add", "-A"], cwd=repo, timeout=15)
    subprocess.run(["git", "commit", "-qm", "toy: init"], cwd=repo, timeout=15)
    env = dict(os.environ, AGENT_UTILS_HOST_LIMITS_CONFIG=str(repo / "host-limits.toml"),
               AGENT_UTILS_HOST_LIMITS_STAT_A=str(repo / "stat-a.txt"),
               AGENT_UTILS_HOST_LIMITS_STAT_B=str(repo / "stat-b.txt"),
               GATE_DEFER_CONFIG=str(repo / "no-defer.toml"), GATE_DEFER_HOLDS_DIR="",
               GATE_DEFER_SHARED_HOLDS_DIR="", GATE_DEFER_QUIET_COMMAND="")
    run = subprocess.run([sys.executable, str(BIN), "--json", "run", "start"],
                         cwd=repo, capture_output=True, text=True, timeout=60, env=env)
    assert run.returncode == 0, run.stdout + run.stderr
    query = subprocess.run([sys.executable, str(BIN), "--json", "ledger", "query"],
                           cwd=repo, capture_output=True, text=True, timeout=60, env=env)
    row = json.loads(query.stdout)[0]
    assert row["schema_version"] == 6 and row["repo"] == "repo-a"
    assert row["result"] == "PASS" and row["producer"] == "local-ci-hub"  # fold spelling; writer is lowercase
    assert re.fullmatch(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z", str(row["finished_at"]))
    assert row["admission"] == "host-limits-live" and row["tooling_sha"]
