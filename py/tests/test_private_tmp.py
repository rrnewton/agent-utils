"""`#2383 private-run-tmpdir`: one private temp root per run, outside the host temp dir.

Covers scripts/private_tmp.py (roots, removal, the mount guard, the stale sweep, activation and
reuse), its use by validate.py and gate.py, and scripts/with-private-tmp. Every wait is on a
condition with a generous hang backstop; no verdict depends on elapsed time.
"""

from __future__ import annotations

import fcntl
import importlib.machinery
import importlib.util
import json
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
    # Host-temp roots too: no test touches the real /tmp/aut-<uid>.
    (tmp_path / "host").mkdir()
    monkeypatch.setenv("AGENT_UTILS_HOST_TMP_BASE", str(tmp_path / "host" / "aut-test"))
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


# ---- the scoped host-temp exception (wrkslots steps only, until #2415; liaison 1081) ----------


def _host_base(tmp_path: Path) -> Path:
    return tmp_path / "host" / "aut-test"


def _owner_locked(record: Path) -> bool:
    descriptor = os.open(record, os.O_RDONLY)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        return True
    finally:
        os.close(descriptor)
    return False


def test_a_host_temp_run_gets_a_fresh_locked_root_even_inside_an_outer_run(
    pt: ModuleType, base: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    outer = tmp_path / "outer"
    outer.mkdir()
    monkeypatch.setenv("AGENT_UTILS_RUN_TMP", str(outer))
    host = _host_base(tmp_path)
    with pt.activate(host_temp=True) as run:
        root = run.root
        assert root is not None and root.parent == host and run.owned
        assert stat.S_IMODE(host.stat().st_mode) == 0o700 and stat.S_IMODE(root.stat().st_mode) == 0o700
        assert os.environ["TMPDIR"] == os.environ["AGENT_UTILS_RUN_TMP"] == str(root)
        record = host / f"{root.name}.owner"
        owner = json.loads(record.read_text())
        assert owner["pid"] == os.getpid() and owner["start"] == pt.process_start(os.getpid())
        assert _owner_locked(record), "a live run holds its owner record locked"
        assert pt.sweep_host_temp(host) == [] and root.exists(), "a live root is never swept"
        run.failed = False
    assert not root.exists() and not record.exists()
    assert os.environ["AGENT_UTILS_RUN_TMP"] == str(outer)


def test_the_host_temp_sweep_removes_only_finished_runs_and_logs_bytes(
    pt: ModuleType, base: Path, tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    host = _host_base(tmp_path)
    host.mkdir(mode=0o700)
    namespace = os.readlink("/proc/self/ns/pid")

    def root_with_record(name: str, owner: object) -> Path:
        root = host / name
        (root / "deep").mkdir(parents=True)
        (root / "deep" / "data").write_bytes(b"x" * 65536)
        (host / f"{name}.owner").write_text(json.dumps(owner))
        return root

    dead = root_with_record("101", {"pid": _dead_pid(), "start": 1, "pid_namespace": namespace})
    reused = root_with_record(
        "102", {"pid": os.getpid(), "start": -5, "pid_namespace": namespace}
    )  # the pid lives, but its start time says it is another process
    alive = root_with_record(
        "103", {"pid": os.getpid(), "start": pt.process_start(os.getpid()), "pid_namespace": namespace}
    )  # unlocked, but the recorded process is this one: kept
    unreadable = root_with_record("104", {"pid": "x"})
    no_record = host / "105"
    no_record.mkdir()
    held = root_with_record("106", {"pid": _dead_pid(), "start": 1, "pid_namespace": "pid:[other]"})
    lock = os.open(host / "106.owner", os.O_RDONLY)
    fcntl.flock(lock, fcntl.LOCK_EX)
    try:
        swept = pt.sweep_host_temp(host)
    finally:
        os.close(lock)
    assert sorted(path.name for path, _ in swept) == ["101", "102"]
    assert all(size >= 65536 for _, size in swept)
    assert not dead.exists() and not reused.exists()
    assert not (host / "101.owner").exists() and not (host / "102.owner").exists()
    assert alive.exists() and unreadable.exists() and no_record.exists() and held.exists()
    err = capsys.readouterr().err
    assert "swept 2 root(s)" in err and "bytes" in err
    assert "105 has no owner record; kept" in err and "unreadable owner record" in err
    # Once its holder lets go, a record from another PID namespace is judged by its lock.
    assert [path.name for path, _ in pt.sweep_host_temp(host)] == ["106"]


def test_a_host_temp_base_that_is_not_a_private_directory_is_refused(
    pt: ModuleType, base: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    trap = tmp_path / "host" / "trap"
    trap.symlink_to(elsewhere)
    monkeypatch.setenv("AGENT_UTILS_HOST_TMP_BASE", str(trap))
    with pytest.raises(pt.HostTempUnavailable, match="not a real directory"):
        with pt.activate(host_temp=True):
            pass
    assert list(elsewhere.iterdir()) == []
    done = subprocess.run(
        [str(WRAPPER), "--host-temp", sys.executable, "-c", "raise SystemExit(0)"],
        capture_output=True, text=True, check=False, env={**os.environ},
    )
    assert done.returncode == pt.OVER_LIMIT_EXIT and "REFUSED: no host-temp root" in done.stderr


def test_the_wrapper_runs_a_host_temp_command_and_removes_its_root(base: Path, tmp_path: Path) -> None:
    report = tmp_path / "report"
    done = subprocess.run(
        [str(WRAPPER), "--host-temp", sys.executable, "-c",
         f"import os; open({str(report)!r}, 'w').write(os.environ['TMPDIR'])"],
        capture_output=True, text=True, check=False,
    )
    assert done.returncode == 0, done.stderr
    root = Path(report.read_text())
    assert root.parent == _host_base(tmp_path) and not root.exists()
    assert list(_host_base(tmp_path).iterdir()) == []


def _wrapper_module() -> ModuleType:
    loader = importlib.machinery.SourceFileLoader("_with_private_tmp_under_test", str(WRAPPER))
    spec = importlib.util.spec_from_loader(loader.name, loader)
    assert spec is not None
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


def test_a_host_temp_root_past_its_bound_stops_the_command_with_a_named_refusal(
    base: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    wrapper = _wrapper_module()
    monkeypatch.setattr(wrapper.private_tmp, "HOST_TEMP_LIMIT_BYTES", 1_000_000)
    monkeypatch.setattr(wrapper.private_tmp, "HOST_TEMP_POLL_SECONDS", 0.05)
    report = tmp_path / "report"
    child_code = (
        "import os, time; root = os.environ['TMPDIR']; "
        "open(os.path.join(root, 'big'), 'wb').write(os.urandom(4_000_000)); "
        f"open({str(report)!r}, 'w').write(root); time.sleep({HANG_BACKSTOP_S})"
    )
    code = wrapper.main(["--host-temp", sys.executable, "-c", child_code])
    assert code == wrapper.private_tmp.OVER_LIMIT_EXIT == 73
    err = capsys.readouterr().err
    assert "with-private-tmp: REFUSED: host-temp root" in err and "past its bound of 1000000" in err
    root = Path(report.read_text())
    assert not root.exists() and list(_host_base(tmp_path).iterdir()) == []


def test_the_sweep_command_reports_both_bases(base: Path, tmp_path: Path) -> None:
    host = _host_base(tmp_path)
    host.mkdir(mode=0o700)
    (host / "201").mkdir()
    (host / "201" / "f").write_bytes(b"y" * 8192)
    (host / "201.owner").write_text(json.dumps({"pid": _dead_pid(), "start": 1, "pid_namespace": ""}))
    done = subprocess.run(
        [sys.executable, str(SCRIPTS / "private_tmp.py"), "sweep"],
        capture_output=True, text=True, check=False,
    )
    assert done.returncode == 0, done.stderr
    assert f"swept {host / '201'}" in done.stdout
    assert "private tmp sweep: 1 host-temp root(s)" in done.stdout and not (host / "201").exists()


def test_gate_start_sweeps_finished_host_temp_roots(
    base: Path, tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    import inspect

    gate = _load("_gate_host_temp_sweep_under_test", SCRIPTS / "gate.py")
    host = _host_base(tmp_path)
    host.mkdir(mode=0o700)
    (host / "301").mkdir()
    (host / "301" / "f").write_bytes(b"z" * 8192)
    (host / "301.owner").write_text(json.dumps({"pid": _dead_pid(), "start": 1, "pid_namespace": ""}))
    gate._sweep_host_temp_roots()
    assert not (host / "301").exists() and not (host / "301.owner").exists()
    assert "swept 1 root(s)" in capsys.readouterr().err
    assert "_sweep_host_temp_roots()" in inspect.getsource(gate.run_admitted), "every admitted gate sweeps first"
