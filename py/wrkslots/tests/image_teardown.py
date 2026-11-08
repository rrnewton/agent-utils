"""Unmount every slot image a test left mounted under its temporary directory.

A test that fails between mounting a slot image and destroying it used to leave
the mount, and the loop device behind it, until someone noticed (#1064
test-temp-and-mount-leaks): fourteen devices on one host, some six days old.
Cleanup inside each test's ``finally`` misses a failure that comes before the
``try`` or inside provisioning itself, so this teardown runs after every test
that used ``tmp_path``, whatever happened in it.

It acts only on mounts whose mount point lies under that test's own
``tmp_path`` and only through :func:`slotimage.unmount`, which identifies the
image before it unmounts anything and detaches the loop device. A mount it
cannot release fails the test's teardown with the mount named, so a leak is
reported where it happened instead of found days later.

A teardown hook, loaded as a plugin by ``conftest.py``; a subprocess pytest can load it with
``-p wrkslots.tests.image_teardown``.
"""

from __future__ import annotations

import json
import os
from collections.abc import Iterator
from pathlib import Path

import pytest

from wrkslots import slotimage


def _under(path: Path, roots: tuple[Path, ...]) -> bool:
    return any(path == root or root in path.parents for root in roots)


def _roots(root: Path) -> tuple[Path, ...]:
    spellings = {Path(os.path.abspath(root)), Path(os.path.realpath(root))}
    return tuple(sorted(spellings))


def mounts_under(root: Path) -> list[slotimage.MountEntry]:
    """Mounts in the host's table whose mount point lies under ``root``, deepest first."""

    roots = _roots(root)
    entries = [entry for entry in slotimage.verification_table() if _under(entry.mount_point, roots)]
    return sorted(entries, key=lambda entry: len(entry.mount_point.parts), reverse=True)


def _recorded_location(directory: Path) -> Path | None:
    """The slot location an image directory's record names, read without layout checks.

    The test's configuration may never have been loaded in this process (the
    failure can come first), so :func:`slotimage.read_record`, which needs the
    registered layout, is not usable here.
    """

    try:
        raw = json.loads((directory / slotimage.RECORD_NAME).read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError):
        return None
    location = raw.get("location") if isinstance(raw, dict) else None
    return Path(location) if isinstance(location, str) and os.path.isabs(location) else None


def _image_directories(mount_point: Path, roots: tuple[Path, ...]) -> list[Path]:
    """Image directories that could own ``mount_point``: its parent, and every
    ``slot-images/<type>/<slot>`` under an ancestor inside the test's root."""

    found: list[Path] = []
    if (mount_point.parent / slotimage.RECORD_NAME).is_file():
        found.append(mount_point.parent)
    for ancestor in mount_point.parents:
        if not _under(ancestor, roots):
            break
        images = ancestor / "slot-images"
        if images.is_dir() and not images.is_symlink():
            found.extend(sorted(path.parent for path in images.glob(f"*/*/{slotimage.RECORD_NAME}")))
    return found


def _image_file_for(mount_point: Path, roots: tuple[Path, ...]) -> Path | None:
    for directory in _image_directories(mount_point, roots):
        if mount_point == directory / slotimage.STATE_MOUNT_NAME:
            return directory / slotimage.STATE_IMAGE_NAME
        location = _recorded_location(directory)
        if location is not None and Path(os.path.abspath(location)) == Path(os.path.abspath(mount_point)):
            return directory / slotimage.SLOT_IMAGE_NAME
    return None


def release_images_under(root: Path) -> list[str]:
    """Unmount and detach every slot image mounted under ``root``; return what stayed.

    Each returned line names a mount still present afterwards, or a loop device
    that still holds a released image.
    """

    roots = _roots(root)
    problems: list[str] = []
    for entry in mounts_under(root):
        image_file = _image_file_for(entry.mount_point, roots)
        if image_file is None:
            continue
        try:
            residual = slotimage.unmount(image_file, entry.mount_point)
        except slotimage.ImageError as exc:
            problems.append(f"{entry.mount_point}: {exc}")
            continue
        problems.extend(f"{image_file}: loop device {label} still holds it" for label in residual)
        slotimage._forget_host_table()
    for entry in mounts_under(root):
        problems.append(f"{entry.mount_point} is still mounted ({entry.fstype} from {entry.source})")
    return problems


@pytest.hookimpl(wrapper=True)
def pytest_runtest_teardown(item: pytest.Item, nextitem: pytest.Item | None) -> Iterator[None]:
    """After a test that used ``tmp_path``, release every slot image it left mounted there.

    This runs after the test's own fixtures are torn down, its patches undone among them: many
    tests replace the mount table with a fake one, and the release must read the real one.
    """

    funcargs = getattr(item, "funcargs", {})
    tmp_path = funcargs.get("tmp_path") if isinstance(funcargs, dict) else None
    result = yield
    if not isinstance(tmp_path, Path) or not mounts_under(tmp_path):
        return result
    problems = release_images_under(tmp_path)
    if problems:
        pytest.fail(
            "slot images left mounted under this test's tmp_path could not be released:\n  "
            + "\n  ".join(problems),
            pytrace=False,
        )
    return result
