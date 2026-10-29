"""A process link readlink(2) cannot read (ENAMETOOLONG) is placed by its parents.

2026-10-28: one hung test process held directories deeper than PATH_MAX, and
every validate-checkout removal on the host refused with "File name too long".
The find censuses drop such a diagnostic by a device proof (#651); the direct
inode census and the slot-use census decide a same-device link by its parents.
"""

from __future__ import annotations

import errno
import os
from collections.abc import Iterator
from pathlib import Path

import pytest

from wrkslots import cli as wrkslots

PID = 4242


def _deep_directory(top: Path) -> Path:
    """Open a directory below ``top`` whose path exceeds PATH_MAX; return its /proc link."""

    current = os.open(top, os.O_RDONLY | os.O_DIRECTORY)
    try:
        for _ in range(40):  # 40 * 201 bytes > 4096
            os.mkdir("d" * 200, dir_fd=current)
            below = os.open("d" * 200, os.O_RDONLY | os.O_DIRECTORY, dir_fd=current)
            os.close(current)
            current = below
    except BaseException:
        os.close(current)
        raise
    return Path(f"/proc/{os.getpid()}/fd/{current}")


@pytest.fixture
def layout(tmp_path: Path) -> Iterator[tuple[Path, Path, list[int]]]:
    target = tmp_path / "slots" / "selected"
    target.mkdir(parents=True)
    outside = tmp_path / "outside"
    outside.mkdir()
    opened: list[int] = []
    yield target, outside, opened
    for fd in opened:
        os.close(fd)


def _held(top: Path, opened: list[int]) -> Path:
    link = _deep_directory(top)
    opened.append(int(link.name))
    return link


def test_the_walk_places_a_directory_readlink_cannot_name(
    layout: tuple[Path, Path, list[int]],
) -> None:
    target, outside, opened = layout
    inner = _held(target, opened)
    other = _held(outside, opened)
    with pytest.raises(OSError) as raised:
        os.readlink(inner)
    assert raised.value.errno == errno.ENAMETOOLONG
    identities = wrkslots._target_directory_identities({target: "slot-a"})
    assert wrkslots._directory_link_holder(inner, identities) == (target, "slot-a")
    assert wrkslots._directory_link_holder(other, identities) is None
    plain = outside / "plain"
    plain.write_text("x")
    with open(plain) as handle:
        with pytest.raises(OSError):
            wrkslots._directory_link_holder(
                Path(f"/proc/{os.getpid()}/fd/{handle.fileno()}"), identities
            )


def test_an_absent_target_has_no_identity(tmp_path: Path) -> None:
    assert wrkslots._target_directory_identities({tmp_path / "gone": "slot"}) == {}


def _proc_with_fd(tmp_path: Path, destination: Path) -> Path:
    proc_root = tmp_path / "proc"
    base = proc_root / str(PID)
    (base / "fd").mkdir(parents=True)
    neutral = tmp_path / "neutral"
    neutral.mkdir(exist_ok=True)
    for name in ("cwd", "root", "exe"):
        (base / name).symlink_to(neutral)
    (base / "fd" / "0").symlink_to(destination)
    return proc_root


def _census(
    monkeypatch: pytest.MonkeyPatch, proc_root: Path, targets: dict[Path, str]
) -> tuple[
    tuple[tuple[int, str, str, str], ...],
    tuple[tuple[int, str, str, str], ...],
    dict[int, set[int]],
    tuple[wrkslots._AbsentProcessObservation, ...],
]:
    monkeypatch.setattr(wrkslots, "_process_filesystem_uid", lambda _path: os.getuid())
    monkeypatch.setattr(wrkslots, "_process_generation_is_current", lambda _process: True)
    real_readlink = os.readlink

    def too_long(path: str | Path) -> str:
        if str(path).endswith("/fd/0"):
            raise OSError(errno.ENAMETOOLONG, "File name too long", str(path))
        return real_readlink(path)

    monkeypatch.setattr(os, "readlink", too_long)
    return wrkslots._direct_process_identity_matches(
        [wrkslots._AbsentProcessObservation(PID, 17, "", "mnt:[1]")],
        targets,
        {},
        wrkslots._ReadOnlyCommandBudget.start(
            timeout_seconds=30, stdout_limit=64, stderr_limit=64
        ),
        proc_root=proc_root,
    )


def test_direct_census_counts_a_too_long_directory_inside_a_target(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    target = tmp_path / "slots" / "selected"
    (target / "deep").mkdir(parents=True)
    links, inodes, _sockets, fallback = _census(
        monkeypatch, _proc_with_fd(tmp_path, target / "deep"), {target: "slot-a"}
    )
    assert [(pid, slot, kind) for pid, slot, kind, _ in links] == [(PID, "slot-a", "link")]
    assert "exceeds PATH_MAX" in links[0][3]
    assert inodes == () and fallback == ()


def test_direct_census_omits_a_too_long_directory_outside_every_target(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    target = tmp_path / "slots" / "selected"
    target.mkdir(parents=True)
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    links, inodes, _sockets, fallback = _census(
        monkeypatch, _proc_with_fd(tmp_path, elsewhere), {target: "slot-a"}
    )
    assert links == () and inodes == () and fallback == ()


def test_direct_census_refuses_a_too_long_link_it_cannot_walk(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    target = tmp_path / "slots" / "selected"
    target.mkdir(parents=True)
    plain = tmp_path / "plain"
    plain.write_text("x")
    with pytest.raises(wrkslots.Refusal, match="inode evidence"):
        _census(monkeypatch, _proc_with_fd(tmp_path, plain), {target: "slot-a"})


@pytest.mark.parametrize("reader", ["_link_target", "_readable_link_target"])
def test_slot_use_places_a_too_long_directory_on_the_same_device(
    layout: tuple[Path, Path, list[int]], reader: str
) -> None:
    # The device proof (#618) cannot decide a same-device target; the walk can.
    target, outside, opened = layout
    read = getattr(wrkslots, reader)
    device = os.stat(target).st_dev
    inner = _held(target, opened)
    other = _held(outside, opened)
    assert os.stat(other).st_dev == device
    assert str(read(inner, device, target)) == str(target)
    assert read(other, device, target) is None
    # Without the slot root the old refusal stands.
    with pytest.raises(wrkslots.Refusal, match="process use is indeterminate"):
        read(other, device)
    plain = outside / "plain"
    plain.write_text("x")
    with open(plain) as handle:
        link = Path(f"/proc/{os.getpid()}/fd/{handle.fileno()}")
        # A short link still reads normally and is not walked.
        assert read(link, device, target) is not None
