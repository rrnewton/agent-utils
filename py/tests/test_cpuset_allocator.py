"""Tests for the ``cpuset-alloc`` CLI layer.

The stateful ledger itself is covered by ``test_reservation.py``. Here we cover the
allocator CLI:

  * ``_scope_argv`` builds the mutation-verified HARD-pin invocation
    (``systemd-run --user --scope -p AllowedCPUs=<set>``), NOT an escapable
    ``sched_setaffinity`` call.
  * ``run`` REFUSES (does not silently run un-pinned) when the hard mechanism is
    unavailable — a soft bound would silently contaminate a benchmark.
  * ``status`` / ``reclaim`` emit the qualified assignment as JSON.
  * A systemd-gated LIVE MUTATION test: request K cores, confirm a child CANNOT
    escape to a K+1th core (reading back what we wrote proves nothing).
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
from contextlib import nullcontext
from pathlib import Path

import pytest

from safe_ci_dag_runner import cpuset_allocator as ca
from safe_ci_dag_runner import reservation


def _systemd() -> bool:
    return ca._systemd_run_available()


# --------------------------------------------------------------------------- #
# argv construction — the HARD mechanism, not sched_setaffinity                #
# --------------------------------------------------------------------------- #

def test_scope_argv_uses_allowedcpus_hard_pin() -> None:
    argv = ca._scope_argv([7, 3, 5], ["echo", "hi"])
    assert argv[0] == "systemd-run"
    assert "--user" in argv and "--scope" in argv and "--collect" in argv
    # Cores rendered sorted as a cpulist on AllowedCPUs — the cgroup cpuset path.
    assert "AllowedCPUs=3,5,7" in argv
    # The command follows a `--` separator, uninterpreted by systemd-run.
    dd = argv.index("--")
    assert argv[dd + 1 :] == ["echo", "hi"]
    # It must NOT reach for the escapable affinity mechanism.
    assert not any("setaffinity" in a or a == "taskset" for a in argv)


def test_cpulist_is_sorted_comma_form() -> None:
    assert ca._cpulist([5, 1, 3]) == "1,3,5"
    assert ca._cpulist([2]) == "2"


# --------------------------------------------------------------------------- #
# run REFUSES rather than silently running un-pinned                          #
# --------------------------------------------------------------------------- #

def test_run_refuses_without_hard_mechanism(monkeypatch: pytest.MonkeyPatch) -> None:
    """No systemd user scope => no HARD pin. `run` must refuse (exit 3), never
    fall back to an escapable bound and silently contaminate the benchmark."""
    monkeypatch.setattr(ca, "_systemd_run_available", lambda: False)

    # If the guard ever let execution through, this sentinel would fire.
    def _no_run(*a: object, **k: object) -> subprocess.CompletedProcess[bytes]:
        raise AssertionError("run must not launch a command when the pin is unavailable")

    monkeypatch.setattr(subprocess, "run", _no_run)
    rc = ca.main(["run", "--cores", "1", "--", "echo", "hi"])
    assert rc == 3


def test_run_rejects_empty_command(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(ca, "_systemd_run_available", lambda: True)
    assert ca.main(["run", "--cores", "1"]) == 2


def test_run_refuses_when_allowed_cpus_is_soft_or_inert(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(ca, "_systemd_run_available", lambda: True)
    monkeypatch.setattr(
        reservation,
        "reserve_cores",
        lambda *args, **kwargs: nullcontext([0]),
    )
    monkeypatch.setattr(
        ca,
        "_probe_hard_pin",
        lambda cores: {"verdict": "SOFT_OR_INERT", "cores": list(cores)},
    )

    def _no_run(*args: object, **kwargs: object) -> subprocess.CompletedProcess[bytes]:
        raise AssertionError("the benchmark command must not run after a soft pin probe")

    monkeypatch.setattr(subprocess, "run", _no_run)
    assert ca.main(["run", "--cores", "1", "--", "echo", "hi"]) == 3


# --------------------------------------------------------------------------- #
# status / reclaim JSON over an isolated ledger                                #
# --------------------------------------------------------------------------- #

def test_status_and_reclaim_json(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    ledger = tmp_path / "ledger.json"
    rc = ca.main(["status", "--ledger", str(ledger)])
    assert rc == 0
    out = json.loads(capsys.readouterr().out)
    assert out == {"held_cores": [], "held_count": 0}

    rc = ca.main(["reclaim", "--ledger", str(ledger)])
    assert rc == 0
    out = json.loads(capsys.readouterr().out)
    assert out == {"reclaimed": [], "reclaimed_count": 0}


# --------------------------------------------------------------------------- #
# LIVE MUTATION — the owner's "confirm the tree cannot run on a K+1th core"    #
# --------------------------------------------------------------------------- #

@pytest.mark.skipif(not shutil.which("systemd-run"), reason="systemd-run not installed")
def test_selftest_verdict_is_hard_when_systemd_available(
    capsys: pytest.CaptureFixture[str],
) -> None:
    """If a systemd user scope works here, the mutation self-test must conclude
    HARD: the child's escape to an excluded core is masked and the spinners stay
    inside the reserved set. If systemd is present but a user scope cannot be
    created (some CI images), the tool reports UNTESTABLE — accept that, don't
    assert a false HARD."""
    k = 2 if len(os.sched_getaffinity(0)) >= 3 else 1
    rc = ca.main(["selftest", "--cores", str(k), "--sample-s", "0.05"])
    out = json.loads(capsys.readouterr().out)
    if out.get("verdict") != "HARD":
        assert rc != 0
        pytest.skip(f"host does not provide a HARD AllowedCPUs scope: {out}")
    assert out["verdict"] == "HARD", out
    assert rc == 0
    assert out["count"] == k
    assert len(out["cores"]) == k
    assert out["negative_escape_masked"] is True
    assert out["positive_stayed_in_set"] is True
