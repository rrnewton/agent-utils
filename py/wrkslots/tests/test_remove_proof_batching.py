"""#1681: advertised-tips proof is asked once per removal; parent death stops a remove."""

from __future__ import annotations

import subprocess
import sys
import time
from pathlib import Path
from typing import Mapping, Sequence

import pytest
from wrkslots import cli as wrkslots


def _git(cwd: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(cwd), *args],
        capture_output=True, text=True, check=True, timeout=60,
    )
    return result.stdout.strip()


def _repo(tmp_path: Path) -> Path:
    repo = tmp_path / "repo"
    repo.mkdir()
    _git(repo, "init", "-q", "-b", "main")
    _git(repo, "config", "user.email", "t@e.invalid")
    _git(repo, "config", "user.name", "T")
    (repo / "f.txt").write_text("x\n")
    _git(repo, "add", "f.txt")
    _git(repo, "commit", "-q", "-m", "one")
    _git(repo, "remote", "add", "origin", str(repo))
    return repo


def test_advertised_tips_memoized_within_removal_scope(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _repo(tmp_path)
    vcs = wrkslots._GitVcs()
    authority = vcs.remote_authority(repo, "origin")
    calls: list[str] = []
    original = wrkslots._GitVcs._ls_remote_inventory

    def counting(self: wrkslots._GitVcs, *args: object, **kwargs: object) -> dict[str, str]:
        calls.append("ls-remote")
        return original(self, *args, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr(wrkslots._GitVcs, "_ls_remote_inventory", counting)
    with wrkslots._remote_containment_scope():
        first = vcs.remote_advertised_tips(repo, "origin", authority)
        second = vcs.remote_advertised_tips(repo, "origin", authority)
    assert first == second and first
    assert calls == ["ls-remote"]


def test_advertised_tips_not_memoized_outside_scope(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _repo(tmp_path)
    vcs = wrkslots._GitVcs()
    authority = vcs.remote_authority(repo, "origin")
    calls: list[str] = []
    original = wrkslots._GitVcs._ls_remote_inventory

    def counting(self: wrkslots._GitVcs, *args: object, **kwargs: object) -> dict[str, str]:
        calls.append("ls-remote")
        return original(self, *args, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr(wrkslots._GitVcs, "_ls_remote_inventory", counting)
    vcs.remote_advertised_tips(repo, "origin", authority)
    vcs.remote_advertised_tips(repo, "origin", authority)
    assert calls == ["ls-remote", "ls-remote"]


@pytest.mark.skipif(sys.platform != "linux", reason="prctl parent-death signal is Linux-only")
def test_parent_death_signal_stops_child(tmp_path: Path) -> None:
    py_dir = Path(wrkslots.__file__).resolve().parent.parent
    child_code = (
        "import sys, time; sys.path.insert(0, {d!r}); "
        "from wrkslots.cli import _install_parent_death_signal; "
        "_install_parent_death_signal(); print('ready', flush=True); time.sleep(60)"
    ).format(d=str(py_dir))
    wrapper = tmp_path / "wrapper.py"
    wrapper.write_text(
        "import subprocess, sys\n"
        f"p = subprocess.Popen([sys.executable, '-c', {child_code!r}], stdout=subprocess.PIPE, text=True)\n"
        "assert p.stdout is not None and p.stdout.readline().strip() == 'ready'\n"
        "print(p.pid, flush=True)\n"
    )
    proc = subprocess.Popen(
        [sys.executable, str(wrapper)], stdout=subprocess.PIPE, text=True
    )
    assert proc.stdout is not None
    child_pid = int(proc.stdout.readline().strip())
    proc.wait(timeout=30)
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        try:
            Path(f"/proc/{child_pid}").stat()
        except FileNotFoundError:
            break
        time.sleep(0.1)
    else:
        subprocess.run(["kill", "-9", str(child_pid)], check=False)
        raise AssertionError("child survived its parent's death")
