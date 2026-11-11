"""Tests for the opt-in project worktrees image (one btrfs image per project).

Pure tests always run. The real-image lifecycle (mkfs/mount/grow/trim) needs
passwordless ``sudo -n`` and is exercised manually in the trial recorded in
``ai_docs/transient/2026-11-05-wtimg-worktrees-image/DESIGN.md``; it is not
repeated here because the shared test host cannot guarantee loop devices.
"""

from __future__ import annotations

import os
import shutil
import socket
import subprocess
import time
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
    assert [call[0] for call in fake.calls] == ["mount", "umount", "mount"]

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
    assert [call[0] for call in fake.calls] == ["umount", "mount"]
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
    with pytest.raises(OSError):
        cli._remove_slot_directory(config, slot_path)
    # Nothing was dropped: slot, subvolume and record are all still there.
    assert slot_path.is_dir() and (mount_dir / "s1").is_dir()
    assert worktreesimage.load_binds(control) == {"s1": "s1"}

    monkeypatch.setattr(worktreesimage, "_run", _FakeMounts().run)
    cli._remove_slot_directory(config, slot_path)
    assert not slot_path.exists() and not (mount_dir / "s1").exists()
    assert worktreesimage.load_binds(control) == {}


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
        worktreesimage.unbind_slot,
        worktreesimage.relocate_bind,
        worktreesimage.save_bind_records,
    ):
        assert "_update_bind_records" in inspect.getsource(function), function.__name__
    assert "save_bind_records" in inspect.getsource(worktreesimage.save_binds)
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
    assert [call[0] for call in fake.calls] == ["umount", "mount"]


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
        [sys.executable, "-c", "import time,sys; f=open(sys.argv[1],'rb'); time.sleep(60)", str(held_file)],
        cwd="/tmp",
    )
    try:
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            fds = list(Path(f"/proc/{child.pid}/fd").glob("*"))
            if any(os.readlink(fd) == str(held_file) for fd in fds):
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

    holder = subprocess.Popen(["sleep", "60"], cwd=slot_path)
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
