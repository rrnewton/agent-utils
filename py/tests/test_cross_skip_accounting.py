"""A skipped differential check is its own result, never a pass.

``cross/differential.py`` cannot run every case everywhere: some need cgroup-v2 boxing, some must
not create a live systemd scope from inside a parent-owned delegated cgroup, some need a HARD
cpuset pin, and some need two online CPUs. Such a check used to be recorded with ``Report.ok``,
so a run in which none of those checks executed printed the same ``OK - N checks`` line and exit
status as a run in which they all passed.

These tests pin the replacement contract without running the minutes-long harness:

* a skip never increments the pass count and is listed by name with a total;
* a run that claims full coverage (no ``--allow-skip``) exits nonzero when anything was skipped;
* a run that declares itself partial prints PARTIAL, not OK, and still lists every skip;
* each lane that runs the harness states its coverage policy explicitly.
"""

from __future__ import annotations

import importlib.util
import sys
from collections.abc import Callable
from pathlib import Path
from types import ModuleType

import pytest

from dagrun.io import dag_from_path

REPO_ROOT = Path(__file__).resolve().parents[2]
BOXING_LABELS = [
    "profile-timeseries",
    "operator-build-width:stated",
    "operator-build-width:unstated",
    "boxed-cpu-bandwidth",
]


def _differential() -> ModuleType:
    # differential.py imports its sibling modules by bare name, as the harness runs it by path.
    cross = str(REPO_ROOT / "cross")
    if cross not in sys.path:
        sys.path.insert(0, cross)
    spec = importlib.util.spec_from_file_location(
        "_cross_differential_skip_accounting", REPO_ROOT / "cross" / "differential.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _unexpected_run(*_args: object, **_kwargs: object) -> object:
    raise AssertionError("a check that must be skipped spawned an engine")


def _noop(*_args: object, **_kwargs: object) -> None:
    return None


def _dagrun_with_only_boxing_checks(
    differential: ModuleType, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Drive the real ``main`` -> ``compare_dagrun`` summary with every non-boxing case stubbed.

    The outer scheduler is made to say that it has no cgroup subtree to delegate, which is the
    state of a hosted runner started with ``--allow-cgroup-failure``. The three boxing-only
    comparisons then take their real skip paths, and nothing may spawn an engine.
    """

    kept = {
        "compare_dagrun",
        "compare_profile_timeseries_trace",
        "compare_operator_build_width",
        "compare_boxed_cpu_bandwidth",
    }
    for name in dir(differential):
        if name.startswith("compare_") and name not in kept:
            monkeypatch.setattr(differential, name, _noop)
    for name in (
        "representative_fixtures",
        "randomized_fixtures",
        "example_fixtures",
        "yaml_fixture_paths",
    ):
        monkeypatch.setattr(differential, name, lambda *_args: [])
    monkeypatch.setattr(differential, "rs_command", lambda _tool: ["rust"])
    monkeypatch.setattr(differential, "run", _unexpected_run)
    monkeypatch.setattr(differential, "_effective_validation_jobs", lambda: 4)
    monkeypatch.delenv("AGENT_UTILS_VALIDATION_JOBS", raising=False)
    monkeypatch.delenv("AGENT_UTILS_CROSS_ALLOW_SKIP", raising=False)
    monkeypatch.delenv("DAGRUN_DELEGATED_CGROUP", raising=False)
    monkeypatch.setenv("DAGRUN_DELEGATED_UNBOXED", "1")
    monkeypatch.setenv("DAGRUN_OUTER_RUN", "outer-run")


def test_a_skipped_check_is_not_counted_as_a_pass() -> None:
    differential = _differential()
    report = differential.Report()

    report.ok("ran")
    report.skip("did-not-run", "boxing", "no scope")

    assert report.checks == 1
    assert report.failures == []
    assert [(skip.label, skip.kind, skip.reason) for skip in report.skipped] == [
        ("did-not-run", "boxing", "no scope")
    ]
    with pytest.raises(ValueError, match="unknown skip kind"):
        report.skip("typo", "boxng", "misspelled kind")


def test_full_coverage_verdict_refuses_any_skip(capsys: pytest.CaptureFixture[str]) -> None:
    differential = _differential()
    report = differential.Report()
    report.ok("ran")
    report.skip("did-not-run", "boxing", "no scope")

    status = differential.coverage_verdict("demo", report, "1 checks agree", frozenset())
    out = capsys.readouterr().out

    assert status == 1
    assert "cross[demo]: SKIPPED/UNVERIFIED 1 check(s), not counted as passes:" in out
    assert "SKIPPED [boxing; REQUIRED] did-not-run: no scope" in out
    assert "cross[demo]: INCOMPLETE - 1 checks agree, but 1 required check(s)" in out
    assert "OK -" not in out


def test_an_allowance_covers_only_its_own_kind(capsys: pytest.CaptureFixture[str]) -> None:
    differential = _differential()
    report = differential.Report()
    report.skip("boxed", "boxing", "no scope")
    report.skip("pinned", "hard-cpuset", "refused")

    status = differential.coverage_verdict("demo", report, "0 checks agree", frozenset({"boxing"}))
    out = capsys.readouterr().out

    assert status == 1
    assert "SKIPPED [boxing; allowed] boxed: no scope" in out
    assert "SKIPPED [hard-cpuset; REQUIRED] pinned: refused" in out
    assert "did not run: pinned." in out


def test_a_run_without_skips_is_still_ok(capsys: pytest.CaptureFixture[str]) -> None:
    differential = _differential()
    report = differential.Report()
    report.ok("ran")

    assert differential.coverage_verdict("demo", report, "1 checks agree", frozenset()) == 0
    assert capsys.readouterr().out == "cross[demo]: OK - 1 checks agree\n"


def test_full_dagrun_run_with_skipped_boxing_checks_exits_nonzero(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    differential = _differential()
    _dagrun_with_only_boxing_checks(differential, monkeypatch)

    status = differential.main(["--tool", "dagrun"])
    out = capsys.readouterr().out

    assert status == 1
    assert "cross[dagrun]: SKIPPED/UNVERIFIED 4 check(s), not counted as passes:" in out
    for label in BOXING_LABELS:
        assert f"SKIPPED [boxing; REQUIRED] {label}:" in out
    assert "cross[dagrun]: INCOMPLETE - 0 checks across 0 fixtures agree" in out
    assert "cross[dagrun]: OK" not in out


@pytest.mark.parametrize(
    ("argv", "environment", "source"),
    [
        (["--allow-skip", "boxing"], {}, "--allow-skip"),
        (["--allow-skip", "multi-cpu,boxing"], {}, "--allow-skip"),
        ([], {"AGENT_UTILS_CROSS_ALLOW_SKIP": "boxing"}, "AGENT_UTILS_CROSS_ALLOW_SKIP"),
    ],
)
def test_partial_dagrun_run_says_partial_and_prints_its_skips(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    argv: list[str],
    environment: dict[str, str],
    source: str,
) -> None:
    differential = _differential()
    _dagrun_with_only_boxing_checks(differential, monkeypatch)
    for key, value in environment.items():
        monkeypatch.setenv(key, value)

    status = differential.main(["--tool", "dagrun", *argv])
    out = capsys.readouterr().out

    assert status == 0
    assert out.startswith("cross: PARTIAL-coverage run declared;")
    assert f"boxing (from {source})" in out
    assert "cross[dagrun]: SKIPPED/UNVERIFIED 4 check(s), not counted as passes:" in out
    for label in BOXING_LABELS:
        assert f"SKIPPED [boxing; allowed] {label}:" in out
    assert "cross[dagrun]: PARTIAL - 0 checks across 0 fixtures agree" in out
    assert "4 check(s) were skipped and are UNVERIFIED (allowed skip kinds: boxing)" in out
    assert "cross[dagrun]: OK" not in out


@pytest.mark.parametrize(
    ("argv", "environment"),
    [
        (["--allow-skip", "boxng"], {}),
        ([], {"AGENT_UTILS_CROSS_ALLOW_SKIP": "boxing,everything"}),
    ],
)
def test_an_unknown_skip_kind_is_refused(
    monkeypatch: pytest.MonkeyPatch, argv: list[str], environment: dict[str, str]
) -> None:
    differential = _differential()
    _dagrun_with_only_boxing_checks(differential, monkeypatch)
    for key, value in environment.items():
        monkeypatch.setenv(key, value)

    with pytest.raises(SystemExit) as refused:
        differential.main(["--tool", "dagrun", *argv])
    assert refused.value.code == 2


def _pin_run_engine(
    differential: ModuleType, *, hard_refusal: bool
) -> Callable[[object, tuple[str, ...]], object]:
    """Fake both engines for ``compare_pin_run``, optionally refusing every HARD pin."""

    outcome = differential.Outcome

    def fake_run(_cmd: object, args: tuple[str, ...], *_rest: object, **_kw: object) -> object:
        command = args[args.index("--") + 1 :] if "--" in args else ()
        if "--" not in args:
            return outcome(2, "", "a command is required")
        if args[2] == "0":
            return outcome(2, "", "--cores must be >= 1")
        if command[:1] == ("/definitely/missing/command",):
            return outcome(3, "", "cannot execute")
        if hard_refusal:
            return outcome(3, "", "HARD pin unavailable")
        return outcome(143 if "os.kill" in " ".join(command) else 0, "", "")

    return fake_run


def test_a_refused_hard_pin_is_parity_but_the_pin_itself_is_skipped(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    differential = _differential()
    monkeypatch.delenv("DAGRUN_DELEGATED_CGROUP", raising=False)
    monkeypatch.delenv("DAGRUN_DELEGATED_UNBOXED", raising=False)
    monkeypatch.setattr(differential, "run", _pin_run_engine(differential, hard_refusal=True))
    report = differential.Report()

    differential.compare_pin_run(["python"], ["rust"], report)

    # missing-command, nonpositive, identical HARD refusal x2, missing-executable.
    assert report.checks == 5
    assert report.failures == []
    assert [(skip.label, skip.kind) for skip in report.skipped] == [
        ("pin-run:reserve-apply-release", "hard-cpuset"),
        ("pin-run:signal-status", "hard-cpuset"),
    ]


def test_a_working_hard_pin_records_no_skip(monkeypatch: pytest.MonkeyPatch) -> None:
    differential = _differential()
    monkeypatch.delenv("DAGRUN_DELEGATED_CGROUP", raising=False)
    monkeypatch.delenv("DAGRUN_DELEGATED_UNBOXED", raising=False)
    monkeypatch.setattr(differential, "run", _pin_run_engine(differential, hard_refusal=False))
    report = differential.Report()

    differential.compare_pin_run(["python"], ["rust"], report)

    assert report.checks == 5
    assert report.failures == []
    assert report.skipped == []


def test_delegated_pin_run_lists_both_live_checks_it_did_not_run(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    differential = _differential()
    monkeypatch.setenv("DAGRUN_DELEGATED_CGROUP", "/delegated")
    monkeypatch.setattr(differential, "run", _pin_run_engine(differential, hard_refusal=False))
    report = differential.Report()

    differential.compare_pin_run(["python"], ["rust"], report)

    # missing-command, nonpositive, missing-executable: the only cases that actually ran.
    assert report.checks == 3
    assert [(skip.label, skip.kind) for skip in report.skipped] == [
        ("pin-run:reserve-apply-release", "delegated-live-scope"),
        ("pin-run:signal-status", "delegated-live-scope"),
    ]


def test_each_validation_lane_states_its_coverage_policy() -> None:
    """The delegated validation lanes are partial only for the structural live-scope skip.

    Both run inside a parent-owned delegated cgroup, where the harness must not create a live
    systemd scope. Boxing stays required: a boxed local run that loses boxing fails. A lane
    that cannot box must say so itself through ``AGENT_UTILS_CROSS_ALLOW_SKIP``.
    """

    by_tag = dag_from_path(REPO_ROOT / "validation.dag.yaml").by_tag()
    expected = {
        "cross.dagrun.differential": "--tool dagrun --allow-skip delegated-live-scope",
        "cross.dagrun.cpuset-differential": (
            "--tool cpuset-alloc --allow-skip delegated-live-scope"
        ),
    }
    for tag, arguments in expected.items():
        assert by_tag[tag].cmd == f"python3 cross/differential.py {arguments}"
    for tag, step in by_tag.items():
        if "cross/differential.py" in step.cmd and tag not in expected:
            assert "--allow-skip" not in step.cmd, tag
            assert "AGENT_UTILS_CROSS_ALLOW_SKIP" not in step.cmd, tag

    makefile = (REPO_ROOT / "Makefile").read_text(encoding="utf-8")
    assert "python3 cross/differential.py --tool all\n" in makefile
    assert "--allow-skip" not in makefile


def test_hosted_lanes_that_cannot_box_declare_themselves_partial() -> None:
    workflows = REPO_ROOT / ".github" / "workflows"
    for name in ("cross-dagrun.yml", "nightly-all.yml"):
        text = (workflows / name).read_text(encoding="utf-8")
        assert "BOX_FLAGS: --allow-cgroup-failure" in text, name
        assert "AGENT_UTILS_CROSS_ALLOW_SKIP: boxing" in text, name
