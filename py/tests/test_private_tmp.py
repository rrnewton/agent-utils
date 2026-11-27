"""`#2383 private-run-tmpdir`: one private temp root per run, outside the host temp dir.

Covers scripts/private_tmp.py (roots, removal, the mount guard, the stale sweep, activation and
reuse), its use by validate.py and gate.py, and scripts/with-private-tmp. Every wait is on a
condition with a generous hang backstop; no verdict depends on elapsed time.
"""

from __future__ import annotations

import importlib.util
import os
import signal
import stat
import subprocess
import sys
import tempfile
import time
from collections.abc import Iterator
from pathlib import Path
from types import ModuleType

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = REPO_ROOT / "scripts"
WRAPPER = SCRIPTS / "with-private-tmp"
HANG_BACKSTOP_S = 120.0


def _load(name: str, path: Path) -> ModuleType:
    if str(SCRIPTS) not in sys.path:
        sys.path.insert(0, str(SCRIPTS))
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


@pytest.fixture
def pt() -> ModuleType:
    return _load("_private_tmp_under_test", SCRIPTS / "private_tmp.py")


@pytest.fixture
def base(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Iterator[Path]:
    """A private base for roots; no inherited root, the mechanism switched on."""
    where = tmp_path / "base"
    monkeypatch.setenv("AGENT_UTILS_TMP_BASE", str(where))
    monkeypatch.delenv("AGENT_UTILS_RUN_TMP", raising=False)
    monkeypatch.delenv("AGENT_UTILS_PRIVATE_TMP", raising=False)
    yield where


def _wait_for(condition_path: Path) -> None:
    deadline = time.monotonic() + HANG_BACKSTOP_S
    while not condition_path.exists():
        assert time.monotonic() < deadline, f"hang backstop: {condition_path} never appeared"
        time.sleep(0.05)


def test_roots_are_distinct_private_and_short_named(pt: ModuleType, base: Path) -> None:
    first = pt.make_root(base)
    second = pt.make_root(base)
    assert first != second and first.parent == base and second.parent == base
    assert first.name == str(os.getpid()) and second.name == f"{os.getpid()}a"
    for root in (first, second):
        assert stat.S_IMODE(root.stat().st_mode) == 0o700
    assert stat.S_IMODE(base.stat().st_mode) == 0o700


def test_activate_points_this_process_and_children_at_the_root_then_removes_it(
    pt: ModuleType, base: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("TMPDIR", "/before")
    monkeypatch.delenv("TMP", raising=False)
    cached = tempfile.tempdir
    with pt.activate() as run:
        root = run.root
        assert root is not None and run.owned and root.parent == base
        for name in ("TMPDIR", "TMP", "TEMP", "AGENT_UTILS_RUN_TMP"):
            assert os.environ[name] == str(root)
        assert tempfile.gettempdir() == str(root)
        child = subprocess.run(
            [sys.executable, "-c", "import os, tempfile; print(os.environ['TMPDIR'], tempfile.gettempdir())"],
            capture_output=True, text=True, check=True,
        )
        assert child.stdout.split() == [str(root), str(root)]
        (root / "made-by-a-test").write_text("x")
        run.failed = False
    assert not root.exists()
    assert os.environ["TMPDIR"] == "/before" and "TMP" not in os.environ
    assert "AGENT_UTILS_RUN_TMP" not in os.environ
    assert tempfile.tempdir == cached


def test_activate_removes_after_an_exception_and_keeps_a_failed_run_only_when_asked(
    pt: ModuleType, base: Path
) -> None:
    with pytest.raises(RuntimeError), pt.activate() as run:
        raised_root = run.root
        raise RuntimeError("a failing run")
    assert raised_root is not None and not raised_root.exists()

    with pt.activate(keep_on_failure=True) as run:
        kept = run.root  # failed stays True: a failed verdict
    assert kept is not None and kept.exists()

    with pt.activate(keep_on_failure=True) as run:
        passed = run.root
        run.failed = False
    assert passed is not None and not passed.exists()


def test_an_inherited_root_is_reused_and_left_for_its_maker(
    pt: ModuleType, base: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    outer = tmp_path / "outer"
    outer.mkdir()
    monkeypatch.setenv("AGENT_UTILS_RUN_TMP", str(outer))
    with pt.activate() as run:
        assert run.root == outer and not run.owned
        assert os.environ["TMPDIR"] == str(outer)
        run.failed = False
    assert outer.exists()
    assert not base.exists() or not any(base.iterdir())


def test_a_marker_that_is_not_a_directory_is_ignored(
    pt: ModuleType, base: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENT_UTILS_RUN_TMP", str(tmp_path / "gone"))
    with pt.activate() as run:
        assert run.owned and run.root is not None and run.root.parent == base
        run.failed = False


def test_the_switch_opts_a_run_out(pt: ModuleType, base: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("AGENT_UTILS_PRIVATE_TMP", "0")
    monkeypatch.setenv("TMPDIR", "/unchanged")
    with pt.activate() as run:
        assert run.root is None
        assert os.environ["TMPDIR"] == "/unchanged"
    assert not base.exists()


def test_removal_restores_access_to_read_only_parts(pt: ModuleType, base: Path, tmp_path: Path) -> None:
    root = pt.make_root(base)
    locked = root / "locked"
    (locked / "inner").mkdir(parents=True)
    (locked / "inner" / "file").write_text("x")
    (locked / "inner").chmod(0o500)
    locked.chmod(0o500)
    pt.remove_root(root, tmp_path / "no-mountinfo")
    assert not root.exists()


def _mountinfo(path: Path, *points: str) -> Path:
    lines = [f"{100 + i} 1 0:{50 + i} / {point} rw,relatime - tmpfs tmpfs rw" for i, point in enumerate(points)]
    path.write_text("\n".join(["22 1 0:21 / / rw - ext4 /dev/root rw", *lines]) + "\n")
    return path


def test_the_mount_guard_refuses_to_remove_through_a_mount(pt: ModuleType, base: Path, tmp_path: Path) -> None:
    root = pt.make_root(base)
    (root / "bound").mkdir()
    (root / "bound" / "source-data").write_text("must survive")
    (root / "other").write_text("x")
    fake = _mountinfo(tmp_path / "mountinfo", str(root / "bound"))
    with pytest.raises(pt.MountUnderRoot) as refused:
        pt.remove_root(root, fake)
    assert refused.value.mounts == [root / "bound"]
    assert (root / "bound" / "source-data").read_text() == "must survive"
    assert (root / "other").exists()


def test_the_mount_guard_reads_escaped_paths_and_ignores_lookalike_siblings(
    pt: ModuleType, base: Path, tmp_path: Path
) -> None:
    root = pt.make_root(base)
    spaced = root / "with space"
    escaped = str(spaced).replace(" ", "\\040")
    fake = _mountinfo(tmp_path / "mountinfo", escaped, str(root) + "x", str(base))
    assert pt.mounts_under(root, fake) == [spaced]
    assert pt.mounts_under(root, _mountinfo(tmp_path / "at-root", str(root))) == [root]


def _age(path: Path, seconds: float) -> None:
    then = time.time() - seconds
    os.utime(path, (then, then))


def _dead_pid() -> int:
    child = subprocess.Popen([sys.executable, "-c", "pass"])
    child.wait()
    return child.pid


def test_the_sweep_removes_only_stale_roots_of_dead_runs(pt: ModuleType, base: Path, tmp_path: Path) -> None:
    base.mkdir(mode=0o700)
    dead = _dead_pid()
    stale = base / f"{dead}"
    (stale / "left").mkdir(parents=True)
    live = base / f"{os.getpid()}b"
    live.mkdir()
    fresh = base / f"{dead}c"
    fresh.mkdir()
    mounted = base / f"{dead}d"
    (mounted / "m").mkdir(parents=True)
    unrelated = base / "not-a-root"
    unrelated.mkdir()
    for path in (stale, live, mounted, unrelated):
        _age(path, 2 * pt.STALE_SECONDS)
    fake = _mountinfo(tmp_path / "mountinfo", str(mounted / "m"))
    removed = pt.sweep_stale(base, mountinfo=fake)
    assert removed == [stale]
    assert live.exists() and fresh.exists() and mounted.exists() and unrelated.exists()


def test_validate_runs_its_graph_inside_one_root_used_for_pytest_too(
    base: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    validate = _load("_validate_private_tmp_under_test", SCRIPTS / "validate.py")
    seen: dict[str, str] = {}

    def fake_run(*_args: object, **kwargs: object) -> int:
        seen["TMPDIR"] = os.environ["TMPDIR"]
        seen["marker"] = os.environ["AGENT_UTILS_RUN_TMP"]
        seen["pytest_root"] = str(kwargs["pytest_root"])
        seen["gettempdir"] = tempfile.gettempdir()
        return 3

    monkeypatch.setattr(validate, "run", fake_run)
    monkeypatch.setattr(validate, "selected_cross_nodes", lambda *_a, **_k: [])
    monkeypatch.setattr(validate, "selected_timing_gates", lambda *_a, **_k: [])
    monkeypatch.setattr(validate, "host_env", lambda _environ: {})
    monkeypatch.delenv("AGENT_UTILS_KEEP_TEST_TMP", raising=False)
    assert validate.main(["--all"]) == 3
    root = Path(seen["TMPDIR"])
    assert root.parent == base
    assert seen["marker"] == seen["pytest_root"] == seen["gettempdir"] == str(root)
    assert not root.exists()


def test_validate_reuses_a_root_its_gate_made(
    base: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    validate = _load("_validate_private_tmp_reuse_under_test", SCRIPTS / "validate.py")
    outer = tmp_path / "gate-root"
    outer.mkdir()
    monkeypatch.setenv("AGENT_UTILS_RUN_TMP", str(outer))
    seen: list[str] = []

    def fake_run(*_args: object, **_kwargs: object) -> int:
        seen.append(os.environ["TMPDIR"])
        return 3

    monkeypatch.setattr(validate, "run", fake_run)
    monkeypatch.setattr(validate, "selected_cross_nodes", lambda *_a, **_k: [])
    monkeypatch.setattr(validate, "selected_timing_gates", lambda *_a, **_k: [])
    monkeypatch.setattr(validate, "host_env", lambda _environ: {})
    assert validate.main(["--all"]) == 3
    assert seen == [str(outer)] and outer.exists()


def test_gate_children_and_scratch_live_in_the_gate_root_which_goes_at_the_end(
    base: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    gate = _load("_gate_private_tmp_under_test", SCRIPTS / "gate.py")
    monkeypatch.delenv("AGENTCTL_REGISTRY", raising=False)
    monkeypatch.delenv("AGENT_UTILS_KEEP_TEST_TMP", raising=False)
    env = gate._child_environment()
    root = Path(env["TMPDIR"])
    assert root.parent == base
    assert env["TMP"] == env["TEMP"] == env["AGENT_UTILS_RUN_TMP"] == str(root)
    assert Path(env["AGENTCTL_REGISTRY"]).parent.parent == root
    assert gate._child_environment()["TMPDIR"] == str(root)  # one root per gate
    monkeypatch.setattr(gate, "_CHILD_REGISTRIES_FAILED", False)
    gate._cleanup_child_registries()
    assert not root.exists()


def test_gate_leaves_an_inherited_root_alone(
    base: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    gate = _load("_gate_private_tmp_reuse_under_test", SCRIPTS / "gate.py")
    outer = tmp_path / "outer-root"
    outer.mkdir()
    monkeypatch.setenv("AGENT_UTILS_RUN_TMP", str(outer))
    monkeypatch.delenv("AGENTCTL_REGISTRY", raising=False)
    env = gate._child_environment()
    assert env["TMPDIR"] == str(outer)
    monkeypatch.setattr(gate, "_CHILD_REGISTRIES_FAILED", False)
    gate._cleanup_child_registries()
    assert outer.exists()


def test_the_wrapper_runs_a_command_in_a_root_keeps_its_status_and_removes_the_root(
    base: Path, tmp_path: Path
) -> None:
    report = tmp_path / "report"
    done = subprocess.run(
        [str(WRAPPER), sys.executable, "-c",
         f"import os, sys; open({str(report)!r}, 'w').write(os.environ['TMPDIR']); sys.exit(7)"],
        capture_output=True, text=True, check=False,
    )
    assert done.returncode == 7, done.stderr
    root = Path(report.read_text())
    assert root.parent == base and not root.exists()


def test_the_wrapper_forwards_sigterm_and_still_removes_the_root(base: Path, tmp_path: Path) -> None:
    report = tmp_path / "report"
    child_code = (
        "import os, time; "
        f"p = {str(report)!r}; open(p + '.tmp', 'w').write(os.environ['TMPDIR']); os.rename(p + '.tmp', p); "
        "time.sleep(3600)"
    )
    wrapper = subprocess.Popen([str(WRAPPER), sys.executable, "-c", child_code])
    try:
        _wait_for(report)
        wrapper.send_signal(signal.SIGTERM)
        code = wrapper.wait(timeout=HANG_BACKSTOP_S)
    finally:
        if wrapper.poll() is None:
            wrapper.kill()
            wrapper.wait()
    assert code == 128 + signal.SIGTERM
    root = Path(report.read_text())
    assert root.parent == base and not root.exists()


def test_the_wrapper_without_a_command_prints_usage() -> None:
    done = subprocess.run([str(WRAPPER)], capture_output=True, text=True, check=False)
    assert done.returncode == 2 and "with-private-tmp COMMAND" in done.stdout
