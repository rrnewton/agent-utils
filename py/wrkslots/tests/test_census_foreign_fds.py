"""Census: an unreadable foreign descriptor must not refuse the scan.

#618 wrkslots-census-foreign-procs. When readlink on a process's
cwd/exe/fd fails (ENAMETOOLONG on an unrelated process's very long
target, measured on a loaded host), the census stats the descriptor
instead: a different filesystem device than the slot's proves the
descriptor lies outside the slot (slots are separate filesystem
images), while a same-device descriptor stays indeterminate,
because a very long path inside the slot fails readlink the same
way and must not fail open.
"""

from __future__ import annotations

import errno
import os
import subprocess
import sys
from pathlib import Path

import pytest
from wrkslots import cli


_REAL_READLINK = os.readlink


def _failing_readlink(path: str | Path) -> str:
    if "/fd/" in str(path) or str(path).endswith("/cwd"):
        raise OSError(errno.ENAMETOOLONG, "File name too long", str(path))
    return _REAL_READLINK(path)


def _fd_link(tmp_path: Path, held: Path) -> Path:
    fd_dir = tmp_path / "fd"
    fd_dir.mkdir(exist_ok=True)
    link = fd_dir / "3"
    link.symlink_to(held)
    return link


def test_unreadable_foreign_device_descriptor_is_not_use(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    held = tmp_path / "held.txt"
    held.write_text("x\n")
    device = os.stat(held).st_dev
    monkeypatch.setattr(os, "readlink", _failing_readlink)
    assert cli._readable_link_target(_fd_link(tmp_path, held), slot_device=device + 4096) is None


def test_unreadable_same_device_descriptor_stays_indeterminate(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    held = tmp_path / "held.txt"
    held.write_text("x\n")
    device = os.stat(held).st_dev
    monkeypatch.setattr(os, "readlink", _failing_readlink)
    with pytest.raises(cli.Refusal, match="process use is indeterminate"):
        cli._readable_link_target(_fd_link(tmp_path, held), slot_device=device)


def test_unreadable_descriptor_with_failed_stat_stays_indeterminate(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    missing = tmp_path / "fd" / "9"
    monkeypatch.setattr(os, "readlink", _failing_readlink)
    with pytest.raises(cli.Refusal, match="process use is indeterminate"):
        cli._readable_link_target(missing, slot_device=12345)


def _holder(path: Path) -> subprocess.Popen[str]:
    proc = subprocess.Popen(
        [
            sys.executable, "-c",
            "import os, sys, time\n"
            "fd = os.open(sys.argv[1], os.O_RDONLY)\n"
            "print('ready', flush=True)\n"
            "time.sleep(60)\n",
            str(path),
        ],
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        stdin=subprocess.DEVNULL, text=True,
    )
    assert proc.stdout is not None
    assert proc.stdout.readline() == "ready\n"
    return proc


def _scan(
    tmp_path: Path, slot: Path, holder_pid: int, monkeypatch: pytest.MonkeyPatch
) -> object:
    proc_root = tmp_path / "proc"
    proc_root.mkdir()
    (proc_root / str(holder_pid)).symlink_to(Path("/proc") / str(holder_pid))
    monkeypatch.setattr(os, "readlink", _failing_readlink)
    return cli._agent_batch_fresh_use(
        slot, None, capture_generation=False, proc_root=proc_root
    )


def test_fresh_use_ignores_unreadable_foreign_descriptor(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # The slot must sit on a different device than the held file, as
    # real slots are separate filesystem images; /dev/shm serves as
    # the second device here.
    shm = Path("/dev/shm")
    if not shm.is_dir() or os.stat(shm).st_dev == os.stat(tmp_path).st_dev:
        pytest.skip("no second filesystem device available")
    slot = shm / f"wrkslots-census-slot-{os.getpid()}"
    slot.mkdir()
    outside = tmp_path / "outside.txt"
    outside.write_text("x\n")
    holder = _holder(outside)
    try:
        assert _scan(tmp_path, slot, holder.pid, monkeypatch) is None
    finally:
        holder.kill()
        holder.wait()
        slot.rmdir()


def test_link_target_foreign_device_is_not_use(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    held = tmp_path / "held.txt"
    held.write_text("x\n")
    device = os.stat(held).st_dev
    monkeypatch.setattr(os, "readlink", _failing_readlink)
    link = _fd_link(tmp_path, held)
    assert cli._link_target(link, slot_device=device + 4096) is None
    with pytest.raises(cli.Refusal, match="process use is indeterminate"):
        cli._link_target(link, slot_device=device)


def test_fresh_use_refuses_unreadable_same_device_descriptor(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    slot = tmp_path / "slot"
    slot.mkdir()
    inside = slot / "held.txt"
    inside.write_text("x\n")
    holder = _holder(inside)
    try:
        with pytest.raises(cli.Refusal, match="process use is indeterminate"):
            _scan(tmp_path, slot, holder.pid, monkeypatch)
    finally:
        holder.kill()
        holder.wait()
