"""Tests for the DERIVED enforcement manifest (capabilities.py) and the resource-exclusivity
admission gate (admission.py, the ``solo_validate`` capability).

Brackets each guard from both sides: the manifest is generated from the registry (not a literal),
and admission both REFUSES a validate while a live competing holder holds the box (negative) and
ADMITS it when the box is free (positive), so a gate that refused or admitted everything fails.
"""

from __future__ import annotations

import contextlib
import io
import os
import subprocess
from pathlib import Path

import pytest

from safe_ci_dag_runner import ENFORCEMENT_CAPABILITIES
from safe_ci_dag_runner import admission
from safe_ci_dag_runner.capabilities import (
    ENFORCEMENT_REGISTRY,
    capability_count,
    enforcement_manifest,
    is_enforced,
)
from safe_ci_dag_runner.cli import PROG, main

_DEMO = '{"steps": [{"group": "g", "job": "j", "cmd": "true", "deps": []}]}'


def _capture(args: list[str]) -> tuple[int, str, str]:
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        rc = main(args)
    return rc, out.getvalue(), err.getvalue()


# --------------------------------------------------------------------------- derived manifest


def test_manifest_is_derived_key_sorted_and_compact() -> None:
    # Generated from the registry: exactly its keys, sorted, compact JSON, lowercase bools.
    assert enforcement_manifest() == (
        '{"cpu_timeout":true,"memory_max":true,"oom_detection":true,'
        '"pids_guard":false,"solo_validate":true,"wall_timeout":true}'
    )
    # The package-level constant is the derived value, not a separate literal.
    assert ENFORCEMENT_CAPABILITIES == enforcement_manifest()
    assert capability_count() == 6 == len(ENFORCEMENT_REGISTRY)


def test_is_enforced_reads_the_registry() -> None:
    assert is_enforced("cpu_timeout")
    assert is_enforced("solo_validate")
    assert not is_enforced("pids_guard")


def test_unknown_capability_raises_loudly() -> None:
    with pytest.raises(KeyError):
        is_enforced("no_such_guard")


# --------------------------------------------------------------------------- admission predicate


def _h(role: str, pid: int) -> admission.Holder:
    return admission.Holder(role=role, pid=pid, path=Path("x"))


def test_validate_is_refused_by_any_live_holder() -> None:
    assert admission.solo_validate_refusal(admission.VALIDATE, [_h(admission.BENCHMARK, 1)])
    assert admission.solo_validate_refusal(admission.VALIDATE, [_h(admission.VALIDATE, 2)])


def test_validate_is_admitted_when_box_is_free() -> None:
    assert admission.solo_validate_refusal(admission.VALIDATE, []) is None


def test_benchmark_yields_only_to_validate() -> None:
    assert admission.solo_validate_refusal(admission.BENCHMARK, [_h(admission.VALIDATE, 1)])
    assert admission.solo_validate_refusal(admission.BENCHMARK, [_h(admission.BENCHMARK, 1)]) is None


def test_scan_skips_dead_and_self_and_reads_live(tmp_path: Path) -> None:
    admission.acquire(tmp_path, admission.BENCHMARK, 999_999_999)  # dead pid
    admission.acquire(tmp_path, admission.VALIDATE, os.getpid())  # self
    admission.acquire(tmp_path, admission.BENCHMARK, 1)  # live foreign (pid 1)
    live = admission.scan_live_holders(tmp_path, exclude_pid=os.getpid())
    assert [h.pid for h in live] == [1]
    assert not (tmp_path / "benchmark.999999999.holder").exists()  # stale unlinked


# --------------------------------------------------------------------------- CLI admission gate


def test_cli_admission_both_directions(tmp_path: Path) -> None:
    dag = tmp_path / "dag.json"
    dag.write_text(_DEMO, encoding="utf-8")
    holders = tmp_path / "holders"
    holders.mkdir()
    base = [
        "run", "--dag", str(dag), "-q", "--allow-cgroup-failure",
        "--no-profile", "--no-profile-feedback",
    ]

    # POSITIVE: empty holders -> validate admitted, noop DAG passes.
    rc, _, _ = _capture([*base, "--exclusivity-role", "validate", "--box-holders-dir", str(holders)])
    assert rc == 0

    # A role without a holders dir is a usage error (exit 2).
    rc, _, err = _capture([*base, "--exclusivity-role", "validate"])
    assert rc == 2 and "requires --box-holders-dir" in err

    # NEGATIVE: a LIVE foreign holder -> refusal (exit 4). The sleep is OUR OWN child; we kill it.
    child = subprocess.Popen(["sleep", "120"])
    try:
        (holders / f"benchmark.{child.pid}.holder").write_text(
            f"role=benchmark\npid={child.pid}\n"
        )
        rc, _, err = _capture(
            [*base, "--exclusivity-role", "validate", "--box-holders-dir", str(holders)]
        )
        assert rc == 4 and "refused admission" in err
        # Other direction: benchmark refused while a live validate holds the box.
        (holders / f"benchmark.{child.pid}.holder").unlink()
        (holders / f"validate.{child.pid}.holder").write_text(
            f"role=validate\npid={child.pid}\n"
        )
        rc, _, err = _capture(
            [*base, "--exclusivity-role", "benchmark", "--box-holders-dir", str(holders)]
        )
        assert rc == 4 and "refused admission" in err
    finally:
        child.kill()
        child.wait()

    # POSITIVE: the holder's pid is now dead -> stale, ignored, admit.
    rc, _, _ = _capture(
        [*base, "--exclusivity-role", "validate", "--box-holders-dir", str(holders)]
    )
    assert rc == 0
    assert PROG  # sanity: module imported
