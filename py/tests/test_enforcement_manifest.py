"""#79 derived-enforcement-manifest: the manifest and the guards move together.

The ``capabilities`` manifest used to be a hand-typed JSON literal with nothing tying it to
the code that enforces the guards it advertises, so it could claim enforcement that was not
happening. It is now generated from
:data:`safe_ci_dag_runner.capabilities.ENFORCEMENT_REGISTRY`, and five of its nine keys are
consulted by :func:`safe_ci_dag_runner.capabilities.is_enforced` at the guard site itself.

These tests pin exactly that. The manifest test writes the published bytes out LITERALLY --
re-deriving them from the registry would pass no matter what the registry said. The
behaviour tests flip one flag and assert BOTH the advertisement and the observed guard move.
"""

from __future__ import annotations

import subprocess
from collections.abc import Mapping
from pathlib import Path

import pytest

from safe_ci_dag_runner.capabilities import (
    ENFORCEMENT_REGISTRY,
    enforcement_manifest,
    is_enforced,
    registry_override,
)
from safe_ci_dag_runner.cgroup import Cgroups, NoopCgroups
from safe_ci_dag_runner.model import DagConfig, ResourceHint, Step
from safe_ci_dag_runner.scheduler import run_dag_limited

#: The exact bytes the `capabilities` subcommand promises, and which the Rust engine must
#: print too. Deliberately a literal: a test that rebuilt this from ENFORCEMENT_REGISTRY
#: would pin nothing, because it would agree with any registry at all.
PUBLISHED_MANIFEST = (
    '{"cpu_affinity":true,"cpu_bandwidth":true,"cpu_timeout":true,"memory_max":true,'
    '"oom_detection":true,"pids_guard":false,"run_timeout":true,"wall_timeout":true,'
    '"write_domains":true}'
)


def test_manifest_is_exactly_the_published_bytes() -> None:
    assert enforcement_manifest() == PUBLISHED_MANIFEST


def test_capabilities_subcommand_prints_the_derived_manifest() -> None:
    out = subprocess.run(
        ["python3", "-m", "safe_ci_dag_runner", "capabilities"],
        capture_output=True,
        text=True,
        check=True,
        cwd=Path(__file__).resolve().parents[1],
    ).stdout
    assert out == PUBLISHED_MANIFEST + "\n"


def test_every_registry_summary_says_something() -> None:
    # The summaries are the only prose left describing each guard; an empty one would make
    # the registry strictly less informative than the comment block it replaced.
    assert len(ENFORCEMENT_REGISTRY) == 9
    assert all(c.summary.strip() for c in ENFORCEMENT_REGISTRY)
    keys = [c.key for c in ENFORCEMENT_REGISTRY]
    assert keys == sorted(keys), "registry order should read as the manifest does"


def test_a_misspelled_guard_site_raises_instead_of_reading_as_unenforced() -> None:
    # The quiet reading -- "unknown, so False" -- would silently disable the guard whose
    # name was typo'd, which is the failure this module exists to prevent.
    with pytest.raises(KeyError, match="unknown enforcement capability 'cpu_timout'"):
        is_enforced("cpu_timout")


def test_registry_override_moves_the_manifest_and_restores_it() -> None:
    with registry_override("memory_max", False):
        assert '"memory_max":false' in enforcement_manifest()
    assert enforcement_manifest() == PUBLISHED_MANIFEST
    with pytest.raises(KeyError):
        with registry_override("no_such_guard", False):
            pass


def _boxed_scope(root: Path) -> Cgroups:
    """A Cgroups pointed at a plain temp dir standing in for a delegated scope root."""
    (root / "cpu.max").write_text("100000 100000")
    (root / "memory.max").write_text(str(8 * 1024**3))
    cg = Cgroups()
    cg.enabled = True
    cg.root = root
    return cg


def test_inner_memory_cap_follows_the_memory_max_capability_flag(tmp_path: Path) -> None:
    """``memory_max`` is load-bearing: flipping it off must stop the inner cap being written.

    Otherwise the manifest could say ``"memory_max":false`` while the kernel cap was still
    applied -- the same class of lie in the other direction.
    """
    cg = _boxed_scope(tmp_path)
    applied = tmp_path / "step-g.j" / "memory.max"

    cg.prepare_command("g.j", "true", mem_max=4096)
    assert applied.read_text() == "4096"
    assert '"memory_max":true' in enforcement_manifest()

    applied.unlink()
    with registry_override("memory_max", False):
        assert '"memory_max":false' in enforcement_manifest()
        cg.prepare_command("g.j", "true", mem_max=4096)
    assert not applied.exists(), (
        "memory.max was written even though the registry says memory_max is unenforced"
    )


def test_pids_ceiling_follows_the_pids_guard_capability_flag(tmp_path: Path) -> None:
    """``pids_guard`` is declared FALSE, and that declaration is now real.

    Before this change the manifest said "no PID ceiling" while ``prepare_command`` would
    happily write ``pids.max`` for any caller that set the limit. Nothing in the runner does,
    which is why the false went unnoticed -- but "no caller happens to use it" is not the same
    statement as "this engine does not enforce it", and only the second one is publishable.
    """
    cg = _boxed_scope(tmp_path)
    cg.set_worker_pids_max(64)
    applied = tmp_path / "step-g.j" / "pids.max"

    cg.prepare_command("g.j", "true")
    assert not applied.exists(), (
        "pids.max was written although the manifest advertises pids_guard as unenforced"
    )

    with registry_override("pids_guard", True):
        assert '"pids_guard":true' in enforcement_manifest()
        cg.prepare_command("g.j", "true")
    assert applied.read_text() == "64"


def _one_step_dag(cmd: str, *, timeout: int, cpu_timeout: int = 0) -> DagConfig:
    return DagConfig(
        steps=(
            Step(
                group="g",
                job="s",
                desc="",
                cmd=cmd,
                timeout=timeout,
                cpu_timeout=cpu_timeout,
                hint=ResourceHint(),
            ),
        ),
        default_step_cpu_timeout=0,
    )


def test_per_step_wall_ceiling_follows_the_wall_timeout_capability_flag() -> None:
    cfg = _one_step_dag("sleep 2", timeout=1)

    assert '"wall_timeout":true' in enforcement_manifest()
    enforced = run_dag_limited(cfg, max_steps=1, max_cpus=1, verbosity=0)
    assert not enforced.ok, "a 2s step under a 1s wall ceiling must be cut"
    assert "TIMEOUT >1s" in enforced.outcomes[0].reason

    with registry_override("wall_timeout", False):
        assert '"wall_timeout":false' in enforcement_manifest()
        unenforced = run_dag_limited(cfg, max_steps=1, max_cpus=1, verbosity=0)
    assert unenforced.ok, (
        "with wall_timeout declared unenforced the step must be allowed to finish; it was "
        "still cut, so the manifest and the guard can disagree"
    )


class _OomingCgroups(NoopCgroups):
    """A boxed manager whose cgroup reports two OOM kills for every step."""

    enabled = True

    def oom_kills(self, tag: str) -> int:
        return 2


def test_oom_attribution_follows_the_oom_detection_capability_flag() -> None:
    cfg = _one_step_dag("exit 1", timeout=30)

    assert '"oom_detection":true' in enforcement_manifest()
    enforced = run_dag_limited(
        cfg, max_steps=1, max_cpus=1, cgroups=_OomingCgroups(), verbosity=0
    )
    assert "OOM-KILLED" in enforced.outcomes[0].reason

    with registry_override("oom_detection", False):
        assert '"oom_detection":false' in enforcement_manifest()
        unenforced = run_dag_limited(
            cfg, max_steps=1, max_cpus=1, cgroups=_OomingCgroups(), verbosity=0
        )
    assert "OOM-KILLED" not in unenforced.outcomes[0].reason, (
        "the memory.events counter was still consulted although oom_detection is declared "
        "unenforced; the manifest would be advertising an attribution it does not make"
    )


class _BurningCgroups(NoopCgroups):
    """A boxed manager whose cpu.stat reports 10 CPU-seconds already consumed."""

    enabled = True

    def cpu_stats(self, tag: str) -> Mapping[str, int]:
        return {"usage_usec": 10_000_000, "user_usec": 10_000_000, "system_usec": 0}


def test_cpu_budget_reaping_follows_the_cpu_timeout_capability_flag() -> None:
    # Wall timeout is generous: the only thing that can cut this step is the CPU-time guard.
    cfg = _one_step_dag("sleep 4", timeout=60, cpu_timeout=1)

    assert '"cpu_timeout":true' in enforcement_manifest()
    enforced = run_dag_limited(
        cfg, max_steps=1, max_cpus=1, cgroups=_BurningCgroups(), verbosity=0
    )
    assert not enforced.ok
    assert "CPU-TIMEOUT" in enforced.outcomes[0].reason

    with registry_override("cpu_timeout", False):
        assert '"cpu_timeout":false' in enforcement_manifest()
        unenforced = run_dag_limited(
            cfg, max_steps=1, max_cpus=1, cgroups=_BurningCgroups(), verbosity=0
        )
    assert unenforced.ok, (
        "the step was still reaped over its CPU budget although cpu_timeout is declared "
        "unenforced; the manifest and the monitor can disagree"
    )
