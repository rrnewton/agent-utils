"""Tests for the opt-in project worktrees image (one btrfs image per project).

Pure tests always run. The real-image lifecycle (mkfs/mount/grow/trim) needs
passwordless ``sudo -n`` and is exercised manually in the trial recorded in
``ai_docs/transient/2026-11-05-wtimg-worktrees-image/DESIGN.md``; it is not
repeated here because the shared test host cannot guarantee loop devices.
"""

from __future__ import annotations

import os
import shutil
import subprocess
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


def test_special_files_are_not_migratable(tmp_path: Path) -> None:
    (tmp_path / "keep.txt").write_text("x", encoding="utf-8")
    os.mkfifo(tmp_path / "pipe")
    signature = worktreesimage.listing_signature(tmp_path)
    assert signature == [("keep.txt", "file", 1)]
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
    assert ("a.txt", "file", 5) in signature
    assert ("sub", "dir", 0) in signature
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
