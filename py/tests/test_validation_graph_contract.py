"""Structural guards for the composed repository validation graph."""

from __future__ import annotations

import ast
import importlib.util
import os
import shlex
import subprocess
import sys
from collections import Counter
from pathlib import Path
from types import ModuleType

import pytest

from dagrun.io import dag_from_path
from dagrun.scheduler import cap_config_max_cpus


SCRIPTS = Path(__file__).resolve().parents[2] / "scripts"
sys.path.insert(0, str(SCRIPTS))

from validate import GROUP_LABELS, selected_timing_gates, selection_for  # noqa: E402
from dagrun.cli import _select_steps_by_labels  # noqa: E402


REPO_ROOT = Path(__file__).resolve().parents[2]
DOCS_STEP = "repository.docs.embedded-userguides"
RUST_BUILD_STEP = "repository.build.rust-launchers"


def _example_checker() -> ModuleType:
    spec = importlib.util.spec_from_file_location(
        "_dagrun_example_checker_under_test", SCRIPTS / "check_dagrun_examples.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _listed(command: str) -> set[str]:
    completed = subprocess.run(
        [sys.executable, *command.split()],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 0, completed.stderr
    return {line.split(" ", 1)[0] for line in completed.stdout.splitlines() if line}


def _flag_values(command: str, flag: str) -> list[str]:
    tokens = shlex.split(command)
    return [tokens[index + 1] for index, token in enumerate(tokens[:-1]) if token == flag]


def _workspace_crates() -> set[str]:
    workspace = (REPO_ROOT / "rs/Cargo.toml").read_text(encoding="utf-8")
    members_text = workspace.split("members = [", 1)[1].split("]", 1)[0]
    members = [line.strip().strip('",') for line in members_text.splitlines() if '"' in line]
    names: set[str] = set()
    for member in members:
        manifest = (REPO_ROOT / "rs" / member / "Cargo.toml").read_text(encoding="utf-8")
        package = manifest.split("[package]", 1)[1].split("[", 1)[0]
        name_line = next(line for line in package.splitlines() if line.startswith("name = "))
        names.add(name_line.split('"', 2)[1])
    return names


def test_package_nodes_cover_every_artifact_once_after_generated_docs() -> None:
    config = dag_from_path(REPO_ROOT / "validation.dag.yaml")
    python_steps = [
        step for step in config.steps if "scripts/check_python_packages.py" in step.cmd
    ]
    rust_steps = [
        step for step in config.steps if "scripts/check_rust_packages.py" in step.cmd
    ]

    python_projects = Counter(
        project for step in python_steps for project in _flag_values(step.cmd, "--project")
    )
    rust_crates = Counter(
        crate for step in rust_steps for crate in _flag_values(step.cmd, "--crate")
    )

    assert python_projects == Counter(
        {name: 1 for name in _listed("scripts/check_python_packages.py --list-projects")}
    )
    assert rust_crates == Counter(
        {name: 1 for name in _listed("scripts/check_rust_packages.py --list-crates")}
    )
    assert all(DOCS_STEP in step.deps for step in python_steps)
    assert all(RUST_BUILD_STEP in step.deps for step in rust_steps)
    build = next(step for step in config.steps if step.tag == RUST_BUILD_STEP)
    assert DOCS_STEP in build.deps


def test_javascript_validation_entrypoints_are_owned_once() -> None:
    config = dag_from_path(REPO_ROOT / "validation.dag.yaml")
    commands = "\n".join(step.cmd for step in config.steps)

    # Playwright discovers every *.spec.js file. Every standalone *.test.cjs benchmark is instead
    # required to occur exactly once in a graph command, so adding a third cannot pass silently.
    playwright = (REPO_ROOT / "py/tests/js/browser/playwright.config.cjs").read_text(
        encoding="utf-8"
    )
    assert "*.spec.js" in playwright
    assert list((REPO_ROOT / "py/tests/js/browser").glob("*.spec.js"))
    benchmark_suites = sorted(
        (REPO_ROOT / "py/tests/js/browser").glob("*.test.cjs")
    )
    assert benchmark_suites
    for path in benchmark_suites:
        suite = path.name
        assert commands.count(suite) == 1

    # The vibe page suites are owned by their Rust cargo-test harnesses. Running them directly in
    # the graph as well would double their several-hundred-case cost.
    harnesses = [
        path.read_text(encoding="utf-8")
        for path in sorted((REPO_ROOT / "vibe-talk/tests").glob("*.rs"))
    ]
    vibe_suites = sorted((REPO_ROOT / "vibe-talk/tests/js").glob("*.test.mjs"))
    assert vibe_suites
    for path in vibe_suites:
        suite = path.name
        assert suite not in commands
        assert sum(suite in harness for harness in harnesses) == 1


def test_rust_and_cross_nodes_cover_their_registries_once() -> None:
    config = dag_from_path(REPO_ROOT / "validation.dag.yaml")
    rust_tests = Counter(
        crate
        for step in config.steps
        if step.tag.startswith("rust.")
        for crate in _flag_values(step.cmd, "-p")
    )
    cross_tools = Counter(
        tool
        for step in config.steps
        if step.tag.startswith("cross.")
        for tool in _flag_values(step.cmd, "--tool")
    )

    assert rust_tests == Counter({name: 1 for name in _workspace_crates()})
    assert cross_tools == Counter(
        {name: 1 for name in _listed("cross/differential.py --list-tools")}
    )


def test_wrkslots_component_selection_includes_every_lifecycle_shard() -> None:
    config = dag_from_path(REPO_ROOT / "validation.dag.yaml")
    lifecycle = [
        step for step in config.steps if step.tag.startswith("python-lifecycle.")
    ]

    assert len(lifecycle) == 9
    assert all("component-wrkslots" in step.labels for step in lifecycle)


def test_every_example_is_validated_under_both_dagrun_engines() -> None:
    config = dag_from_path(REPO_ROOT / "validation.dag.yaml")
    example_steps = [
        step for step in config.steps if "scripts/check_dagrun_examples.py" in step.cmd
    ]

    assert Counter(
        engine for step in example_steps for engine in _flag_values(step.cmd, "--engine")
    ) == Counter({"python": 1, "rust": 1})
    assert all(step.delegated_children for step in example_steps)
    assert all(step.jobs_env == "AGENT_UTILS_VALIDATION_JOBS" for step in example_steps)

    differential = config.by_tag()["cross.dagrun.differential"]
    assert differential.jobs_env == "AGENT_UTILS_VALIDATION_JOBS"
    assert differential.hint.preferred_inner_jobs == 8
    assert all(step.hint.preferred_inner_jobs == 8 for step in example_steps)


def test_full_validation_graph_can_be_honestly_capped_to_four_cpus() -> None:
    capped = cap_config_max_cpus(dag_from_path(REPO_ROOT / "validation.dag.yaml"), 4)
    oversized = [
        (step.tag, step.hint.preferred_inner_jobs)
        for step in capped.steps
        if step.skip_reason is None
        and step.hint.preferred_inner_jobs is not None
        and step.hint.preferred_inner_jobs > 4
    ]
    assert oversized == []


def test_fixed_width_example_has_the_documented_narrow_runner_refusal() -> None:
    environment = os.environ.copy()
    python_path = str(REPO_ROOT / "py")
    inherited = environment.get("PYTHONPATH")
    environment["PYTHONPATH"] = (
        os.pathsep.join((python_path, inherited)) if inherited else python_path
    )
    completed = subprocess.run(
        [
            sys.executable,
            "-m",
            "dagrun",
            "run",
            "--dag",
            "examples/05-inner-jobs.json",
            "--max-cpus",
            "4",
            "--unsafe-no-cgroups",
            "--no-profile",
            "--no-profile-feedback",
        ],
        cwd=REPO_ROOT,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )

    assert completed.returncode == 2
    assert "cannot lower guest parallelism" in completed.stderr
    assert "build.app (preferred_inner_jobs=8)" in completed.stderr


def test_example_checker_accepts_the_documented_narrow_refusal(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    checker = _example_checker()
    example = tmp_path / "05-inner-jobs.json"
    example.write_text("{}", encoding="utf-8")
    calls: list[tuple[list[str], dict[str, object]]] = []
    profile_widths: list[int] = []

    def fake_run(argv: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        calls.append((argv, kwargs))
        return subprocess.CompletedProcess(
            argv,
            2,
            stdout="",
            stderr=(
                "dagrun: run: --max-cpus 4 cannot lower guest parallelism for step(s) "
                "that offer no width channel: build.app (preferred_inner_jobs=8)"
            ),
        )

    def fake_profile(
        _command: list[str], _environment: dict[str, str], *, validation_jobs: int
    ) -> None:
        profile_widths.append(validation_jobs)

    monkeypatch.setenv("AGENT_UTILS_VALIDATION_JOBS", "4")
    monkeypatch.setattr(checker, "_effective_validation_jobs", lambda: 16)
    monkeypatch.setattr(checker, "REPO_ROOT", tmp_path)
    monkeypatch.setattr(checker, "_examples", lambda: [example])
    monkeypatch.setattr(checker, "_engine", lambda _name: (["fake-dagrun"], {}))
    monkeypatch.setattr(checker.subprocess, "run", fake_run)
    monkeypatch.setattr(checker, "_profile_smoke", fake_profile)

    assert checker.main(["--engine", "python"]) == 0
    assert calls[0][0][-2:] == ["--max-cpus", "4"]
    assert calls[0][1]["capture_output"] is True
    assert profile_widths == [4]


def test_example_checker_runs_fixed_width_example_at_full_strength(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    checker = _example_checker()
    example = tmp_path / "05-inner-jobs.json"
    example.write_text("{}", encoding="utf-8")
    calls: list[tuple[list[str], dict[str, object]]] = []

    def fake_run(argv: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        calls.append((argv, kwargs))
        return subprocess.CompletedProcess(argv, 0)

    monkeypatch.setenv("AGENT_UTILS_VALIDATION_JOBS", "8")
    monkeypatch.setattr(checker, "_effective_validation_jobs", lambda: 16)
    monkeypatch.setattr(checker, "REPO_ROOT", tmp_path)
    monkeypatch.setattr(checker, "_examples", lambda: [example])
    monkeypatch.setattr(checker, "_engine", lambda _name: (["fake-dagrun"], {}))
    monkeypatch.setattr(checker.subprocess, "run", fake_run)
    monkeypatch.setattr(checker, "_profile_smoke", lambda *args, **kwargs: None)

    assert checker.main(["--engine", "rust"]) == 0
    assert calls[0][0][-2:] == ["--max-cpus", "8"]
    assert calls[0][1]["capture_output"] is False


def test_example_checker_rejects_a_post_spawn_fixed_width_failure(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    checker = _example_checker()
    example = tmp_path / "05-inner-jobs.json"
    example.write_text("{}", encoding="utf-8")

    def fake_run(argv: list[str], **_kwargs: object) -> subprocess.CompletedProcess[str]:
        return subprocess.CompletedProcess(
            argv,
            2,
            stdout="[build] make -j8 build\n",
            stderr=(
                "dagrun: run: --max-cpus 4 cannot lower guest parallelism for step(s) "
                "that offer no width channel: build.app (preferred_inner_jobs=8)"
            ),
        )

    monkeypatch.setenv("AGENT_UTILS_VALIDATION_JOBS", "4")
    monkeypatch.setattr(checker, "_effective_validation_jobs", lambda: 16)
    monkeypatch.setattr(checker, "REPO_ROOT", tmp_path)
    monkeypatch.setattr(checker, "_examples", lambda: [example])
    monkeypatch.setattr(checker, "_engine", lambda _name: (["fake-dagrun"], {}))
    monkeypatch.setattr(checker.subprocess, "run", fake_run)

    assert checker.main(["--engine", "python"]) == 1


def test_example_checker_width_parser_is_strict_and_effective_cap_aware() -> None:
    checker = _example_checker()
    assert checker._validation_jobs({}, effective_jobs=4) == 4
    assert checker._validation_jobs(
        {"AGENT_UTILS_VALIDATION_JOBS": "3"}, effective_jobs=4
    ) == 3
    with pytest.raises(ValueError, match="must be a positive integer"):
        checker._validation_jobs(
            {"AGENT_UTILS_VALIDATION_JOBS": "+4"}, effective_jobs=16
        )


def test_example_checker_strips_its_width_control_from_nested_engines(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    checker = _example_checker()
    monkeypatch.setenv("AGENT_UTILS_VALIDATION_JOBS", "4")
    _command, environment = checker._engine("rust")
    assert "AGENT_UTILS_VALIDATION_JOBS" not in environment


def test_fixed_memory_example_has_the_documented_small_runner_refusal(
    tmp_path: Path,
) -> None:
    checker = _example_checker()
    sweep_root = tmp_path / "sweep"
    sweep_root.mkdir()
    environment = os.environ.copy()
    python_path = str(REPO_ROOT / "py")
    inherited = environment.get("PYTHONPATH")
    environment["PYTHONPATH"] = (
        os.pathsep.join((python_path, inherited)) if inherited else python_path
    )
    environment["DAGRUN_CARGO_SWEEP_ROOT"] = str(sweep_root)
    budget = 1 << 30
    completed = subprocess.run(
        [
            sys.executable,
            "-m",
            "dagrun",
            "run",
            "--dag",
            "examples/08-dagrun-clean-build-sweep.yaml",
            "--max-cpus",
            "4",
            "--max-mem",
            str(budget),
            "--unsafe-no-cgroups",
            "--no-profile",
            "--no-profile-feedback",
        ],
        cwd=REPO_ROOT,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )

    combined = completed.stdout + completed.stderr
    assert checker._is_documented_memory_refusal(
        completed.returncode, combined, budget=budget
    )
    # The refusal is pre-spawn: the guest never created a build target directory.
    assert list(sweep_root.iterdir()) == []


@pytest.mark.parametrize("engine", ["python", "rust"])
def test_example_checker_accepts_the_documented_memory_refusal(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, engine: str
) -> None:
    checker = _example_checker()
    example = tmp_path / "08-dagrun-clean-build-sweep.yaml"
    example.write_text("{}", encoding="utf-8")
    calls: list[tuple[list[str], dict[str, object]]] = []
    # A realistic 16 GiB total-memory runner reports a little under the binary
    # 16 GiB hard cap once firmware and kernel reservations are subtracted.
    budget = checker.FIXED_MEMORY_BYTES - (512 << 20)

    def fake_run(argv: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        calls.append((argv, kwargs))
        return subprocess.CompletedProcess(
            argv,
            2,
            stdout="",
            stderr=(
                f"dagrun: --max-mem {budget}: REFUSED — minimum runnable footprint "
                f"{checker.FIXED_MEMORY_BYTES} bytes cannot fit safely within "
                f"budget {budget} bytes"
            ),
        )

    monkeypatch.setattr(checker, "_effective_validation_jobs", lambda: 4)
    monkeypatch.setattr(checker, "_effective_memory_budget_bytes", lambda: budget)
    monkeypatch.setattr(checker, "REPO_ROOT", tmp_path)
    monkeypatch.setattr(checker, "_examples", lambda: [example])
    monkeypatch.setattr(checker, "_engine", lambda _name: (["fake-dagrun"], {}))
    monkeypatch.setattr(checker.subprocess, "run", fake_run)
    monkeypatch.setattr(checker, "_profile_smoke", lambda *args, **kwargs: None)

    assert checker.main(["--engine", engine]) == 0
    assert calls[0][0][-4:] == [
        "--max-cpus",
        "4",
        "--max-mem",
        str(budget),
    ]
    assert calls[0][1]["capture_output"] is True


def test_example_checker_runs_fixed_memory_example_at_full_strength(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    checker = _example_checker()
    example = tmp_path / "08-dagrun-clean-build-sweep.yaml"
    example.write_text("{}", encoding="utf-8")
    calls: list[tuple[list[str], dict[str, object]]] = []

    def fake_run(argv: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        calls.append((argv, kwargs))
        return subprocess.CompletedProcess(argv, 0)

    monkeypatch.setattr(checker, "_effective_validation_jobs", lambda: 4)
    monkeypatch.setattr(
        checker, "_effective_memory_budget_bytes", lambda: checker.FIXED_MEMORY_BYTES
    )
    monkeypatch.setattr(checker, "REPO_ROOT", tmp_path)
    monkeypatch.setattr(checker, "_examples", lambda: [example])
    monkeypatch.setattr(checker, "_engine", lambda _name: (["fake-dagrun"], {}))
    monkeypatch.setattr(checker.subprocess, "run", fake_run)
    monkeypatch.setattr(checker, "_profile_smoke", lambda *args, **kwargs: None)

    assert checker.main(["--engine", "python"]) == 0
    assert "--max-mem" not in calls[0][0]
    assert calls[0][1]["capture_output"] is False


@pytest.mark.parametrize(
    ("returncode", "stdout", "stderr"),
    [
        # The guest started before failing: not a pre-spawn refusal.
        (
            2,
            "[build.dagrun] START clean release build of the dagrun binary\n",
            "REFUSED — minimum runnable footprint 17179869184 bytes cannot fit "
            "safely within budget 1073741824 bytes",
        ),
        # A quiet success on a too-small runner would be a silent skip.
        (0, "", ""),
        # A refusal for some other graph (smaller footprint) is not this contract.
        (
            2,
            "",
            "REFUSED — minimum runnable footprint 4294967296 bytes cannot fit "
            "safely within budget 1073741824 bytes",
        ),
    ],
)
def test_example_checker_rejects_non_refusal_memory_outcomes(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    returncode: int,
    stdout: str,
    stderr: str,
) -> None:
    checker = _example_checker()
    example = tmp_path / "08-dagrun-clean-build-sweep.yaml"
    example.write_text("{}", encoding="utf-8")

    def fake_run(argv: list[str], **_kwargs: object) -> subprocess.CompletedProcess[str]:
        return subprocess.CompletedProcess(argv, returncode, stdout=stdout, stderr=stderr)

    monkeypatch.setattr(checker, "_effective_validation_jobs", lambda: 4)
    monkeypatch.setattr(checker, "_effective_memory_budget_bytes", lambda: 1 << 30)
    monkeypatch.setattr(checker, "REPO_ROOT", tmp_path)
    monkeypatch.setattr(checker, "_examples", lambda: [example])
    monkeypatch.setattr(checker, "_engine", lambda _name: (["fake-dagrun"], {}))
    monkeypatch.setattr(checker.subprocess, "run", fake_run)

    assert checker.main(["--engine", "python"]) == 1


def test_example_checker_memory_budget_uses_the_smaller_honest_signal() -> None:
    checker = _example_checker()
    gib = 1 << 30
    assert checker._memory_budget_bytes(cgroup_cap=8 * gib, mem_total=16 * gib) == 8 * gib
    assert checker._memory_budget_bytes(cgroup_cap=32 * gib, mem_total=16 * gib) == 16 * gib
    assert checker._memory_budget_bytes(cgroup_cap=None, mem_total=16 * gib) == 16 * gib
    assert checker._memory_budget_bytes(cgroup_cap=8 * gib, mem_total=None) == 8 * gib
    assert checker._memory_budget_bytes(cgroup_cap=None, mem_total=None) is None
    # An unknown budget must not be treated as a small one: without a signal the
    # checker runs the example rather than claiming a refusal it cannot prove.
    assert not checker._expected_memory_refusal("08-dagrun-clean-build-sweep.yaml", None)
    assert checker._expected_memory_refusal(
        "08-dagrun-clean-build-sweep.yaml", checker.FIXED_MEMORY_BYTES - 1
    )
    assert not checker._expected_memory_refusal(
        "08-dagrun-clean-build-sweep.yaml", checker.FIXED_MEMORY_BYTES
    )
    assert not checker._expected_memory_refusal("05-inner-jobs.json", 1)


def test_example_checker_memory_refusal_matcher_bounds_the_named_budget() -> None:
    checker = _example_checker()
    refusal = (
        "dagrun: --max-mem 1073741824: REFUSED — minimum runnable footprint "
        "17179869184 bytes cannot fit safely within budget {budget} bytes"
    )
    # The engine may name a tighter inherited ceiling than the harness passed.
    assert checker._is_documented_memory_refusal(
        2, refusal.format(budget=1 << 29), budget=1 << 30
    )
    # A named budget above what the harness passed is not this run's refusal.
    assert not checker._is_documented_memory_refusal(
        2, refusal.format(budget=1 << 31), budget=1 << 30
    )
    # A named budget at the hard cap would have admitted the step.
    assert not checker._is_documented_memory_refusal(
        2, refusal.format(budget=checker.FIXED_MEMORY_BYTES), budget=1 << 34
    )


def test_example_checker_ancestor_cap_walk_finds_the_tightest_slice(
    tmp_path: Path,
) -> None:
    checker = _example_checker()
    root = tmp_path / "cgroup"
    leaf = root / "user.slice" / "runner.scope" / "step"
    leaf.mkdir(parents=True)
    (root / "memory.max").write_text("max\n", encoding="utf-8")
    (root / "user.slice" / "memory.max").write_text(
        f"{12 << 30}\n", encoding="utf-8"
    )
    (root / "user.slice" / "runner.scope" / "memory.max").write_text(
        "max\n", encoding="utf-8"
    )
    (leaf / "memory.max").write_text(f"{16 << 30}\n", encoding="utf-8")
    proc = tmp_path / "cgroup-proc"
    proc.write_text("0::/user.slice/runner.scope/step\n", encoding="utf-8")

    assert (
        checker._ancestor_memory_cap_bytes(cgroup_root=root, proc_cgroup=proc)
        == 12 << 30
    )
    proc.write_text("0::/\n", encoding="utf-8")
    assert checker._ancestor_memory_cap_bytes(cgroup_root=root, proc_cgroup=proc) is None


def test_timing_gates_cover_every_runner_family() -> None:
    gates = set(selected_timing_gates(frozenset(), frozenset(), all_contract=True))
    # One representative per runner family in #27 cross-runner-test-timing.
    assert {
        "python.dagrun.shard-0",  # pytest shards
        "python-lifecycle.namespace.shard-0",  # lifecycle shards
        "python.repository-infrastructure.validation-graph",  # separate suites
        "rust.dagrun.test",  # Rust/libtest via run-cargo
        "vibe-talk.test.unit",  # application cargo suites
        "timeline.browser.functional",  # JavaScript/browser (Playwright)
        "timeline.browser.benchmark",  # JavaScript/browser (node:test)
        "cross.dagrun.differential",  # cross-language
        "cross.dagrun.cpu-footprint",  # cross pytest
        "packages.python.dagrun",  # Python packages
        "rust-packages.rust.dagrun",  # Rust packages
        "examples.run.python",  # examples, Python engine
        "examples.run.rust",  # examples, Rust engine
    } <= gates
    # These differentials keep their own counts and emit no per-check record;
    # expecting one would fail every green run.
    assert "cross.agentctl.differential" not in gates
    assert "cross.agentctl.herdr-agent-differential" not in gates
    # Self-tests of the runners themselves are not case inventories.
    assert "repository.hygiene.pytest-sharder" not in gates
    assert "repository.hygiene.component-manifest" not in gates


def test_fixed_memory_contract_matches_example_and_gate_hints() -> None:
    checker = _example_checker()
    example = dag_from_path(REPO_ROOT / "examples" / checker.FIXED_MEMORY_EXAMPLE)
    assert {step.hint.hard_mem_max_bytes for step in example.steps} == {
        checker.FIXED_MEMORY_BYTES
    }

    config = dag_from_path(REPO_ROOT / "validation.dag.yaml")
    example_steps = [
        step for step in config.steps if "scripts/check_dagrun_examples.py" in step.cmd
    ]
    # The gate still advertises (and must be delegated) the full hard cap: the
    # small-runner path documents a refusal, it does not shrink the envelope.
    assert {step.hint.hard_mem_max_bytes for step in example_steps} == {
        checker.FIXED_MEMORY_BYTES
    }


def _selected_tags_for_paths(paths: list[str]) -> set[str]:
    config = dag_from_path(REPO_ROOT / "validation.dag.yaml")
    groups, _because, components, _component_because = selection_for(paths)
    labels = [GROUP_LABELS[group] for group in groups]
    labels.extend(f"component-{component}" for component in components)
    selected = _select_steps_by_labels(config, labels)
    return {step.tag for step in selected.steps}


def test_examples_are_selected_by_their_sources_and_workspace_manifests() -> None:
    example_tags = {"examples.run.python", "examples.run.rust"}

    assert example_tags <= _selected_tags_for_paths(["examples/05-inner-jobs.json"])
    assert example_tags <= _selected_tags_for_paths(["rs/Cargo.lock"])
    assert example_tags.isdisjoint(_selected_tags_for_paths(["py/tick_hub/cli.py"]))


def test_wrkviz_source_selection_reaches_real_browser_gates() -> None:
    browser_tags = {
        "timeline.browser.functional",
        "timeline.browser.benchmark-instrument",
        "timeline.browser.benchmark",
    }

    assert browser_tags <= _selected_tags_for_paths(["py/wrkviz/synthetic.py"])
    assert browser_tags.isdisjoint(_selected_tags_for_paths(["py/tick_hub/cli.py"]))


def test_python_source_selection_reaches_cross_package_cli_surface() -> None:
    selected = _selected_tags_for_paths(["py/tick_hub/cli.py"])

    assert "python.repository-infrastructure.cli-surface" in selected
    assert "python.repository-infrastructure.test" not in selected


def test_cross_cutting_python_contracts_follow_every_observed_input() -> None:
    by_tag = dag_from_path(REPO_ROOT / "validation.dag.yaml").by_tag()
    expected_commands = {
        "python.repository-infrastructure.cli-no-optional-deps": (
            "test_cli_smoke_no_optional_deps.py"
        ),
        "python.repository-infrastructure.cli-surface": "test_cli_surface.py",
        "python.repository-infrastructure.cross-environment": (
            "test_cross_env_is_hermetic.py"
        ),
        "python.repository-infrastructure.cross-skip-accounting": (
            "test_cross_skip_accounting.py"
        ),
        "python.repository-infrastructure.dispatch-wiring": "test_repo_dispatch_wiring.py",
        "python.repository-infrastructure.package-contract": (
            "test_packaging_infrastructure.py"
        ),
        "python.repository-infrastructure.validation-graph": (
            "test_validation_graph_contract.py"
        ),
    }
    for tag, filename in expected_commands.items():
        assert filename in by_tag[tag].cmd

    python_tool = _selected_tags_for_paths(["py/tick_hub/cli.py"])
    assert {
        "python.repository-infrastructure.cli-no-optional-deps",
        "python.repository-infrastructure.cli-surface",
        "python.repository-infrastructure.dispatch-wiring",
        "python.repository-infrastructure.package-contract",
    } <= python_tool

    assert "python.repository-infrastructure.dispatch-wiring" in _selected_tags_for_paths(
        ["rs/tick-hub/src/lib.rs"]
    )
    assert "python.repository-infrastructure.package-contract" in _selected_tags_for_paths(
        ["py/wrkslots/tests/test_lifecycle.py"]
    )
    assert "python.repository-infrastructure.validation-graph" in _selected_tags_for_paths(
        ["vibe-talk/tests.dag.yaml"]
    )
    cross_harness = _selected_tags_for_paths(["cross/agentctl_differential.py"])
    assert {
        "python.repository-infrastructure.cross-environment",
        "python.repository-infrastructure.validation-graph",
    } <= cross_harness
    assert "python.repository-infrastructure.cross-skip-accounting" in _selected_tags_for_paths(
        ["cross/differential.py"]
    )


def test_python_and_rust_package_checkers_select_only_their_artifact_family() -> None:
    config = dag_from_path(REPO_ROOT / "validation.dag.yaml")
    python_packages = {
        step.tag for step in config.steps if "scripts/check_python_packages.py" in step.cmd
    }
    rust_packages = {
        step.tag for step in config.steps if "scripts/check_rust_packages.py" in step.cmd
    }

    selected_python = _selected_tags_for_paths(["scripts/check_python_packages.py"])
    selected_rust = _selected_tags_for_paths(["scripts/check_rust_packages.py"])
    assert python_packages <= selected_python
    assert rust_packages.isdisjoint(selected_python)
    assert rust_packages <= selected_rust
    assert python_packages.isdisjoint(selected_rust)


def test_shared_pytest_infrastructure_selects_every_actual_consumer() -> None:
    config = dag_from_path(REPO_ROOT / "validation.dag.yaml")
    component_tests = {
        step.tag
        for step in config.steps
        if "scripts/run_component_tests.py --component" in step.cmd
        or "python3 -m pytest -q tests/" in step.cmd
    }
    sharded_tests = {
        step.tag
        for step in config.steps
        if "scripts/run_pytest_shard.py" in step.cmd and "--self-test" not in step.cmd
    }
    namespace_tests = {
        step.tag for step in config.steps if "scripts/pid_namespace_init.py" in step.cmd
    }

    assert component_tests
    assert sharded_tests
    assert namespace_tests
    assert all(
        "validation-python-component-tests" in step.labels
        for step in config.steps
        if step.tag in component_tests
    )
    assert all(
        "validation-pytest-shards" in step.labels
        for step in config.steps
        if step.tag in sharded_tests
    )
    assert all(
        "validation-python-namespace-tests" in step.labels
        for step in config.steps
        if step.tag in namespace_tests
    )

    assert component_tests <= _selected_tags_for_paths(["py/tests/conftest.py"])
    assert sharded_tests <= _selected_tags_for_paths(["scripts/run_pytest_shard.py"])
    assert namespace_tests <= _selected_tags_for_paths(["scripts/pid_namespace_init.py"])


def test_rust_workspace_manifests_select_every_rust_test_without_python_tests() -> None:
    config = dag_from_path(REPO_ROOT / "validation.dag.yaml")
    rust_tests = {step.tag for step in config.steps if step.tag.startswith("rust.")}
    unrelated_python_suites = {
        step.tag
        for step in config.steps
        if (
            step.tag.startswith("python-lifecycle.")
            or step.tag.startswith("python.")
            and not step.tag.startswith("python.repository-infrastructure.")
        )
    }
    selected = _selected_tags_for_paths(["rs/Cargo.toml"])

    assert rust_tests <= selected
    assert unrelated_python_suites.isdisjoint(selected)


def test_distinct_cli_smokes_live_in_the_canonical_graph() -> None:
    config = dag_from_path(REPO_ROOT / "validation.dag.yaml")
    by_tag = config.by_tag()

    expected_commands = {
        "python.experiment-runner.cli-smoke": "plan-round",
        "python.planner.cli-smoke": "--format actions",
        "python.tick-hub.cli-smoke": "tick-hub-state.yaml",
        "python.wrkviz.cli-smoke": "wrkviz quickstart",
        "vibe-talk.smoke.cli-help": "--help",
    }
    for tag, command_fragment in expected_commands.items():
        assert command_fragment in by_tag[tag].cmd


# ---- `#362 ci-failure-masking`: one failure must not hide the rest ------------------------------


def _validate_module() -> ModuleType:
    import validate

    return validate


def test_keep_going_reaches_dagrun_only_when_asked(monkeypatch: pytest.MonkeyPatch) -> None:
    validate = _validate_module()
    commands: list[list[str]] = []

    def capture(command: list[str], **_kw: object) -> subprocess.CompletedProcess[str]:
        commands.append(list(command))
        return subprocess.CompletedProcess(command, 0)

    for name in ("VALIDATE_DAGRUN_FLAGS", "BOX_FLAGS"):
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setattr(validate.subprocess, "run", capture)
    assert validate.run(frozenset(), frozenset(), all_contract=True) == 0
    assert validate.run(frozenset(), frozenset(), all_contract=True, keep_going=True) == 0
    eager, keep_going = commands
    assert "--keep-going" not in eager
    assert keep_going.count("--keep-going") == 1
    assert [arg for arg in keep_going if arg != "--keep-going"] == eager


def test_the_keep_going_flag_reaches_the_graph_run(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    validate = _validate_module()
    seen: list[bool] = []

    def fake_run(*_args: object, keep_going: bool = False, **_kw: object) -> int:
        seen.append(keep_going)
        return 1

    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path))  # no host settings file
    monkeypatch.setattr(validate, "run", fake_run)
    assert validate.main(["--all", "--keep-going"]) == 1
    assert validate.main(["--all"]) == 1
    assert seen == [True, False]


def _workflow(name: str) -> dict[str, object]:
    import yaml

    loaded = yaml.safe_load((REPO_ROOT / ".github" / "workflows" / name).read_text(encoding="utf-8"))
    assert isinstance(loaded, dict)
    return loaded


def _job(workflow: dict[str, object], name: str) -> dict[str, object]:
    jobs = workflow["jobs"]
    assert isinstance(jobs, dict)
    job = jobs[name]
    assert isinstance(job, dict)
    return job


def _steps(job: dict[str, object]) -> list[dict[str, object]]:
    steps = job["steps"]
    assert isinstance(steps, list) and all(isinstance(step, dict) for step in steps)
    return steps


def _runs(job: dict[str, object]) -> list[str]:
    return [str(step.get("run", "")) for step in _steps(job)]


def test_ci_runs_the_whole_graph_past_a_failure() -> None:
    validate_job = _job(_workflow("cross-dagrun.yml"), "validate")
    assert any("python3 scripts/validate.py --keep-going $VALIDATE_ARGS" in run
               for run in _runs(validate_job))
    nightly = _job(_workflow("nightly-all.yml"), "everything")
    assert "python3 scripts/validate.py --all --keep-going" in _runs(nightly)


def test_the_rust_suites_run_past_a_failing_test_binary() -> None:
    by_tag = dag_from_path(REPO_ROOT / "validation.dag.yaml").by_tag()
    unit = by_tag["vibe-talk.test.unit"].cmd
    assert "cargo test" in unit and "--no-fail-fast" in shlex.split(unit)
    rust_tests = [step for tag, step in by_tag.items() if tag.startswith("rust.")
                  and "cargo test" in step.cmd]
    assert rust_tests
    assert all("--no-fail-fast" in shlex.split(step.cmd) for step in rust_tests)


def test_the_app_page_suite_runs_once_beside_the_rust_suites() -> None:
    # `#472 gate-flake-hardening`: the page suite's harness is skipped by the unit step and run by
    # its own, so neither step pays for it twice and neither loses it.
    by_tag = dag_from_path(REPO_ROOT / "validation.dag.yaml").by_tag()
    unit = shlex.split(by_tag["vibe-talk.test.unit"].cmd)
    page = shlex.split(by_tag["vibe-talk.test.page"].cmd)
    harness = (REPO_ROOT / "vibe-talk/tests/voice_page.rs").read_text(encoding="utf-8")
    assert "fn the_voice_page_suite_passes()" in harness
    # Skipped by its exact name: libtest's --skip is otherwise a substring, and would skip any test
    # whose name contained it in every binary, run by no step at all.
    assert unit[unit.index("--skip") + 1] == "the_voice_page_suite_passes" and "--exact" in unit
    assert page[page.index("--test") + 1] == "voice_page" and "--skip" not in page
    assert "--no-fail-fast" in page
    assert "vibe-talk.build.tests" in by_tag["vibe-talk.test.page"].deps
    assert "vibe-talk.build.tests" in by_tag["vibe-talk.test.unit"].deps
    assert "vibe-talk.test.page" not in by_tag["vibe-talk.test.unit"].deps


def _declared_parts(script: Path) -> list[str] | None:
    """The names in a browser check's module-level `PARTS` literal, or None if it has none."""
    for node in ast.parse(script.read_text(encoding="utf-8")).body:
        if isinstance(node, ast.Assign) and len(node.targets) == 1:
            target, value = node.targets[0], node.value
        elif isinstance(node, ast.AnnAssign) and node.value is not None:
            target, value = node.target, node.value
        else:
            continue
        if isinstance(target, ast.Name) and target.id == "PARTS":
            return list(ast.literal_eval(value))
    return None


def _parts_named(tokens: list[str]) -> list[str]:
    named = [token.split("=", 1)[1] for token in tokens if token.startswith("--part=")]
    return named + [tokens[at + 1] for at, token in enumerate(tokens[:-1]) if token == "--part"]


def test_a_browser_check_split_into_parts_runs_every_part_once() -> None:
    # `#474 browser-lane-critical-path`: a long browser check is split into parts the gate runs
    # side by side. Together the parts are the whole check only if every part is some step's, once,
    # and no step still runs the whole (which would pay for it twice). One part per step keeps each
    # step's name, budget and failure about one part.
    by_tag = dag_from_path(REPO_ROOT / "validation.dag.yaml").by_tag()
    split: dict[str, list[str]] = {}
    for script in sorted((REPO_ROOT / "vibe-talk/tests").glob("*.py")):
        parts = _declared_parts(script)
        if parts is not None:
            split[script.name] = parts
    assert "approval_card_browser.py" in split
    for script, parts in split.items():
        target = script.removesuffix(".py").replace("_", "-")
        runs = [tokens for tokens in (shlex.split(step.cmd) for step in by_tag.values())
                if any(Path(token).name == script for token in tokens)]
        assert runs, f"no step runs {script}"
        assert not [step.tag for step in by_tag.values() if target in shlex.split(step.cmd)], \
            f"a step runs all of {script} through its make target"
        stepped: list[str] = []
        for tokens in runs:
            named = _parts_named(tokens)
            assert len(named) == 1, f"a step runs {script} with parts {named}, not exactly one"
            stepped += named
        assert sorted(stepped) == sorted(parts), f"{script}: steps run parts {stepped}, it declares {parts}"


def test_the_timeline_redraw_budget_holds_every_browser_slot() -> None:
    # `#474 browser-lane-critical-path`: the browser cap went from 1 to 4. The redraw budget is a
    # timing, so it still runs with no other browser step beside it, whatever the cap is.
    cfg = dag_from_path(REPO_ROOT / "validation.dag.yaml")
    benchmark = cfg.by_tag()["timeline.browser.benchmark"]
    assert benchmark.hint.resources["browser"] == cfg.resource_caps["browser"]


# ---- `#361 python-minimum-timeout` --------------------------------------------------------------

PY310_SHARDS = 6


def test_the_minimum_python_tests_run_as_shards_that_each_report() -> None:
    workflow = _workflow("cross-dagrun.yml")
    # A workflow edit runs the complete graph, which keeps going past a failure: as long red as
    # green, and close to the old 60 minutes.
    assert _job(workflow, "validate")["timeout-minutes"] == 90
    short = _job(workflow, "python-minimum")
    assert "make check-python-packages" in _runs(short)
    assert not any("-m pytest" in run or "run_pytest_shard" in run for run in _runs(short))

    tests = _job(workflow, "python-minimum-tests")
    strategy = tests["strategy"]
    assert isinstance(strategy, dict)
    assert strategy["fail-fast"] is False
    assert strategy["matrix"] == {"shard": list(range(PY310_SHARDS))}
    steps = _steps(tests)
    [pytest_step] = [step for step in steps if "run_pytest_shard.py" in str(step.get("run", ""))]
    command = str(pytest_step["run"])
    assert f"--shard ${{{{ matrix.shard }}}}/{PY310_SHARDS} " in command
    assert "-p run_component_tests" in command  # names a failure the moment it happens
    assert "set -o pipefail" in command and pytest_step["shell"] == "bash"
    # A slow shard is stopped by its step's own limit, so the steps after it still report.
    step_limit, job_limit = pytest_step["timeout-minutes"], tests["timeout-minutes"]
    assert isinstance(step_limit, int) and isinstance(job_limit, int)
    assert step_limit < job_limit
    later = steps[steps.index(pytest_step) + 1:]
    assert later and all(step.get("if") == "${{ !cancelled() }}" for step in later)


def _shard_summary_program() -> str:
    tests = _job(_workflow("cross-dagrun.yml"), "python-minimum-tests")
    [run] = [run for run in _runs(tests) if "GITHUB_STEP_SUMMARY" in run]
    lines = run.splitlines()
    start = next(index for index, line in enumerate(lines) if line.endswith("<<'EOF'"))
    end = max(index for index, line in enumerate(lines) if line == "EOF")
    return "\n".join(lines[start + 1:end]) + "\n"


def _summarize(tmp_path: Path, report: Path, log: Path) -> str:
    program = tmp_path / "summary.py"
    program.write_text(_shard_summary_program(), encoding="utf-8")
    done = subprocess.run(
        [sys.executable, str(program), str(report), str(log)],
        env={**os.environ, "SHARD": "3"}, capture_output=True, text=True, check=False,
    )
    assert done.returncode == 0, done.stderr
    return done.stdout


def test_a_shard_summary_names_its_failures_from_the_report_or_else_the_log(
    tmp_path: Path,
) -> None:
    report, log = tmp_path / "shard.xml", tmp_path / "shard.log"
    assert "shard 3: 0 failed (no report and no log" in _summarize(tmp_path, report, log)

    log.write_text(
        "....F\ncomponent-tests: FAILED tests/test_a.py::test_one (call)\n"
        "component-tests: FAILED tests/test_a.py::test_one (call)\n.",
        encoding="utf-8",
    )
    stopped = _summarize(tmp_path, report, log)
    assert "shard 3: 1 failed (log only" in stopped
    assert "- `tests/test_a.py::test_one (call)`" in stopped

    report.write_text(
        '<?xml version="1.0"?><testsuites><testsuite>'
        '<testcase classname="tests.test_b" name="test_two"><properties>'
        '<property name="nodeid" value="tests/test_b.py::test_two[x]"/></properties>'
        '<failure message="boom"/></testcase>'
        '<testcase classname="tests.test_b" name="test_three"><error message="e"/></testcase>'
        '<testcase classname="tests.test_b" name="test_fine"/>'
        "</testsuite></testsuites>",
        encoding="utf-8",
    )
    finished = _summarize(tmp_path, report, log)
    assert "shard 3: 2 failed (JUnit report)" in finished
    assert "- `tests/test_b.py::test_two[x]`" in finished
    assert "- `tests.test_b::test_three`" in finished
    assert "test_fine" not in finished

    # A shard killed mid-write leaves a truncated report; the log still names its failures.
    report.write_text('<testsuites><testsuite><testcase classname="tests.test_b"', encoding="utf-8")
    truncated = _summarize(tmp_path, report, log)
    assert "shard 3: 1 failed (log only" in truncated
    assert "- `tests/test_a.py::test_one (call)`" in truncated


# ---- `#365 package-step-cpu-budget` -------------------------------------------------------------


def test_every_package_build_declares_a_cpu_budget_above_the_small_step_default() -> None:
    from dagrun.model import DEFAULT_SMALL_CPU_TIMEOUT

    config = dag_from_path(REPO_ROOT / "validation.dag.yaml")
    packages = [step for step in config.steps if step.tag.startswith("packages.python.")]
    assert len(packages) == 8
    for step in packages:
        assert step.cpu_timeout > DEFAULT_SMALL_CPU_TIMEOUT, step.tag
