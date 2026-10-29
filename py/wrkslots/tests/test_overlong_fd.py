"""An overlong fd target must not make every slot on the host indeterminate.

``readlink`` on ``/proc/<pid>/fd/<n>`` fails with ``ENAMETOOLONG`` when the
target path is very deep (a live Detcore guest holding a file deep in a
cargo tree, measured on devbig014 on 2026-10-28).  The census decides that
one descriptor by identity instead: stat it, resolve it through an
``O_PATH`` handle when it can, and refuse only for that descriptor when it
sits on the slot's own device and still cannot be resolved.
"""

from __future__ import annotations

import errno
import os
from pathlib import Path

import pytest

from wrkslots import cli


def _fake_process(proc: Path, pid: int) -> Path:
    directory = proc / str(pid)
    (directory / "fd").mkdir(parents=True)
    (directory / "cwd").symlink_to("/")
    (directory / "root").symlink_to("/")
    (directory / "exe").symlink_to("/bin/true")
    (directory / "maps").write_text("", encoding="utf-8")
    return directory


def _overlong_readlink(monkeypatch: pytest.MonkeyPatch, *, block_self_fd: bool) -> None:
    real_readlink = os.readlink

    def readlink(path: str | os.PathLike[str], *, dir_fd: int | None = None) -> str:
        text = str(path)
        if "/fd/" in text and (block_self_fd or "/proc/self/fd/" not in text):
            raise OSError(errno.ENAMETOOLONG, "File name too long", text)
        return real_readlink(path, dir_fd=dir_fd)

    monkeypatch.setattr(os, "readlink", readlink)


def test_an_overlong_fd_outside_the_slot_is_no_use(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    slot = tmp_path / "slot"
    slot.mkdir()
    outside = tmp_path / "outside"
    outside.write_text("x", encoding="utf-8")
    process = _fake_process(tmp_path / "proc", 10)
    (process / "fd" / "3").symlink_to(outside)
    _overlong_readlink(monkeypatch, block_self_fd=False)
    assert cli._process_uses_slot(process, slot) == []


def test_an_overlong_fd_inside_the_slot_is_use(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    slot = tmp_path / "slot"
    slot.mkdir()
    held = slot / "held"
    held.write_text("x", encoding="utf-8")
    process = _fake_process(tmp_path / "proc", 11)
    (process / "fd" / "3").symlink_to(held)
    _overlong_readlink(monkeypatch, block_self_fd=False)
    uses = cli._process_uses_slot(process, slot)
    assert any(use.startswith("fd/3=") and str(held) in use for use in uses), uses


def test_an_overlong_fd_on_another_device_is_no_use_even_unresolved(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    slot = tmp_path / "slot"
    slot.mkdir()
    process = _fake_process(tmp_path / "proc", 12)
    (process / "fd" / "3").symlink_to("/dev/null")
    if os.stat("/dev/null").st_dev == os.stat(slot).st_dev:
        pytest.skip("the slot shares a device with /dev/null")
    _overlong_readlink(monkeypatch, block_self_fd=True)
    assert cli._process_uses_slot(process, slot) == []
    assert cli._readable_link_target(process / "fd" / "3", slot) is None


def test_an_unresolvable_overlong_fd_proven_outside_by_inode_search_is_no_use(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    slot = tmp_path / "slot"
    slot.mkdir()
    (slot / "held").write_text("x", encoding="utf-8")
    outside = tmp_path / "outside"
    outside.write_text("x", encoding="utf-8")
    process = _fake_process(tmp_path / "proc", 13)
    (process / "fd" / "3").symlink_to(outside)
    _overlong_readlink(monkeypatch, block_self_fd=True)
    assert cli._process_uses_slot(process, slot) == []


def test_an_unresolvable_overlong_fd_inside_is_found_by_inode_search(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    slot = tmp_path / "slot"
    slot.mkdir()
    held = slot / "held"
    held.write_text("x", encoding="utf-8")
    process = _fake_process(tmp_path / "proc", 15)
    (process / "fd" / "3").symlink_to(held)
    _overlong_readlink(monkeypatch, block_self_fd=True)
    uses = cli._process_uses_slot(process, slot)
    assert any(use.startswith("fd/3=") and str(held) in use for use in uses), uses


def test_an_unresolvable_overlong_fd_with_an_unsearchable_slot_refuses_for_that_fd(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    slot = tmp_path / "slot"
    sealed = slot / "sealed"
    sealed.mkdir(parents=True)
    outside = tmp_path / "outside"
    outside.write_text("x", encoding="utf-8")
    process = _fake_process(tmp_path / "proc", 13)
    (process / "fd" / "3").symlink_to(outside)
    _overlong_readlink(monkeypatch, block_self_fd=True)
    sealed.chmod(0)
    try:
        if os.access(sealed, os.R_OK):
            pytest.skip("this user is not denied an unsearchable directory")
        with pytest.raises(cli.Refusal, match="overlong"):
            cli._process_uses_slot(process, slot)
    finally:
        sealed.chmod(0o700)


def test_an_overlong_fd_to_the_slot_root_is_use_by_identity(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    slot = tmp_path / "slot"
    slot.mkdir()
    process = _fake_process(tmp_path / "proc", 14)
    (process / "fd" / "3").symlink_to(slot)
    _overlong_readlink(monkeypatch, block_self_fd=True)
    assert cli._link_target(process / "fd" / "3", slot) == slot
