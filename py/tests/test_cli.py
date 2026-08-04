"""Tests for the safe-ci-dag-runner CLI surface (in-process, stdlib capture)."""

from __future__ import annotations

import contextlib
import io
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest

from safe_ci_dag_runner import __version__
from safe_ci_dag_runner.cli import PROG, _load_userguide, main

_DEMO = '{"steps": [{"group": "g", "job": "j", "cmd": "true", "deps": []}]}'


def _capture(args: list[str]) -> tuple[int, str, str]:
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        rc = main(args)
    return rc, out.getvalue(), err.getvalue()


def _demo_path(tmp: str) -> str:
    path = Path(tmp) / "dag.json"
    path.write_text(_DEMO, encoding="utf-8")
    return str(path)


def test_no_args_prints_help() -> None:
    rc, out, _ = _capture([])
    assert rc == 0
    assert PROG in out and "quickstart" in out


def test_quickstart_is_self_contained() -> None:
    rc, out, _ = _capture(["quickstart"])
    assert rc == 0
    for marker in ("Install", "Write a DAG", "run", "DAG schema", "Exit codes"):
        assert marker in out


def test_userguide_prints_embedded_guide() -> None:
    """`--userguide` prints the full embedded guide VERBATIM (byte-for-byte, no coloring)."""
    rc, out, _ = _capture(["--userguide"])
    assert rc == 0
    embedded = _load_userguide()
    assert out == embedded
    # A real, substantial guide (not a stub) with recognizable content.
    assert len(out) > 5000
    assert "safe-ci-dag-runner" in out


def test_list_and_ascii() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        dag = _demo_path(tmp)
        rc, out, _ = _capture(["list", "--dag", dag])
        assert rc == 0 and "g.j" in out
        rc, out, _ = _capture(["ascii", "--dag", dag])
        assert rc == 0 and "layer 0:" in out and "g.j" in out


def test_dot_and_json() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        dag = _demo_path(tmp)
        rc, out, _ = _capture(["dot", "--dag", dag])
        assert rc == 0 and out.startswith("digraph")
        rc, out, _ = _capture(["json", "--dag", dag])
        assert rc == 0 and '"steps"' in out


# Cgroup boxing is ON by default; the in-process CLI tests use --allow-cgroup-failure so `run`
# does NOT re-exec into a systemd scope (which would replace the pytest process). The default
# require-boxing path and the require-but-unavailable error are covered by
# test_run_default_requires_cgroups_or_flag via a subprocess instead.
_ACF = "--allow-cgroup-failure"


def test_run_exit_codes() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        ok = Path(tmp) / "ok.json"
        ok.write_text('{"steps": [{"group": "g", "job": "j", "cmd": "true"}]}', encoding="utf-8")
        assert _capture(["run", "--dag", str(ok), "-q", _ACF])[0] == 0
        bad = Path(tmp) / "bad.json"
        bad.write_text('{"steps": [{"group": "g", "job": "j", "cmd": "false"}]}', encoding="utf-8")
        assert _capture(["run", "--dag", str(bad), "-q", _ACF])[0] == 1


def test_missing_and_malformed_dag_exit_2() -> None:
    assert _capture(["run", "--dag", "/nonexistent/nope.json", "-q"])[0] == 2
    with tempfile.TemporaryDirectory() as tmp:
        junk = Path(tmp) / "junk.json"
        junk.write_text("not json", encoding="utf-8")
        assert _capture(["list", "--dag", str(junk)])[0] == 2


def test_run_max_mem_exits_0() -> None:
    # --max-mem picks a memory-aware -j; a passing DAG still exits 0.
    with tempfile.TemporaryDirectory() as tmp:
        dag = _demo_path(tmp)
        rc, _, err = _capture(["run", "--max-mem", "8G", "--dag", dag, "-q", _ACF])
        assert rc == 0
        assert "--max-mem 8G" in err  # the sizing decision is surfaced


def test_run_max_mem_no_throttle_note() -> None:
    # A DAG with no per-step rss_baseline_bytes: the modeled footprint collapses to the
    # mem_cap_floor_bytes floor, so a budget at/above the floor picks the full -j (CPU count)
    # and a note explains why --max-mem did not throttle (No Silent Failure).
    with tempfile.TemporaryDirectory() as tmp:
        dag = _demo_path(tmp)
        rc, _, err = _capture(["run", "--max-mem", "16G", "--dag", dag, "-q", _ACF])
        assert rc == 0
        assert "no step carries rss_baseline_bytes" in err
        assert "did not throttle" in err


def test_run_max_mem_with_baseline_no_note() -> None:
    # A DAG whose step carries an rss_baseline_bytes participates in the memory model, so the
    # no-throttle note is NOT emitted even when the chosen -j lands at the CPU count.
    with tempfile.TemporaryDirectory() as tmp:
        dag = Path(tmp) / "dag.json"
        dag.write_text(
            '{"steps": [{"group": "g", "job": "j", "cmd": "true",'
            ' "hint": {"rss_baseline_bytes": 1073741824}}]}',
            encoding="utf-8",
        )
        rc, _, err = _capture(["run", "--max-mem", "64G", "--dag", str(dag), "-q", _ACF])
        assert rc == 0
        assert "no step carries rss_baseline_bytes" not in err


def test_run_jobs_overrides_max_mem() -> None:
    # When both --jobs and --max-mem are given, --jobs wins with a visible note.
    with tempfile.TemporaryDirectory() as tmp:
        dag = _demo_path(tmp)
        rc, _, err = _capture(["run", "--jobs", "2", "--max-mem", "8G", "--dag", dag, "-q", _ACF])
        assert rc == 0
        assert "--jobs=2 wins" in err


def test_run_perf_dir_writes_csv() -> None:
    # --perf-dir writes per-step + whole-run CSVs (the CsvMetricsSink path).
    with tempfile.TemporaryDirectory() as tmp:
        dag = _demo_path(tmp)
        perf = Path(tmp) / "perf"
        rc, _, err = _capture(["run", "--perf-dir", str(perf), "--dag", dag, "-q", _ACF])
        assert rc == 0
        csvs = list(perf.glob("*.csv"))
        assert csvs, "expected at least one perf CSV to be written"
        assert any(p.stat().st_size > 0 for p in csvs)
        assert "perf CSVs written under" in err
        # No stray flock sidecar left behind in the user's --perf-dir.
        assert not list(perf.glob("*.lock"))


def test_run_only_runs_exactly_selected_step() -> None:
    # --only runs EXACTLY the named step(s) and nothing else (deps not run). The selected step's
    # dep (build.app) is NOT executed, so exactly one step passes.
    dag = (
        '{"steps": ['
        '{"group": "build", "job": "app", "cmd": "true"},'
        '{"group": "test", "job": "unit", "cmd": "true", "deps": ["build.app"]}'
        "]}"
    )
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "dag.json"
        path.write_text(dag, encoding="utf-8")
        rc, _, err = _capture(["run", "--dag", str(path), "--only", "test.unit", "-q", _ACF])
        assert rc == 0
        assert "1 passed, 0 failed, 0 aborted, 0 skipped" in err


def test_run_only_multiple_steps() -> None:
    # A comma-separated selection runs exactly those steps.
    dag = (
        '{"steps": ['
        '{"group": "a", "job": "x", "cmd": "true"},'
        '{"group": "b", "job": "y", "cmd": "true"},'
        '{"group": "c", "job": "z", "cmd": "true"}'
        "]}"
    )
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "dag.json"
        path.write_text(dag, encoding="utf-8")
        rc, _, err = _capture(["run", "--dag", str(path), "--only", "a.x,c.z", "-q", _ACF])
        assert rc == 0
        assert "2 passed, 0 failed, 0 aborted, 0 skipped" in err


def test_run_only_unknown_tag_exits_2() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        dag = _demo_path(tmp)
        rc, _, err = _capture(["run", "--dag", dag, "--only", "no.pe", "-q", _ACF])
        assert rc == 2
        assert "unknown step tag" in err


def test_run_profile_prints_table() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        dag = _demo_path(tmp)
        rc, out, _ = _capture(["run", "--dag", dag, "--profile", "-q", _ACF])
        assert rc == 0
        assert "per-step profile:" in out
        for column in ("step", "wall_s", "user_s", "sys_s", "rss_hwm", "oom", "inner_jobs"):
            assert column in out


def test_run_default_profile_store(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # With NEITHER --perf-dir NOR $SAFE_CI_DAG_RUNNER_PROFILE_DIR set, a run auto-logs to the
    # repo-local default ./.safe-ci-dag-runner/profiles/ (relative to CWD) and says where.
    monkeypatch.delenv("SAFE_CI_DAG_RUNNER_PROFILE_DIR", raising=False)
    monkeypatch.chdir(tmp_path)
    dag = _demo_path(str(tmp_path))
    rc, _, err = _capture(["run", "--dag", dag, "-q", _ACF])
    assert rc == 0
    store = tmp_path / ".safe-ci-dag-runner" / "profiles"
    csvs = list(store.glob("*.csv"))
    assert csvs, "expected the default profile store to be created and written"
    assert "default profile store" in err


def test_run_no_profile_writes_nothing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.delenv("SAFE_CI_DAG_RUNNER_PROFILE_DIR", raising=False)
    monkeypatch.chdir(tmp_path)
    dag = _demo_path(str(tmp_path))
    rc, _, err = _capture(["run", "--dag", dag, "--no-profile", "-q", _ACF])
    assert rc == 0
    assert not (tmp_path / ".safe-ci-dag-runner").exists()
    assert "profile data appended" not in err


def test_sweep_prints_table_and_writes_store() -> None:
    # sweep runs the ONE step at inner -j1..-j2 and prints a speedup table; each run auto-appends
    # to the profile store (redirected to a temp dir by the autouse conftest fixture).
    dag = '{"steps": [{"group": "g", "job": "j", "cmd": "true"}]}'
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "dag.json"
        path.write_text(dag, encoding="utf-8")
        store = Path(tmp) / "perf"
        rc, out, err = _capture(
            ["sweep", "--dag", str(path), "--step", "g.j", "--jobs", "1..2",
             "--perf-dir", str(store), _ACF]
        )
        assert rc == 0, err
        assert "parallel-speedup sweep: g.j" in out
        for column in ("jobs", "wall_s", "user_s", "sys_s", "rss_hwm", "speedup(vs j1)"):
            assert column in out
        assert list(store.glob("*.csv")), "sweep should append to the profile store"


def test_sweep_unknown_step_exits_2() -> None:
    dag = '{"steps": [{"group": "g", "job": "j", "cmd": "true"}]}'
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "dag.json"
        path.write_text(dag, encoding="utf-8")
        rc, _, err = _capture(
            ["sweep", "--dag", str(path), "--step", "no.pe", "--jobs", "1..2", _ACF]
        )
        assert rc == 2
        assert "unknown --step tag" in err


def test_sweep_bad_range_exits_2() -> None:
    dag = '{"steps": [{"group": "g", "job": "j", "cmd": "true"}]}'
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "dag.json"
        path.write_text(dag, encoding="utf-8")
        rc, _, err = _capture(
            ["sweep", "--dag", str(path), "--step", "g.j", "--jobs", "5..2", _ACF]
        )
        assert rc == 2
        assert "invalid --jobs range" in err


def test_version_via_module() -> None:
    # argparse --version exits(0); run as a subprocess so it doesn't kill the test process.
    result = subprocess.run(
        [sys.executable, "-m", "safe_ci_dag_runner", "--version"],
        capture_output=True,
        text=True,
        check=True,
    )
    assert result.stdout.strip() == f"{PROG} {__version__}"


def test_run_default_requires_cgroups_or_flag() -> None:
    # Cgroup boxing is ON by default. Run in a SUBPROCESS (so a re-exec cannot replace pytest)
    # with CI=1 set, which makes the boxing bring-up skip the re-exec -> boxing is "unavailable".
    # Default (no flag): the run must ERROR with a distinct nonzero exit (3). With
    # --allow-cgroup-failure it must downgrade to a best-effort UNBOXED run and exit 0.
    import os

    with tempfile.TemporaryDirectory() as tmp:
        dag = _demo_path(tmp)
        env = dict(os.environ, CI="1")  # force the boxing re-exec to be skipped
        required = subprocess.run(
            [sys.executable, "-m", "safe_ci_dag_runner", "run", "--dag", dag, "-q"],
            capture_output=True,
            text=True,
            env=env,
        )
        assert required.returncode == 3, required.stderr
        assert "cgroup boxing" in required.stderr.lower()

        allowed = subprocess.run(
            [sys.executable, "-m", "safe_ci_dag_runner", "run", "--dag", dag, "-q",
             "--allow-cgroup-failure"],
            capture_output=True,
            text=True,
            env=env,
        )
        assert allowed.returncode == 0, allowed.stderr
        assert "UNBOXED" in allowed.stderr


def test_boxed_run_enforces_cpu_timeout() -> None:
    # Behavioral parity anchor for the Rust `cpu_timeout_smoke.rs`: prove the DEFAULT boxed run
    # actually ENFORCES a per-step CPU-time budget. A step that busy-loops forever with a tiny
    # cpu_timeout must be reaped as a CPU-TIMEOUT well before its (much larger) wall timeout.
    #
    # Run boxed (no --allow-cgroup-failure) in a SUBPROCESS so the boxing re-exec is real; do NOT
    # set CI=1 here (that would skip boxing). Cgroup boxing is environment-dependent, so when it
    # genuinely cannot be established the default run exits 3 and we skip LOUDLY, never silently.
    import os

    dag = (
        '{"steps": [{"group": "cpu", "job": "burn", "desc": "burn CPU past budget",'
        ' "cmd": "while :; do :; done", "cpu_timeout": 1, "timeout": 30}]}'
    )
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "cpu.json"
        path.write_text(dag, encoding="utf-8")
        proc = subprocess.run(
            [sys.executable, "-m", "safe_ci_dag_runner", "run", "--dag", str(path),
             "-q", "--no-profile"],
            capture_output=True,
            text=True,
            env=dict(os.environ),
        )
        if proc.returncode == 3:
            pytest.skip(
                "cgroup boxing unavailable (need cgroup-v2 + a working systemd --user scope); "
                f"cannot verify CPU-timeout enforcement here. Details:\n{proc.stderr}"
            )
        combined = proc.stdout + proc.stderr
        assert proc.returncode == 1, (
            "boxed run should FAIL (exit 1) when a step exceeds its CPU-time budget; "
            f"got {proc.returncode}\n{combined}"
        )
        assert "CPU-TIMEOUT" in combined, (
            "expected a CPU-TIMEOUT report proving the per-step CPU-time budget fired "
            f"(not the wall TIMEOUT):\n{combined}"
        )


def test_boxed_reexec_via_symlink_imports_package() -> None:
    # Regression guard for the fleet-wide local-validate breakage: a DEFAULT boxed run invoked
    # through the py/bin symlink (NOT `python -m`, NOT pip-installed) must re-exec a child that
    # can still import the package. The old code re-exec'd `python -m safe_ci_dag_runner`, whose
    # fresh child had py/ off sys.path -> `No module named safe_ci_dag_runner`, so every non-CI
    # `run` (i.e. validate.sh) died. The fix re-execs __main__.py by absolute path, which does its
    # own sys.path fixup. Run from a CWD *outside* py/ so an accidental cwd-relative import can't
    # mask the bug. Boxing is environment-dependent, so exit 3 (boxing genuinely unavailable) is a
    # valid LOUD skip; the one thing that must NEVER appear is the import failure.
    import os

    symlink = Path(__file__).resolve().parent.parent / "bin" / "safe-ci-dag-runner"
    assert symlink.exists(), f"expected the console symlink at {symlink}"
    dag = (
        '{"steps": [{"group": "cpu", "job": "burn", "desc": "burn",'
        ' "cmd": "while :; do :; done", "cpu_timeout": 1, "timeout": 30}]}'
    )
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "cpu.json"
        path.write_text(dag, encoding="utf-8")
        # Scrub CI markers so boxing is NOT skipped: this must exercise the real re-exec.
        env = {k: v for k, v in os.environ.items() if k not in ("CI", "GITHUB_ACTIONS")}
        proc = subprocess.run(
            [sys.executable, str(symlink), "run", "--dag", str(path), "-q", "--no-profile"],
            capture_output=True,
            text=True,
            cwd=tmp,
            env=env,
        )
        combined = proc.stdout + proc.stderr
        assert "No module named safe_ci_dag_runner" not in combined, (
            "boxed re-exec child failed to import the package (the regression this guards): "
            f"rc={proc.returncode}\n{combined}"
        )
        if proc.returncode == 3:
            pytest.skip(
                "cgroup boxing unavailable here; re-exec child imported OK (no import error), "
                f"which is what this test guards.\n{proc.stderr}"
            )
        assert proc.returncode == 1 and "CPU-TIMEOUT" in combined, (
            "boxed symlink run should enforce the CPU budget once boxing is active; "
            f"got rc={proc.returncode}\n{combined}"
        )


def test_cores_flag_constrains_the_whole_run_tree() -> None:
    # Behavioral parity anchor for the Rust `core_box_smoke.rs`: prove `--cores K` actually
    # constrains the WHOLE run process tree (the runner AND every step it forks), not just the
    # runner. The step below is a FORKED descendant of the runner that reads its own
    # `nproc` (which honors sched-affinity); under `--cores 1` it must see exactly one CPU, so the
    # step passes iff the size-1 core box was inherited across fork+execve to the whole tree.
    #
    # We run with `--allow-cgroup-failure`, so `apply_core_box` still runs (the boxing manager
    # resolves to a no-op with rc 0) and exercises the `sched_setaffinity` fallback even where no
    # delegated cpuset scope exists — the mechanism that must work in the 3pai sandbox and on
    # plain CI. The box is environment-dependent (sched_setaffinity can be denied, or the host may
    # expose only one CPU), so when the runner does not log that it constrained the tree to 1 core
    # we skip LOUDLY rather than assert on an environment that cannot pin.
    import os

    allowed = os.sched_getaffinity(0)
    if len(allowed) < 2:
        pytest.skip(
            f"host exposes only {len(allowed)} allowed CPU(s); a 1-core box is "
            "indistinguishable from the ambient affinity here"
        )

    # POSITIVE leg: a forked step that PASSES iff it sees exactly one CPU (whole-tree inheritance).
    pos_dag = (
        '{"steps": [{"group": "box", "job": "one", "desc": "step sees exactly 1 CPU",'
        ' "cmd": "test \\"$(nproc)\\" -eq 1", "timeout": 30}]}'
    )
    # NEGATIVE leg: the SAME step WITHOUT --cores must see >1 CPU (proves the box, not nproc, is
    # what changes the count) — i.e. the constraint is not vacuously always-true.
    neg_dag = (
        '{"steps": [{"group": "box", "job": "many", "desc": "step sees >1 CPU unconstrained",'
        ' "cmd": "test \\"$(nproc)\\" -gt 1", "timeout": 30}]}'
    )
    with tempfile.TemporaryDirectory() as tmp:
        pos = Path(tmp) / "pos.json"
        pos.write_text(pos_dag, encoding="utf-8")
        neg = Path(tmp) / "neg.json"
        neg.write_text(neg_dag, encoding="utf-8")
        env = dict(os.environ)

        pproc = subprocess.run(
            [sys.executable, "-m", "safe_ci_dag_runner", "run", "--dag", str(pos),
             "--cores", "1", "-q", "--no-profile", _ACF],
            capture_output=True, text=True, env=env,
        )
        pcombined = pproc.stdout + pproc.stderr
        if "core box: constrained to 1 core" not in pcombined:
            pytest.skip(
                "the runner could not verify a 1-core box in this environment (neither cgroup "
                f"cpuset nor sched_setaffinity engaged):\n{pcombined}"
            )
        assert pproc.returncode == 0, (
            "with --cores 1 the forked step must see exactly one CPU (proving the size-1 box was "
            f"inherited by the whole tree); got rc={pproc.returncode}\n{pcombined}"
        )

        # Negative control: unconstrained, the same step sees the ambient (>1) CPU count.
        nproc = subprocess.run(
            [sys.executable, "-m", "safe_ci_dag_runner", "run", "--dag", str(neg),
             "-q", "--no-profile", _ACF],
            capture_output=True, text=True, env=env,
        )
        ncombined = nproc.stdout + nproc.stderr
        assert nproc.returncode == 0, (
            "without --cores the step must see the ambient (>1) CPU count, proving the box (not "
            f"nproc) is what changes it; got rc={nproc.returncode}\n{ncombined}"
        )
