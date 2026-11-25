"""Tests for the opt-in project worktrees image (one btrfs image per project).

Pure tests always run. The real-image lifecycle (mkfs/mount/grow/trim) needs
passwordless ``sudo -n`` and is exercised manually in the trial recorded in
``ai_docs/transient/2026-11-05-wtimg-worktrees-image/DESIGN.md``; it is not
repeated here because the shared test host cannot guarantee loop devices.
"""

from __future__ import annotations

import contextlib
import fcntl
import os
import shutil
import socket
import subprocess
import threading
import time
from collections.abc import Callable
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
    assert settings.eden_backing_repo is None


def test_parse_qgroup_referenced() -> None:
    text = (
        "qgroupid         rfer         excl     max_rfer     max_excl  groupid\n"
        "0/256        16384        16384        none         none     ---\n"
        "0/257      1048576       999424        none         none     ---\n"
    )
    assert worktreesimage._parse_qgroup_referenced(text, "0/256") == 16384
    assert worktreesimage._parse_qgroup_referenced(text, "0/999") is None


def test_settings_accept_eden_backing_repo() -> None:
    settings = worktreesimage.settings_from_obj(
        {"path": "worktrees.img", "size_bytes": 2 * GIB, "eden_backing_repo": "source-repo"}
    )
    assert settings.eden_backing_repo == "source-repo"
    control = Path("/proj/worktrees")
    slots = Path("/proj/worktrees/slots")
    root = worktreesimage.eden_backing_root(settings.mount_point(control, slots))
    assert worktreesimage.eden_backing_repo_path(
        settings.mount_point(control, slots), "source-repo"
    ) == root / "source-repo"
    assert root == control / ".slots-image" / ".eden-backing"


@pytest.mark.parametrize("name", ["", "a/b", "..", ".", "validate", "a b", 7])
def test_settings_refuse_bad_eden_backing_repo(name: object) -> None:
    with pytest.raises(worktreesimage.WorktreesImageError, match="eden_backing_repo"):
        worktreesimage.settings_from_obj(
            {"path": "worktrees.img", "size_bytes": 2 * GIB, "eden_backing_repo": name}
        )


def test_settings_accept_eden_backing_list_and_map() -> None:
    listed = worktreesimage.settings_from_obj(
        {
            "path": "worktrees.img",
            "size_bytes": 2 * GIB,
            "eden_backing_repo": ["alpha", "beta"],
        }
    )
    assert listed.eden_backing_repos == (
        worktreesimage.EdenBacking("alpha", "alpha"),
        worktreesimage.EdenBacking("beta", "beta"),
    )
    assert listed.eden_backing_repo is None
    mapped = worktreesimage.settings_from_obj(
        {
            "path": "worktrees.img",
            "size_bytes": 2 * GIB,
            "eden_backing_repo": {"alpha": "team/alpha-store"},
        }
    )
    assert mapped.eden_backing_repos == (
        worktreesimage.EdenBacking("alpha", "team/alpha-store"),
    )
    assert mapped.eden_backing_repo == "alpha"


@pytest.mark.parametrize(
    "value",
    [
        ["alpha", "alpha"],
        {"alpha": "../escape"},
        {"alpha": "/absolute"},
        {"alpha": "a/../b"},
        {"alpha": "a//b"},
        {"alpha": "validate/x"},
        {"alpha": ""},
        [],
        {},
    ],
)
def test_settings_refuse_bad_eden_backing_forms(value: object) -> None:
    with pytest.raises(worktreesimage.WorktreesImageError, match="eden_backing_repo"):
        worktreesimage.settings_from_obj(
            {"path": "worktrees.img", "size_bytes": 2 * GIB, "eden_backing_repo": value}
        )


def test_eden_backing_path_refuses_a_symlink_escape(tmp_path: Path) -> None:
    mount = tmp_path / "mount"
    root = worktreesimage.eden_backing_root(mount)
    root.mkdir(parents=True)
    outside = tmp_path / "outside"
    outside.mkdir()
    (root / "link").symlink_to(outside, target_is_directory=True)

    with pytest.raises(worktreesimage.WorktreesImageError, match="outside"):
        worktreesimage.eden_backing_path(
            mount, worktreesimage.EdenBacking("link", "link/repo")
        )


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
    assert cli._parse_worktrees_image(
        {}, tmp_path, tmp_path / "worktrees" / "slots", "node-a"
    ) == (None, None)


def test_parse_refuses_image_inside_slots(tmp_path: Path) -> None:
    worktrees = tmp_path / "worktrees" / "slots"
    worktrees.mkdir(parents=True)
    with pytest.raises(cli.StateError, match="outside the slots directory"):
        cli._parse_worktrees_image(
            {"worktrees_image": {"path": "worktrees/slots/inner.img", "size_bytes": 2 * GIB}},
            tmp_path,
            worktrees,
            "node-a",
        )


def test_machines_scope_named_and_unnamed_hosts(tmp_path: Path) -> None:
    raw = {
        "worktrees_image": {
            "path": "worktrees.img",
            "size_bytes": 2 * GIB,
            "machines": ["image-host"],
        }
    }
    worktrees = tmp_path / "worktrees" / "slots"
    named, skipped = cli._parse_worktrees_image(raw, tmp_path, worktrees, "image-host")
    assert named is not None and named.machines == ("image-host",) and skipped is None
    unnamed, reason = cli._parse_worktrees_image(raw, tmp_path, worktrees, "donor-host")
    assert unnamed is None
    assert reason is not None and "image-host" in reason and "donor-host" in reason
    # No machines list: every host, as before.
    unscoped = {"worktrees_image": {"path": "worktrees.img", "size_bytes": 2 * GIB}}
    settings, skipped = cli._parse_worktrees_image(
        unscoped, tmp_path, worktrees, "any-host"
    )
    assert settings is not None and settings.machines is None and skipped is None


@pytest.mark.parametrize(
    "machines",
    [[], "image-host", [""], ["  "], [7], ["a", "a"], ["a", 1]],
)
def test_malformed_machines_refused(machines: object) -> None:
    with pytest.raises(worktreesimage.WorktreesImageError, match="machines"):
        worktreesimage.settings_from_obj(
            {"path": "worktrees.img", "size_bytes": 2 * GIB, "machines": machines}
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


def test_special_files_are_not_migratable(tmp_path: Path) -> None:
    (tmp_path / "keep.txt").write_text("x", encoding="utf-8")
    os.mkfifo(tmp_path / "pipe")
    signature = worktreesimage.listing_signature(tmp_path)
    assert [(row[0], row[1], row[2]) for row in signature] == [("keep.txt", "file", 1)]
    assert worktreesimage._skip_special(str(tmp_path), ["keep.txt", "pipe"]) == ["pipe"]


def test_bind_record_round_trip(tmp_path: Path) -> None:
    assert worktreesimage.load_binds(tmp_path) == {}
    worktreesimage.save_binds(tmp_path, {"s1": "s1"})
    assert worktreesimage.load_binds(tmp_path) == {"s1": "s1"}
    assert worktreesimage.image_mount_dir(tmp_path) == tmp_path / ".slots-image"


def test_bind_records_keep_mount_id_and_read_legacy(tmp_path: Path) -> None:
    worktreesimage.save_binds(tmp_path, {"old": "old"})
    assert worktreesimage.load_bind_records(tmp_path) == {
        "old": worktreesimage.BindRecord(subvolume="old")
    }
    worktreesimage.save_bind_records(
        tmp_path, {"s1": worktreesimage.BindRecord(subvolume="s1", mount_id=4242)}
    )
    assert worktreesimage.load_bind_records(tmp_path)["s1"].mount_id == 4242
    assert worktreesimage.load_binds(tmp_path) == {"s1": "s1"}


def test_mount_id_at_reads_the_topmost_mount(tmp_path: Path) -> None:
    table = tmp_path / "mountinfo"
    table.write_text(
        "10 1 0:1 / /proj/worktrees/slots/s1 rw - btrfs /dev/loop0 rw\n"
        "11 1 0:1 /s1 /proj/worktrees/slots/s1 rw - btrfs /dev/loop0 rw\n",
        encoding="utf-8",
    )
    assert worktreesimage.mount_id_at(Path("/proj/worktrees/slots/s1"), table) == 11
    assert worktreesimage.mount_id_at(Path("/proj/worktrees/slots/s2"), table) is None


def test_own_bind_is_not_use_but_a_foreign_mount_is(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # The recorded bind (target + device + subvolume root) is the slot's own
    # mount and is skipped; a foreign mount at the same path, and any mount
    # under the slot, still count as use (#898).
    slot = "/p/worktrees/slots/s1"
    own = f"11 1 0:1 /s1 {slot} rw - btrfs /dev/loop0 rw"
    foreign_same_path = f"12 1 0:2 /other {slot} rw - btrfs /dev/loop1 rw"
    foreign_under = f"13 1 0:2 /x {slot}/sub rw - tmpfs tmpfs rw"
    monkeypatch.setattr(
        cli,
        "_OWNED_WORKTREES_BIND_MOUNTS",
        frozenset({(Path(slot), "0:1", "/s1")}),
    )
    parsed = cli._parse_mountinfo_paths(f"{own}\n{foreign_same_path}\n{foreign_under}\n", "test")
    paths = [str(path) for path, _raw in parsed]
    assert paths.count(slot) == 1  # only the foreign mount at the same path
    assert f"{slot}/sub" in paths
    uses = cli._mountinfo_uses_slot(f"{own}\n{foreign_same_path}\n", Path(slot), "test")
    assert uses == (f"mount={slot}",)


class _FakeMounts:
    """A worktrees-image host in miniature: commands act on real directories."""

    def __init__(self, fail_umount: bool = False) -> None:
        self.calls: list[list[str]] = []
        self.fail_umount = fail_umount

    def run(self, argv: list[str], *, sudo: bool = False) -> object:
        self.calls.append(list(argv))
        if argv[:1] == ["mount"]:
            return None
        if argv[:1] == ["umount"]:
            if self.fail_umount:
                raise worktreesimage.WorktreesImageError("umount: target is busy")
            return None
        if argv[:3] == ["btrfs", "subvolume", "delete"]:
            Path(argv[3]).rmdir()
            return None
        raise AssertionError(f"unexpected command: {argv}")


def test_bind_fence_and_remove_cycle(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    config = _config(tmp_path, worktreesimage.MOUNTED)
    control = config.control
    mount_dir = worktreesimage.image_mount_dir(control)
    (mount_dir / "s1").mkdir(parents=True)
    slot_path = config.worktrees / "s1"
    fake = _FakeMounts()
    monkeypatch.setattr(worktreesimage, "_run", fake.run)
    monkeypatch.setattr(worktreesimage, "mount_id_at", lambda point: 777)

    worktreesimage.bind_slot(control, mount_dir, "s1", slot_path)
    assert worktreesimage.load_bind_records(control)["s1"].mount_id == 777
    assert worktreesimage.load_bind_records(control)["s1"].target == str(slot_path)

    fenced = config.worktrees / (".s1.fenced.1." + "a" * 32)
    cli._rename_slot_path(config, slot_path, fenced)
    assert fenced.is_dir() and not slot_path.exists()
    assert worktreesimage.load_binds(control) == {"s1": "s1"}
    assert worktreesimage.load_bind_records(control)["s1"].target == str(fenced)
    # #1733: every bind is privatised at creation, and the fence
    # privatises again before unmounting.
    assert fake.calls == [
        ["mount", "--bind", str(mount_dir / "s1"), str(slot_path)],
        ["mount", "--make-private", str(slot_path)],
        ["mount", "--make-private", str(slot_path)],
        ["umount", str(slot_path)],
        ["mount", "--bind", str(mount_dir / "s1"), str(fenced)],
        ["mount", "--make-private", str(fenced)],
    ]

    cli._remove_slot_directory(config, fenced)
    assert not fenced.exists()
    assert not (mount_dir / "s1").exists()
    assert worktreesimage.load_binds(control) == {}


def test_validate_mount_fence_without_a_bind_record(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # #1318: a validation checkout that is itself an image mount point with
    # no bind record reaches the fence still mounted; rename(2) refuses
    # EBUSY. The fence must relocate the mount, not rename it mounted.
    config = _config(tmp_path, worktreesimage.MOUNTED)
    control = config.control
    mount_dir = worktreesimage.image_mount_dir(control)
    (mount_dir / "v1").mkdir(parents=True)
    slot_path = cli._slot_directory(config, "v1", "validate")
    slot_path.mkdir(parents=True)
    fake = _FakeMounts()
    monkeypatch.setattr(worktreesimage, "_run", fake.run)
    monkeypatch.setattr(worktreesimage, "mount_id_at", lambda point: 777)
    monkeypatch.setattr(
        slotimage,
        "mount_table",
        lambda: [
            slotimage.MountEntry(
                mount_point=slot_path,
                fstype="btrfs",
                source="/dev/loop9",
                device="0:999",
                root="/v1",
            )
        ],
    )
    assert worktreesimage.load_binds(control) == {}

    fenced = slot_path.parent / (".v1.fenced.1." + "c" * 32)
    cli._rename_slot_path(config, slot_path, fenced)
    assert fenced.is_dir() and not slot_path.exists()
    assert fake.calls == [
        ["mount", "--make-private", str(slot_path)],
        ["umount", str(slot_path)],
        ["mount", "--bind", str(mount_dir / "v1"), str(fenced)],
        ["mount", "--make-private", str(fenced)],
    ]
    assert worktreesimage.load_bind_records(control)["v1"].target == str(fenced)


def test_validate_mount_fence_refuses_when_the_unmount_fails(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # #1318: an unmount failure fails closed naming the mount; the
    # checkout, its mount and the subvolume stay for a retry.
    config = _config(tmp_path, worktreesimage.MOUNTED)
    control = config.control
    mount_dir = worktreesimage.image_mount_dir(control)
    (mount_dir / "v1").mkdir(parents=True)
    slot_path = cli._slot_directory(config, "v1", "validate")
    slot_path.mkdir(parents=True)
    failing = _FakeMounts(fail_umount=True)
    monkeypatch.setattr(worktreesimage, "_run", failing.run)
    monkeypatch.setattr(worktreesimage, "_sleep", lambda seconds: None)
    monkeypatch.setattr(worktreesimage, "mount_id_at", lambda point: 777)
    monkeypatch.setattr(
        slotimage,
        "mount_table",
        lambda: [
            slotimage.MountEntry(
                mount_point=slot_path,
                fstype="btrfs",
                source="/dev/loop9",
                device="0:999",
                root="/v1",
            )
        ],
    )

    fenced = slot_path.parent / (".v1.fenced.1." + "d" * 32)
    with pytest.raises(OSError, match="cannot unmount image mount"):
        cli._rename_slot_path(config, slot_path, fenced)
    assert slot_path.is_dir() and not fenced.exists()
    assert (mount_dir / "v1").is_dir()


def test_remove_refuses_when_the_bind_unmount_fails_and_a_retry_finishes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    config = _config(tmp_path, worktreesimage.MOUNTED)
    control = config.control
    mount_dir = worktreesimage.image_mount_dir(control)
    (mount_dir / "s1").mkdir(parents=True)
    slot_path = config.worktrees / "s1"
    slot_path.mkdir(parents=True)
    worktreesimage.save_bind_records(
        control, {"s1": worktreesimage.BindRecord(subvolume="s1", mount_id=5)}
    )
    failing = _FakeMounts(fail_umount=True)
    monkeypatch.setattr(worktreesimage, "_run", failing.run)
    monkeypatch.setattr(worktreesimage, "_sleep", lambda seconds: None)
    with pytest.raises(OSError):
        cli._remove_slot_directory(config, slot_path)
    # Nothing was dropped: slot, subvolume and record are all still there.
    assert slot_path.is_dir() and (mount_dir / "s1").is_dir()
    assert worktreesimage.load_binds(control) == {"s1": "s1"}

    monkeypatch.setattr(worktreesimage, "_run", _FakeMounts().run)
    cli._remove_slot_directory(config, slot_path)
    assert not slot_path.exists() and not (mount_dir / "s1").exists()
    assert worktreesimage.load_binds(control) == {}


def _bind_entry(point: Path, propagation: str = "") -> slotimage.MountEntry:
    return slotimage.MountEntry(
        mount_point=point,
        fstype="btrfs",
        source="/dev/loop9",
        device="0:999",
        root="/s1",
        propagation=propagation,
    )


def test_bind_creates_private_on_every_peer_view(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # #1733: a bind under a mirrored tree shows once per mirror root;
    # creation privatises every view, not only the canonical path.
    config = _config(tmp_path, worktreesimage.MOUNTED)
    control = config.control
    mount_dir = worktreesimage.image_mount_dir(control)
    (mount_dir / "s1").mkdir(parents=True)
    slot_path = config.worktrees / "s1"
    mirror = tmp_path / "mirror" / "s1"
    fake = _FakeMounts()
    monkeypatch.setattr(worktreesimage, "_run", fake.run)
    monkeypatch.setattr(worktreesimage, "mount_id_at", lambda point: 777)
    monkeypatch.setattr(
        slotimage, "mount_table", lambda: [_bind_entry(slot_path), _bind_entry(mirror)]
    )

    worktreesimage.bind_slot(control, mount_dir, "s1", slot_path)
    assert ["mount", "--make-private", str(mirror)] in fake.calls
    assert ["mount", "--make-private", str(slot_path)] in fake.calls
    assert fake.calls[0] == ["mount", "--bind", str(mount_dir / "s1"), str(slot_path)]


def test_unmount_bind_takes_deleted_peer_views_and_leaves_live_binds(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # #1771: unmounting a deleted-subvolume bind takes its mirror peer
    # view with it (#1733 peer matching on device+root, //deleted root
    # included); a second, LIVE bind from the same device stays mounted.
    target = tmp_path / "slots" / "deadbind"
    mirror = tmp_path / "mirror" / "deadbind"
    live = tmp_path / "slots" / "live"
    root = "/deadbind//deleted"
    table = [
        slotimage.MountEntry(
            mount_point=target, fstype="btrfs", source="/dev/loop1",
            device="0:41", root=root,
        ),
        slotimage.MountEntry(
            mount_point=mirror, fstype="btrfs", source="/dev/loop1",
            device="0:41", root=root,
        ),
        slotimage.MountEntry(
            mount_point=live, fstype="btrfs", source="/dev/loop1",
            device="0:41", root="/live",
        ),
    ]
    fake = _FakeMounts()
    monkeypatch.setattr(worktreesimage, "_run", fake.run)
    monkeypatch.setattr(slotimage, "mount_table", lambda mountinfo=None: table)

    worktreesimage.unmount_bind(target)

    assert ["mount", "--make-private", str(target)] in fake.calls
    assert ["mount", "--make-private", str(mirror)] in fake.calls
    assert ["umount", str(target)] in fake.calls
    assert ["umount", str(mirror)] in fake.calls
    assert not any(str(live) in str(call) for call in fake.calls)


def test_ensure_binds_privatizes_an_existing_shared_bind(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # #1733 migration: a shared bind left by an older version becomes
    # private at the next re-establish, with no operator step.
    config = _config(tmp_path, worktreesimage.MOUNTED)
    control = config.control
    mount_dir = worktreesimage.image_mount_dir(control)
    (mount_dir / "s1").mkdir(parents=True)
    canonical = config.worktrees / "s1"
    canonical.mkdir(parents=True)
    mirror = tmp_path / "mirror" / "s1"
    worktreesimage.save_bind_records(
        control,
        {"s1": worktreesimage.BindRecord(subvolume="s1", target=str(canonical))},
    )
    fake = _FakeMounts()
    monkeypatch.setattr(worktreesimage, "_run", fake.run)
    monkeypatch.setattr(
        slotimage,
        "mount_table",
        lambda: [
            _bind_entry(canonical, "shared:42"),
            _bind_entry(mirror, "shared:42"),
        ],
    )

    actions = worktreesimage.ensure_binds(control, mount_dir, config.worktrees)
    assert actions == ["made slot s1 bind private"]
    assert ["mount", "--make-private", str(canonical)] in fake.calls
    assert ["mount", "--make-private", str(mirror)] in fake.calls
    assert not any(call[:2] == ["mount", "--bind"] for call in fake.calls)


def test_ensure_binds_skips_a_journaled_removal(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # #1733: config-load healing must not re-bind a slot whose removal
    # is journaled (finish journal or validate-batch seal).
    import json as json_module

    config = _config(tmp_path, worktreesimage.MOUNTED)
    control = config.control
    mount_dir = worktreesimage.image_mount_dir(control)
    (mount_dir / "s1").mkdir(parents=True)
    (mount_dir / "s2").mkdir(parents=True)
    worktreesimage.save_bind_records(
        control,
        {
            "s1": worktreesimage.BindRecord(subvolume="s1"),
            "s2": worktreesimage.BindRecord(subvolume="s2"),
        },
    )
    (control / "FINISH.1.s1.journal").write_text(
        json_module.dumps({"kind": "finish", "slot": "s1"}), encoding="utf-8"
    )
    (control / "VALIDATE-BATCH-SEAL.testhost.journal").write_text(
        json_module.dumps(
            {"kind": "validate-batch-seal", "targets": [{"slot": "s2"}]}
        ),
        encoding="utf-8",
    )
    fake = _FakeMounts()
    monkeypatch.setattr(worktreesimage, "_run", fake.run)
    monkeypatch.setattr(slotimage, "mount_table", lambda: [])

    assert worktreesimage.ensure_binds(control, mount_dir, config.worktrees) == []
    assert fake.calls == []
    assert not (config.worktrees / "s1").exists()
    assert not (config.worktrees / "s2").exists()


def test_unmount_busy_names_the_holding_pid_and_unit(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # #1733: a busy unmount that never clears names its holders
    # instead of a bare 'target is busy'.
    target = tmp_path / "slots" / "s1"
    target.mkdir(parents=True)
    proc = tmp_path / "proc"
    holder = proc / "4321"
    holder.mkdir(parents=True)
    (holder / "mountinfo").write_text(
        f"36 25 0:999 /s1 {target} rw,relatime shared:7 - btrfs /dev/loop9 rw\n",
        encoding="utf-8",
    )
    (holder / "cgroup").write_text(
        "0::/system.slice/holder-crash.service\n", encoding="utf-8"
    )
    fake = _FakeMounts(fail_umount=True)
    monkeypatch.setattr(worktreesimage, "_run", fake.run)
    monkeypatch.setattr(slotimage, "mount_table", lambda: [_bind_entry(target)])
    waits: list[float] = []

    with pytest.raises(
        worktreesimage.WorktreesImageError,
        match=r"pid 4321 \(holder-crash\.service\)",
    ):
        worktreesimage.unmount_bind(
            target, sleeper=waits.append, proc_root=proc
        )
    assert len(waits) == (worktreesimage.UMOUNT_ATTEMPTS - 1) * worktreesimage.UMOUNT_RECHECKS
    assert [call for call in fake.calls if call[0] == "umount"] == [
        ["umount", str(target)]
    ] * worktreesimage.UMOUNT_ATTEMPTS


def test_unmount_busy_clears_on_a_bounded_retry(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # #1733: a holder that exits during the backoff lets the retry
    # succeed without any holder diagnostic.
    target = tmp_path / "slots" / "s1"
    target.mkdir(parents=True)

    class _BusyTwice:
        def __init__(self) -> None:
            self.umounts = 0

        def run(self, argv: list[str], *, sudo: bool = False) -> None:
            if argv[:1] == ["umount"]:
                self.umounts += 1
                if self.umounts < 3:
                    raise worktreesimage.WorktreesImageError("umount: target is busy")
            return None

    busy = _BusyTwice()
    monkeypatch.setattr(worktreesimage, "_run", busy.run)
    monkeypatch.setattr(slotimage, "mount_table", lambda: [_bind_entry(target)])
    waits: list[float] = []

    worktreesimage.unmount_bind(target, sleeper=waits.append)
    assert busy.umounts == 3
    # No holder exists, so each re-check wait ends on its first rescan.
    assert len(waits) == 2


def _holder_proc(
    proc: Path, target: Path, *, start_epoch: float, comm: str = "holder-svc"
) -> Path:
    """One fake holder process: mountinfo, cgroup unit, comm, ns, stat (#1743)."""
    holder = proc / "4321"
    holder.mkdir(parents=True)
    (holder / "mountinfo").write_text(
        f"36 25 0:999 /s1 {target} rw,relatime shared:7 - btrfs /dev/loop9 rw\n",
        encoding="utf-8",
    )
    (holder / "cgroup").write_text(
        "0::/system.slice/holder-svc.service\n", encoding="utf-8"
    )
    (holder / "comm").write_text(f"{comm}\n", encoding="utf-8")
    (holder / "ns").mkdir()
    (holder / "ns" / "mnt").symlink_to("mnt:[4026532001]")
    ticks = int(start_epoch * os.sysconf("SC_CLK_TCK"))
    fields = ["0"] * 20
    fields[0] = "S"
    fields[19] = str(ticks)
    (holder / "stat").write_text(
        f"4321 ({comm}) " + " ".join(fields) + "\n", encoding="utf-8"
    )
    (proc / "stat").write_text("btime 0\n", encoding="utf-8")
    return holder


def test_unmount_busy_names_holder_pid_comm_namespace_and_unit(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # #1743: a holder proven older than the bind is named in full --
    # pid, comm, namespace id and unit -- without wasted retries.
    target = tmp_path / "slots" / "s1"
    target.mkdir(parents=True)
    proc = tmp_path / "proc"
    proc.mkdir()
    _holder_proc(proc, target, start_epoch=500.0, comm="old-svc")
    fake = _FakeMounts(fail_umount=True)
    monkeypatch.setattr(worktreesimage, "_run", fake.run)
    monkeypatch.setattr(slotimage, "mount_table", lambda: [_bind_entry(target)])

    with pytest.raises(worktreesimage.WorktreesImageError) as raised:
        worktreesimage.unmount_bind(
            target, sleeper=lambda _seconds: None, proc_root=proc, bind_epoch=1000.0
        )
    message = str(raised.value)
    assert "pid 4321" in message
    assert "old-svc" in message
    assert "4026532001" in message
    assert "holder-svc.service" in message
    assert [call for call in fake.calls if call[0] == "umount"] == [
        ["umount", str(target)]
    ]


def test_unmount_transient_holder_disappearing_on_recheck_retries(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # #1743: a young holder that disappears on re-check lets a bounded,
    # condition-driven retry succeed; no holder is killed or entered.
    target = tmp_path / "slots" / "s1"
    target.mkdir(parents=True)
    proc = tmp_path / "proc"
    proc.mkdir()
    holder = _holder_proc(proc, target, start_epoch=2000.0, comm="young-svc")

    class _BusyWhileHeld:
        def __init__(self) -> None:
            self.umounts = 0

        def run(self, argv: list[str], *, sudo: bool = False) -> None:
            if argv[:1] == ["umount"]:
                self.umounts += 1
                if holder.exists():
                    raise worktreesimage.WorktreesImageError("umount: target is busy")
            return None

    busy = _BusyWhileHeld()
    monkeypatch.setattr(worktreesimage, "_run", busy.run)
    monkeypatch.setattr(slotimage, "mount_table", lambda: [_bind_entry(target)])
    waits: list[float] = []

    def vanish(_seconds: float) -> None:
        waits.append(_seconds)
        shutil.rmtree(holder, ignore_errors=True)

    worktreesimage.unmount_bind(
        target, sleeper=vanish, proc_root=proc, bind_epoch=1000.0
    )
    assert busy.umounts == 2
    assert waits  # waited on a re-check, not a fixed sleep budget


def test_reload_never_rebinds_a_fenced_slot_at_its_canonical_path(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # The lead-host failure: a removal fenced mqfix, then a config reload
    # re-established its bind at the canonical path and recover refused that
    # both paths exist. ensure_binds must leave a fenced slot alone, and
    # must re-bind at the recorded (fenced) target after a reboot.
    config = _config(tmp_path, worktreesimage.MOUNTED)
    control = config.control
    mount_dir = worktreesimage.image_mount_dir(control)
    (mount_dir / "s1").mkdir(parents=True)
    canonical = config.worktrees / "s1"
    fenced = config.worktrees / (".s1.fenced.1." + "b" * 32)
    fake = _FakeMounts()
    monkeypatch.setattr(worktreesimage, "_run", fake.run)
    monkeypatch.setattr(slotimage, "mount_table", lambda: [])

    worktreesimage.save_bind_records(
        control,
        {"s1": worktreesimage.BindRecord(subvolume="s1", mount_id=5, target=str(canonical))},
    )
    fenced.mkdir(parents=True)
    assert worktreesimage.ensure_binds(control, mount_dir, config.worktrees) == []
    assert not canonical.exists()

    worktreesimage.save_bind_records(
        control,
        {"s1": worktreesimage.BindRecord(subvolume="s1", mount_id=5, target=str(fenced))},
    )
    fenced.rmdir()
    actions = worktreesimage.ensure_binds(control, mount_dir, config.worktrees)
    assert actions == ["bound slot s1 from the worktrees image"]
    assert fenced.is_dir() and not canonical.exists()


def test_drop_rebound_canonical_bind(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    config = _config(tmp_path, worktreesimage.MOUNTED)
    control = config.control
    canonical = config.worktrees / "s1"
    fenced = config.worktrees / (".s1.fenced.1." + "c" * 32)
    canonical.mkdir(parents=True)
    fenced.mkdir(parents=True)
    worktreesimage.save_bind_records(
        control,
        {"s1": worktreesimage.BindRecord(subvolume="s1", mount_id=5, target=str(fenced))},
    )
    def entry(point: Path, root: str) -> slotimage.MountEntry:
        return slotimage.MountEntry(
            mount_point=point, fstype="btrfs", source="/dev/loop2", device="0:1", root=root
        )

    monkeypatch.setattr(
        slotimage, "mount_table", lambda: [entry(canonical, "/s1"), entry(fenced, "/s1")]
    )
    fake = _FakeMounts()
    monkeypatch.setattr(worktreesimage, "_run", fake.run)
    assert cli._drop_rebound_canonical_bind(config, "s1", canonical, fenced)
    assert not canonical.exists() and fenced.is_dir()

    # A canonical path whose mount is not the recorded subvolume is content
    # in the way, and recovery must keep refusing.
    canonical.mkdir(parents=True)
    monkeypatch.setattr(
        slotimage, "mount_table", lambda: [entry(canonical, "/other"), entry(fenced, "/s1")]
    )
    assert not cli._drop_rebound_canonical_bind(config, "s1", canonical, fenced)
    assert canonical.is_dir()


def test_finish_recognises_an_empty_fenced_bind_root(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Exactly the lead host's stuck mqfix state: the first remove's git
    # step emptied the flat checkout at the fenced bind root and dropped
    # its registration, then died on the mount-point rmdir, so the journal
    # still sits in the fence phase with nothing removed. Recovery must
    # recognise the removal as finished, not demand a .git to repair.
    config = _config(tmp_path, worktreesimage.MOUNTED)
    monkeypatch.setenv("GIT_AUTHOR_NAME", "bind-test")
    monkeypatch.setenv("GIT_AUTHOR_EMAIL", "bind-test@example.invalid")
    monkeypatch.setenv("GIT_COMMITTER_NAME", "bind-test")
    monkeypatch.setenv("GIT_COMMITTER_EMAIL", "bind-test@example.invalid")
    subprocess.run(["git", "init", "-q", "-b", "main"], cwd=tmp_path, check=True)
    subprocess.run(
        ["git", "commit", "-q", "--allow-empty", "-m", "init"], cwd=tmp_path, check=True
    )
    head = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=tmp_path, check=True,
        capture_output=True, text=True,
    ).stdout.strip()
    control = config.control
    mount_dir = worktreesimage.image_mount_dir(control)
    (mount_dir / "s1").mkdir(parents=True)
    fenced = config.worktrees / (".s1.fenced.1." + "d" * 32)
    fenced.mkdir(parents=True)
    worktreesimage.save_bind_records(
        control,
        {"s1": worktreesimage.BindRecord(subvolume="s1", mount_id=5, target=str(fenced))},
    )
    monkeypatch.setattr(
        slotimage,
        "mount_table",
        lambda: [
            slotimage.MountEntry(
                mount_point=fenced, fstype="btrfs", source="/dev/loop2",
                device="0:99", root="/s1",
            )
        ],
    )
    fake = _FakeMounts()
    monkeypatch.setattr(worktreesimage, "_run", fake.run)
    identity = cli.ProcessIdentity(
        pid=123, start_ticks=456, boot_id="boot", host_id="host", cgroup_path="/"
    )
    checkout = cli.Checkout(
        name="repo",
        path="worktrees/slots/s1",
        repository=".",
        branch="main",
        start_point=head,
        remote="origin",
        remote_url_sha256="0" * 64,
        landed_ref="refs/remotes/origin/main",
        head=head,
    )
    record = cli.ActiveRecord(
        slot="s1",
        agent="worker-a",
        task="t1",
        purpose="probe",
        slot_type="agent",
        machine="testhost",
        generation=1,
        created_at="2026-11-05T00:00:00+00:00",
        heartbeat_at="2026-11-05T00:00:00+00:00",
        heartbeat_ttl_seconds=600,
        owner=None,
        coordinator_lease=identity,
        coordinator_recovery_note=None,
        handoff=None,
        checkouts=(checkout,),
        layout="flat",
    )
    journal: dict[str, object] = {
        "schema": cli.SCHEMA,
        "kind": "finish",
        "machine": "testhost",
        "slot": "s1",
        "phase": "fenced",
        "fenced": fenced.relative_to(tmp_path).as_posix(),
        "removed": [],
        "salvage": [],
    }
    journal_path = control / "ACTIVE.testhost.journal"
    cli._atomic_write_json(
        cli._active_path(config),
        cli._active_to_obj(cli.ActiveState(machine="testhost", revision=0, slots=(record,))),
    )
    cli._atomic_write_json(
        cli._archive_path(config),
        cli._archive_to_obj(cli.ArchiveState(machine="testhost", revision=0, records=())),
    )
    finish = cli._FinishContext(mode="remove", actor="coordinator")
    result = cli._finish_remove_paths(
        config, record, journal, cli._GitVcs(), finish, journal_path=journal_path
    )
    assert result["removed"] == ["repo"]
    assert result["phase"] == "removed"
    assert not fenced.exists()
    assert not (mount_dir / "s1").exists()
    assert worktreesimage.load_binds(control) == {}

    # Safety: the same empty root is refused while Git still registers the
    # checkout, or while the mount there is not the recorded subvolume.
    fenced.mkdir(parents=True)
    (mount_dir / "s1").mkdir(parents=True)
    worktreesimage.save_bind_records(
        control,
        {"s1": worktreesimage.BindRecord(subvolume="s1", mount_id=5, target=str(fenced))},
    )
    vcs = cli._GitVcs()
    assert cli._git_removal_finished_at_bind_root(config, "s1", checkout, fenced, vcs)
    (fenced / "leftover").write_text("x", encoding="utf-8")
    assert not cli._git_removal_finished_at_bind_root(config, "s1", checkout, fenced, vcs)
    (fenced / "leftover").unlink()
    subprocess.run(
        ["git", "worktree", "add", str(fenced), "-b", "again"],
        cwd=tmp_path, check=True, capture_output=True, text=True,
    )
    assert not cli._git_removal_finished_at_bind_root(config, "s1", checkout, fenced, vcs)


def test_remove_worktree_accepts_the_recorded_bind_root(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    if os.environ.get("WRKSLOTS_ALLOW_ROOT_TESTS") != "1":
        pytest.skip(
            "root-mount coverage lost: recorded bind-root test "
            "not run; set WRKSLOTS_ALLOW_ROOT_TESTS=1 to "
            "run it"
        )
    # Real bind mount, real Git: a flat checkout at the bind root cannot be
    # rmdir'ed by git, but git still empties it and drops its registration;
    # the recorded bind root accepts exactly that outcome (#898 follow-up).
    if shutil.which("sudo") is None or shutil.which("git") is None:
        pytest.skip("needs sudo and git")
    probe = subprocess.run(
        ["sudo", "-n", "true"], capture_output=True, text=True, check=False
    )
    if probe.returncode != 0:
        pytest.skip("needs passwordless sudo for a real bind mount")
    monkeypatch.setenv("GIT_AUTHOR_NAME", "bind-test")
    monkeypatch.setenv("GIT_AUTHOR_EMAIL", "bind-test@example.invalid")
    monkeypatch.setenv("GIT_COMMITTER_NAME", "bind-test")
    monkeypatch.setenv("GIT_COMMITTER_EMAIL", "bind-test@example.invalid")
    repository = tmp_path / "repo"
    repository.mkdir()
    subprocess.run(["git", "init", "-q", "-b", "main"], cwd=repository, check=True)
    subprocess.run(
        ["git", "commit", "-q", "--allow-empty", "-m", "init"], cwd=repository, check=True
    )
    source = tmp_path / "source"
    source.mkdir()
    slot = tmp_path / "slot"
    slot.mkdir()
    mounted = subprocess.run(
        ["sudo", "-n", "mount", "--bind", str(source), str(slot)],
        capture_output=True, text=True, check=False,
    )
    if mounted.returncode != 0:
        pytest.skip(f"real bind mount unavailable: {mounted.stderr.strip()}")
    try:
        subprocess.run(
            ["git", "worktree", "add", str(slot), "-b", "flat1"],
            cwd=repository, check=True, capture_output=True, text=True,
        )
        (slot / "work.txt").write_text("work\n", encoding="utf-8")
        monkeypatch.setattr(
            cli, "_OWNED_WORKTREES_BIND_MOUNTS", frozenset({(slot, "0:1", "/")})
        )
        cli._GitVcs().remove_worktree(repository, slot, force=True)
        assert slot.absolute() not in cli._GitVcs().listed_worktrees(repository)
        assert list(slot.iterdir()) == []
        # A leftover file means git did not finish: the same call refuses.
        (slot / "leftover.txt").write_text("x", encoding="utf-8")

        def refuse_run(repo: Path, args: object, **kwargs: object) -> subprocess.CompletedProcess[str]:
            return subprocess.CompletedProcess(
                ["git"], 255, "", "failed to delete: Device or resource busy"
            )

        monkeypatch.setattr(cli._GitVcs, "_run", staticmethod(refuse_run))
        with pytest.raises(cli.Refusal, match="git worktree remove failed"):
            cli._GitVcs().remove_worktree(repository, slot, force=True)
    finally:
        subprocess.run(
            ["sudo", "-n", "umount", str(slot)], capture_output=True, check=False
        )


def test_listing_signature_and_quota(tmp_path: Path) -> None:
    (tmp_path / "a.txt").write_text("hello", encoding="utf-8")
    (tmp_path / "sub").mkdir()
    (tmp_path / "sub" / "b.txt").write_text("x", encoding="utf-8")
    signature = worktreesimage.listing_signature(tmp_path)
    triples = {(row[0], row[1], row[2]) for row in signature}
    assert ("a.txt", "file", 5) in triples
    assert ("sub", "dir", 0) in triples
    assert not worktreesimage.process_inside(tmp_path / "nowhere")
    settings = worktreesimage.settings_from_obj(
        {"path": "w.img", "size_bytes": 2 * GIB, "slot_quota_bytes": 7}
    )
    assert settings.slot_quota_bytes == 7
    with pytest.raises(worktreesimage.WorktreesImageError):
        worktreesimage.settings_from_obj(
            {"path": "w.img", "size_bytes": 2 * GIB, "slot_quota_bytes": 0}
        )


def test_create_guard_refuses_unmounted(tmp_path: Path) -> None:
    with pytest.raises(cli.Refusal, match="never written onto the host"):
        cli._require_worktrees_image_mounted(_config(tmp_path, worktreesimage.NOT_MOUNTED), "create a slot")


def test_create_guard_allows_mounted_and_disabled(tmp_path: Path) -> None:
    cli._require_worktrees_image_mounted(_config(tmp_path, worktreesimage.MOUNTED), "create a slot")
    disabled = _config(tmp_path, "disabled")
    object.__setattr__(disabled, "worktrees_image", None)
    cli._require_worktrees_image_mounted(disabled, "create a slot")


def _bind_worker(control: str, mount_dir: str, slots_dir: str, slot: str, rounds: int) -> None:
    """One concurrent writer: fake mounts, real record file and flock."""

    def fake_run(
        argv: list[str], *, sudo: bool = False
    ) -> subprocess.CompletedProcess[str]:
        return subprocess.CompletedProcess(argv, 0, "", "")

    def fake_mount_id(
        mount_point: Path, mountinfo: Path = Path("/proc/self/mountinfo")
    ) -> int | None:
        return 4242

    worktreesimage._run = fake_run
    worktreesimage.mount_id_at = fake_mount_id
    control_path, mount_path = Path(control), Path(mount_dir)
    slot_path = Path(slots_dir) / slot
    for _ in range(rounds):
        worktreesimage.bind_slot(control_path, mount_path, slot, slot_path)
        worktreesimage.unbind_slot(control_path, slot, slot_path)
    worktreesimage.bind_slot(control_path, mount_path, slot, slot_path)


def test_concurrent_bind_writers_never_lose_entries(tmp_path: Path) -> None:
    import multiprocessing

    control = tmp_path / "worktrees"
    mount_dir = worktreesimage.image_mount_dir(control)
    slots_dir = tmp_path / "slots"
    slots = [f"w{i}" for i in range(6)]
    for slot in slots:
        (mount_dir / slot).mkdir(parents=True)
    context = multiprocessing.get_context("fork")
    workers = [
        context.Process(
            target=_bind_worker,
            args=(str(control), str(mount_dir), str(slots_dir), slot, 15),
        )
        for slot in slots
    ]
    for worker in workers:
        worker.start()
    for worker in workers:
        worker.join(timeout=60)
        assert worker.exitcode == 0
    assert worktreesimage.load_binds(control) == {slot: slot for slot in slots}
    assert list(control.glob(".worktrees-image.json.tmp.*")) == []
    lock = control / worktreesimage.RECORD_LOCK_NAME
    assert lock.is_file() and oct(lock.stat().st_mode & 0o777) == "0o600"


def test_no_unlocked_record_writer_remains() -> None:
    import inspect

    for function in (
        worktreesimage.bind_slot,
        worktreesimage.forget_bind,
        worktreesimage.relocate_bind,
        worktreesimage.save_bind_records,
        worktreesimage.record_bind_if_absent,
        worktreesimage.restore_bind_records,
    ):
        assert "_update_bind_records" in inspect.getsource(function), function.__name__
    assert "forget_bind(" in inspect.getsource(worktreesimage.unbind_slot)
    assert "save_bind_records" in inspect.getsource(worktreesimage.save_binds)
    # #2238: no command saves a whole map read earlier without the lock;
    # each changes only its own slots through a locked writer.
    from wrkslots import imagecmd

    for caller in (cli, imagecmd):
        assert "save_bind_records(" not in inspect.getsource(caller), caller.__name__
    module = inspect.getsource(worktreesimage)
    assert 'path.name + ".tmp"' not in module  # the old fixed temp name
    assert module.count("os.replace(temporary, path)") == 1  # one write path


def test_unchanged_records_are_not_rewritten(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    control = tmp_path / "worktrees"
    control.mkdir(parents=True)
    records = {"s1": worktreesimage.BindRecord(subvolume="s1", mount_id=5)}
    worktreesimage.save_bind_records(control, records)
    path = worktreesimage.record_path(control)
    before = (path.read_bytes(), path.stat().st_mtime_ns)
    worktreesimage.save_bind_records(control, dict(records))
    assert (path.read_bytes(), path.stat().st_mtime_ns) == before

    mount_dir = worktreesimage.image_mount_dir(control)
    (mount_dir / "s1").mkdir(parents=True)
    slots_dir = tmp_path / "slots"
    target = slots_dir / "s1"
    monkeypatch.setattr(
        slotimage,
        "mount_table",
        lambda: [
            slotimage.MountEntry(mount_point=target, fstype="btrfs", source="/dev/loop0")
        ],
    )
    fake = _FakeMounts()
    monkeypatch.setattr(worktreesimage, "_run", fake.run)
    assert worktreesimage.ensure_binds(control, mount_dir, slots_dir) == []
    assert fake.calls == []
    assert (path.read_bytes(), path.stat().st_mtime_ns) == before


def test_ensure_binds_skips_a_record_whose_subvolume_is_gone(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Remove crashed (or deleted) outside unbind's crash order: the record
    # still says bound but the subvolume is gone. Config load must not
    # fail, must not re-create the slot path, and must still bind others.
    control = tmp_path / "worktrees"
    mount_dir = worktreesimage.image_mount_dir(control)
    (mount_dir / "ok").mkdir(parents=True)
    slots_dir = tmp_path / "slots"
    worktreesimage.save_bind_records(
        control,
        {
            "gone": worktreesimage.BindRecord(
                subvolume="gone", target=str(slots_dir / "gone")
            ),
            "ok": worktreesimage.BindRecord(
                subvolume="ok", target=str(slots_dir / "ok")
            ),
        },
    )
    fake = _FakeMounts()
    monkeypatch.setattr(worktreesimage, "_run", fake.run)
    monkeypatch.setattr(slotimage, "mount_table", lambda: [])
    actions = worktreesimage.ensure_binds(control, mount_dir, slots_dir)
    assert actions == ["bound slot ok from the worktrees image"]
    assert not (slots_dir / "gone").exists()
    assert set(worktreesimage.load_binds(control)) == {"gone", "ok"}


def _doubled_setup(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    layers: list[slotimage.MountEntry],
) -> tuple[Path, Path, Path, list[slotimage.MountEntry]]:
    """One recorded slot whose target carries the given bind layers.

    The fake mount table is the mutable ``table`` list; a fake umount
    removes the current top layer by the mount-id chain, exactly as a
    plain umount of the mountpoint removes the top copy.
    """

    control = tmp_path / "worktrees"
    mount_dir = worktreesimage.image_mount_dir(control)
    (mount_dir / "s1").mkdir(parents=True)
    slots_dir = tmp_path / "slots"
    canonical = slots_dir / "s1"
    canonical.mkdir(parents=True)
    worktreesimage.save_bind_records(
        control,
        {
            "s1": worktreesimage.BindRecord(
                subvolume="s1", mount_id=111, target=str(canonical)
            )
        },
    )
    table = list(layers)
    monkeypatch.setattr(slotimage, "mount_table", lambda: list(table))

    def top_id() -> int | None:
        ordered = worktreesimage._ordered_stack(
            [entry for entry in table if entry.mount_point == canonical]
        )
        return ordered[-1].mount_id if ordered else None

    monkeypatch.setattr(worktreesimage, "mount_id_at", lambda point: top_id())
    return control, slots_dir, canonical, table


def _stack_entry(
    point: Path, mount_id: int | None, parent_id: int | None, **overrides: object
) -> slotimage.MountEntry:
    values: dict[str, object] = {
        "mount_point": point,
        "fstype": "btrfs",
        "source": "/dev/loop9",
        "device": "0:999",
        "root": "/s1",
        "mount_id": mount_id,
        "parent_id": parent_id,
    }
    values.update(overrides)
    return slotimage.MountEntry(**values)  # type: ignore[arg-type]


def test_collapse_binds_dry_run_reports_without_acting(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    canonical = tmp_path / "slots" / "s1"
    control, slots_dir, _canonical, table = _doubled_setup(
        tmp_path,
        monkeypatch,
        [_stack_entry(canonical, 20, 10), _stack_entry(canonical, 30, 20)],
    )
    calls: list[list[str]] = []
    monkeypatch.setattr(
        worktreesimage, "_run", lambda argv, sudo=False: calls.append(list(argv))
    )

    done, skipped = worktreesimage.collapse_duplicate_binds(
        control, slots_dir, apply=False
    )

    assert skipped == []
    assert done == [
        "slot s1: 2 identical binds stacked at "
        f"{canonical}; would unmount 1 top copy, leaving mount id 20"
    ]
    assert calls == []
    assert len(table) == 2
    assert worktreesimage.load_bind_records(control)["s1"].mount_id == 111


def test_collapse_binds_apply_unmounts_only_the_top_copy(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # The table lists the top copy first; the mount-id chain, not
    # table order, decides which layer is top, and each unmount is
    # preceded by a fresh table read.
    canonical = tmp_path / "slots" / "s1"
    bottom = _stack_entry(canonical, 20, 10)
    top = _stack_entry(canonical, 30, 20)
    control, slots_dir, _canonical, table = _doubled_setup(
        tmp_path, monkeypatch, [top, bottom]
    )
    umounts: list[list[str]] = []

    def run(argv: list[str], *, sudo: bool = False) -> None:
        umounts.append(list(argv))
        ordered = worktreesimage._ordered_stack(table)
        assert ordered is not None
        top = ordered[-1]
        table[:] = [entry for entry in table if entry is not top]

    monkeypatch.setattr(worktreesimage, "_run", run)

    done, skipped = worktreesimage.collapse_duplicate_binds(
        control, slots_dir, apply=True
    )

    assert skipped == []
    assert done == [
        f"slot s1: removed 1 duplicate bind(s) at {canonical}; "
        "surviving mount id 20"
    ]
    assert umounts == [["umount", str(canonical)]]
    assert table == [bottom]
    assert worktreesimage.load_bind_records(control)["s1"].mount_id == 20


def test_collapse_binds_busy_top_is_skipped_as_busy(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    canonical = tmp_path / "slots" / "s1"
    control, slots_dir, _canonical, table = _doubled_setup(
        tmp_path,
        monkeypatch,
        [_stack_entry(canonical, 20, 10), _stack_entry(canonical, 30, 20)],
    )

    def run(argv: list[str], *, sudo: bool = False) -> None:
        raise worktreesimage.WorktreesImageError("umount: target is busy")

    monkeypatch.setattr(worktreesimage, "_run", run)

    done, skipped = worktreesimage.collapse_duplicate_binds(
        control, slots_dir, apply=True
    )

    assert done == []
    assert len(skipped) == 1
    assert "busy" in skipped[0] and "needs root" not in skipped[0]
    assert len(table) == 2
    assert worktreesimage.load_bind_records(control)["s1"].mount_id == 111


def test_collapse_binds_partial_failure_reports_what_happened(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Three layers; the second unmount is busy. The report says how
    # many layers went, what remains, and the record names the fresh
    # top mount id rather than the unmounted one.
    canonical = tmp_path / "slots" / "s1"
    control, slots_dir, _canonical, table = _doubled_setup(
        tmp_path,
        monkeypatch,
        [
            _stack_entry(canonical, 20, 10),
            _stack_entry(canonical, 30, 20),
            _stack_entry(canonical, 40, 30),
        ],
    )
    attempts: list[list[str]] = []

    def run(argv: list[str], *, sudo: bool = False) -> None:
        attempts.append(list(argv))
        if len(attempts) == 2:
            raise worktreesimage.WorktreesImageError("umount: target is busy")
        ordered = worktreesimage._ordered_stack(table)
        assert ordered is not None
        top = ordered[-1]
        table[:] = [entry for entry in table if entry is not top]

    monkeypatch.setattr(worktreesimage, "_run", run)

    done, skipped = worktreesimage.collapse_duplicate_binds(
        control, slots_dir, apply=True
    )

    assert done == [
        f"slot s1: removed 1 duplicate bind(s) at {canonical}; "
        "2 layers remain, top mount id 30"
    ]
    assert len(skipped) == 1
    assert "stopped after 1 removal(s)" in skipped[0] and "busy" in skipped[0]
    assert worktreesimage.load_bind_records(control)["s1"].mount_id == 30


def test_collapse_binds_differing_options_are_left_alone(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # A read-only self-bind over the slot is not a duplicate: the
    # per-mount options differ, so nothing is unmounted or rewritten.
    canonical = tmp_path / "slots" / "s1"
    control, slots_dir, _canonical, _table = _doubled_setup(
        tmp_path,
        monkeypatch,
        [
            _stack_entry(canonical, 20, 10, options="rw"),
            _stack_entry(canonical, 30, 20, options="ro"),
        ],
    )
    calls: list[list[str]] = []
    monkeypatch.setattr(
        worktreesimage, "_run", lambda argv, sudo=False: calls.append(list(argv))
    )

    done, skipped = worktreesimage.collapse_duplicate_binds(
        control, slots_dir, apply=True
    )

    assert done == []
    assert len(skipped) == 1 and "differ" in skipped[0]
    assert calls == []
    assert worktreesimage.load_bind_records(control)["s1"].mount_id == 111


def test_collapse_binds_ambiguous_mount_ids_refuse(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    canonical = tmp_path / "slots" / "s1"
    control, slots_dir, _canonical, _table = _doubled_setup(
        tmp_path,
        monkeypatch,
        [_stack_entry(canonical, None, None), _stack_entry(canonical, None, None)],
    )
    calls: list[list[str]] = []
    monkeypatch.setattr(
        worktreesimage, "_run", lambda argv, sudo=False: calls.append(list(argv))
    )

    done, skipped = worktreesimage.collapse_duplicate_binds(
        control, slots_dir, apply=True
    )

    assert done == []
    assert len(skipped) == 1 and "cannot order" in skipped[0]
    assert calls == []


def test_ensure_binds_never_collapses_on_config_load(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Prevention only: a doubled target found on load is left for
    # the explicit collapse-binds command; no umount runs here.
    canonical = tmp_path / "slots" / "s1"
    control = tmp_path / "worktrees"
    mount_dir = worktreesimage.image_mount_dir(control)
    (mount_dir / "s1").mkdir(parents=True)
    slots_dir = tmp_path / "slots"
    canonical.mkdir(parents=True)
    worktreesimage.save_bind_records(
        control,
        {
            "s1": worktreesimage.BindRecord(
                subvolume="s1", mount_id=111, target=str(canonical)
            )
        },
    )
    table = [_bind_entry(canonical), _bind_entry(canonical)]
    monkeypatch.setattr(slotimage, "mount_table", lambda: list(table))
    calls: list[list[str]] = []
    monkeypatch.setattr(
        worktreesimage, "_run", lambda argv, sudo=False: calls.append(list(argv))
    )

    actions = worktreesimage.ensure_binds(control, mount_dir, slots_dir)

    assert actions == []
    assert calls == []
    assert len(table) == 2


def test_flock_records_until_gap_acquisition_is_not_leaked(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # The helper can publish its acquired descriptor in the gap
    # between the waiter's deadline expiring and its check: the
    # waiter must then use that descriptor, never leak the lock on
    # it for the rest of the process.
    lock_path = tmp_path / ".worktrees-image.json.lock"
    descriptor = os.open(lock_path, os.O_RDWR | os.O_CREAT, 0o600)
    fcntl.flock(descriptor, fcntl.LOCK_EX)
    real_event = threading.Event
    real_wait = threading.Event.wait

    class LyingEvent(threading.Event):
        def wait(self, timeout: float | None = None) -> bool:
            real_wait(self, timeout)
            return False

    monkeypatch.setattr(threading, "Event", LyingEvent)
    helper_blocking = real_event()
    real_flock = fcntl.flock

    def flock_spy(fd: int, operation: int) -> None:
        if operation & fcntl.LOCK_EX and not operation & fcntl.LOCK_NB:
            helper_blocking.set()
        real_flock(fd, operation)

    monkeypatch.setattr(fcntl, "flock", flock_spy)
    outcome: list[int | None] = []

    def call() -> None:
        fd = os.open(lock_path, os.O_RDWR)
        outcome.append(
            worktreesimage._flock_records_until(fd, time.monotonic() + 60)
        )
        if outcome[-1] is None:
            os.close(fd)

    waiter = threading.Thread(target=call)
    waiter.start()
    # Free the lock only once the helper is inside its blocking
    # flock: it then acquires and publishes, while the lying wait
    # reports a timeout, which is exactly the gap interleaving.
    assert helper_blocking.wait(timeout=60)
    real_flock(descriptor, fcntl.LOCK_UN)
    os.close(descriptor)
    waiter.join(timeout=60)
    assert not waiter.is_alive()
    assert outcome[0] is not None
    held_fd = outcome[0]
    assert held_fd is not None
    fcntl.flock(held_fd, fcntl.LOCK_UN)
    os.close(held_fd)
    # Nothing leaked: a fresh non-blocking acquisition succeeds.
    fresh = os.open(lock_path, os.O_RDWR)
    try:
        fcntl.flock(fresh, fcntl.LOCK_EX | fcntl.LOCK_NB)
        fcntl.flock(fresh, fcntl.LOCK_UN)
    finally:
        os.close(fresh)


def test_collapse_binds_raced_empty_target_is_an_error_not_a_success(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Another unmount takes the second-to-last layer between the
    # fresh read and our umount, so ours removes the last one: that
    # is reported as an error with a rebind hint, the record keeps
    # its mount id, and nothing claims success.
    canonical = tmp_path / "slots" / "s1"
    control, slots_dir, _canonical, table = _doubled_setup(
        tmp_path,
        monkeypatch,
        [_stack_entry(canonical, 20, 10), _stack_entry(canonical, 30, 20)],
    )

    def run(argv: list[str], *, sudo: bool = False) -> None:
        table.clear()

    monkeypatch.setattr(worktreesimage, "_run", run)

    done, skipped = worktreesimage.collapse_duplicate_binds(
        control, slots_dir, apply=True
    )

    assert done == []
    assert len(skipped) == 1
    assert "NO bind remains" in skipped[0] and "rebind" in skipped[0]
    assert worktreesimage.load_bind_records(control)["s1"].mount_id == 111


def test_collapse_binds_reports_earlier_progress_when_a_later_slot_fails(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Slot a collapses; slot b's mount table read fails. The done
    # line for a is still returned, and b lands in skipped, not a
    # traceback.
    control = tmp_path / "worktrees"
    mount_dir = worktreesimage.image_mount_dir(control)
    (mount_dir / "a1").mkdir(parents=True)
    (mount_dir / "b1").mkdir(parents=True)
    slots_dir = tmp_path / "slots"
    target_a = slots_dir / "a1"
    target_b = slots_dir / "b1"
    target_a.mkdir(parents=True)
    target_b.mkdir(parents=True)
    worktreesimage.save_bind_records(
        control,
        {
            "a1": worktreesimage.BindRecord(
                subvolume="a1", mount_id=111, target=str(target_a)
            ),
            "b1": worktreesimage.BindRecord(
                subvolume="b1", mount_id=111, target=str(target_b)
            ),
        },
    )
    table = [
        _stack_entry(target_a, 20, 10),
        _stack_entry(target_a, 30, 20),
        _stack_entry(target_b, 20, 10),
        _stack_entry(target_b, 30, 20),
    ]
    reads = {"count": 0}

    def table_read() -> list[slotimage.MountEntry]:
        reads["count"] += 1
        if reads["count"] > 4:
            raise slotimage.ImageError("mount table vanished")
        return list(table)

    monkeypatch.setattr(slotimage, "mount_table", table_read)

    def run(argv: list[str], *, sudo: bool = False) -> None:
        target = Path(argv[-1])
        ordered = worktreesimage._ordered_stack(
            [entry for entry in table if entry.mount_point == target]
        )
        assert ordered is not None
        top = ordered[-1]
        table[:] = [entry for entry in table if entry is not top]

    monkeypatch.setattr(worktreesimage, "_run", run)
    monkeypatch.setattr(worktreesimage, "mount_id_at", lambda point: 20)

    done, skipped = worktreesimage.collapse_duplicate_binds(
        control, slots_dir, apply=True
    )

    assert done == [
        f"slot a1: removed 1 duplicate bind(s) at {target_a}; "
        "surviving mount id 20"
    ]
    assert len(skipped) == 1 and "b1" in skipped[0]


def test_ensure_binds_skips_the_lock_when_everything_is_mounted(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Double-checked locking: the common load finds every target
    # mounted and private, and never waits on the records lock.
    control = tmp_path / "worktrees"
    mount_dir = worktreesimage.image_mount_dir(control)
    (mount_dir / "s1").mkdir(parents=True)
    slots_dir = tmp_path / "slots"
    canonical = slots_dir / "s1"
    canonical.mkdir(parents=True)
    worktreesimage.save_bind_records(
        control,
        {
            "s1": worktreesimage.BindRecord(
                subvolume="s1", mount_id=111, target=str(canonical)
            )
        },
    )
    monkeypatch.setattr(
        slotimage, "mount_table", lambda: [_bind_entry(canonical)]
    )

    def fail_if_locked(*args: object, **kwargs: object) -> object:
        raise AssertionError("ensure_binds took the records lock")

    monkeypatch.setattr(worktreesimage, "_records_lock", fail_if_locked)

    assert worktreesimage.ensure_binds(control, mount_dir, slots_dir) == []


def test_records_lock_wait_is_bounded(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # A wedged holder must fail its own waiter, not freeze it: the
    # records lock refuses after its bound with the holder named as
    # the retry condition.
    control = tmp_path / "worktrees"
    control.mkdir(parents=True)
    monkeypatch.setattr(worktreesimage, "RECORDS_LOCK_TIMEOUT_SECONDS", 0.2)
    failures: list[BaseException] = []

    def wait_for_it() -> None:
        try:
            with worktreesimage._records_lock(control):
                pass
        except BaseException as exc:  # asserted below
            failures.append(exc)

    with worktreesimage._records_lock(control):
        waiter = threading.Thread(target=wait_for_it)
        waiter.start()
        waiter.join(timeout=60)
    assert not waiter.is_alive()
    assert len(failures) == 1
    assert isinstance(failures[0], worktreesimage.WorktreesImageError)
    assert "bind records lock is busy" in str(failures[0])


def test_ensure_image_mounted_mounts_under_the_records_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # The image mount's check and mount hold the same lock as bind
    # establishment, so two post-boot commands cannot stack image
    # mounts either.
    config = _config(tmp_path, worktreesimage.NOT_MOUNTED)
    control = config.control
    (config.root / "worktrees.img").write_bytes(b"x")
    lock_path = control / worktreesimage.RECORD_LOCK_NAME
    held: list[bool] = []

    def run(argv: list[str], *, sudo: bool = False) -> None:
        if argv[:1] == ["mount"]:
            descriptor = os.open(lock_path, os.O_RDWR)
            try:
                try:
                    fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                except BlockingIOError:
                    held.append(True)
                else:
                    held.append(False)
                    fcntl.flock(descriptor, fcntl.LOCK_UN)
            finally:
                os.close(descriptor)

    monkeypatch.setattr(worktreesimage, "_run", run)
    monkeypatch.setattr(slotimage, "mount_table", lambda: [])
    monkeypatch.setattr(
        worktreesimage, "apply_quota_mode", lambda *args, **kwargs: None
    )

    settings = config.worktrees_image
    assert settings is not None
    worktreesimage.ensure_image_mounted(
        config.root, control, settings, config.worktrees
    )

    assert held == [True]


def test_concurrent_ensure_binds_binds_exactly_once(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # #2178: two config loads racing on one unmounted checkout must
    # produce one bind, not a stack. The fake bind blocks on an event
    # while the first caller is inside it; whether the second caller
    # then attempts its own bind is decided by the records lock's
    # state, never by timing: while the first caller binds, the lock
    # is either held (serialised) or free (the second bind follows).
    control = tmp_path / "worktrees"
    mount_dir = worktreesimage.image_mount_dir(control)
    (mount_dir / "s1").mkdir(parents=True)
    slots_dir = tmp_path / "slots"
    canonical = slots_dir / "s1"
    worktreesimage.save_bind_records(
        control,
        {"s1": worktreesimage.BindRecord(subvolume="s1", target=str(canonical))},
    )
    table: list[slotimage.MountEntry] = []
    binds: list[list[str]] = []
    bind_entered = threading.Event()
    second_bind = threading.Event()
    proceed = threading.Event()

    def run(argv: list[str], *, sudo: bool = False) -> None:
        if argv[:2] == ["mount", "--bind"]:
            binds.append(list(argv))
            if len(binds) == 1:
                bind_entered.set()
                proceed.wait(timeout=60)
            else:
                second_bind.set()
            table.append(_bind_entry(canonical))

    monkeypatch.setattr(worktreesimage, "_run", run)
    monkeypatch.setattr(slotimage, "mount_table", lambda: list(table))
    monkeypatch.setattr(worktreesimage, "mount_id_at", lambda point: 777)
    errors: list[BaseException] = []

    def load() -> None:
        try:
            worktreesimage.ensure_binds(control, mount_dir, slots_dir)
        except BaseException as exc:  # surfaced by the assertions below
            errors.append(exc)

    first = threading.Thread(target=load)
    first.start()
    assert bind_entered.wait(timeout=60)
    second = threading.Thread(target=load)
    second.start()
    lock_path = control / worktreesimage.RECORD_LOCK_NAME
    descriptor = os.open(lock_path, os.O_RDWR)
    try:
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            serialized = True
        else:
            serialized = False
            fcntl.flock(descriptor, fcntl.LOCK_UN)
    finally:
        os.close(descriptor)
    if not serialized:
        # No lock is held across the bind: the second caller reaches
        # its own bind, which is the race this test guards against.
        assert second_bind.wait(timeout=60)
    proceed.set()
    first.join(timeout=60)
    second.join(timeout=60)
    assert not first.is_alive() and not second.is_alive()
    assert errors == []
    assert len(binds) == 1
    assert len(table) == 1


def _scoped_project(tmp_path: Path) -> Path:
    """A loadable project whose worktrees_image names one other host."""

    root = tmp_path / "project"
    (root / "worktrees" / "slots").mkdir(parents=True)
    (root / "live.py").write_text("# probe\n", encoding="utf-8")
    (root / ".wrkslots.yml").write_text(
        "schema: 2\n"
        "machine: donor-host\n"
        "worktrees_dir: worktrees/slots\n"
        "default_remote: origin\n"
        "default_landed_ref: refs/remotes/origin/main\n"
        "heartbeat_ttl_seconds: 60\n"
        "liveness_command: live.py\n"
        "layout: flat\n"
        "worktrees_image:\n"
        "  path: worktrees.img\n"
        "  size_bytes: 2147483648\n"
        "  machines: [image-host]\n",
        encoding="utf-8",
    )
    return root


def _forbid_mounts(monkeypatch: pytest.MonkeyPatch) -> None:
    def forbidden(*args: object, **kwargs: object) -> None:
        raise AssertionError("mount side effect attempted")

    monkeypatch.setattr(worktreesimage, "ensure_image_mounted", forbidden)
    monkeypatch.setattr(worktreesimage, "ensure_binds", forbidden)
    monkeypatch.setattr(worktreesimage, "current_state", forbidden)


def test_load_on_unnamed_host_has_no_mount_side_effects(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _scoped_project(tmp_path)
    _forbid_mounts(monkeypatch)
    config = cli._load_config(str(root), "donor-host")
    assert config.worktrees_image is None
    assert config.worktrees_image_state == "disabled"
    assert config.worktrees_image_skipped is not None
    assert "image-host" in config.worktrees_image_skipped
    # Exactly as absent: slot-writing commands are not refused.
    cli._require_worktrees_image_mounted(config, "create a slot")


def test_load_on_named_host_keeps_todays_behaviour(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _scoped_project(tmp_path)
    calls: list[str] = []

    def fake_mount(*args: object) -> None:
        calls.append("mount")

    def fake_binds(*args: object) -> list[str]:
        calls.append("binds")
        return []

    monkeypatch.setattr(worktreesimage, "ensure_image_mounted", fake_mount)
    monkeypatch.setattr(worktreesimage, "ensure_binds", fake_binds)
    monkeypatch.setattr(
        worktreesimage, "current_state", lambda *a: worktreesimage.MOUNTED
    )
    config = cli._load_config(str(root), "image-host")
    assert config.worktrees_image is not None
    assert config.worktrees_image_skipped is None
    assert calls == ["mount", "binds"]


def test_doctor_reports_a_named_host_row_plainly_from_an_unnamed_host(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    import argparse
    import json as json_module

    root = _scoped_project(tmp_path)
    _forbid_mounts(monkeypatch)
    config = cli._load_config(str(root), "donor-host")
    identity = cli.ProcessIdentity(
        pid=123, start_ticks=456, boot_id="boot", host_id="host", cgroup_path="/"
    )
    checkout = cli.Checkout(
        name="repo",
        path="worktrees/slots/s1",
        repository=".",
        branch="main",
        start_point="0" * 40,
        remote="origin",
        remote_url_sha256="0" * 64,
        landed_ref="refs/remotes/origin/main",
        head="0" * 40,
    )
    (config.worktrees / "s1").mkdir(parents=True, exist_ok=True)
    record = cli.ActiveRecord(
        slot="s1",
        agent="worker-a",
        task="t1",
        purpose="probe",
        slot_type="agent",
        machine="donor-host",
        generation=1,
        created_at="2026-11-05T00:00:00+00:00",
        heartbeat_at="2026-11-05T00:00:00+00:00",
        heartbeat_ttl_seconds=600,
        owner=None,
        coordinator_lease=identity,
        coordinator_recovery_note=None,
        handoff=None,
        checkouts=(checkout,),
        layout="flat",
    )
    cli._atomic_write_json(
        cli._active_path(config),
        cli._active_to_obj(
            cli.ActiveState(machine="donor-host", revision=0, slots=(record,))
        ),
    )
    cli._atomic_write_json(
        cli._archive_path(config),
        cli._archive_to_obj(cli.ArchiveState(machine="donor-host", revision=0, records=())),
    )
    # A bind record left by the named host: read, never acted on here.
    worktreesimage.save_binds(config.control, {"s1": "s1"})
    args = argparse.Namespace(
        project_root=str(root),
        machine="donor-host",
        wait_lock=0.0,
        all_machines=False,
        format="json",
    )
    assert cli._cmd_doctor(args) == 0
    report = json_module.loads(capsys.readouterr().out)
    assert report["worktrees_image"].startswith("skipped:")
    assert [row["slot"] for row in report["active"]] == ["s1"]


def _machine_project(tmp_path: Path, machine_line: str) -> Path:
    root = tmp_path / "project"
    (root / "worktrees" / "slots").mkdir(parents=True)
    (root / "live.py").write_text("# probe\n", encoding="utf-8")
    (root / ".wrkslots.yml").write_text(
        "schema: 2\n"
        f"{machine_line}"
        "worktrees_dir: worktrees/slots\n"
        "default_remote: origin\n"
        "default_landed_ref: refs/remotes/origin/main\n"
        "heartbeat_ttl_seconds: 60\n"
        "liveness_command: live.py\n"
        "layout: flat\n",
        encoding="utf-8",
    )
    return root


def test_machine_absent_and_auto_resolve_to_the_short_host_name(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(socket, "gethostname", lambda: "testhost.example.com")
    monkeypatch.delenv("WRKSLOTS_MACHINE", raising=False)
    absent = cli._load_config(str(_machine_project(tmp_path / "a", "")), None)
    assert absent.machine == "testhost"
    assert absent.machine_source == "host name (machine absent)"
    auto = cli._load_config(str(_machine_project(tmp_path / "b", "machine: auto\n")), None)
    assert auto.machine == "testhost"
    assert auto.machine_source == "host name (machine: auto)"


def test_machine_explicit_and_overrides_still_win(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(socket, "gethostname", lambda: "testhost")
    monkeypatch.delenv("WRKSLOTS_MACHINE", raising=False)
    explicit = cli._load_config(
        str(_machine_project(tmp_path / "a", "machine: node-a\n")), None
    )
    assert explicit.machine == "node-a"
    assert explicit.machine_source == "configuration.machine"
    monkeypatch.setenv("WRKSLOTS_MACHINE", "node-b")
    env = cli._load_config(str(_machine_project(tmp_path / "b", "machine: auto\n")), None)
    assert env.machine == "node-b"
    assert env.machine_source == "WRKSLOTS_MACHINE"
    override = cli._load_config(
        str(_machine_project(tmp_path / "c", "machine: node-a\n")), "node-c"
    )
    assert override.machine == "node-c"
    assert override.machine_source == "--machine"
    # An override also beats an absent key, with no host-name lookup needed.
    absent_override = cli._load_config(str(_machine_project(tmp_path / "d", "")), "node-d")
    assert absent_override.machine == "node-d"


def test_machine_source_never_breaks_the_mutation_coherence_reload(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # _mutation_locks reloads the configuration passing the resolved
    # machine back as an override and compares for equality; the
    # diagnostics-only source must not read as "configuration changed".
    monkeypatch.setattr(socket, "gethostname", lambda: "testhost")
    monkeypatch.delenv("WRKSLOTS_MACHINE", raising=False)
    for label, line in (("absent", ""), ("auto", "machine: auto\n"), ("explicit", "machine: testhost\n")):
        config = cli._load_config(str(_machine_project(tmp_path / label, line)), None)
        with cli._mutation_locks(config, 0.0):
            pass


def test_doctor_shows_the_host_name_source(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    import argparse
    import json as json_module

    monkeypatch.setattr(socket, "gethostname", lambda: "testhost")
    monkeypatch.delenv("WRKSLOTS_MACHINE", raising=False)
    root = _machine_project(tmp_path, "machine: auto\n")
    config = cli._load_config(str(root), None)
    cli._atomic_write_json(
        cli._active_path(config),
        cli._active_to_obj(cli.ActiveState(machine="testhost", revision=0, slots=())),
    )
    cli._atomic_write_json(
        cli._archive_path(config),
        cli._archive_to_obj(cli.ArchiveState(machine="testhost", revision=0, records=())),
    )
    args = argparse.Namespace(
        project_root=str(root), machine=None, wait_lock=0.0,
        all_machines=False, format="json",
    )
    assert cli._cmd_doctor(args) == 0
    report = json_module.loads(capsys.readouterr().out)
    assert report["machine"] == "testhost (host name (machine: auto))"


def _image_project(tmp_path: Path, mount_at_line: str = "") -> Path:
    """A loadable project whose worktrees_image applies on image-host."""

    root = tmp_path / "project"
    (root / "worktrees" / "slots").mkdir(parents=True)
    (root / "live.py").write_text("# probe\n", encoding="utf-8")
    (root / ".wrkslots.yml").write_text(
        "schema: 2\n"
        "machine: image-host\n"
        "worktrees_dir: worktrees/slots\n"
        "default_remote: origin\n"
        "default_landed_ref: refs/remotes/origin/main\n"
        "heartbeat_ttl_seconds: 60\n"
        "liveness_command: live.py\n"
        "layout: flat\n"
        "worktrees_image:\n"
        "  path: worktrees.img\n"
        "  size_bytes: 2147483648\n"
        f"{mount_at_line}"
        "  machines: [image-host]\n",
        encoding="utf-8",
    )
    return root


def _fail_the_mount(monkeypatch: pytest.MonkeyPatch) -> None:
    def fail(*args: object) -> None:
        raise worktreesimage.WorktreesImageError("mount: permission denied (fake)")

    monkeypatch.setattr(worktreesimage, "ensure_image_mounted", fail)
    monkeypatch.setattr(
        worktreesimage, "current_state", lambda *a: worktreesimage.NOT_MOUNTED
    )


@pytest.mark.parametrize(
    "mount_at_line", ["", "  mount_at: worktrees_dir\n"], ids=["binds", "direct"]
)
def test_failed_mount_warns_once_on_stderr_in_both_modes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
    mount_at_line: str,
) -> None:
    root = _image_project(tmp_path, mount_at_line)
    _fail_the_mount(monkeypatch)
    first = cli._load_config(str(root), "image-host")
    second = cli._load_config(str(root), "image-host")
    assert first.worktrees_image_state == worktreesimage.NOT_MOUNTED
    assert second.worktrees_image_state == worktreesimage.NOT_MOUNTED
    captured = capsys.readouterr()
    assert captured.out == ""
    assert captured.err.count("worktrees image not mounted") == 1
    assert "permission denied (fake)" in captured.err


def test_warning_leaves_json_stdout_parseable(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    import argparse
    import json as json_module

    root = _image_project(tmp_path)
    _fail_the_mount(monkeypatch)
    config = cli._load_config(str(root), "image-host")
    cli._atomic_write_json(
        cli._active_path(config),
        cli._active_to_obj(cli.ActiveState(machine="image-host", revision=0, slots=())),
    )
    cli._atomic_write_json(
        cli._archive_path(config),
        cli._archive_to_obj(cli.ArchiveState(machine="image-host", revision=0, records=())),
    )
    capsys.readouterr()  # the load above already warned, once
    args = argparse.Namespace(
        project_root=str(root), machine="image-host", wait_lock=0.0,
        all_machines=False, format="json",
    )
    assert cli._cmd_doctor(args) == 0
    captured = capsys.readouterr()
    report = json_module.loads(captured.out)
    assert report["worktrees_image"] == "applies: state=not-mounted"
    assert "worktrees image not mounted" not in captured.err  # once per process


def test_image_status_reports_the_worktrees_image_when_configured(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    import argparse
    import json as json_module

    from wrkslots import imagecmd

    root = _image_project(tmp_path, "  mount_at: worktrees_dir\n")
    _fail_the_mount(monkeypatch)
    human = argparse.Namespace(
        project_root=str(root), machine="image-host", format="human"
    )
    assert imagecmd._cmd_image_status(human) == 0
    out = capsys.readouterr().out
    assert "worktrees image: not-mounted at " in out
    assert "(mount_at=worktrees_dir)" in out
    as_json = argparse.Namespace(
        project_root=str(root), machine="image-host", format="json"
    )
    assert imagecmd._cmd_image_status(as_json) == 0
    payload = json_module.loads(capsys.readouterr().out)
    assert payload["worktrees_image"]["state"] == "not-mounted"
    assert payload["worktrees_image"]["mount_at"] == "worktrees_dir"


def test_signature_catches_content_mode_and_link_differences(tmp_path: Path) -> None:
    left, right = tmp_path / "left", tmp_path / "right"
    for root, text in ((left, "aaaa"), (right, "bbbb")):  # same size
        root.mkdir()
        (root / "f.txt").write_text(text, encoding="utf-8")
    assert worktreesimage.listing_signature(left) != worktreesimage.listing_signature(right)
    (right / "f.txt").write_text("aaaa", encoding="utf-8")
    assert worktreesimage.listing_signature(left) == worktreesimage.listing_signature(right)
    os.chmod(right / "f.txt", 0o600)
    assert worktreesimage.listing_signature(left) != worktreesimage.listing_signature(right)
    os.chmod(right / "f.txt", 0o644)
    os.symlink("f.txt", left / "link")
    os.symlink("other", right / "link")
    assert worktreesimage.listing_signature(left) != worktreesimage.listing_signature(right)


def test_git_signature_catches_head_and_status_differences(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    if shutil.which("git") is None:
        pytest.skip("needs git")
    for key in ("GIT_AUTHOR_NAME", "GIT_COMMITTER_NAME"):
        monkeypatch.setenv(key, "sig-test")
    for key in ("GIT_AUTHOR_EMAIL", "GIT_COMMITTER_EMAIL"):
        monkeypatch.setenv(key, "sig-test@example.invalid")
    repo = tmp_path / "repo"
    repo.mkdir()
    subprocess.run(["git", "init", "-q", "-b", "main"], cwd=repo, check=True)
    (repo / "f.txt").write_text("one\n", encoding="utf-8")
    subprocess.run(["git", "add", "."], cwd=repo, check=True)
    subprocess.run(["git", "commit", "-q", "-m", "one"], cwd=repo, check=True)
    slot = tmp_path / "slot"
    subprocess.run(
        ["git", "worktree", "add", str(slot), "-b", "b1"],
        cwd=repo, check=True, capture_output=True,
    )
    copied = tmp_path / "copied"
    shutil.copytree(slot, copied, symlinks=True)
    assert worktreesimage.git_state_signature(slot) == worktreesimage.git_state_signature(copied)
    (copied / "f.txt").write_text("changed\n", encoding="utf-8")
    assert worktreesimage.git_state_signature(slot) != worktreesimage.git_state_signature(copied)


class _FakeBtrfs:
    """Fake privileged commands over real directories (create/mount/delete)."""

    def __init__(self) -> None:
        self.calls: list[list[str]] = []

    def run(
        self, argv: list[str], *, sudo: bool = False
    ) -> subprocess.CompletedProcess[str]:
        self.calls.append(list(argv))
        if argv[:3] == ["btrfs", "subvolume", "create"]:
            Path(argv[3]).mkdir(parents=True, exist_ok=True)
        elif argv[:3] == ["btrfs", "subvolume", "delete"]:
            shutil.rmtree(argv[3])
        return subprocess.CompletedProcess(argv, 0, "", "")


def test_remove_refuses_before_deleting_when_content_sits_beneath(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    config = _config(tmp_path, worktreesimage.MOUNTED)
    control = config.control
    mount_dir = worktreesimage.image_mount_dir(control)
    (mount_dir / "s1").mkdir(parents=True)
    slot_path = config.worktrees / "s1"
    slot_path.mkdir(parents=True)
    (slot_path / "stale-original.txt").write_text("kept\n", encoding="utf-8")
    worktreesimage.save_bind_records(
        control,
        {"s1": worktreesimage.BindRecord(subvolume="s1", target=str(slot_path))},
    )
    fake = _FakeBtrfs()
    monkeypatch.setattr(worktreesimage, "_run", fake.run)
    monkeypatch.setattr(worktreesimage, "mount_id_at", lambda point: 9)
    with pytest.raises(OSError, match="beneath the worktrees-image bind"):
        cli._remove_slot_directory(config, slot_path)
    # Nothing deleted, record intact, bind re-established.
    assert (mount_dir / "s1").is_dir()
    assert worktreesimage.load_binds(control) == {"s1": "s1"}
    assert (slot_path / "stale-original.txt").is_file()
    assert fake.calls == [
        ["mount", "--make-private", str(slot_path)],
        ["umount", str(slot_path)],
        ["mount", "--bind", str(mount_dir / "s1"), str(slot_path)],
        ["mount", "--make-private", str(slot_path)],
    ]


def test_binds_migrate_reclaims_the_original(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    import argparse

    from wrkslots import imagecmd

    root = _image_project(tmp_path)
    fake = _FakeBtrfs()
    monkeypatch.setattr(worktreesimage, "_run", fake.run)
    monkeypatch.setattr(worktreesimage, "mount_id_at", lambda point: 9)
    monkeypatch.setattr(worktreesimage, "ensure_image_mounted", lambda *a: None)
    monkeypatch.setattr(worktreesimage, "ensure_binds", lambda *a: [])
    monkeypatch.setattr(
        worktreesimage, "current_state", lambda *a: worktreesimage.MOUNTED
    )
    monkeypatch.setattr(imagecmd, "_lock_slot_checkouts", lambda *a: None)
    config = cli._load_config(str(root), "image-host")
    _register_slot(config)
    slot_path = config.worktrees / "s1"
    slot_path.mkdir(parents=True, exist_ok=True)
    (slot_path / "work.txt").write_text("work\n", encoding="utf-8")
    args = argparse.Namespace(
        project_root=str(root), machine="image-host", slot="s1",
        slot_type="agent", wait_lock=0.0,
    )
    assert imagecmd._cmd_migrate(args) == 0
    assert "(original reclaimed)" in capsys.readouterr().out
    mount_dir = worktreesimage.image_mount_dir(config.control)
    assert (mount_dir / "s1" / "work.txt").read_text(encoding="utf-8") == "work\n"
    assert list(config.worktrees.glob(".s1.premigrate-*")) == []
    assert worktreesimage.load_binds(config.control) == {"s1": "s1"}


def _register_slot(config: cli.Config, owner: cli.ProcessIdentity | None = None) -> cli.ActiveRecord:
    identity = cli.ProcessIdentity(
        pid=123, start_ticks=456, boot_id="boot", host_id="host", cgroup_path="/"
    )
    checkout = cli.Checkout(
        name="repo", path="worktrees/slots/s1", repository=".", branch="main",
        start_point="0" * 40, remote="origin", remote_url_sha256="0" * 64,
        landed_ref="refs/remotes/origin/main", head="0" * 40,
    )
    record = cli.ActiveRecord(
        slot="s1", agent="worker-a", task="t1", purpose="probe",
        slot_type="agent", machine=config.machine, generation=1,
        created_at="2026-11-05T00:00:00+00:00",
        heartbeat_at="2026-11-05T00:00:00+00:00", heartbeat_ttl_seconds=600,
        owner=owner, coordinator_lease=identity, coordinator_recovery_note=None,
        handoff=None, checkouts=(checkout,), layout="flat",
    )
    cli._atomic_write_json(
        cli._active_path(config),
        cli._active_to_obj(
            cli.ActiveState(machine=config.machine, revision=0, slots=(record,))
        ),
    )
    cli._atomic_write_json(
        cli._archive_path(config),
        cli._archive_to_obj(cli.ArchiveState(machine=config.machine, revision=0, records=())),
    )
    return record


def _migrate_fakes(monkeypatch: pytest.MonkeyPatch) -> None:
    from wrkslots import imagecmd

    monkeypatch.setattr(worktreesimage, "_run", _FakeBtrfs().run)
    monkeypatch.setattr(worktreesimage, "mount_id_at", lambda point: 9)
    monkeypatch.setattr(worktreesimage, "ensure_image_mounted", lambda *a: None)
    monkeypatch.setattr(worktreesimage, "ensure_binds", lambda *a: [])
    monkeypatch.setattr(worktreesimage, "current_state", lambda *a: worktreesimage.MOUNTED)
    monkeypatch.setattr(imagecmd, "_lock_slot_checkouts", lambda *a: None)


def test_migrate_refuses_an_unregistered_slot(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import argparse

    from wrkslots import imagecmd

    root = _image_project(tmp_path)
    _migrate_fakes(monkeypatch)
    config = cli._load_config(str(root), "image-host")
    cli._atomic_write_json(
        cli._active_path(config),
        cli._active_to_obj(cli.ActiveState(machine="image-host", revision=0, slots=())),
    )
    cli._atomic_write_json(
        cli._archive_path(config),
        cli._archive_to_obj(cli.ArchiveState(machine="image-host", revision=0, records=())),
    )
    (config.worktrees / "s1").mkdir(parents=True, exist_ok=True)
    args = argparse.Namespace(
        project_root=str(root), machine="image-host", slot="s1",
        slot_type="agent", wait_lock=0.0,
    )
    with pytest.raises(cli.Refusal, match="not registered.*registry row"):
        imagecmd._cmd_migrate(args)


def test_migrate_refuses_held_and_live_owner_slots(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from wrkslots import imagecmd

    root = _image_project(tmp_path)
    _migrate_fakes(monkeypatch)
    config = cli._load_config(str(root), "image-host")
    record = _register_slot(config)
    slot_path = config.worktrees / "s1"
    slot_path.mkdir(parents=True, exist_ok=True)
    key = (record.machine, record.slot)
    monkeypatch.setattr(
        cli, "_load_record_holds",
        lambda cfg, states: {key: cli._HoldResult(value={"reason": "keep"})},
    )
    with pytest.raises(cli.Refusal, match="unhold it before migrate"):
        imagecmd._assert_migratable(config, "s1", "agent", slot_path)
    monkeypatch.setattr(cli, "_load_record_holds", lambda cfg, states: {key: cli._HoldResult(None)})
    monkeypatch.setattr(
        cli, "_owner_liveness_states",
        lambda cfg, records: {(record.machine, record.slot, record.generation): ("alive", "pid 1")},
    )
    with pytest.raises(cli.Refusal, match="not proven dead"):
        imagecmd._assert_migratable(config, "s1", "agent", slot_path)


def test_create_image_file_is_owner_only(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(worktreesimage, "_run", _FakeBtrfs().run)
    target = tmp_path / "worktrees.img"
    worktreesimage.create_image(target, 2 * GIB)
    assert oct(target.stat().st_mode & 0o777) == "0o600"
    assert target.stat().st_size == 2 * GIB


def test_create_before_enabling_needs_no_section(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    import argparse

    from wrkslots import imagecmd

    root = _machine_project(tmp_path, "machine: node-a\n")  # no worktrees_image
    monkeypatch.setattr(worktreesimage, "_run", _FakeBtrfs().run)
    args = argparse.Namespace(
        project_root=str(root), machine=None,
        path="pre.img", size_bytes=2 * GIB,
    )
    assert imagecmd._cmd_wt_image_create(args) == 0
    assert "created" in capsys.readouterr().out
    assert (root / "pre.img").is_file()
    lone = argparse.Namespace(
        project_root=str(root), machine=None, path="pre2.img", size_bytes=None
    )
    with pytest.raises(cli.Refusal, match="must be given together"):
        imagecmd._cmd_wt_image_create(lone)


def test_migrate_refuses_a_slot_used_only_through_an_open_fd(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import sys

    from wrkslots import imagecmd

    root = _image_project(tmp_path)
    _migrate_fakes(monkeypatch)
    config = cli._load_config(str(root), "image-host")
    record = _register_slot(config)
    slot_path = config.worktrees / "s1"
    slot_path.mkdir(parents=True, exist_ok=True)
    held_file = slot_path / "held.bin"
    held_file.write_bytes(b"x")
    key = (record.machine, record.slot)
    monkeypatch.setattr(cli, "_load_record_holds", lambda cfg, states: {key: cli._HoldResult(None)})
    monkeypatch.setattr(
        cli, "_owner_liveness_states",
        lambda cfg, records: {(record.machine, record.slot, record.generation): ("dead", "gone")},
    )
    child = subprocess.Popen(
        [sys.executable, "-c", "import time,sys; f=open(sys.argv[1],'rb'); time.sleep(3600)", str(held_file)],
        cwd="/tmp",
    )
    try:
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            fds = list(Path(f"/proc/{child.pid}/fd").glob("*"))

            def _held(fd: Path) -> bool:
                # The child may close a startup descriptor between the
                # listing and the read; that fd is simply not the held one.
                with contextlib.suppress(OSError):
                    return os.readlink(fd) == str(held_file)
                return False

            if any(_held(fd) for fd in fds):
                break
            time.sleep(0.05)
        with pytest.raises(cli.Refusal, match="in use"):
            imagecmd._assert_migratable(config, "s1", "agent", slot_path)
    finally:
        child.terminate()
        child.wait(timeout=10)


# --- #1162 worktrees-image-at-slots-dir: direct layout, quota modes, locks ---


def test_settings_mount_at_and_quota_mode() -> None:
    defaults = worktreesimage.settings_from_obj(
        {"path": "worktrees.img", "size_bytes": 2 * GIB}
    )
    assert defaults.mount_at == "binds" and not defaults.direct
    assert defaults.quota_mode == "simple"
    direct = worktreesimage.settings_from_obj(
        {
            "path": "worktrees.img",
            "size_bytes": 2 * GIB,
            "mount_at": "worktrees_dir",
            "quota_mode": "full",
        }
    )
    assert direct.direct and direct.quota_mode == "full"
    assert direct.mount_point(Path("/c"), Path("/c/slots")) == Path("/c/slots")
    assert defaults.mount_point(Path("/c"), Path("/c/slots")) == Path("/c/.slots-image")


@pytest.mark.parametrize(
    "extra",
    [{"mount_at": "elsewhere"}, {"mount_at": 3}, {"quota_mode": "none"}, {"quota_mode": ""}],
)
def test_settings_refuse_bad_mount_at_and_quota_mode(extra: dict[str, object]) -> None:
    with pytest.raises(worktreesimage.WorktreesImageError):
        worktreesimage.settings_from_obj(
            {"path": "worktrees.img", "size_bytes": 2 * GIB, **extra}
        )


class _FakeQuotaHost:
    """btrfs quota commands in miniature, with a switchable mode."""

    def __init__(
        self, *, enabled: bool = False, simple: bool = False, simple_supported: bool = True
    ) -> None:
        self.calls: list[list[str]] = []
        self.enabled = enabled
        self.simple = simple
        self.simple_supported = simple_supported

    def run(
        self, argv: list[str], *, sudo: bool = False
    ) -> subprocess.CompletedProcess[str]:
        self.calls.append(list(argv))
        if argv[:3] == ["btrfs", "qgroup", "show"]:
            if not self.enabled:
                raise worktreesimage.WorktreesImageError("quotas not enabled")
            return subprocess.CompletedProcess(argv, 0, "0/5 0 0\n", "")
        if argv[:3] == ["btrfs", "quota", "enable"]:
            if "--simple" in argv:
                if not self.simple_supported:
                    raise worktreesimage.WorktreesImageError("unknown flag --simple")
                self.simple = True
            self.enabled = True
            return subprocess.CompletedProcess(argv, 0, "", "")
        if argv[:3] == ["btrfs", "inspect-internal", "dump-super"]:
            flags = "incompat_flags\t0x1\n\t\t\t  SIMPLE_QUOTA )\n" if self.simple else "incompat_flags\t0x1\n"
            return subprocess.CompletedProcess(argv, 0, flags, "")
        raise AssertionError(f"unexpected command: {argv}")


def test_quota_mode_apply_and_report(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    mount = tmp_path / "mnt"
    image = tmp_path / "wt.img"

    fresh = _FakeQuotaHost()
    monkeypatch.setattr(worktreesimage, "_run", fresh.run)
    assert worktreesimage.quota_mode_at(mount, image) == "none"
    worktreesimage.apply_quota_mode(mount, image, "simple")
    assert ["btrfs", "quota", "enable", "--simple", str(mount)] in fresh.calls
    assert worktreesimage.quota_mode_at(mount, image) == "simple"
    # Already accounting: only the plain idempotent enable, never a flip.
    calls_before = len(fresh.calls)
    worktreesimage.apply_quota_mode(mount, image, "full")
    new_enables = [
        call for call in fresh.calls[calls_before:] if call[:3] == ["btrfs", "quota", "enable"]
    ]
    assert new_enables == [["btrfs", "quota", "enable", str(mount)]]
    assert worktreesimage.quota_mode_at(mount, image) == "simple"

    unsupported = _FakeQuotaHost(simple_supported=False)
    monkeypatch.setattr(worktreesimage, "_run", unsupported.run)
    worktreesimage.apply_quota_mode(mount, image, "simple")
    assert worktreesimage.quota_mode_at(mount, image) == "full"

    full = _FakeQuotaHost(enabled=True)
    monkeypatch.setattr(worktreesimage, "_run", full.run)
    assert worktreesimage.quota_mode_at(mount, image) == "full"


def _direct_config(tmp_path: Path, state: str) -> cli.Config:
    config = _config(tmp_path, state)
    object.__setattr__(
        config,
        "worktrees_image",
        worktreesimage.WorktreesImageSettings(
            relative_path="worktrees.img",
            size_bytes=2 * GIB,
            mount_at="worktrees_dir",
            slot_quota_bytes=64 * 1024 * 1024,
        ),
    )
    return config


def test_direct_ensure_binds_reads_and_writes_no_records(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    control = tmp_path / "worktrees"
    control.mkdir(parents=True)
    worktreesimage.save_binds(control, {"s1": "s1"})
    before = (control / worktreesimage.RECORD_NAME).read_bytes()
    fake = _FakeMounts()
    monkeypatch.setattr(worktreesimage, "_run", fake.run)
    settings = worktreesimage.WorktreesImageSettings(
        relative_path="wt.img", size_bytes=2 * GIB, mount_at="worktrees_dir"
    )
    actions = worktreesimage.ensure_binds(
        control, worktreesimage.image_mount_dir(control), tmp_path / "slots", settings
    )
    assert actions == []
    assert fake.calls == []
    assert (control / worktreesimage.RECORD_NAME).read_bytes() == before


def test_direct_state_and_status_classify_the_slots_directory(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    config = _direct_config(tmp_path, worktreesimage.MOUNTED)
    assert config.worktrees_image is not None
    config.worktrees.mkdir(parents=True)
    (config.worktrees / "s1").mkdir()
    monkeypatch.setattr(
        slotimage, "mount_table", lambda: [_entry(config.worktrees, "btrfs")]
    )
    assert (
        worktreesimage.current_state(config.root, config.worktrees, config.worktrees_image)
        == worktreesimage.MOUNTED
    )
    fake = _FakeQuotaHost(enabled=True, simple=True)
    monkeypatch.setattr(worktreesimage, "_run", fake.run)
    report = worktreesimage.status_report(
        config.root, config.worktrees, config.worktrees_image
    )
    assert report["mount_point"] == str(config.worktrees)
    assert report["mount_at"] == "worktrees_dir"
    assert report["quota_mode"] == "simple"
    assert report["configured_quota_mode"] == "simple"
    assert report["binds"] == {}
    assert cli._validate_slots_directory(config) == (
        tmp_path / "worktrees" / "validate"
    )
    assert report["validate_dir"] == str(tmp_path / "worktrees" / "validate")
    with pytest.raises(cli.Refusal, match="reserved"):
        cli._slot_directory(config, "validate", "agent")
    assert cli._slot_directory(config, "v1", "validate") == (
        tmp_path / "worktrees" / "validate" / "v1"
    )
    assert worktreesimage.validate_external_dir(config.worktrees) == (
        tmp_path / "worktrees" / "validate"
    )
    assert worktreesimage.validate_image_dir(config.worktrees) == (
        config.worktrees / "validate"
    )


def _checkout(path: str) -> cli.Checkout:
    return cli.Checkout(
        name="repo",
        path=path,
        repository=".",
        branch="b1",
        start_point="0" * 40,
        remote="origin",
        remote_url_sha256="0" * 64,
        landed_ref="refs/remotes/origin/main",
        head="0" * 40,
    )


def test_historical_validate_paths_survive_direct_config(tmp_path: Path) -> None:
    # #1183: archived/replayed validate rows at either validate root must
    # not brick a direct-layout registry; active rows stay strict.
    import dataclasses as _dc

    config = _direct_config(tmp_path, worktreesimage.MOUNTED)
    for path in ("worktrees/validate/v1", "worktrees/slots/validate/v1"):
        record = _dc.replace(
            _record("v1", "validate"), checkouts=(_checkout(path),), layout="flat"
        )
        cli._assert_record_paths(config, record, require_repository=False, historical=True)
    active_old = _dc.replace(
        _record("v1", "validate"),
        checkouts=(_checkout("worktrees/slots/validate/v1"),),
        layout="flat",
    )
    with pytest.raises(cli.StateError, match="does not match its slot"):
        cli._assert_record_paths(config, active_old, require_repository=False)
    cli._assert_record_paths(
        config,
        _dc.replace(
            _record("v1", "validate"),
            checkouts=(_checkout("worktrees/validate/v1"),),
            layout="flat",
        ),
        require_repository=False,
    )
    bogus = _dc.replace(
        _record("v1", "validate"),
        checkouts=(_checkout("worktrees/elsewhere/v1"),),
        layout="flat",
    )
    with pytest.raises(cli.StateError):
        cli._assert_record_paths(config, bogus, require_repository=False, historical=True)


def test_archive_validate_paths_accept_either_root(tmp_path: Path) -> None:
    config = _direct_config(tmp_path, worktreesimage.MOUNTED)

    def archive(path: str) -> dict[str, object]:
        finished = "2026-11-05T01:00:00+00:00"
        return {
            "schema": cli.SCHEMA,
            "machine": "testhost",
            "revision": 1,
            "records": [
                {
                    "archive_id": f"testhost:v1:1:{finished}",
                    "slot": "v1",
                    "agent": "worker-a",
                    "task": "t1",
                    "purpose": "probe",
                    "machine": "testhost",
                    "generation": 1,
                    "created_at": "2026-11-05T00:00:00+00:00",
                    "finished_at": finished,
                    "mode": "remove",
                    "actor": "coordinator",
                    "physical_storage": "removed",
                    "validation": ["ok"],
                    "limitations": [],
                    "continuation": "done",
                    "slot_type": "validate",
                    "layout": "flat",
                    "checkouts": [
                        {
                            "name": "repo",
                            "path": path,
                            "repository": ".",
                            "branch": "b1",
                            "start_point": "0" * 40,
                            "remote": "origin",
                            "remote_url_sha256": "0" * 64,
                            "landed_ref": "refs/remotes/origin/main",
                            "head": "0" * 40,
                            "containing_remote_refs": [],
                            "vcs": "git",
                        }
                    ],
                }
            ],
        }

    for path in ("worktrees/validate/v1", "worktrees/slots/validate/v1"):
        state = cli._archive_from_obj(config, archive(path), "testhost", "archive state")
        assert state.records[0]["slot"] == "v1"
    with pytest.raises(cli.StateError, match="escaped its archived slot"):
        cli._archive_from_obj(
            config, archive("worktrees/elsewhere/v1"), "testhost", "archive state"
        )


def test_migrate_preflight_refuses_before_switching(tmp_path: Path) -> None:
    from wrkslots import imagecmd
    import dataclasses as _dc

    config = _config(tmp_path, worktreesimage.MOUNTED)
    assert config.worktrees_image is not None
    bad = _dc.replace(
        _record("validate", "agent"),
        checkouts=(_checkout("worktrees/slots/validate"),),
        layout="flat",
    )
    with pytest.raises(cli.Refusal, match="would leave slot validate unusable"):
        imagecmd._assert_target_layout_paths(config, config.worktrees_image, [bad], True)
    good = _dc.replace(
        _record("s1", "agent"), checkouts=(_checkout("worktrees/slots/s1"),), layout="flat"
    )
    imagecmd._assert_target_layout_paths(config, config.worktrees_image, [good], True)
    imagecmd._assert_target_layout_paths(config, config.worktrees_image, [good], False)


def test_validate_bind_ensure_and_unmount_use_one_bind(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    slots = tmp_path / "worktrees" / "slots"
    slots.mkdir(parents=True)
    fake = _FakeMounts()
    mounted: list[Path] = []
    monkeypatch.setattr(worktreesimage, "_run", fake.run)
    monkeypatch.setattr(
        slotimage, "mount_table", lambda: [slotimage.MountEntry(mount_point=p, fstype="btrfs", source="/dev/loop0") for p in mounted]
    )
    orig_run = fake.run

    def run(argv: list[str], *, sudo: bool = False) -> object:
        if argv[:2] == ["mount", "--bind"]:
            mounted.append(Path(argv[-1]))
        if argv[:1] == ["umount"]:
            point = Path(argv[-1])
            if point in mounted:
                mounted.remove(point)
        return orig_run(argv, sudo=sudo)

    monkeypatch.setattr(worktreesimage, "_run", run)
    worktreesimage.ensure_validate_bind(slots)
    assert (slots / "validate").is_dir()
    assert mounted == [tmp_path / "worktrees" / "validate"]
    worktreesimage.ensure_validate_bind(slots)
    assert mounted == [tmp_path / "worktrees" / "validate"]
    worktreesimage.unmount_validate_bind(slots)
    assert mounted == []
    assert [c for c in fake.calls if c[:2] == ["mount", "--bind"]] == [
        ["mount", "--bind", str(slots / "validate"), str(tmp_path / "worktrees" / "validate")]
    ]


def test_direct_fence_renames_plainly_and_remove_deletes_the_subvolume(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    config = _direct_config(tmp_path, worktreesimage.MOUNTED)
    slot_path = config.worktrees / "s1"
    slot_path.mkdir(parents=True)
    fake = _FakeMounts()
    monkeypatch.setattr(worktreesimage, "_run", fake.run)
    monkeypatch.setattr(worktreesimage, "is_subvolume", lambda path: path.is_dir())
    monkeypatch.setattr(cli, "_DIRECT_WORKTREES_IMAGE_ROOT", None)
    cli._refresh_worktrees_bind_mounts(config)
    assert cli._DIRECT_WORKTREES_IMAGE_ROOT == config.worktrees.absolute()
    assert cli._is_owned_direct_subvolume_root(slot_path)
    assert cli._worktrees_bind_slot(config, slot_path) is None

    fenced = config.worktrees / (".s1.fenced.1." + "e" * 32)
    cli._rename_slot_path(config, slot_path, fenced)
    assert fenced.is_dir() and not slot_path.exists()
    assert fake.calls == []  # no mount dance: a subvolume renames like a directory

    cli._remove_slot_directory(config, fenced)
    assert not fenced.exists()
    assert fake.calls == [["btrfs", "subvolume", "delete", str(fenced)]]
    assert not (config.control / worktreesimage.RECORD_NAME).exists()


def _git_repo_with_worktrees(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    monkeypatch.setenv("GIT_AUTHOR_NAME", "lock-test")
    monkeypatch.setenv("GIT_AUTHOR_EMAIL", "lock-test@example.invalid")
    monkeypatch.setenv("GIT_COMMITTER_NAME", "lock-test")
    monkeypatch.setenv("GIT_COMMITTER_EMAIL", "lock-test@example.invalid")
    repository = tmp_path / "repo"
    repository.mkdir()
    subprocess.run(["git", "init", "-q", "-b", "main"], cwd=repository, check=True)
    subprocess.run(
        ["git", "commit", "-q", "--allow-empty", "-m", "init"], cwd=repository, check=True
    )
    return repository


def test_image_lock_survives_prune_and_only_its_own_reason_unlocks(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    if shutil.which("git") is None:
        pytest.skip("needs git")
    repository = _git_repo_with_worktrees(tmp_path, monkeypatch)
    vcs = cli._GitVcs()
    locked = tmp_path / "slots" / "locked-one"
    plain = tmp_path / "slots" / "plain-one"
    for path, branch in ((locked, "b-locked"), (plain, "b-plain")):
        path.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(
            ["git", "worktree", "add", str(path), "-b", branch],
            cwd=repository, check=True, capture_output=True, text=True,
        )
    vcs.lock_worktree(repository, locked)
    assert vcs.worktree_lock_state(repository, locked) == (
        True,
        cli.WORKTREES_IMAGE_LOCK_REASON,
    )
    assert vcs.worktree_lock_state(repository, plain) == (False, None)
    # Simulate the unmounted image: both paths vanish, then a prune runs
    # with immediate expiry (the form that would detach a missing slot).
    away = tmp_path / "away"
    away.mkdir()
    locked.rename(away / "locked-one")
    plain.rename(away / "plain-one")
    subprocess.run(
        ["git", "worktree", "prune", "--expire", "now"], cwd=repository, check=True,
        capture_output=True, text=True,
    )
    listed = vcs.listed_worktrees(repository)
    assert locked.absolute() in listed  # locked survives
    assert plain.absolute() not in listed  # unlocked was pruned away
    # Bring the locked tree back and remove it: the wrapper drops only
    # wrkslots' own lock, so a plain remove (no force at all) succeeds.
    (away / "locked-one").rename(locked)
    vcs.remove_worktree(repository, locked)
    assert locked.absolute() not in vcs.listed_worktrees(repository)


def test_foreign_lock_still_refuses_remove(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    if shutil.which("git") is None:
        pytest.skip("needs git")
    repository = _git_repo_with_worktrees(tmp_path, monkeypatch)
    vcs = cli._GitVcs()
    held = tmp_path / "held"
    subprocess.run(
        ["git", "worktree", "add", str(held), "-b", "b-held"],
        cwd=repository, check=True, capture_output=True, text=True,
    )
    subprocess.run(
        ["git", "worktree", "lock", "--reason", "operator hold", str(held)],
        cwd=repository, check=True, capture_output=True, text=True,
    )
    assert vcs.worktree_lock_state(repository, held) == (True, "operator hold")
    vcs.drop_own_worktree_lock(repository, held)
    assert vcs.worktree_lock_state(repository, held) == (True, "operator hold")
    with pytest.raises(cli.Refusal, match="locked"):
        vcs.remove_worktree(repository, held, force=True)
    assert held.absolute() in vcs.listed_worktrees(repository)
    # Taking wrkslots' lock over a foreign one also refuses.
    with pytest.raises(cli.Refusal, match="already locked"):
        vcs.lock_worktree(repository, held)


def _record(slot: str = "s1", slot_type: str = "agent") -> cli.ActiveRecord:
    return cli.ActiveRecord(
        slot=slot,
        agent="worker-a",
        task="t1",
        purpose="probe",
        slot_type=slot_type,
        machine="testhost",
        generation=1,
        created_at="2026-11-05T00:00:00+00:00",
        heartbeat_at="2026-11-05T00:00:00+00:00",
        heartbeat_ttl_seconds=600,
        owner=None,
        coordinator_lease=cli.ProcessIdentity(
            pid=123, start_ticks=456, boot_id="boot", host_id="host", cgroup_path="/"
        ),
        coordinator_recovery_note=None,
        handoff=None,
        checkouts=(),
        layout="flat",
    )


def test_layout_census_refuses_a_live_holder_and_validation_slots(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from wrkslots import imagecmd

    config = _direct_config(tmp_path, worktreesimage.MOUNTED)
    slot_path = config.worktrees / "s1"
    slot_path.mkdir(parents=True)
    record = _record()
    state = cli.ActiveState(machine="testhost", revision=0, slots=(record,))
    monkeypatch.setattr(
        cli,
        "_load_record_holds",
        lambda cfg, states: {("testhost", "s1"): cli._HoldResult(None)},
    )
    monkeypatch.setattr(
        cli,
        "_owner_liveness_states",
        lambda cfg, records: {
            (r.machine, r.slot, r.generation): ("dead", "probe") for r in records
        },
    )
    assert imagecmd._layout_census(config, state) == [record]

    holder = subprocess.Popen(["sleep", "3600"], cwd=slot_path)
    try:
        with pytest.raises(cli.Refusal):
            imagecmd._layout_census(config, state)
    finally:
        holder.terminate()
        holder.wait(timeout=10)

    validate_state = cli.ActiveState(
        machine="testhost", revision=0, slots=(record, _record("v1", "validate"))
    )
    with pytest.raises(cli.Refusal, match="validation slots exist"):
        imagecmd._layout_census(config, validate_state)


class _FakeLayoutHost:
    """Mounts in miniature: the image top level moves between mount points."""

    def __init__(self, mount_dir: Path, slots_dir: Path) -> None:
        self.calls: list[list[str]] = []
        self.mount_dir = mount_dir
        self.slots_dir = slots_dir

    def run(
        self, argv: list[str], *, sudo: bool = False
    ) -> subprocess.CompletedProcess[str]:
        self.calls.append(list(argv))
        ok = subprocess.CompletedProcess(argv, 0, "", "")
        if argv[:1] == ["mount"] and "--bind" not in argv:
            point = Path(argv[-1])
            if point == self.slots_dir:
                for entry in list(self.mount_dir.iterdir()):
                    entry.rename(self.slots_dir / entry.name)
            return ok
        if argv[:1] in (["mount"], ["chown"]):
            return ok
        if argv[:1] == ["umount"]:
            point = Path(argv[-1])
            if point == self.slots_dir and self.slots_dir.is_dir():
                for entry in list(self.slots_dir.iterdir()):
                    if entry.name != worktreesimage.VALIDATE_SUBDIR:
                        entry.rename(self.mount_dir / entry.name)
            return ok
        if argv[:3] == ["btrfs", "qgroup", "show"]:
            return subprocess.CompletedProcess(argv, 0, "0/5 0 0\n", "")
        if argv[:2] in (["btrfs", "quota"], ["btrfs", "qgroup"]):
            return ok
        if argv[:3] == ["btrfs", "inspect-internal", "dump-super"]:
            return subprocess.CompletedProcess(
                argv, 0, "incompat_flags\t0x1\n\t\t\t  SIMPLE_QUOTA )\n", ""
            )
        if argv[:3] == ["btrfs", "subvolume", "create"]:
            Path(argv[3]).mkdir(parents=True, exist_ok=False)
            return ok
        if argv[:3] == ["btrfs", "subvolume", "delete"]:
            shutil.rmtree(argv[3])
            return ok
        raise AssertionError(f"unexpected command: {argv}")


def _layout_project(tmp_path: Path) -> cli.Config:
    config = _config(tmp_path, worktreesimage.MOUNTED)
    config.config_path.write_text(
        "schema: 2\n"
        "machine: testhost\n"
        "worktrees_dir: worktrees/slots\n"
        "default_remote: origin\n"
        "default_landed_ref: refs/remotes/origin/main\n"
        "heartbeat_ttl_seconds: 60\n"
        "liveness_command: live.py\n"
        "layout: flat\n"
        "worktrees_image:\n"
        "  path: worktrees.img\n"
        "  size_bytes: 2147483648\n",
        encoding="utf-8",
    )
    return config


def test_migrate_layout_round_trip_with_fake_mounts(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from wrkslots import imagecmd

    config = _layout_project(tmp_path)
    assert config.worktrees_image is not None
    config.worktrees_image.image_file(config.root).touch()
    control = config.control
    mount_dir = worktreesimage.image_mount_dir(control)
    (mount_dir / "s1").mkdir(parents=True)
    (mount_dir / "s1" / "keep.txt").write_text("image copy\n", encoding="utf-8")
    slot_path = config.worktrees / "s1"
    slot_path.mkdir(parents=True)
    (slot_path / "orig.txt").write_text("stale original\n", encoding="utf-8")
    worktreesimage.save_bind_records(
        control,
        {"s1": worktreesimage.BindRecord(subvolume="s1", target=str(slot_path))},
    )
    fake = _FakeLayoutHost(mount_dir, config.worktrees)
    monkeypatch.setattr(worktreesimage, "_run", fake.run)
    monkeypatch.setattr(worktreesimage, "mount_id_at", lambda point: 99)
    monkeypatch.setattr(worktreesimage, "is_subvolume", lambda path: path.is_dir())
    monkeypatch.setattr(slotimage, "mount_table", lambda: [])
    locked: list[str] = []
    monkeypatch.setattr(
        imagecmd, "_lock_slot_checkouts", lambda cfg, slot, slot_type: locked.append(slot)
    )
    record = _record()

    imagecmd._migrate_layout_to_direct(config, config.worktrees_image, [record])
    assert (slot_path / "keep.txt").read_text(encoding="utf-8") == "image copy\n"
    assert not (mount_dir / "s1").exists()  # now a top level of the direct mount
    assert worktreesimage.load_binds(control) == {}
    assert (config.worktrees / "validate").is_dir()
    assert "mount_at: worktrees_dir" in config.config_path.read_text(encoding="utf-8")
    assert locked == ["s1"]

    direct = worktreesimage.settings_from_obj(
        {"path": "worktrees.img", "size_bytes": 2 * GIB, "mount_at": "worktrees_dir"}
    )
    imagecmd._migrate_layout_to_binds(config, direct, [record])
    assert (mount_dir / "s1" / "keep.txt").read_text(encoding="utf-8") == "image copy\n"
    assert slot_path.is_dir()
    assert worktreesimage.load_binds(control) == {"s1": "s1"}
    assert "mount_at: binds" in config.config_path.read_text(encoding="utf-8")
    assert locked == ["s1", "s1"]


def _archive_record(slot: str, slot_type: str, path: str) -> dict[str, object]:
    finished = "2026-11-05T01:00:00+00:00"
    return {
        "archive_id": f"testhost:{slot}:1:{finished}",
        "slot": slot,
        "agent": "worker-a",
        "task": "t1",
        "purpose": "probe",
        "machine": "testhost",
        "generation": 1,
        "created_at": "2026-11-05T00:00:00+00:00",
        "finished_at": finished,
        "mode": "remove",
        "actor": "coordinator",
        "physical_storage": "removed",
        "validation": ["ok"],
        "limitations": [],
        "continuation": "done",
        "slot_type": slot_type,
        "layout": "flat",
        "checkouts": [
            {
                "name": "repo",
                "path": path,
                "repository": ".",
                "branch": "b1",
                "start_point": "0" * 40,
                "remote": "origin",
                "remote_url_sha256": "0" * 64,
                "landed_ref": "refs/remotes/origin/main",
                "head": "0" * 40,
                "containing_remote_refs": [],
                "vcs": "git",
            }
        ],
    }


def test_migrate_layout_with_archived_validate_rows(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # #1183 acceptance in miniature: archived validate rows at
    # worktrees/validate (and an archived agent row) must not brick the
    # switch to direct, nor the switch back.
    from wrkslots import imagecmd

    config = _layout_project(tmp_path)
    assert config.worktrees_image is not None
    config.worktrees_image.image_file(config.root).touch()
    control = config.control
    mount_dir = worktreesimage.image_mount_dir(control)
    (mount_dir / "s1").mkdir(parents=True)
    (mount_dir / "s1" / "keep.txt").write_text("image copy\n", encoding="utf-8")
    slot_path = config.worktrees / "s1"
    slot_path.mkdir(parents=True)
    worktreesimage.save_bind_records(
        control,
        {"s1": worktreesimage.BindRecord(subvolume="s1", target=str(slot_path))},
    )
    archive = {
        "schema": cli.SCHEMA,
        "machine": "testhost",
        "revision": 2,
        "records": [
            _archive_record("v-old", "validate", "worktrees/validate/v-old"),
            _archive_record("a-old", "agent", "worktrees/slots/a-old"),
        ],
    }
    cli._atomic_write_json(cli._archive_path(config), archive)
    assert cli._load_archive(config).records[0]["slot"] == "v-old"
    fake = _FakeLayoutHost(mount_dir, config.worktrees)
    monkeypatch.setattr(worktreesimage, "_run", fake.run)
    monkeypatch.setattr(worktreesimage, "mount_id_at", lambda point: 99)
    monkeypatch.setattr(worktreesimage, "is_subvolume", lambda path: path.is_dir())
    monkeypatch.setattr(slotimage, "mount_table", lambda: [])
    monkeypatch.setattr(
        imagecmd, "_lock_slot_checkouts", lambda cfg, slot, slot_type: None
    )
    import dataclasses as _dc

    record = _dc.replace(
        _record(), checkouts=(_checkout("worktrees/slots/s1"),), layout="flat"
    )
    imagecmd._assert_target_layout_paths(config, config.worktrees_image, [record], True)
    imagecmd._migrate_layout_to_direct(config, config.worktrees_image, [record])
    assert cli._load_archive(config).records[0]["slot"] == "v-old"
    assert cli._slot_directory(config, "v-new", "validate") == (
        tmp_path / "worktrees" / "validate" / "v-new"
    )
    direct = worktreesimage.settings_from_obj(
        {"path": "worktrees.img", "size_bytes": 2 * GIB, "mount_at": "worktrees_dir"}
    )
    imagecmd._assert_target_layout_paths(config, direct, [record], False)
    imagecmd._migrate_layout_to_binds(config, direct, [record])
    assert cli._load_archive(config).records[0]["slot"] == "v-old"


def test_doctor_reports_a_missing_image_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    if shutil.which("git") is None:
        pytest.skip("needs git")
    monkeypatch.setenv("GIT_AUTHOR_NAME", "lock-test")
    monkeypatch.setenv("GIT_AUTHOR_EMAIL", "lock-test@example.invalid")
    monkeypatch.setenv("GIT_COMMITTER_NAME", "lock-test")
    monkeypatch.setenv("GIT_COMMITTER_EMAIL", "lock-test@example.invalid")
    subprocess.run(["git", "init", "-q", "-b", "main"], cwd=tmp_path, check=True)
    subprocess.run(
        ["git", "commit", "-q", "--allow-empty", "-m", "init"], cwd=tmp_path, check=True
    )
    head = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=tmp_path, check=True,
        capture_output=True, text=True,
    ).stdout.strip()
    config = _direct_config(tmp_path, worktreesimage.MOUNTED)
    slot_path = config.worktrees / "s1"
    slot_path.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        ["git", "worktree", "add", str(slot_path), "-b", "b-s1"],
        cwd=tmp_path, check=True, capture_output=True, text=True,
    )
    monkeypatch.setattr(worktreesimage, "is_subvolume", lambda path: path == slot_path)
    checkout = cli.Checkout(
        name="repo",
        path="worktrees/slots/s1",
        repository=".",
        branch="b-s1",
        start_point=head,
        remote="origin",
        remote_url_sha256="0" * 64,
        landed_ref="refs/remotes/origin/main",
        head=head,
    )
    import dataclasses as _dc

    record = _dc.replace(_record(), checkouts=(checkout,))
    states = [cli.ActiveState(machine="testhost", revision=0, slots=(record,))]
    findings = cli._image_lock_findings(config, states)
    assert [item["kind"] for item in findings] == ["worktrees-image-lock-missing"]
    cli._GitVcs().lock_worktree(tmp_path, slot_path)
    assert cli._image_lock_findings(config, states) == []
    # Advisory only: the storage-inconsistency channel that targeted
    # mutations gate on never carries this kind.
    cli._GitVcs().drop_own_worktree_lock(tmp_path, slot_path)
    kinds = {
        item.kind
        for item in cli._registry_storage_inconsistencies(
            config, states, tolerate_unavailable_repositories=True
        )
    }
    assert "worktrees-image-lock-missing" not in kinds


def _real_btrfs_or_skip() -> None:
    if shutil.which("sudo") is None or shutil.which("mkfs.btrfs") is None:
        pytest.skip("needs sudo and mkfs.btrfs")
    probe = subprocess.run(
        ["sudo", "-n", "true"], capture_output=True, text=True, check=False
    )
    if probe.returncode != 0:
        pytest.skip("needs passwordless sudo for a loop-mounted btrfs image")


def test_real_direct_image_lifecycle(tmp_path: Path) -> None:
    # Real loop-mounted scratch image (never a live one): create, mount at
    # the slots directory, simple quotas, a quota-limited subvolume slot,
    # a fence rename, status, delete, and the unmounted refusal state.
    _real_btrfs_or_skip()
    root = tmp_path / "project"
    control = root / "worktrees"
    slots = control / "slots"
    slots.mkdir(parents=True)
    settings = worktreesimage.WorktreesImageSettings(
        relative_path="worktrees.img",
        size_bytes=GIB,
        mount_at="worktrees_dir",
        quota_mode="simple",
        slot_quota_bytes=64 * 1024 * 1024,
    )
    image = settings.image_file(root)
    worktreesimage.create_image(image, settings.size_bytes)
    try:
        worktreesimage.ensure_image_mounted(root, control, settings, slots)
        assert (
            worktreesimage.current_state(root, slots, settings)
            == worktreesimage.MOUNTED
        )
        assert slots.stat().st_uid == os.getuid()
        assert (slots / "validate").is_dir()
        validate_surface = worktreesimage.validate_external_dir(slots)
        assert validate_surface == control / "validate"
        assert validate_surface.is_dir()
        vslot = validate_surface / "v1"
        worktreesimage.create_slot_subvolume(vslot, settings.slot_quota_bytes)
        assert worktreesimage.is_subvolume(vslot)
        assert (slots / "validate" / "v1").is_dir()
        (vslot / "val.txt").write_text("val\n", encoding="utf-8")
        assert (slots / "validate" / "v1" / "val.txt").read_text(encoding="utf-8") == "val\n"
        worktreesimage.delete_slot_subvolume(vslot)
        assert not vslot.exists()
        mode = worktreesimage.quota_mode_at(slots, image)
        if mode != "simple":
            pytest.skip(f"simple quotas unsupported on this host (mode={mode})")
        slot = slots / "s1"
        worktreesimage.create_slot_subvolume(slot, settings.slot_quota_bytes)
        assert worktreesimage.is_subvolume(slot)
        (slot / "work.txt").write_text("work\n", encoding="utf-8")
        fenced = slots / (".s1.fenced.1." + "f" * 32)
        os.rename(slot, fenced)
        assert (fenced / "work.txt").read_text(encoding="utf-8") == "work\n"
        os.rename(fenced, slot)
        report = worktreesimage.status_report(root, slots, settings)
        assert report["mount_point"] == str(slots)
        assert report["quota_mode"] == "simple"
        assert report["binds"] == {}
        assert report["validate_bind"] == "mounted"
        assert report["validate_dir"] == str(validate_surface)
        # A remount never flips the existing mode.
        worktreesimage.apply_quota_mode(slots, image, "full")
        assert worktreesimage.quota_mode_at(slots, image) == "simple"
        worktreesimage.delete_slot_subvolume(slot)
        assert not slot.exists()
    finally:
        subprocess.run(
            ["sudo", "-n", "umount", str(worktreesimage.validate_external_dir(slots))],
            capture_output=True,
            check=False,
        )
        subprocess.run(
            ["sudo", "-n", "umount", str(slots)], capture_output=True, check=False
        )
    assert (
        worktreesimage.current_state(root, slots, settings)
        == worktreesimage.NOT_MOUNTED
    )


def test_real_migrate_layout_round_trip(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # The movers on a real image: binds -> direct -> binds, contents kept,
    # host original beneath the bind reclaimed on the way to direct.
    from wrkslots import imagecmd

    _real_btrfs_or_skip()
    config = _layout_project(tmp_path)
    assert config.worktrees_image is not None
    binds = config.worktrees_image
    control = config.control
    mount_dir = worktreesimage.image_mount_dir(control)
    slot_path = config.worktrees / "s1"
    config.worktrees.mkdir(parents=True, exist_ok=True)
    locked: list[str] = []
    monkeypatch.setattr(
        imagecmd, "_lock_slot_checkouts", lambda cfg, slot, slot_type: locked.append(slot)
    )
    record = _record()
    try:
        worktreesimage.create_image(binds.image_file(config.root), binds.size_bytes)
        worktreesimage.ensure_image_mounted(config.root, control, binds, config.worktrees)
        worktreesimage.create_slot_subvolume(mount_dir / "s1", binds.slot_quota_bytes)
        slot_path.mkdir(parents=True)
        (slot_path / "orig.txt").write_text("stale original\n", encoding="utf-8")
        worktreesimage.bind_slot(control, mount_dir, "s1", slot_path)
        (slot_path / "keep.txt").write_text("image copy\n", encoding="utf-8")

        imagecmd._migrate_layout_to_direct(config, binds, [record])
        assert (slot_path / "keep.txt").read_text(encoding="utf-8") == "image copy\n"
        assert worktreesimage.is_subvolume(slot_path)
        assert not (slot_path / "orig.txt").exists()
        assert worktreesimage.load_binds(control) == {}

        direct = worktreesimage.settings_from_obj(
            {"path": "worktrees.img", "size_bytes": 2 * GIB, "mount_at": "worktrees_dir"}
        )
        imagecmd._migrate_layout_to_binds(config, direct, [record])
        assert (slot_path / "keep.txt").read_text(encoding="utf-8") == "image copy\n"
        assert worktreesimage.load_binds(control) == {"s1": "s1"}
        assert locked == ["s1", "s1"]
    finally:
        subprocess.run(
            ["sudo", "-n", "umount", str(slot_path)], capture_output=True, check=False
        )
        subprocess.run(
            ["sudo", "-n", "umount", str(worktreesimage.validate_external_dir(config.worktrees))],
            capture_output=True,
            check=False,
        )
        subprocess.run(
            ["sudo", "-n", "umount", str(mount_dir)], capture_output=True, check=False
        )
        subprocess.run(
            ["sudo", "-n", "umount", str(config.worktrees)],
            capture_output=True,
            check=False,
        )


# --- #2238: reclaim markers -------------------------------------------------
#
# A reclaimer holds RECLAIM.<slot>.lock (an exclusive flock) from before its
# first unmount until the slot's bind record is dropped; ensure_binds leaves
# such a slot alone. Every test here uses fake mount tables and a fake mount
# runner: nothing is mounted. A flock held on one open file description
# blocks a test made through another, even in one process, so a marker held
# by the test is exactly what another process would see.


def _reclaim_setup(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, slots: tuple[str, ...] = ("s1",)
) -> tuple[Path, Path, Path, _FakeMounts]:
    control = tmp_path / "worktrees"
    mount_dir = worktreesimage.image_mount_dir(control)
    slots_dir = control / "slots"
    for slot in slots:
        (mount_dir / slot).mkdir(parents=True)
    worktreesimage.save_bind_records(
        control,
        {
            slot: worktreesimage.BindRecord(
                subvolume=slot, mount_id=100, target=str(slots_dir / slot)
            )
            for slot in slots
        },
    )
    fake = _FakeMounts()
    monkeypatch.setattr(worktreesimage, "_run", fake.run)
    monkeypatch.setattr(worktreesimage, "mount_id_at", lambda point: 777)
    # Namespace B's view: every recorded bind target is unmounted
    # (namespace A's reclaimer unmounted it there).
    monkeypatch.setattr(slotimage, "mount_table", lambda: [])
    return control, mount_dir, slots_dir, fake


def _record_identity(control: Path) -> tuple[bytes, int, int]:
    path = worktreesimage.record_path(control)
    status = path.stat()
    return path.read_bytes(), status.st_ino, status.st_mtime_ns


def _binds_of(fake: _FakeMounts) -> list[list[str]]:
    return [call for call in fake.calls if call[:2] == ["mount", "--bind"]]


def test_ensure_binds_leaves_a_slot_under_reclaim_alone(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    control, mount_dir, slots_dir, fake = _reclaim_setup(
        tmp_path, monkeypatch, ("s1", "s2")
    )
    with worktreesimage.hold_reclaim_marker(control, "s1", writer="test reclaimer"):
        assert worktreesimage.reclaim_marker_state(control, "s1") == worktreesimage.RECLAIM_LIVE
        actions = worktreesimage.ensure_binds(control, mount_dir, slots_dir)
        records = worktreesimage.load_bind_records(control)
    # The slot under reclaim is not re-bound and its record is not
    # rewritten; the other slot binds exactly as before.
    assert actions == ["bound slot s2 from the worktrees image"]
    assert _binds_of(fake) == [["mount", "--bind", str(mount_dir / "s2"), str(slots_dir / "s2")]]
    assert records["s1"] == worktreesimage.BindRecord(
        subvolume="s1", mount_id=100, target=str(slots_dir / "s1")
    )
    assert records["s2"].mount_id == 777


def test_ensure_binds_does_not_privatise_a_slot_under_reclaim(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    control, mount_dir, slots_dir, fake = _reclaim_setup(tmp_path, monkeypatch)
    monkeypatch.setattr(
        slotimage, "mount_table", lambda: [_bind_entry(slots_dir / "s1", "shared:42")]
    )
    before = _record_identity(control)
    with worktreesimage.hold_reclaim_marker(control, "s1", writer="test reclaimer"):
        assert worktreesimage.ensure_binds(control, mount_dir, slots_dir) == []
    assert fake.calls == []
    assert _record_identity(control) == before


def test_read_only_commands_never_rebind_a_slot_under_reclaim(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    # #2238 acceptance: a recorded bind unmounted in namespace A (the
    # reclaimer's) and a read-only command in namespace B, whose mount
    # table shows the image mounted and the target unmounted: with the
    # marker held the target is not re-bound and the record is
    # untouched; without it, today's re-bind still happens.
    import json as json_module

    root = tmp_path / "project"
    control = root / "worktrees"
    slots_dir = control / "slots"
    mount_dir = worktreesimage.image_mount_dir(control)
    (mount_dir / "s1").mkdir(parents=True)
    slots_dir.mkdir(parents=True)
    (root / "live.py").write_text("# probe\n")
    (root / ".wrkslots.yml").write_text(
        "schema: 2\n"
        "machine: node-a\n"
        "worktrees_dir: worktrees/slots\n"
        "default_remote: origin\n"
        "default_landed_ref: refs/remotes/origin/main\n"
        "heartbeat_ttl_seconds: 60\n"
        "liveness_command: live.py\n"
        "worktrees_image:\n  path: wt.img\n  size_bytes: 1073741824\n"
    )
    (control / "ACTIVE.node-a.json").write_text(
        json_module.dumps({"schema": 2, "machine": "node-a", "revision": 0, "slots": []})
    )
    (control / "ARCHIVED.node-a.json").write_text(
        json_module.dumps({"schema": 2, "machine": "node-a", "revision": 0, "records": []})
    )
    worktreesimage.save_bind_records(
        control,
        {
            "s1": worktreesimage.BindRecord(
                subvolume="s1", mount_id=100, target=str(slots_dir / "s1")
            )
        },
    )
    image_mount = slotimage.MountEntry(
        mount_point=mount_dir,
        fstype="btrfs",
        source="/dev/loop9",
        device="0:999",
        root="/",
    )
    monkeypatch.setattr(slotimage, "mount_table", lambda: [image_mount])
    monkeypatch.setattr(slotimage, "ensure_all_mounted", lambda *a, **k: ([], []))
    monkeypatch.setattr(cli, "_OWNED_REPRESENTATION_MOUNTS", frozenset())
    monkeypatch.setattr(worktreesimage, "quota_mode_at", lambda *a, **k: "none")
    fake = _FakeMounts()
    monkeypatch.setattr(worktreesimage, "_run", fake.run)
    monkeypatch.setattr(worktreesimage, "mount_id_at", lambda point: 777)
    project = ["--project-root", str(root), "--machine", "node-a"]
    before = _record_identity(control)

    with worktreesimage.hold_reclaim_marker(control, "s1", writer="test reclaimer"):
        assert cli.main([*project, "status"]) == 0
        assert cli.main([*project, "worktrees-image", "status", "--format", "json"]) == 0
        assert _binds_of(fake) == []
        assert _record_identity(control) == before
        out = capsys.readouterr().out
        assert '"reclaim_markers": {\n    "s1": "live"\n  }' in out

    # Released (the reclaimer abandoned the reclaim): binds behave as before.
    assert not worktreesimage.reclaim_marker_path(control, "s1").exists()
    assert cli.main([*project, "status"]) == 0
    assert _binds_of(fake) == [
        ["mount", "--bind", str(mount_dir / "s1"), str(slots_dir / "s1")]
    ]
    assert worktreesimage.load_bind_records(control)["s1"].mount_id == 777


def test_ensure_binds_waits_out_a_reclaimer_holding_the_records_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # With the records lock held by a reclaimer that drops the record
    # before releasing it, a racing config load that found the target
    # unmounted waits on the lock, re-reads, and binds nothing.
    control, mount_dir, slots_dir, fake = _reclaim_setup(tmp_path, monkeypatch)
    waiting = threading.Event()
    real_flock = worktreesimage._flock_records_until
    main = threading.current_thread()

    def observed(descriptor: int, deadline: float) -> int | None:
        if threading.current_thread() is not main:
            waiting.set()
        return real_flock(descriptor, deadline)

    monkeypatch.setattr(worktreesimage, "_flock_records_until", observed)
    results: list[object] = []

    def load() -> None:
        try:
            results.append(worktreesimage.ensure_binds(control, mount_dir, slots_dir))
        except BaseException as exc:  # asserted below
            results.append(exc)

    with worktreesimage._records_lock(control):
        loader = threading.Thread(target=load)
        loader.start()
        assert waiting.wait(timeout=60)
        worktreesimage.forget_bind(control, "s1")
    loader.join(timeout=60)
    assert not loader.is_alive()
    assert results == [[]]
    assert _binds_of(fake) == []
    assert worktreesimage.load_bind_records(control) == {}


def test_a_reclaim_finished_after_the_unlocked_pre_read_is_not_rebound(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # R1 (#2238): the unlocked pre-read only decides whether to take the
    # lock. A reclaimer that pops the record and unlinks its marker
    # between that pre-read and the locked pass must not see the slot
    # re-bound from the pre-read's stale list: the binding decision uses
    # the records and the mount table read under the lock, and the
    # marker probe sits per slot after them, right before the bind.
    control, mount_dir, slots_dir, fake = _reclaim_setup(
        tmp_path, monkeypatch, ("s1", "s2")
    )
    real_pre_check = worktreesimage._binds_need_locked_pass
    pre_read: list[set[str]] = []

    with worktreesimage.hold_reclaim_marker(control, "s1", writer="test reaper") as reclaim:

        def pre_check_then_reaper_finishes(
            control_: Path,
            mount_dir_: Path,
            slots_dir_: Path,
            records: dict[str, worktreesimage.BindRecord],
            entries: list[slotimage.MountEntry],
        ) -> bool:
            pre_read.append(set(records))
            needed = real_pre_check(control_, mount_dir_, slots_dir_, records, entries)
            # The reaper completes now: record popped and marker
            # unlinked under the records lock, before the locked pass.
            worktreesimage.forget_bind(control, "s1", reclaim=reclaim)
            return needed

        monkeypatch.setattr(
            worktreesimage, "_binds_need_locked_pass", pre_check_then_reaper_finishes
        )
        actions = worktreesimage.ensure_binds(control, mount_dir, slots_dir)

    assert pre_read == [{"s1", "s2"}]  # the stale pre-read still named s1
    assert actions == ["bound slot s2 from the worktrees image"]
    assert _binds_of(fake) == [["mount", "--bind", str(mount_dir / "s2"), str(slots_dir / "s2")]]
    assert set(worktreesimage.load_bind_records(control)) == {"s2"}
    assert not worktreesimage.reclaim_marker_path(control, "s1").exists()


@pytest.mark.parametrize("marker", ["absent", "stale"])
def test_without_a_live_reclaim_marker_binds_come_back_as_before(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, marker: str
) -> None:
    control, mount_dir, slots_dir, fake = _reclaim_setup(tmp_path, monkeypatch)
    if marker == "stale":
        # A crashed reclaimer: its file stays, its flock is gone.
        worktreesimage.reclaim_marker_path(control, "s1").write_text(
            '{"kind": "wrkslots-bind-reclaim", "slot": "s1"}\n', encoding="utf-8"
        )
        assert worktreesimage.reclaim_marker_state(control, "s1") == worktreesimage.RECLAIM_STALE
    else:
        assert worktreesimage.reclaim_marker_state(control, "s1") is None
    actions = worktreesimage.ensure_binds(control, mount_dir, slots_dir)
    assert actions == ["bound slot s1 from the worktrees image"]
    assert _binds_of(fake) == [["mount", "--bind", str(mount_dir / "s1"), str(slots_dir / "s1")]]
    # wrkslots never deletes a marker it did not write.
    assert worktreesimage.reclaim_marker_path(control, "s1").exists() is (marker == "stale")


def test_an_unverifiable_reclaim_marker_is_honoured_as_live(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    control, mount_dir, slots_dir, fake = _reclaim_setup(tmp_path, monkeypatch)
    worktreesimage.reclaim_marker_path(control, "s1").mkdir()
    assert (
        worktreesimage.reclaim_marker_state(control, "s1")
        == worktreesimage.RECLAIM_UNVERIFIABLE
    )
    assert worktreesimage.ensure_binds(control, mount_dir, slots_dir) == []
    assert fake.calls == []
    assert worktreesimage.reclaim_markers(control) == {"s1": "unverifiable"}


def test_a_reclaim_marker_is_not_registry_state(tmp_path: Path) -> None:
    # Older readers ignore the marker: the registry and read identities
    # skip *.lock names, so creating, rewriting or removing a marker
    # costs no concurrent mutation a validation round.
    import json as json_module

    config = _config(tmp_path, worktreesimage.MOUNTED)
    config.control.mkdir(parents=True)
    (config.control / "ACTIVE.testhost.json").write_text(
        json_module.dumps({"schema": 2, "machine": "testhost", "revision": 0, "slots": []})
    )
    identity = cli._registry_identity(config)
    read_identity = cli._registry_read_identity(config)
    with worktreesimage.hold_reclaim_marker(config.control, "s1", writer="test reclaimer"):
        assert cli._registry_identity(config) == identity
        assert cli._registry_read_identity(config) == read_identity
    worktreesimage.reclaim_marker_path(config.control, "s2").write_text("{}\n")
    assert cli._registry_identity(config) == identity
    assert cli._registry_read_identity(config) == read_identity


def test_unbind_slot_holds_the_reclaim_marker_until_the_record_is_dropped(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    control, mount_dir, slots_dir, fake = _reclaim_setup(tmp_path, monkeypatch)
    seen: list[object] = []

    def umount_then_race(argv: list[str], *, sudo: bool = False) -> object:
        if argv[:1] == ["umount"]:
            seen.append(worktreesimage.reclaim_marker_state(control, "s1"))
            payload = worktreesimage.reclaim_marker_path(control, "s1").read_text()
            seen.append(payload)
            # Another wrkslots process loads its config right after
            # the unmount, before the record is dropped.
            racer = threading.Thread(
                target=lambda: seen.append(
                    worktreesimage.ensure_binds(control, mount_dir, slots_dir)
                )
            )
            racer.start()
            racer.join(timeout=60)
        return fake.run(argv, sudo=sudo)

    monkeypatch.setattr(worktreesimage, "_run", umount_then_race)
    worktreesimage.unbind_slot(control, "s1", slots_dir / "s1")

    import json as json_module

    assert seen[0] == worktreesimage.RECLAIM_LIVE
    payload = json_module.loads(str(seen[1]))
    assert payload["kind"] == "wrkslots-bind-reclaim"
    assert payload["version"] == 1
    assert payload["slot"] == "s1"
    assert payload["pid"] == os.getpid()
    assert payload["writer"] == "wrkslots unbind"
    assert seen[2] == []  # the racing load bound nothing
    assert _binds_of(fake) == []
    assert worktreesimage.load_bind_records(control) == {}
    assert not worktreesimage.reclaim_marker_path(control, "s1").exists()


def test_a_failed_unbind_removes_its_marker_and_keeps_the_record(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    control, mount_dir, slots_dir, _fake = _reclaim_setup(tmp_path, monkeypatch)
    monkeypatch.setattr(worktreesimage, "_run", _FakeMounts(fail_umount=True).run)
    monkeypatch.setattr(worktreesimage, "_sleep", lambda seconds: None)
    # The busy-holder scan reads a fixture process tree (empty), never
    # the host's: scanning every process on a loaded host takes minutes.
    empty_proc = tmp_path / "proc"
    empty_proc.mkdir()
    monkeypatch.setattr(worktreesimage, "_PROC_ROOT", empty_proc)
    with pytest.raises(worktreesimage.WorktreesImageError):
        worktreesimage.unbind_slot(control, "s1", slots_dir / "s1")
    assert not worktreesimage.reclaim_marker_path(control, "s1").exists()
    assert set(worktreesimage.load_bind_records(control)) == {"s1"}
    # The reclaim was abandoned, so binds behave as before.
    fake = _FakeMounts()
    monkeypatch.setattr(worktreesimage, "_run", fake.run)
    assert worktreesimage.ensure_binds(control, mount_dir, slots_dir) == [
        "bound slot s1 from the worktrees image"
    ]


def test_remove_holds_the_reclaim_marker_across_unmount_and_record_drop(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    config = _config(tmp_path, worktreesimage.MOUNTED)
    control = config.control
    mount_dir = worktreesimage.image_mount_dir(control)
    (mount_dir / "s1").mkdir(parents=True)
    slot_path = config.worktrees / "s1"
    slot_path.mkdir(parents=True)
    worktreesimage.save_bind_records(
        control,
        {"s1": worktreesimage.BindRecord(subvolume="s1", mount_id=5, target=str(slot_path))},
    )
    fake = _FakeMounts()
    states: list[object] = []

    def run(argv: list[str], *, sudo: bool = False) -> object:
        if argv[:1] == ["umount"]:
            states.append(worktreesimage.reclaim_marker_state(control, "s1"))
            states.append(set(worktreesimage.load_bind_records(control)))
        return fake.run(argv, sudo=sudo)

    monkeypatch.setattr(worktreesimage, "_run", run)
    cli._remove_slot_directory(config, slot_path)
    assert states == [worktreesimage.RECLAIM_LIVE, {"s1"}]
    assert not slot_path.exists() and not (mount_dir / "s1").exists()
    assert worktreesimage.load_bind_records(control) == {}
    assert not worktreesimage.reclaim_marker_path(control, "s1").exists()


def test_remove_abandoned_for_content_beneath_releases_its_marker(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # The remove path's own re-bind (a host original beneath the bind)
    # is never refused by its own marker, and the marker goes with the
    # abandoned removal.
    config = _config(tmp_path, worktreesimage.MOUNTED)
    control = config.control
    mount_dir = worktreesimage.image_mount_dir(control)
    (mount_dir / "s1").mkdir(parents=True)
    slot_path = config.worktrees / "s1"
    slot_path.mkdir(parents=True)
    (slot_path / "original.txt").write_text("host original\n")
    worktreesimage.save_bind_records(
        control,
        {"s1": worktreesimage.BindRecord(subvolume="s1", mount_id=5, target=str(slot_path))},
    )
    fake = _FakeMounts()
    monkeypatch.setattr(worktreesimage, "_run", fake.run)
    monkeypatch.setattr(worktreesimage, "mount_id_at", lambda point: 778)
    with pytest.raises(OSError, match="content sits beneath"):
        cli._remove_slot_directory(config, slot_path)
    assert _binds_of(fake) == [["mount", "--bind", str(mount_dir / "s1"), str(slot_path)]]
    assert worktreesimage.load_bind_records(control)["s1"].mount_id == 778
    assert not worktreesimage.reclaim_marker_path(control, "s1").exists()


def test_a_second_reclaimer_of_one_slot_waits_then_refuses(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    control = tmp_path / "worktrees"
    monkeypatch.setattr(worktreesimage, "RECLAIM_MARKER_TIMEOUT_SECONDS", 0.2)
    failures: list[BaseException] = []

    def second() -> None:
        try:
            with worktreesimage.hold_reclaim_marker(control, "s1", writer="second"):
                pass
        except BaseException as exc:  # asserted below
            failures.append(exc)

    with worktreesimage.hold_reclaim_marker(control, "s1", writer="first"):
        thread = threading.Thread(target=second)
        thread.start()
        thread.join(timeout=60)
        assert worktreesimage.reclaim_marker_state(control, "s1") == worktreesimage.RECLAIM_LIVE
    assert not thread.is_alive()
    assert len(failures) == 1
    assert isinstance(failures[0], worktreesimage.WorktreesImageError)
    assert "reclaim marker is busy" in str(failures[0])
    assert not worktreesimage.reclaim_marker_path(control, "s1").exists()


def test_a_reclaimer_takes_over_a_stale_marker(tmp_path: Path) -> None:
    import json as json_module

    control = tmp_path / "worktrees"
    control.mkdir()
    path = worktreesimage.reclaim_marker_path(control, "s1")
    path.write_text("leftover from a crashed reclaimer, much longer than the payload " * 4)
    with worktreesimage.hold_reclaim_marker(control, "s1", writer="next") as marker:
        assert marker.path == path
        assert worktreesimage.reclaim_marker_state(control, "s1") == worktreesimage.RECLAIM_LIVE
        payload = json_module.loads(path.read_text())
        assert payload["writer"] == "next" and payload["slot"] == "s1"
        # Nested reclaims of one slot in one thread reuse the hold.
        with worktreesimage.hold_reclaim_marker(control, "s1", writer="nested") as inner:
            assert inner is marker
        assert path.exists()
    assert not path.exists()


def test_a_reclaimer_reopens_a_marker_unlinked_before_its_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Another reclaimer finished and unlinked the file between our
    # open and our flock: that lock guards nothing, so the path is
    # opened again and the lock taken on the file that is there.
    control = tmp_path / "worktrees"
    control.mkdir()
    path = worktreesimage.reclaim_marker_path(control, "s1")
    real_flock = worktreesimage._flock_records_until
    calls: list[int] = []

    def unlink_first(descriptor: int, deadline: float) -> int | None:
        calls.append(descriptor)
        if len(calls) == 1:
            path.unlink()
        return real_flock(descriptor, deadline)

    monkeypatch.setattr(worktreesimage, "_flock_records_until", unlink_first)
    with worktreesimage.hold_reclaim_marker(control, "s1", writer="test"):
        assert len(calls) == 2
        assert worktreesimage.reclaim_marker_state(control, "s1") == worktreesimage.RECLAIM_LIVE
    assert not path.exists()


def test_status_lists_reclaim_markers_live_and_stale(tmp_path: Path) -> None:
    config = _config(tmp_path, worktreesimage.MOUNTED)
    config.control.mkdir(parents=True)
    worktreesimage.reclaim_marker_path(config.control, "s2").write_text("{}\n")
    (config.control / "RECLAIM.lock").write_text("")  # names no slot
    settings = config.worktrees_image
    assert settings is not None
    with worktreesimage.hold_reclaim_marker(config.control, "s1", writer="test"):
        report = worktreesimage.status_report(config.root, config.worktrees, settings)
    assert report["reclaim_markers"] == {"s1": "live", "s2": "stale"}
    direct = worktreesimage.status_report(
        config.root,
        config.worktrees,
        worktreesimage.WorktreesImageSettings(
            relative_path="worktrees.img",
            size_bytes=2 * GIB,
            mount_at=worktreesimage.MOUNT_AT_WORKTREES_DIR,
        ),
    )
    assert direct["reclaim_markers"] == {}


def _race_records_writer(
    monkeypatch: pytest.MonkeyPatch,
    control: Path,
    mutate: Callable[[dict[str, worktreesimage.BindRecord]], dict[str, worktreesimage.BindRecord]],
) -> list[threading.Thread]:
    """Run ``mutate`` as a concurrent records writer at the caller's first read.

    The writer starts right after the calling thread's first
    ``load_bind_records`` and the caller continues only once the writer
    has either finished (nothing held the records lock) or is waiting on
    the lock the caller holds. Either way the outcome is decided by the
    lock's state, never by timing.
    """

    real_load = worktreesimage.load_bind_records
    real_flock = worktreesimage._flock_records_until
    caller = threading.current_thread()
    settled = threading.Event()
    racers: list[threading.Thread] = []

    def flock(descriptor: int, deadline: float) -> int | None:
        if threading.current_thread() in racers:
            try:
                fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                return descriptor
            except BlockingIOError:
                settled.set()  # waiting on the caller's lock
        return real_flock(descriptor, deadline)

    def race() -> None:
        worktreesimage._update_bind_records(control, mutate)
        settled.set()

    def load(control_: Path) -> dict[str, worktreesimage.BindRecord]:
        result = real_load(control_)
        if threading.current_thread() is caller and not racers:
            racer = threading.Thread(target=race)
            racers.append(racer)
            racer.start()
            assert settled.wait(timeout=60)
        return result

    monkeypatch.setattr(worktreesimage, "_flock_records_until", flock)
    monkeypatch.setattr(worktreesimage, "load_bind_records", load)
    return racers


def test_relocating_an_unrecorded_mount_records_only_its_own_slot(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # #2238: a reclaimer pops "gone" and a create binds "s10" between
    # this writer's read and its write. Neither change is lost or
    # undone: the writer changes only its own slot, under the lock.
    config = _config(tmp_path, worktreesimage.MOUNTED)
    control = config.control
    mount_dir = worktreesimage.image_mount_dir(control)
    (mount_dir / "v1").mkdir(parents=True)
    source = config.worktrees / "v1"
    source.mkdir(parents=True)
    worktreesimage.save_bind_records(
        control,
        {
            "gone": worktreesimage.BindRecord(subvolume="gone"),
            "keep": worktreesimage.BindRecord(subvolume="keep"),
        },
    )
    relocated: list[tuple[str, Path, Path]] = []
    monkeypatch.setattr(
        worktreesimage,
        "relocate_bind",
        lambda control_, mount_dir_, slot, src, dst: relocated.append((slot, src, dst)),
    )
    monkeypatch.setattr(cli, "_refresh_owned_mounts", lambda config_: None)

    def reclaimer_and_create(
        records: dict[str, worktreesimage.BindRecord],
    ) -> dict[str, worktreesimage.BindRecord]:
        records.pop("gone", None)
        records["s10"] = worktreesimage.BindRecord(subvolume="s10")
        return records

    racers = _race_records_writer(monkeypatch, control, reclaimer_and_create)
    entry = slotimage.MountEntry(
        mount_point=source, fstype="btrfs", source="/dev/loop9", device="0:999", root="/v1"
    )
    fenced = config.worktrees / ".v1.fenced.1"
    assert cli._relocate_unrecorded_image_mount(config, source, fenced, entry)
    for racer in racers:
        racer.join(timeout=60)
    assert racers and not any(racer.is_alive() for racer in racers)
    records = worktreesimage.load_bind_records(control)
    assert set(records) == {"keep", "s10", "v1"}
    assert records["v1"] == worktreesimage.BindRecord(subvolume="v1", target=str(source))
    assert relocated == [("v1", source, fenced)]


def test_a_failed_layout_switch_puts_back_only_the_records_it_dropped(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # #2238: the direct-layout switch unbinds every slot and fails. Its
    # rollback restores only the records it dropped, under the lock: a
    # bind another slot recorded meanwhile survives, and a slot whose
    # reclaimer now holds its marker is not resurrected.
    from wrkslots import imagecmd

    config = _layout_project(tmp_path)
    assert config.worktrees_image is not None
    config.worktrees_image.image_file(config.root).touch()
    control = config.control
    mount_dir = worktreesimage.image_mount_dir(control)
    records: dict[str, worktreesimage.BindRecord] = {}
    for slot in ("s1", "s2"):
        (mount_dir / slot).mkdir(parents=True)
        (config.worktrees / slot).mkdir(parents=True)
        records[slot] = worktreesimage.BindRecord(
            subvolume=slot, target=str(config.worktrees / slot)
        )
    worktreesimage.save_bind_records(control, records)
    fake = _FakeLayoutHost(mount_dir, config.worktrees)
    monkeypatch.setattr(worktreesimage, "_run", fake.run)
    monkeypatch.setattr(worktreesimage, "mount_id_at", lambda point: 99)
    monkeypatch.setattr(worktreesimage, "is_subvolume", lambda path: path.is_dir())
    monkeypatch.setattr(slotimage, "mount_table", lambda: [])
    real_mount_image = worktreesimage.mount_image
    held = contextlib.ExitStack()
    failed: list[Path] = []

    def mount_image(image: Path, point: Path, settings: object) -> None:
        if not failed:
            failed.append(point)
            # Meanwhile: a create records a bind for s9, and a
            # reclaimer starts reclaiming s2 (its record is already
            # gone, dropped by this switch's unbind).
            worktreesimage.record_bind_if_absent(
                control, "s9", worktreesimage.BindRecord(subvolume="s9")
            )
            held.enter_context(
                worktreesimage.hold_reclaim_marker(control, "s2", writer="test reaper")
            )
            raise worktreesimage.WorktreesImageError("mount failed")
        real_mount_image(image, point, settings)  # type: ignore[arg-type]

    monkeypatch.setattr(worktreesimage, "mount_image", mount_image)
    with held, pytest.raises(cli.Refusal, match="the bind layout was re-established"):
        imagecmd._migrate_layout_to_direct(
            config, config.worktrees_image, [_record("s1"), _record("s2")]
        )
    assert failed == [config.worktrees]
    after = worktreesimage.load_bind_records(control)
    assert set(after) == {"s1", "s9"}
    assert after["s1"].subvolume == "s1"
