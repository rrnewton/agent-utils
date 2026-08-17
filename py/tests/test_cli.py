"""Tests for the safe-ci-dag-runner CLI surface (in-process, stdlib capture)."""

from __future__ import annotations

import contextlib
import io
import json
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest

from safe_ci_dag_runner import __version__
from safe_ci_dag_runner.cli import (
    CGROUP_SETUP_ENVIRONMENT_ERROR,
    PROG,
    _load_userguide,
    main,
)

_DEMO = '{"steps": [{"group": "g", "job": "j", "cmd": "true", "deps": []}]}'


def test_cgroup_setup_failure_is_environmental_and_pre_node() -> None:
    assert CGROUP_SETUP_ENVIRONMENT_ERROR.startswith("ENVIRONMENT:")
    assert "no DAG node started" in CGROUP_SETUP_ENVIRONMENT_ERROR
    assert "no product build started" in CGROUP_SETUP_ENVIRONMENT_ERROR


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


def _capture_help(args: list[str]) -> tuple[int, str, str]:
    """Capture a subcommand's --help, which argparse emits via SystemExit(0)."""
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        with pytest.raises(SystemExit) as exc:
            main(args)
    code = exc.value.code if isinstance(exc.value.code, int) else 0
    return code, out.getvalue(), err.getvalue()


def test_run_help_lists_flags_and_cores_pinning() -> None:
    """`run --help` exits 0, lists its flags, and surfaces --cores CPU pinning discoverably."""
    code, out, _ = _capture_help(["run", "--help"])
    assert code == 0
    for flag in (
        "--dag",
        "--max-steps",
        "--jobs",
        "--cores",
        "--only",
        "--planner",
        "--keep-going",
    ):
        assert flag in out, flag
    # Discoverable pinning aliases + intent keywords (greppable).
    assert "--cpuset" in out and "--pin" in out
    # argparse line-wraps help, so collapse whitespace before substring checks.
    low = " ".join(out.lower().split())
    assert "pinning" in low or "cpuset" in low or "affinity" in low
    assert "opt-in" in low and "off by default" in low


def test_run_help_has_no_rust_binary_mention() -> None:
    """The Python help must describe the PYTHON tool only - no Rust binary / cargo."""
    _, out, _ = _capture_help(["run", "--help"])
    low = out.lower()
    assert "cargo" not in low
    assert "rs/target" not in low
    assert "rust binary" not in low


@pytest.mark.parametrize("command", ["run", "pin-run"])
def test_core_count_must_be_positive_usage_error(command: str) -> None:
    argv = [command, "--cores", "0"]
    if command == "run":
        argv.extend(["--dag", "missing.json"])
    else:
        argv.extend(["--", "true"])
    with pytest.raises(SystemExit) as raised:
        main(argv)
    assert raised.value.code == 2


@pytest.mark.parametrize(
    ("sub", "expected"),
    [
        ("sweep", "--step"),
        ("plan", "--planner"),
        ("list", "--dag"),
        ("ascii", "--dag"),
        ("json", "--dag"),
        ("yaml", "--dag"),
    ],
)
def test_subcommand_help_exits_zero_and_lists_flags(sub: str, expected: str) -> None:
    code, out, _ = _capture_help([sub, "--help"])
    assert code == 0
    assert expected in out


def test_summary_top_help_describes_local_plan_and_single_input_merge() -> None:
    code, out, _ = _capture_help(["summary", "--help"])
    normalized = " ".join(out.lower().split())

    assert code == 0
    assert "build a plan from a summary json and dag" in normalized
    assert "merge one or more summary json files" in normalized
    assert "plan a summary sync from a backend spec" not in normalized


@pytest.mark.parametrize(
    "args",
    [
        ["summary", "build", "unexpected"],
        [
            "summary",
            "plan",
            "unexpected",
            "--summary",
            "missing-summary.json",
            "--dag",
            "missing-dag.json",
        ],
        ["summary", "stats", "one.json", "two.json"],
        ["summary", "merge"],
        ["summary", "build", "--reservoir-cap", "nope"],
        ["summary", "build", "--reservoir-cap", "1_0"],
        ["summary", "build", "--reservoir-cap", " 10"],
        ["summary", "build", "--reservoir-cap", "10 "],
        ["summary", "build", "--reservoir-cap", "١٠"],
        ["summary", "build", "--reservoir-cap", "0"],
        ["summary", "build", "--reservoir-cap", "9223372036854775808"],
        ["summary", "merge", "missing.json", "--reservoir-cap", "-1"],
        [
            "summary",
            "plan",
            "--summary",
            "missing-summary.json",
            "--dag",
            "missing-dag.json",
            "--planner",
            "unknown",
        ],
        [
            "summary",
            "plan",
            "--summary",
            "missing-summary.json",
            "--dag",
            "missing-dag.json",
            "--format",
            "xml",
        ],
        ["summary", "build", "--perf-dir"],
        ["summary", "build", "--", "--looks-like-an-option"],
        ["summary", "plan", "--summary", "--dag", "missing-dag.json"],
    ],
)
def test_summary_action_schema_rejects_invalid_invocations(args: list[str]) -> None:
    with pytest.raises(SystemExit) as raised:
        main(args)

    assert raised.value.code == 2


def test_python_quickstart_mentions_python_install_only() -> None:
    """The Python quickstart install step must mention pip, never cargo/the Rust binary."""
    rc, out, _ = _capture(["quickstart"])
    assert rc == 0
    low = out.lower()
    assert "pip install" in low
    assert "cargo" not in low
    assert "rs/target" not in low


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
    # --max-mem picks a memory-aware active-step ceiling; a passing DAG still exits 0.
    with tempfile.TemporaryDirectory() as tmp:
        dag = _demo_path(tmp)
        rc, _, err = _capture(["run", "--max-mem", "8G", "--dag", dag, "-q", _ACF])
        assert rc == 0
        assert "--max-mem 8G" in err  # the sizing decision is surfaced


def test_run_max_mem_no_throttle_note() -> None:
    # A DAG with no per-step rss_baseline_bytes: the modeled footprint collapses to the
    # mem_cap_floor_bytes floor, so a budget at/above the floor picks the full CPU-count ceiling
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


def test_run_jobs_and_max_mem_control_independent_limits() -> None:
    # --jobs remains the CPU-width budget while --max-mem independently derives max active steps.
    with tempfile.TemporaryDirectory() as tmp:
        dag = _demo_path(tmp)
        rc, _, err = _capture(["run", "--jobs", "2", "--max-mem", "8G", "--dag", dag, "-q", _ACF])
        assert rc == 0
        assert "--max-mem 8G -> --max-steps" in err


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
        assert "1 passed, 0 failed, 0 aborted, 0 intentionally skipped, 0 dependency-skipped" in err


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
        assert "2 passed, 0 failed, 0 aborted, 0 intentionally skipped, 0 dependency-skipped" in err


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


def test_unsafe_no_cgroups_deliberately_skips_boxing() -> None:
    # --unsafe-no-cgroups is the DELIBERATE opt-out: it skips scope bring-up entirely and runs
    # unboxed even where boxing IS available (distinct from --allow-cgroup-failure's
    # capability fallback). It must exit 0, emit a LOUD reviewable warning naming the flag, and
    # never claim boxing is ACTIVE. Run in a SUBPROCESS so any (absent) re-exec is real.
    import os

    with tempfile.TemporaryDirectory() as tmp:
        dag = _demo_path(tmp)
        # Do NOT force the re-exec skip: the point is that the deliberate opt-out short-circuits
        # regardless of whether boxing could have been established.
        env = dict(os.environ)
        result = subprocess.run(
            [sys.executable, "-m", "safe_ci_dag_runner", "run", "--dag", dag, "-q",
             "--unsafe-no-cgroups"],
            capture_output=True,
            text=True,
            env=env,
        )
        assert result.returncode == 0, result.stderr
        assert "DELIBERATELY UNBOXED via --unsafe-no-cgroups" in result.stderr
        assert "cgroup boxing ACTIVE" not in result.stderr


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


def test_default_small_cpu_cap_is_enforced_and_allows_compliant_work() -> None:
    # Plant both sides of the forcing default through the real cgroup path. Each command first
    # reads its own cpu.max, so a pass/failure cannot be attributed to merely configuring the
    # model without applying the one-core quota in the kernel.
    import os

    command = (
        "python3 -c 'import pathlib,time; "
        "cg=next(x.split(\":\",2)[2].strip() for x in open(\"/proc/self/cgroup\") "
        "if x.startswith(\"0::\")); "
        "quota=pathlib.Path(\"/sys/fs/cgroup\"+cg+\"/cpu.max\").read_text().strip(); "
        "assert quota==\"100000 100000\",quota; "
        "start=time.process_time(); exec(\"while time.process_time()-start < {seconds}: pass\")'"
    )

    def run_cpu_burn(tmp: str, label: str, seconds: int) -> subprocess.CompletedProcess[str]:
        dag = (
            '{"steps": [{"group": "cpu", "job": "' + label + '", '
            '"desc": "undeclared one-core CPU burn", "cmd": '
            + json.dumps(command.format(seconds=seconds))
            + "}]}"
        )
        path = Path(tmp) / f"{label}.json"
        path.write_text(dag, encoding="utf-8")
        env = {k: v for k, v in os.environ.items() if k not in ("CI", "GITHUB_ACTIONS")}
        return subprocess.run(
            [sys.executable, "-m", "safe_ci_dag_runner", "run", "--dag", str(path),
             "-q", "--no-profile"],
            capture_output=True,
            text=True,
            env=env,
        )

    with tempfile.TemporaryDirectory() as tmp:
        compliant = run_cpu_burn(tmp, "compliant-default", 1)
        if compliant.returncode == 3:
            pytest.skip(
                "cgroup boxing unavailable (need cgroup-v2 + a working systemd --user scope); "
                f"cannot verify default CPU caps here. Details:\n{compliant.stderr}"
            )
        compliant_output = compliant.stdout + compliant.stderr
        assert compliant.returncode == 0, (
            "an undeclared workload below the 10-second default must pass after reading back "
            f"the one-core kernel quota; got {compliant.returncode}\n{compliant_output}"
        )

        breach = run_cpu_burn(tmp, "breach-default", 12)
        breach_output = breach.stdout + breach.stderr
        assert breach.returncode == 1, (
            "an undeclared workload exceeding 10 CPU-seconds must fail; "
            f"got {breach.returncode}\n{breach_output}"
        )
        assert "CPU-TIMEOUT >10s cpu" in breach_output, (
            "expected the named default CPU-time cap, not a generic process failure:\n"
            f"{breach_output}"
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


def test_cores_flag_refuses_unboxed_soft_affinity() -> None:
    # `--allow-cgroup-failure` deliberately leaves this process outside an owned runner scope.
    # `--cores` must therefore fail closed before launching the step; inherited process affinity
    # is escapable and cannot enforce a collision-free reservation.
    with tempfile.TemporaryDirectory() as tmp:
        dag = _one_step_dag(tmp, "echo SHOULD_NOT_RUN")
        rc, out, err = _capture(
            ["run", "--dag", dag, "--cores", "1", "-q", "--no-profile", _ACF]
        )
        assert rc == 3
        assert "SHOULD_NOT_RUN" not in out
        assert "hard cgroup cpuset unavailable; refusing to run" in err


def test_boxed_stdin_dag_survives_scope_reexec() -> None:
    import os

    symlink = Path(__file__).resolve().parent.parent / "bin" / "safe-ci-dag-runner"
    dag = (
        '{"steps":[{"group":"stress","job":"singleton","cmd":"sleep 1",'
        '"hint":{"hard_mem_max_bytes":67108864}}]}'
    )
    env = {k: v for k, v in os.environ.items() if k not in ("CI", "GITHUB_ACTIONS")}
    proc = subprocess.run(
        [
            sys.executable,
            str(symlink),
            "run",
            "--dag",
            "-",
            "--stress",
            "3",
            "--jobs",
            "3",
            "--no-profile",
            "--no-profile-feedback",
            "-q",
        ],
        input=dag,
        capture_output=True,
        text=True,
        env=env,
    )
    combined = proc.stdout + proc.stderr
    if proc.returncode == 3:
        pytest.skip(
            "cgroup boxing unavailable here; boxed stdin ordering cannot run on this host.\n"
            + combined
        )
    assert proc.returncode == 0, combined
    assert "containment OBSERVED" in combined
    assert "invalid JSON" not in combined
    assert "stress.singleton: 3/3 passed" in proc.stdout
    assert "maximum concurrent steps: 3 (--max-steps 3; --jobs 3 aggregate CPU jobs)" in proc.stdout


# --------------------------------------------------------------------------- --stress
def _one_step_dag(tmp: str, cmd: str = "true") -> str:
    path = Path(tmp) / "dag.json"
    path.write_text(
        '{"steps": [{"group": "g", "job": "j", "cmd": "%s"}]}' % cmd, encoding="utf-8"
    )
    return str(path)


def test_stress_reports_ratio_all_pass() -> None:
    # --stress N duplicates the step N times, runs the copies in parallel, and prints the
    # per-copy PASS/FAIL ratio (the finding). A passing step gives N/N.
    with tempfile.TemporaryDirectory() as tmp:
        dag = _one_step_dag(tmp, "sleep 0.3")
        rc, out, err = _capture(["run", "--dag", dag, "--stress", "3", "-q", _ACF])
        assert rc == 0
        assert "stress results (3 generated graph copies):" in out
        assert "g.j: 3/3 passed" in out
        assert "maximum concurrent steps: 3 (--max-steps" in out
        # All 3 copies actually ran (the run summary counts every copy).
        assert "3 passed, 0 failed" in err
        # The memory-safe OK line names the derivation.
        assert "--stress 3: OK" in err and "max safe" in err


def test_stress_single_node_via_only() -> None:
    # The common case: stress ONE suspect node selected with --only (its deps are dropped).
    dag = (
        '{"steps": ['
        '{"group": "build", "job": "app", "cmd": "true"},'
        '{"group": "test", "job": "unit", "cmd": "sleep 0.3", "deps": ["build.app"]}'
        "]}"
    )
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "dag.json"
        path.write_text(dag, encoding="utf-8")
        rc, out, err = _capture(
            ["run", "--dag", str(path), "--only", "test.unit", "--stress", "4", "-q", _ACF]
        )
        assert rc == 0
        assert "test.unit: 4/4 passed" in out
        # Only the selected node was stressed (build.app was not run).
        assert "build.app" not in out
        assert "4 passed, 0 failed" in err


def test_stress_lists_failing_copies() -> None:
    # A failing step fails every copy; --stress reports the ratio AND names the failed copies
    # (it implies --keep-going, so a failure does NOT eager-cancel the siblings' verdicts).
    with tempfile.TemporaryDirectory() as tmp:
        dag = _one_step_dag(tmp, "false")
        rc, out, _ = _capture(["run", "--dag", dag, "--stress", "3", "-q", _ACF])
        assert rc == 1
        assert "g.j: 0/3 passed" in out
        assert "3 FAILED: #1, #2, #3" in out


def test_stress_refuses_when_exceeds_box_memory() -> None:
    # A step whose per-copy footprint is enormous makes even a small N exceed the box budget:
    # the run REFUSES LOUDLY (exit 2) rather than silently OOMing sibling work.
    with tempfile.TemporaryDirectory() as tmp:
        dag = Path(tmp) / "dag.json"
        # 500 TiB per-copy hard cap — larger than any real box budget.
        dag.write_text(
            '{"steps": [{"group": "g", "job": "j", "cmd": "true",'
            ' "hint": {"hard_mem_max_bytes": 549755813888000}}]}',
            encoding="utf-8",
        )
        rc, _, err = _capture(["run", "--dag", str(dag), "--stress", "2", "-q", _ACF])
        assert rc == 2
        assert "REFUSED" in err
        assert "per-copy footprint" in err
        assert "max safe --stress" in err


def test_stress_generation_removes_named_resource_serialization() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        dag = Path(tmp) / "dag.json"
        dag.write_text(
            '{"resource_caps":{"exclusive":1},"steps":['
            '{"group":"g","job":"j","cmd":"sleep 0.3",'
            '"hint":{"resources":{"exclusive":1}}}]}',
            encoding="utf-8",
        )
        rc, out, err = _capture(
            ["run", "--dag", str(dag), "--stress", "4", "-q", _ACF]
        )
        assert rc == 0, err
        assert "g.j: 4/4 passed" in out
        assert "maximum concurrent steps: 4 (--max-steps" in out


def test_stress_generated_graph_has_no_named_resource_scheduling() -> None:
    from safe_ci_dag_runner.cli import _expand_stress
    from safe_ci_dag_runner.io import dag_from_json
    from safe_ci_dag_runner.scheduler import run_dag

    cfg = dag_from_json(
        '{"resource_caps":{"exclusive":1},"steps":['
        '{"group":"g","job":"j","cmd":"sleep 0.15",'
        '"hint":{"resources":{"exclusive":1}}}]}'
    )
    result = run_dag(_expand_stress(cfg, 4), jobs=4, keep_going=True, verbosity=0)
    assert len(result.outcomes) == 4
    assert result.max_concurrent_steps == 4


def test_stress_defaults_max_steps_to_jobs() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        dag = _one_step_dag(tmp, "sleep 1")
        rc, out, err = _capture(
            ["run", "--dag", dag, "--stress", "4", "--jobs", "3", "-q", _ACF]
        )
        assert rc == 0, err
        assert "g.j: 4/4 passed" in out
        assert "maximum concurrent steps: 3 (--max-steps 3; --jobs 3 aggregate CPU jobs)" in out


def test_stress_n_must_be_positive() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        dag = _one_step_dag(tmp, "true")
        rc, _, err = _capture(["run", "--dag", dag, "--stress", "0", "-q", _ACF])
        assert rc == 2
        assert "--stress N must be >= 1" in err


def test_expand_stress_replicates_steps_and_rewires_deps() -> None:
    # Unit test of the fan-out: N shards, distinct #NN suffixes, intra-shard deps rewired.
    from safe_ci_dag_runner.cli import _expand_stress
    from safe_ci_dag_runner.io import dag_from_json

    cfg = dag_from_json(
        '{"steps": ['
        '{"group": "build", "job": "app", "cmd": "true"},'
        '{"group": "test", "job": "unit", "cmd": "true", "deps": ["build.app"]}'
        "]}"
    )
    expanded = _expand_stress(cfg, 3)
    tags = sorted(s.tag for s in expanded.steps)
    assert tags == [
        "build.app#1", "build.app#2", "build.app#3",
        "test.unit#1", "test.unit#2", "test.unit#3",
    ]
    by_tag = expanded.by_tag()
    # Each shard's test depends on the SAME shard's build, not a cross-shard build.
    assert by_tag["test.unit#2"].deps == ["build.app#2"]
    assert expanded.resource_caps == {}
    assert all(not step.hint.resources for step in expanded.steps)
    # n <= 1 is a no-op.
    assert _expand_stress(cfg, 1).steps == cfg.steps


# --------------------------------------------------------------------------- --args passthrough
def test_args_passthrough_substitutes_placeholder() -> None:
    # A step DECLARES it accepts args via the {args} token; --args is forwarded there verbatim.
    # The command echoes its args, so the captured detail proves substitution happened.
    with tempfile.TemporaryDirectory() as tmp:
        dag = Path(tmp) / "dag.json"
        dag.write_text(
            '{"steps": [{"group": "t", "job": "unit",'
            ' "cmd": "echo GOT: {args}; test \\"{args}\\" = \\"-k foo\\""}]}',
            encoding="utf-8",
        )
        # -v (not -q) so the passing step's one-line summary is printed.
        # Value starts with '-', so the --args=VALUE form is required (argparse).
        rc, out, err = _capture(
            ["run", "--dag", str(dag), "--args=-k foo", _ACF]
        )
        assert rc == 0, err
        assert "GOT: -k foo" in out


def test_args_passthrough_errors_when_no_step_declares() -> None:
    # --args given but no selected step has {args}: refuse loudly (never silently drop the args).
    with tempfile.TemporaryDirectory() as tmp:
        dag = _one_step_dag(tmp, "true")
        rc, _, err = _capture(["run", "--dag", dag, "--args=-k foo", "-q", _ACF])
        assert rc == 2
        assert "no selected step declares" in err


def test_args_declared_but_not_passed_removes_token() -> None:
    # A step declaring {args} but run WITHOUT --args has the token removed so it still runs.
    with tempfile.TemporaryDirectory() as tmp:
        dag = Path(tmp) / "dag.json"
        dag.write_text(
            '{"steps": [{"group": "t", "job": "unit", "cmd": "echo done {args}"}]}',
            encoding="utf-8",
        )
        rc, out, err = _capture(["run", "--dag", str(dag), _ACF])
        assert rc == 0, err
        assert "done" in out
        assert "{args}" not in out


def test_args_composes_with_only_and_stress() -> None:
    # The full owner scenario: scope to one node (--only), pass a specific test case (--args),
    # multiply it (--stress). All three compose; the per-copy ratio is reported.
    with tempfile.TemporaryDirectory() as tmp:
        dag = Path(tmp) / "dag.json"
        dag.write_text(
            '{"steps": ['
            '{"group": "build", "job": "app", "cmd": "true"},'
            '{"group": "dbi", "job": "file_metadata",'
            ' "cmd": "sleep 0.3; test \\"{args}\\" = \\"--case xyz\\"", "deps": ["build.app"]}'
            "]}",
            encoding="utf-8",
        )
        rc, out, err = _capture(
            ["run", "--dag", str(dag), "--only", "dbi.file_metadata",
             "--args=--case xyz", "--stress", "5", "-q", _ACF]
        )
        assert rc == 0, err
        assert "dbi.file_metadata: 5/5 passed" in out
        assert "build.app" not in out
