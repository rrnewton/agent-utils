"""Tests for the opt-in project worktrees image (one btrfs image per project).

Pure tests always run. The real-image lifecycle (mkfs/mount/grow/trim) needs
passwordless ``sudo -n`` and is exercised manually in the trial recorded in
``ai_docs/transient/2026-11-05-wtimg-worktrees-image/DESIGN.md``; it is not
repeated here because the shared test host cannot guarantee loop devices.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from wrkslots import cli, slotimage, worktreesimage

GIB = 1024**3


def _entry(mount_point: Path, fstype: str) -> slotimage.MountEntry:
    return slotimage.MountEntry(mount_point=mount_point, fstype=fstype, source="/dev/loop99")


def test_settings_defaults() -> None:
    settings = worktreesimage.settings_from_obj({"path": "worktrees.img", "size_bytes": 2 * GIB})
    assert settings.filesystem == "btrfs"
    assert settings.mount == "kernel"
    assert settings.image_file(Path("/proj")) == Path("/proj/worktrees.img")


def test_settings_refuse_fuse_honestly() -> None:
    with pytest.raises(worktreesimage.WorktreesImageError, match="no.*FUSE|fuse2fs"):
        worktreesimage.settings_from_obj(
            {"path": "worktrees.img", "size_bytes": 2 * GIB, "mount": "fuse"}
        )


@pytest.mark.parametrize(
    "value",
    [
        {"path": "/abs.img", "size_bytes": 2 * GIB},
        {"path": "../up.img", "size_bytes": 2 * GIB},
        {"path": "worktrees.img", "size_bytes": 1024},
        {"path": "worktrees.img", "size_bytes": 2 * GIB, "fs": "ext4"},
        {"path": "worktrees.img", "size_bytes": 2 * GIB, "bogus": 1},
    ],
)
def test_settings_refuse_bad_values(value: dict[str, object]) -> None:
    with pytest.raises(worktreesimage.WorktreesImageError):
        worktreesimage.settings_from_obj(value)


def test_classify_mount_states(tmp_path: Path) -> None:
    slots = tmp_path / "slots"
    assert (
        worktreesimage.classify_mount(slots, [], slots_dir_exists=False, slots_dir_empty=True)
        == worktreesimage.MISSING
    )
    assert (
        worktreesimage.classify_mount(slots, [], slots_dir_exists=True, slots_dir_empty=True)
        == worktreesimage.NOT_MOUNTED
    )
    assert (
        worktreesimage.classify_mount(slots, [], slots_dir_exists=True, slots_dir_empty=False)
        == worktreesimage.NOT_EMPTY_UNMOUNTED
    )
    assert (
        worktreesimage.classify_mount(
            slots, [_entry(slots, "ext4")], slots_dir_exists=True, slots_dir_empty=False
        )
        == worktreesimage.WRONG_MOUNT
    )
    assert (
        worktreesimage.classify_mount(
            slots, [_entry(slots, "btrfs")], slots_dir_exists=True, slots_dir_empty=False
        )
        == worktreesimage.MOUNTED
    )


def test_parse_absent_means_disabled(tmp_path: Path) -> None:
    assert cli._parse_worktrees_image({}, tmp_path, tmp_path / "worktrees" / "slots") is None


def test_parse_refuses_image_inside_slots(tmp_path: Path) -> None:
    worktrees = tmp_path / "worktrees" / "slots"
    worktrees.mkdir(parents=True)
    with pytest.raises(cli.StateError, match="outside the slots directory"):
        cli._parse_worktrees_image(
            {"worktrees_image": {"path": "worktrees/slots/inner.img", "size_bytes": 2 * GIB}},
            tmp_path,
            worktrees,
        )


def _config(tmp_path: Path, state: str) -> cli.Config:
    worktrees = tmp_path / "worktrees" / "slots"
    return cli.Config(
        root=tmp_path,
        config_path=tmp_path / ".wrkslots.yml",
        worktrees=worktrees,
        control=tmp_path / "worktrees",
        machine="testhost",
        default_remote="origin",
        default_landed_ref="refs/remotes/origin/main",
        heartbeat_ttl_seconds=60,
        liveness_command=tmp_path / "liveness.py",
        worktrees_image=worktreesimage.WorktreesImageSettings(
            relative_path="worktrees.img", size_bytes=2 * GIB
        ),
        worktrees_image_state=state,
    )


def test_create_guard_refuses_unmounted(tmp_path: Path) -> None:
    with pytest.raises(cli.Refusal, match="never written onto the host"):
        cli._require_worktrees_image_mounted(_config(tmp_path, worktreesimage.NOT_MOUNTED), "create a slot")


def test_create_guard_allows_mounted_and_disabled(tmp_path: Path) -> None:
    cli._require_worktrees_image_mounted(_config(tmp_path, worktreesimage.MOUNTED), "create a slot")
    disabled = _config(tmp_path, "disabled")
    object.__setattr__(disabled, "worktrees_image", None)
    cli._require_worktrees_image_mounted(disabled, "create a slot")
